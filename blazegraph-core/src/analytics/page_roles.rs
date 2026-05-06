// Per-page role classification — Block 06 of the document analytics flow.
//
// Sits at the tail of `AnalysisBuilder::finalize` (after `page_stats`),
// consumes the descriptive `PageStats`, and writes a per-page role into
// each `PageSignature` plus a derived `body_pages` extent on `PageStats`.
//
// MVP scope (2026-05-06): two-class classifier — `Body` vs `NonBody` —
// implemented as a scan-inward-from-each-end algorithm. The body extent
// is the deliverable; per-page role labels are the substrate for future
// classifiers (TOC, References, Cover, Blank). Adding those is one new
// `PageRoleKind` variant + one new predicate, no new infrastructure.
//
// Algorithm (default-to-body):
//   - Forward scan from page 1: skip pages where strict_non_body() fires;
//     stop at the first page that passes. body_start_page = that page.
//   - Backward scan from page End: skip pages where strict_non_body()
//     fires, EXCEPT the very last page (End) gets a lenient test
//     (trailing-sparseness exemption — content can run out mid-page).
//     stop at the last page that passes. body_end_page = that page.
//   - All pages in [body_start, body_end] are Body. Pages outside are
//     NonBody. If no body page is found at all, body_start = body_end = 0
//     (sentinel for "no body detected") and every page is NonBody.
//
// Predicates (validated on the 6-PDF corpus, 2026-05-06):
//   - strict_non_body(p) := p.heatmap_fit < 0.15
//   - lenient_non_body(p)  // applies only to End:
//       := p.heatmap_fit < 0.15 AND p.n_tokens < 100
//     i.e., End is non-body only if BOTH the spatial fit signal is low
//     AND the page is near-empty. Lets sparse-but-nonempty trailing body
//     pages pass.
//
// The conservative-default-to-body principle:
//   - False-positive (real body classified as non-body) → loses content
//   - False-negative (non-body classified as body) → some garbage in
//     section detection that downstream rules tolerate
//   So the predicates are tuned for low false-positive rate, accepting
//   that boundary-edge non-body pages (TOC, references, title pages with
//   moderate fit) will slip through. Those are the next classifier's job.

use serde::{Deserialize, Serialize};

use crate::analytics::page_stats::{PageSignature, PageStats};

// ---------------------------------------------------------------------------
// Role kind
// ---------------------------------------------------------------------------

/// Per-page role assigned by the page-roles classifier.
///
/// MVP has two variants. Adding `Toc`, `References`, `Cover`, `Blank`
/// later is purely additive — same module, no rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageRoleKind {
    /// Default role — page is part of the document body. Section
    /// detection and other rules operate normally on these.
    Body,
    /// Page is outside the body extent — front matter (cover, TOC,
    /// blank fillers) or back matter (references, blank fillers).
    /// Section detection should skip these.
    NonBody,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Tunable thresholds for the page-roles classifier. Defaults validated
/// on the 6-PDF corpus.
#[derive(Debug, Clone)]
pub struct PageRolesConfig {
    /// Strict non-body threshold on `heatmap_fit`. A page fails the
    /// strict body test if `heatmap_fit < strict_fit`. Default: 0.15.
    /// Tuned to catch obvious blanks/cover pages without false-positiving
    /// body pages with sparse content (e.g., chapter ends, equation-heavy
    /// pages cluster at fit ≈ 0.20-0.40 in the corpus).
    pub strict_fit: f32,
    /// Trailing-sparseness exemption: the last page (page == End) is
    /// classified as non-body only if BOTH `heatmap_fit < strict_fit`
    /// AND `n_tokens < lenient_min_tokens`. Default: 100. Lets sparse-
    /// but-nonempty trailing body pages pass while still catching
    /// truly-empty trailing fillers.
    pub lenient_min_tokens: u32,
}

impl Default for PageRolesConfig {
    fn default() -> Self {
        Self {
            strict_fit: 0.15,
            lenient_min_tokens: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// Classifier
// ---------------------------------------------------------------------------

/// Classify each page's role and populate `body_start_page` /
/// `body_end_page` on `PageStats`. Mutates in place.
///
/// Idempotent: running twice with the same config produces the same
/// output (the second run reads from `pages[i].heatmap_fit` etc., not
/// from the role field it's about to overwrite).
pub fn classify_page_roles(stats: &mut PageStats, config: &PageRolesConfig) {
    let n = stats.pages.len();
    if n == 0 {
        stats.body_start_page = 0;
        stats.body_end_page = 0;
        return;
    }

    // Forward: walk from page 0 forward, skip strict-non-body, stop at
    // the first page that passes.
    let mut start_idx: Option<usize> = None;
    for i in 0..n {
        if !is_non_body_strict(&stats.pages[i], config) {
            start_idx = Some(i);
            break;
        }
    }

    let Some(body_start) = start_idx else {
        // No body page found at all — every page is non-body.
        for p in stats.pages.iter_mut() {
            p.role = Some(PageRoleKind::NonBody);
        }
        stats.body_start_page = 0;
        stats.body_end_page = 0;
        return;
    };

    // Backward: walk from page n-1 backward, skip non-body, stop at the
    // last page that passes. The very last position (n-1) gets the
    // lenient test; all others get strict.
    let mut body_end = body_start;
    for i in (body_start..n).rev() {
        let is_last_position = i == n - 1;
        let non_body = if is_last_position {
            is_non_body_lenient(&stats.pages[i], config)
        } else {
            is_non_body_strict(&stats.pages[i], config)
        };
        if !non_body {
            body_end = i;
            break;
        }
    }

    // Assign roles.
    for (i, p) in stats.pages.iter_mut().enumerate() {
        p.role = Some(if i >= body_start && i <= body_end {
            PageRoleKind::Body
        } else {
            PageRoleKind::NonBody
        });
    }

    // Translate body_start/body_end indices back to 1-indexed page numbers
    // (using the page_number field, which is the source of truth — pages
    // are not guaranteed to be 1..n contiguously, though in practice they
    // are).
    stats.body_start_page = stats.pages[body_start].page_number;
    stats.body_end_page = stats.pages[body_end].page_number;
}

// ---------------------------------------------------------------------------
// Predicates
// ---------------------------------------------------------------------------

/// Strict non-body test: `heatmap_fit < strict_fit`. Used for all
/// non-edge pages during the inward scan.
fn is_non_body_strict(page: &PageSignature, config: &PageRolesConfig) -> bool {
    page.heatmap_fit < config.strict_fit
}

/// Lenient non-body test: applies only to the very last page (page End).
/// `heatmap_fit < strict_fit` AND `n_tokens < lenient_min_tokens`.
/// Lets sparse-but-nonempty trailing body pages pass.
fn is_non_body_lenient(page: &PageSignature, config: &PageRolesConfig) -> bool {
    page.heatmap_fit < config.strict_fit && page.n_tokens < config.lenient_min_tokens
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::page_stats::PageSignature;

    fn mk_page(page_number: u32, heatmap_fit: f32, n_tokens: u32) -> PageSignature {
        PageSignature {
            page_number,
            n_tokens,
            italic_tokens: 0,
            bold_tokens: 0,
            normal_tokens: n_tokens,
            heatmap_fit,
            n_peaks_y: 0,
            y_peak_cv: 0.0,
            role: None,
        }
    }

    fn mk_stats(pages: Vec<PageSignature>) -> PageStats {
        PageStats {
            pages,
            regions: vec![],
            body_start_page: 0,
            body_end_page: 0,
        }
    }

    // -- 1. All-body single-page document -----------------------------------

    #[test]
    fn all_body_single_page() {
        let mut s = mk_stats(vec![mk_page(1, 0.85, 600)]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.body_end_page, 1);
        assert_eq!(s.pages[0].role, Some(PageRoleKind::Body));
    }

    // -- 2. All-body multi-page document ------------------------------------

    #[test]
    fn all_body_multi_page() {
        let mut s = mk_stats((1..=5).map(|p| mk_page(p, 0.80, 500)).collect());
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.body_end_page, 5);
        for p in &s.pages {
            assert_eq!(p.role, Some(PageRoleKind::Body));
        }
    }

    // -- 3. Cover page at the front -----------------------------------------

    #[test]
    fn cover_page_excluded_from_body_start() {
        let mut s = mk_stats(vec![
            mk_page(1, 0.05, 50), // cover — fails strict
            mk_page(2, 0.80, 500),
            mk_page(3, 0.80, 500),
            mk_page(4, 0.80, 500),
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 2);
        assert_eq!(s.body_end_page, 4);
        assert_eq!(s.pages[0].role, Some(PageRoleKind::NonBody));
        for p in &s.pages[1..] {
            assert_eq!(p.role, Some(PageRoleKind::Body));
        }
    }

    // -- 4. Multiple non-body pages at the front ----------------------------

    #[test]
    fn multiple_front_non_body_pages() {
        let mut s = mk_stats(vec![
            mk_page(1, 0.02, 20),  // cover
            mk_page(2, 0.10, 40),  // blank
            mk_page(3, 0.08, 80),  // TOC-blank
            mk_page(4, 0.80, 500), // body starts here
            mk_page(5, 0.80, 500),
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 4);
        assert_eq!(s.body_end_page, 5);
        for (i, p) in s.pages.iter().enumerate() {
            let expected = if i < 3 {
                PageRoleKind::NonBody
            } else {
                PageRoleKind::Body
            };
            assert_eq!(p.role, Some(expected), "page {}", p.page_number);
        }
    }

    // -- 5. Trailing sparse last page is body (lenient exemption) -----------

    #[test]
    fn trailing_sparse_last_page_is_body() {
        // Last page has fit < 0.15 but plenty of tokens — content runs
        // out mid-page. Should pass lenient test.
        let mut s = mk_stats(vec![
            mk_page(1, 0.80, 500),
            mk_page(2, 0.80, 500),
            mk_page(3, 0.10, 250), // sparse but nonempty — body via lenient
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_end_page, 3);
        assert_eq!(s.pages[2].role, Some(PageRoleKind::Body));
    }

    // -- 6. Trailing truly-empty last page is non-body ---------------------

    #[test]
    fn trailing_empty_last_page_is_non_body() {
        // Last page has fit < 0.15 AND n_tokens < 100 — fails lenient.
        let mut s = mk_stats(vec![
            mk_page(1, 0.80, 500),
            mk_page(2, 0.80, 500),
            mk_page(3, 0.05, 50), // truly empty — non-body even via lenient
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.body_end_page, 2);
        assert_eq!(s.pages[2].role, Some(PageRoleKind::NonBody));
    }

    // -- 7. Trailing chain: empty-empty-empty all caught -------------------

    #[test]
    fn trailing_chain_of_non_body() {
        // Multiple trailing non-body pages. Lenient applies to the LAST
        // position only; everything before that uses strict.
        let mut s = mk_stats(vec![
            mk_page(1, 0.80, 500),
            mk_page(2, 0.80, 500),
            mk_page(3, 0.10, 80), // strict-fail (would pass lenient but isn't last)
            mk_page(4, 0.10, 80), // strict-fail (would pass lenient but isn't last)
            mk_page(5, 0.05, 50), // also fails lenient
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.body_end_page, 2);
        assert_eq!(s.pages[2].role, Some(PageRoleKind::NonBody));
        assert_eq!(s.pages[3].role, Some(PageRoleKind::NonBody));
        assert_eq!(s.pages[4].role, Some(PageRoleKind::NonBody));
    }

    // -- 8. Strict applies to non-End back pages even with lots of tokens --

    #[test]
    fn strict_applies_to_non_end_back_pages() {
        // Page 3 has fit < 0.15 but tokens = 500. Since it's NOT the last
        // position, strict applies and it's non-body. Page 4 (last)
        // passes lenient.
        let mut s = mk_stats(vec![
            mk_page(1, 0.80, 500),
            mk_page(2, 0.80, 500),
            mk_page(3, 0.10, 500), // strict-fail (not last — strict applies)
            mk_page(4, 0.85, 600), // body
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.body_end_page, 4);
        // p3 is interior to the body extent → still labeled Body even
        // though it failed strict. The boundary scan only excludes pages
        // OUTSIDE the body range.
        assert_eq!(s.pages[2].role, Some(PageRoleKind::Body));
    }

    // -- 9. All pages non-body → body_start = body_end = 0 ------------------

    #[test]
    fn all_non_body_yields_zero_extent() {
        let mut s = mk_stats(vec![
            mk_page(1, 0.05, 30),
            mk_page(2, 0.08, 40),
            mk_page(3, 0.06, 50), // last; lenient still fails (tokens < 100)
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 0);
        assert_eq!(s.body_end_page, 0);
        for p in &s.pages {
            assert_eq!(p.role, Some(PageRoleKind::NonBody));
        }
    }

    // -- 10. Empty input → defaults preserved ------------------------------

    #[test]
    fn empty_input_zero_extent() {
        let mut s = mk_stats(vec![]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 0);
        assert_eq!(s.body_end_page, 0);
    }

    // -- 11. Idempotence: running twice produces identical output ----------

    #[test]
    fn idempotent() {
        let mut s = mk_stats(vec![
            mk_page(1, 0.05, 50),
            mk_page(2, 0.80, 500),
            mk_page(3, 0.80, 500),
            mk_page(4, 0.10, 200),
        ]);
        let cfg = PageRolesConfig::default();
        classify_page_roles(&mut s, &cfg);
        let snapshot: Vec<_> = s.pages.iter().map(|p| p.role).collect();
        let start1 = s.body_start_page;
        let end1 = s.body_end_page;
        classify_page_roles(&mut s, &cfg);
        assert_eq!(s.body_start_page, start1);
        assert_eq!(s.body_end_page, end1);
        for (i, p) in s.pages.iter().enumerate() {
            assert_eq!(p.role, snapshot[i]);
        }
    }

    // -- 12. Non-1-indexed page numbers still produce correct extent -------

    #[test]
    fn non_contiguous_page_numbers() {
        // PageRoleStats keys body_start/end on the actual page_number,
        // not the array index.
        let mut s = mk_stats(vec![
            mk_page(7, 0.05, 50),
            mk_page(8, 0.80, 500),
            mk_page(9, 0.80, 500),
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 8);
        assert_eq!(s.body_end_page, 9);
    }

    // -- 13. Front + back non-body symmetric ------------------------------

    #[test]
    fn front_and_back_non_body() {
        let mut s = mk_stats(vec![
            mk_page(1, 0.05, 50), // cover
            mk_page(2, 0.80, 500),
            mk_page(3, 0.80, 500),
            mk_page(4, 0.80, 500),
            mk_page(5, 0.05, 50), // blank trailer (also fails lenient)
        ]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 2);
        assert_eq!(s.body_end_page, 4);
        assert_eq!(s.pages[0].role, Some(PageRoleKind::NonBody));
        assert_eq!(s.pages[4].role, Some(PageRoleKind::NonBody));
    }

    // -- 14. Threshold boundary: fit == strict_fit is body -----------------

    #[test]
    fn fit_at_threshold_is_body() {
        // strict_fit default is 0.15; the predicate is `< 0.15`, so 0.15
        // exactly passes (is body).
        let mut s = mk_stats(vec![mk_page(1, 0.15, 500), mk_page(2, 0.80, 500)]);
        classify_page_roles(&mut s, &PageRolesConfig::default());
        assert_eq!(s.body_start_page, 1);
        assert_eq!(s.pages[0].role, Some(PageRoleKind::Body));
    }
}
