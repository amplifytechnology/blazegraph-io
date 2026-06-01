/// Section Detection V2 — V3 algorithm (Block 09) + CR-41 bookmark substrate.
///
/// Three-tier piecewise classifier on `delta = font_size - body_size`:
///
/// - `delta > structural_size_margin` → ACCEPT (R1: clearly larger than body)
/// - `delta > tolerance`              → bold OR isolated_in_leaf OR bookmark_match (R2: medium)
/// - `|delta| ≤ tolerance`            → bold AND (isolated_in_leaf OR bookmark_match) (R3: at body)
/// - `delta < -tolerance`             → REJECT (below-body noise)
///
/// Pre-gates (rotation rejection + alpha-ratio gate) run before the size
/// classification. Pattern-refinement (`apply_pattern_refinement`) applies
/// exclusion regex demotion + inclusion regex promotion (Article N / CHAPTER N
/// / etc.) as a backup for the size-based pipeline, including for
/// degenerate-font documents where Tika reports body_size = 1pt (CELEX).
///
/// Isolation is leaf-based — `is_isolated_in_leaf` consults the
/// `Placement.region_label` set by `analytics::reading_order::tag_and_resort`
/// (Block 06b). An element is *isolated* when no same-Y neighbor inside its
/// Region tree leaf has a different bold-ness. This catches the canonical
/// false positive (bold emphasis word inside a non-bold body line) at
/// near-zero cost: the leaf substrate is precomputed once per document.
///
/// **Bookmark substrate (CR-41).** The XHTML parser populates
/// `PdfTextElement.bookmark_match` when a span's normalized text exactly
/// equals a PDF outline title. This is an author-declared structural
/// signal that substitutes for `isolated_in_leaf` in R2/R3 when geometry
/// can't see the structure (e.g., a bold body-size heading sharing its
/// leaf with the trailing paragraph). TOC entries naturally do not match
/// because Tika fragments them across multiple spans; only the monolithic
/// body heading hits. PDFs without an outline see no behavior change
/// (`bookmark_match` is `None` everywhere).
///
/// V3 deltas vs the prior pipeline (locked 2026-05-07):
/// - Replaced X-cluster `is_isolated` (per-element O(n) page walk) with
///   `is_isolated_in_leaf` (per-element O(leaf_size) walk). The X-cluster
///   logic was a stand-in that pre-dated `region_label`.
/// - R2 relaxed from `bold AND isolated` to `bold OR isolated` — leaf-based
///   isolation is reliable enough that either signal alone suffices on
///   medium-tier candidates. This catches Computer-Modern / italic /
///   non-bold section titles that V2's AND missed.
/// - R1 threshold raised from `body + 1.5pt` to `body + 4pt` — at smaller
///   deltas the size signal is weaker and the auxiliary-signal requirement
///   in R2 keeps body-emphasis-at-slightly-larger-font from auto-promoting.
/// - Rarity gate dropped — was already dead in production
///   (`font_rarity_threshold: 0.00` made `is_rare_font` always false). The
///   V3 algorithm doesn't need it; isolation+bold cover the "structurally
///   distinctive" notion with cleaner semantics.
use super::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use crate::analytics::DocumentAnalysis;
use crate::config::{ParsingConfig, SectionDetectionV2Config};
use crate::types::*;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

/// Sb8 — leading multi-level numbering token ("3.5.2", "4.1"): at least one dot,
/// so single-level list markers ("1.", "2.") never match. Used by
/// `promote_numbered_line_seeds`.
static NUMBERED_SEED_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)+)").unwrap());

/// Minimum trailing-whitespace fraction (column right edge − line right edge,
/// over column width) for a numbered line to seed a section. A real header
/// occupies only part of the column and leaves whitespace to the right; a TOC
/// entry (title + dotted leader + page number) or a body/table line fills the
/// column. Measured corpus separation is wide: real headers leave ≥ 0.23 of
/// the column trailing, TOC lines ≈ 0.01 — so 0.15 keeps every real header
/// (incl. long deep ones) with margin while dropping full-width lines.
const MIN_HEADER_TRAILING_RATIO: f32 = 0.15;

/// Bold detection on a raw `FontClass`. CR-20: LaTeX PDFs encode bold in the
/// font-family name ("…-Medi", "CMBX10") rather than CSS font-weight, so check
/// both fields.
///
/// CR-75: the Linux Libertine/Biolinum families (the `libertine` LaTeX package,
/// common in arXiv preprints) also report `font-weight: normal` for every cut
/// and encode the weight purely in the family suffix — `…T` regular, `…TB`
/// bold, `…TI` italic, `…TZ` display. The `TB` cut is the bold one, so a family
/// ending in `tb` is bold. Without this, a whole Libertine-typeset paper has no
/// bold signal at all and every body-size header fails the R3 gate.
fn font_is_bold(style: &FontClass) -> bool {
    let weight = style.font_weight.to_lowercase();
    if weight.contains("bold") {
        return true;
    }
    let family = style.font_family.to_lowercase();
    family.contains("bold")
        || family.contains("medi")
        || family.contains("bx")
        || family.ends_with("tb")
}

// ──────────────────────────────────────────────────────────────────────────────
// HierarchyContext (CR-27: stack-based with keyword tiebreaker)
//
// Depth assignment uses font-size delta as the primary signal. When the delta
// is within tolerance (the "tie"), a tiebreaker keyword identifies the
// section's structural tier and the stack of `(keyword, depth)` anchors
// resolves whether the new section is a sibling, a step-back-up to an
// earlier tier, or a new deeper tier.
//
// `None` keyword (Pass-1 promotions with no pattern match) is a real keyword
// class for sibling comparisons, but is *skipped past* when a real-keyword
// section arrives — a Pass-1 subtitle should not become the parent of a
// keyword-bearing structural section that follows it.
// ──────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct HierarchyAnchor {
    keyword: Option<String>,
    depth: u32,
    font_size: f32,
}

#[derive(Debug, Clone, Default)]
struct HierarchyContext {
    stack: Vec<HierarchyAnchor>,
    previous_section_font_size: Option<f32>,
}

impl HierarchyContext {
    fn new() -> Self {
        Self::default()
    }

    fn current_depth(&self) -> u32 {
        self.stack.last().map_or(1, |a| a.depth)
    }

    fn cap_depth(depth: u32, config: &SectionDetectionV2Config) -> u32 {
        if config.enforce_max_depth {
            depth.min(config.max_depth)
        } else {
            depth
        }
    }

    /// Update context when we encounter a new section; returns the assigned depth.
    fn update_for_section(
        &mut self,
        font_size: f32,
        keyword: Option<&str>,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        let prev = self.previous_section_font_size;
        self.previous_section_font_size = Some(font_size);

        match prev {
            None => {
                let d = config.starting_section_level;
                self.stack.push(HierarchyAnchor {
                    keyword: keyword.map(String::from),
                    depth: d,
                    font_size,
                });
                d
            }
            Some(prev_font) => {
                let delta = font_size - prev_font;
                let tol = config.font_size_tolerance;

                if delta > tol {
                    // Larger font → step back up to a level with matching size, or
                    // collapse to a shallower level.
                    self.step_back_up(font_size, keyword, config)
                } else if delta < -tol {
                    // Smaller font → push deeper.
                    let d = Self::cap_depth(self.current_depth() + 1, config);
                    self.stack.push(HierarchyAnchor {
                        keyword: keyword.map(String::from),
                        depth: d,
                        font_size,
                    });
                    d
                } else {
                    // Tie — keyword decides.
                    self.resolve_tie(keyword, font_size, config)
                }
            }
        }
    }

    /// Resolve a section transition where font-delta is within tolerance.
    ///
    /// CR-27 algorithm:
    /// - If a keyword-bearing section arrives and the top of stack is a `None`
    ///   anchor, look past the `None` layer for the structural parent (so a
    ///   Pass-1 subtitle doesn't become the parent of a real keyword section).
    /// - If the incoming keyword equals the (effective) top's keyword → sibling.
    /// - If the incoming keyword exists deeper in the stack → step back up to
    ///   that anchor's depth.
    /// - Otherwise the tiebreaker fires → push a new anchor at depth + 1.
    fn resolve_tie(
        &mut self,
        keyword: Option<&str>,
        font_size: f32,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        // None-skip refinement: when a keyword-bearing section meets a None top,
        // the None anchor is transient and should be popped before resolution.
        if keyword.is_some()
            && self.stack.len() >= 2
            && self.stack.last().is_some_and(|a| a.keyword.is_none())
        {
            self.stack.pop();
        }

        let top_keyword = self.stack.last().and_then(|a| a.keyword.as_deref());

        // Same keyword as effective top → sibling. Replace top's font_size so
        // future deltas measure against the most recent section.
        if keyword == top_keyword {
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
            }
            return self.current_depth();
        }

        // Different keyword: look for it deeper in the stack — but only when the
        // incoming keyword is concrete. A `None` keyword should not step back
        // to an earlier `None` anchor (e.g., the document title), because two
        // unrelated subtitles with no keyword aren't structurally related.
        // None subtitles only ever match the *immediate* top, otherwise they
        // push deeper.
        if keyword.is_some() {
            if let Some(idx) = self
                .stack
                .iter()
                .rposition(|a| a.keyword.as_deref() == keyword)
            {
                self.stack.truncate(idx + 1);
                if let Some(top) = self.stack.last_mut() {
                    top.font_size = font_size;
                }
                return self.stack[idx].depth;
            }
        }

        // Tiebreaker fires — push deeper.
        let d = Self::cap_depth(self.current_depth() + 1, config);
        self.stack.push(HierarchyAnchor {
            keyword: keyword.map(String::from),
            depth: d,
            font_size,
        });
        d
    }

    /// Handle a transition where the incoming font is decisively larger than
    /// the previous section's font. Search strategy mirrors V1's `find_level_for`:
    ///
    /// 1. Exact font-size match in the stack → step back to that anchor.
    /// 2. Else, find the *shallowest* anchor whose font is smaller than the
    ///    incoming size — the incoming section is more important than that
    ///    tier, so it takes that depth and the smaller tier collapses
    ///    (anchors below the chosen depth are popped). This preserves
    ///    hierarchy continuity when font sizes don't exactly match: e.g., a
    ///    14pt heading following 24pt title and 12pt chapters lands at the
    ///    chapter tier rather than restarting at depth 1.
    /// 3. Else (incoming is larger than every anchor), treat as a new
    ///    top-level entry and reset the stack.
    fn step_back_up(
        &mut self,
        font_size: f32,
        keyword: Option<&str>,
        config: &SectionDetectionV2Config,
    ) -> u32 {
        let tol = config.font_size_tolerance;

        // 1. Exact match (rposition: prefer the deepest match for stability).
        if let Some(idx) = self
            .stack
            .iter()
            .rposition(|a| (a.font_size - font_size).abs() < tol)
        {
            self.stack.truncate(idx + 1);
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
                if top.keyword.is_none() {
                    top.keyword = keyword.map(String::from);
                }
            }
            return self.stack[idx].depth;
        }

        // 2. Fallback — first (shallowest) anchor with smaller font. The
        //    incoming section displaces that tier: truncate to it, replace
        //    its font and keyword.
        if let Some(idx) = self.stack.iter().position(|a| a.font_size < font_size) {
            let depth = self.stack[idx].depth;
            self.stack.truncate(idx + 1);
            if let Some(top) = self.stack.last_mut() {
                top.font_size = font_size;
                top.keyword = keyword.map(String::from);
            }
            return depth;
        }

        // 3. Larger than everything in stack — new top-level entry.
        self.stack.clear();
        let d = config.starting_section_level;
        self.stack.push(HierarchyAnchor {
            keyword: keyword.map(String::from),
            depth: d,
            font_size,
        });
        d
    }

    fn get_content_level(&self) -> u32 {
        self.current_depth() + 1
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Rule struct
// ──────────────────────────────────────────────────────────────────────────────

/// Compiled inclusion pattern with its per-pattern gate config. CR-42 —
/// `require_bold` / `require_isolation` flags ride alongside the regex so
/// `apply_pattern_refinement` can filter on a per-pattern basis (e.g., the
/// Section pattern requires bold to reject inline hyperlink spans, while a
/// hypothetical pattern targeting non-bold labels could opt out).
struct CompiledInclusionPattern {
    regex: Regex,
    require_bold: bool,
    require_isolation: bool,
}

pub struct SectionDetectionV2Rule<'a> {
    _engine: &'a RuleEngine,
    text_elements: &'a [PdfTextElement],
    config: &'a ParsingConfig,
    _document_analysis: &'a DocumentAnalysis,
    font_size_analysis: &'a FontSizeAnalysis,
    _style_data: &'a StyleData,
    /// Compiled inclusion patterns (promote weak/rejected candidates to sections)
    /// with per-pattern bold/isolation gates (CR-42).
    inclusion_patterns: Vec<CompiledInclusionPattern>,
    /// Compiled exclusion regexes (demote promoted candidates)
    exclusion_regexes: Vec<Regex>,
    /// Compiled tiebreaker patterns paired with their keyword names. CR-27 —
    /// consulted by `HierarchyContext` to resolve depth when font-delta is
    /// within tolerance.
    tiebreaker_regexes: Vec<(String, Regex)>,
}

impl<'a> SectionDetectionV2Rule<'a> {
    pub fn new(
        engine: &'a RuleEngine,
        text_elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_size_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
    ) -> Self {
        let v2 = &config.section_detection_v2;

        let inclusion_patterns = v2
            .inclusion_patterns
            .iter()
            .filter_map(|ip| {
                Regex::new(&ip.pattern)
                    .ok()
                    .map(|regex| CompiledInclusionPattern {
                        regex,
                        require_bold: ip.require_bold,
                        require_isolation: ip.require_isolation,
                    })
            })
            .collect();

        let exclusion_regexes = v2
            .exclusion_patterns
            .iter()
            .filter_map(|p| Regex::new(p).ok())
            .collect();

        let tiebreaker_regexes = v2
            .tiebreaker_keywords
            .iter()
            .filter_map(|tk| Regex::new(&tk.pattern).ok().map(|re| (tk.name.clone(), re)))
            .collect();

        Self {
            _engine: engine,
            text_elements,
            config,
            _document_analysis: document_analysis,
            font_size_analysis,
            _style_data: style_data,
            inclusion_patterns,
            exclusion_regexes,
            tiebreaker_regexes,
        }
    }

    /// Detect the structural-tier keyword for a section's text. Returns the
    /// name of the first matching tiebreaker pattern, or `None` when no
    /// pattern matches (Pass-1 promotions, generic bold subtitles, etc.).
    fn detect_keyword(&self, text: &str) -> Option<&str> {
        for (name, re) in &self.tiebreaker_regexes {
            if re.is_match(text) {
                return Some(name);
            }
        }
        None
    }

    // ──────────────────────────────────────────────────────────────────────
    // Signal helpers
    // ──────────────────────────────────────────────────────────────────────

    /// Check whether an element's text has a sufficient alpha ratio.
    fn passes_alpha_ratio(&self, text: &str) -> bool {
        let min = self.config.section_detection_v2.min_alpha_ratio;
        if min <= 0.0 {
            return true;
        }
        let non_ws: usize = text.chars().filter(|c| !c.is_whitespace()).count();
        if non_ws == 0 {
            return false;
        }
        let alpha: usize = text.chars().filter(|c| c.is_ascii_alphabetic()).count();
        (alpha as f32 / non_ws as f32) >= min
    }

    /// Check whether the element is bold.
    ///
    /// CR-20: LaTeX PDFs encode bold in the font-family name rather than CSS font-weight.
    /// Common patterns: "NimbusRomNo9L-Medi" (contains "medi"), "CMBX10" (contains "bx").
    /// We check both fields to catch both CSS-based and LaTeX-encoded bold.
    fn is_bold(element: &PdfTextElement) -> bool {
        font_is_bold(&element.style_info)
    }

    /// CR-41: Whether the XHTML parser matched this span's normalized text
    /// exactly against a PDF outline (bookmark) title. Acts as an alternative
    /// to the geometric `isolated_in_leaf` signal in R2/R3 — when the PDF
    /// author has explicitly named this span as a section target, the
    /// structural-atom signal can come from the outline rather than from
    /// the leaf's Y-line count. PDFs without a `BookmarkData` payload yield
    /// `false` for every element by construction.
    fn has_bookmark_match(element: &PdfTextElement) -> bool {
        element.bookmark_match.is_some()
    }

    /// Leaf-based isolation. Two gates compose the predicate:
    ///
    /// 1. **Same-line bold-mismatch** — any same-Y same-(page, leaf)
    ///    neighbor with different bold-ness disqualifies the candidate.
    ///    Catches bold-in-paragraph and symmetric anchor-with-continuation
    ///    cases. Already applied as a hard reject at the top of
    ///    `classify_pass1`; redundant here for defense-in-depth and for
    ///    direct callers (pattern-refinement, tests).
    ///
    /// 2. **Multi-line-leaf rejection** — if the (page, leaf) contains
    ///    ≥ 2 distinct Y-lines (regardless of fontsize), the leaf is a
    ///    flowing multi-line content block (body paragraph, abstract,
    ///    table caption, references entry) and the candidate is part of
    ///    the flow. Genuinely multi-line section titles still classify
    ///    via R1 (large-delta auto-promote) or R2's bold disjunct.
    ///
    /// The single-Y-line constraint is the structural-atom signature: a
    /// leaf produced by XY-cut that contains exactly one visual line is
    /// a header / caption / standalone label. Inline cross-references
    /// like "Section 6 of [QUIC-RECOVERY]" sit in multi-line body
    /// leaves and are correctly rejected — the inclusion-pattern path
    /// (which only ever runs on body-tier candidates) shares this gate.
    ///
    /// The leaf substrate (`Placement.region_label`) is set by
    /// `analytics::reading_order::tag_and_resort` (Block 06b). Elements
    /// with `region_label = None` don't reach this code in production —
    /// they are pre-classified as `Margin` at the `PdfTextElement →
    /// ParsedPdfElement` boundary (Block 07) and the `classify` early-
    /// return guard skips them. Defensive return: `false` for missing labels.
    fn is_isolated_in_leaf(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let Some(label) = element.placement.region_label.as_deref() else {
            return false;
        };

        let elem_page = element.page_number();
        let y = element.bounding_box().y;
        let tol = self.config.section_detection_v2.line_height_tolerance;
        let elem_bold = Self::is_bold(element);

        // Distinct Y-lines anywhere in the (page, leaf). Multi-line means
        // ≥ 2 → reject (the candidate is in flowing content, not a
        // structural one-line atom).
        let mut distinct_y_lines: Vec<f32> = Vec::new();

        for (idx, n) in self.text_elements.iter().enumerate() {
            // Region labels reset per page (XY-cut runs per page); filter
            // by `(page_number, region_label)` to avoid cross-page leaf
            // contamination — leaf "4-1" on page 3 is structurally
            // distinct from leaf "4-1" on page 5.
            if n.page_number() != elem_page || n.placement.region_label.as_deref() != Some(label) {
                continue;
            }

            let ny = n.bounding_box().y;

            // Same-Y same-leaf neighbor with different bold-ness → bold-in-
            // paragraph (bold word inside body, or non-bold continuation
            // next to a bold anchor). Already filtered as a hard reject in
            // `classify_pass1::has_same_y_bold_mismatch`; redundant here as
            // defense-in-depth so the predicate also holds when called
            // directly (pattern-refinement path, tests).
            if idx != element_idx && (ny - y).abs() < tol && Self::is_bold(n) != elem_bold {
                return false;
            }

            // Multi-line-leaf bookkeeping. Both strict and lax modes count
            // distinct Y-lines anywhere in the (page, leaf): a leaf with
            // ≥ 2 distinct Y-lines is a flowing content block. The
            // pattern-inclusion path (lax) needs this gate too — without
            // it, inline cross-references like "Section 6 of [...]"
            // matching `(?i)^section\s+\d+` get admitted simply because
            // their body neighbors don't have a different bold-ness, which
            // then mis-promotes the line via post-pass merge.
            if !distinct_y_lines.iter().any(|&yy| (yy - ny).abs() < tol) {
                distinct_y_lines.push(ny);
                if distinct_y_lines.len() >= 2 {
                    return false;
                }
            }
        }

        true
    }

    /// Hard-reject gate for bold-in-paragraph cases. Returns `true` when any
    /// same-Y same-(page, leaf) neighbor of the candidate has a different
    /// bold-ness. Catches:
    ///
    /// - **Bold word inside non-bold body line** (canonical false positive
    ///   in `**Then** under the section…` shape).
    /// - **Non-bold body next to a bold anchor** (the symmetric case where
    ///   the body fragment is the non-bold neighbor of a bold "Article 5"
    ///   structural anchor).
    ///
    /// Lifted out of `is_isolated_in_leaf` so it acts as a hard reject in
    /// `classify_pass1` *before* the size/bold/isolation tier logic. Without
    /// this lift, R2's `bold OR isolated` admits bold lead-ins like
    /// "Encoder:" via the bold disjunct even when isolation correctly
    /// returns false.
    fn has_same_y_bold_mismatch(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];
        let Some(label) = element.placement.region_label.as_deref() else {
            return false;
        };

        let elem_page = element.page_number();
        let y = element.bounding_box().y;
        let tol = self.config.section_detection_v2.line_height_tolerance;
        let elem_bold = Self::is_bold(element);

        for (idx, n) in self.text_elements.iter().enumerate() {
            if idx == element_idx
                || n.page_number() != elem_page
                || n.placement.region_label.as_deref() != Some(label)
            {
                continue;
            }
            if (n.bounding_box().y - y).abs() < tol && Self::is_bold(n) != elem_bold {
                return true;
            }
        }
        false
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pass 1: Candidate marking + decision tree
    // ──────────────────────────────────────────────────────────────────────

    /// Core classification: returns `true` if the element should be a section after Pass 1.
    ///
    /// Three-tier piecewise decision on `delta = font_size - body_size`:
    ///
    /// - `delta < -tolerance`            → REJECT (below-body noise)
    /// - `delta > structural_size_margin` → ACCEPT (R1: clearly larger than body)
    /// - `delta > tolerance`             → R2 (medium): bold OR isolated_in_leaf OR bookmark_match
    /// - `|delta| ≤ tolerance`           → R3 (at body): bold AND (isolated_in_leaf OR bookmark_match)
    ///
    /// R1 threshold is `body_size + structural_size_margin` by default
    /// (default 4pt — clearly above body); a `body_size * structural_size_ratio`
    /// override is honored when set, useful for documents with proportional
    /// type scales.
    ///
    /// CR-41: `bookmark_match` substitutes for `isolated_in_leaf` in R2/R3
    /// when the PDF outline names this span as a section target. Required
    /// at body-size R3 only as an alternative to isolation; bold is still
    /// required. PDFs without a bookmark outline see no behavior change.
    ///
    /// CR-43: `bookmark_match` also bypasses `passes_alpha_ratio`. The PDF
    /// outline is authoritative — when the author explicitly named this span
    /// as a section target, the generic alpha-ratio safety filter (intended
    /// to reject table data and number-heavy noise) loses authority. This
    /// closes the rfc-quic short-numbered-title miss class
    /// (`'10.1. Idle Timeout'`, `'19.2. PING Frames'`, …) where the
    /// digit-and-dot prefix tanks the ratio below the configured threshold.
    ///
    /// Rotated elements are always rejected upfront (matches the Block 02
    /// statistical filter).
    fn classify_pass1(&self, element_idx: usize) -> bool {
        let element = &self.text_elements[element_idx];

        if element.rotation() != 0 {
            return false;
        }

        // CR-64: Tika emits rotated text as per-glyph zero-width spans on
        // non-first pages (single-pass mode). Their projected vertical extent
        // is recorded as font_size and would otherwise trip the structural-
        // size gate. See DT-07.
        if element.bounding_box().width == 0.0 {
            return false;
        }

        // Hard reject: bold-in-paragraph case. A bold candidate sharing its
        // Y-line in its (page, leaf) with a non-bold neighbor (or vice
        // versa) is body emphasis, not a structural element. Lifted above
        // the size/bold/isolation logic so it overrides R2's `bold OR
        // isolated` disjunct.
        //
        // CR-75: `bookmark_match` overrides this veto, consistent with CR-41
        // (bookmark substitutes for isolation) and CR-43 (bookmark overrides
        // the alpha-ratio gate) — the PDF outline naming a span as a section
        // target is authoritative. Without the override, line-numbered drafts
        // (e.g. FDA guidance) lose every header: the non-bold margin line-
        // number shares the header's Y-line in the same leaf, so the genuine
        // bold + bookmarked header reads as a same-Y bold mismatch and dies.
        if !Self::has_bookmark_match(element) && self.has_same_y_bold_mismatch(element_idx) {
            return false;
        }

        let cfg = &self.config.section_detection_v2;
        let body_size = self.font_size_analysis.body_text_size;
        let font_size = element.style_info.font_size;
        let tolerance = cfg.font_size_tolerance;
        let delta = font_size - body_size;

        if delta < -tolerance {
            return false;
        }

        // CR-43 — bookmark_match overrides the alpha-ratio gate. The PDF outline
        // is authoritative; the alpha-ratio filter is a heuristic safeguard.
        if !Self::has_bookmark_match(element) && !self.passes_alpha_ratio(&element.text) {
            return false;
        }

        let region1_threshold = match cfg.structural_size_ratio {
            Some(ratio) => body_size * ratio,
            None => body_size + cfg.structural_size_margin,
        };

        if font_size > region1_threshold {
            return true;
        }

        let bold = Self::is_bold(element);
        let isolated = self.is_isolated_in_leaf(element_idx);
        let bookmark_promoted = Self::has_bookmark_match(element);

        if delta > tolerance {
            // R2 (medium): bold, isolation, OR bookmark match (CR-41).
            bold || isolated || bookmark_promoted
        } else {
            // R3 (at-body band): bold required; isolation OR bookmark
            // match (CR-41) supplies the structural-atom signal.
            bold && (isolated || bookmark_promoted)
        }
    }

    // ──────────────────────────────────────────────────────────────────────
    // Pass 2: Pattern refinement
    // ──────────────────────────────────────────────────────────────────────

    /// Apply exclusion patterns (demote) then inclusion patterns (promote).
    ///
    /// Inclusion matches additionally require, for each pattern:
    /// 1. text length ≤ `inclusion_max_length` — global synthetic length gate
    ///    that filters body wrap-lines beginning with a structural keyword.
    ///    Pass 1 protects body text via bold/rarity gates that depend on a
    ///    meaningful font_size signal; on documents where Tika reports
    ///    degenerate font sizes (CELEX/EU regulation embedded fonts), Pass 2
    ///    has neither bold nor size to lean on, so length is the discriminator
    ///    — real labels are short, body wraps are long.
    /// 2. `is_bold(element)` if the pattern declares `require_bold: true`
    ///    (CR-42). Closes the rfc-quic FP class where inline hyperlink spans
    ///    (`<span class="f4" style="color:#2222ee">Section 18</span>`)
    ///    matched the `^section\s+\d+` pattern but should never have promoted.
    /// 3. `is_isolated_in_leaf(element_idx)` if the pattern declares
    ///    `require_isolation: true` — element sits alone on its visual line
    ///    (not a paragraph-internal reference like "as described in Article 5
    ///    of this Regulation").
    ///
    /// Patterns ship as written in config; per-pattern case sensitivity is
    /// controlled by `(?i)` in the YAML.
    fn apply_pattern_refinement(
        &self,
        pass1_is_section: bool,
        element_idx: usize,
        text: &str,
    ) -> bool {
        let mut result = pass1_is_section;

        // Exclusion first: demote even if Pass 1 promoted
        if result {
            for re in &self.exclusion_regexes {
                if re.is_match(text) {
                    result = false;
                    break;
                }
            }
        }

        // Inclusion: promote only when text matches AND the per-pattern gates
        // pass AND text length is within the structural-label cap. Char count,
        // not byte length — labels are ASCII in practice but we count chars to
        // stay predictable for any future Latin/Roman/numeric variants.
        if !result {
            let max_len = self.config.section_detection_v2.inclusion_max_length;
            if text.chars().count() <= max_len {
                let element = &self.text_elements[element_idx];
                for rule in &self.inclusion_patterns {
                    if !rule.regex.is_match(text) {
                        continue;
                    }
                    if rule.require_bold && !Self::is_bold(element) {
                        continue;
                    }
                    if rule.require_isolation && !self.is_isolated_in_leaf(element_idx) {
                        continue;
                    }
                    result = true;
                    break;
                }
            }
        }

        result
    }

    // ──────────────────────────────────────────────────────────────────────
    // Per-element classification
    // ──────────────────────────────────────────────────────────────────────

    fn classify(
        &self,
        element_idx: usize,
        hierarchy_context: &mut HierarchyContext,
        current_element: &ParsedPdfElement,
    ) -> (ParsedElementType, u32) {
        // Header / Footer / Margin elements are pre-classified at the
        // PdfTextElement → ParsedPdfElement boundary from
        // `placement.region_label` (Block 07). Skip section detection on them
        // so running headers, footers, and out-of-region marginalia never get
        // promoted to Section.
        if matches!(
            current_element.element_type,
            ParsedElementType::Header | ParsedElementType::Footer | ParsedElementType::Margin
        ) {
            let content_level = hierarchy_context.get_content_level();
            return (current_element.element_type.clone(), content_level);
        }

        let element = &self.text_elements[element_idx];

        // Pass 1
        let pass1 = self.classify_pass1(element_idx);

        // Pass 2
        let is_section = self.apply_pattern_refinement(pass1, element_idx, &element.text);

        if is_section {
            let font_size = element.style_info.font_size;
            let keyword = self.detect_keyword(&element.text);
            let level = hierarchy_context.update_for_section(
                font_size,
                keyword,
                &self.config.section_detection_v2,
            );
            (ParsedElementType::Section, level)
        } else {
            let content_level = hierarchy_context.get_content_level();
            (current_element.element_type.clone(), content_level)
        }
    }

    /// Promote same-Y same-(page, leaf) sibling fragments to Section when
    /// any one of them already classified as Section, **but only in
    /// single-Y-line leaves**. Catches Tika fragmentation of structural
    /// header lines (canonical example: "3.1" + "Encoder and Decoder
    /// Stacks" — the number-only prefix fails `passes_alpha_ratio`, the
    /// title passes, and the post-pass merges them) without mistakenly
    /// promoting body fragments adjacent to inline cross-references like
    /// "Section 6 of [QUIC-RECOVERY]".
    ///
    /// The single-Y-line gate is the structural-atom signature: a leaf
    /// produced by XY-cut that contains exactly one visual line is a
    /// header / caption / standalone label. Multi-line leaves are flowing
    /// content — even if some fragment in them classifies as Section
    /// (e.g. an inline "Section N" cross-reference matched by the
    /// inclusion pattern), the surrounding fragments are body content and
    /// must not be promoted.
    fn promote_same_line_section_fragments(out: &mut [ParsedPdfElement]) {
        use std::collections::HashMap;

        const Y_TOL: f32 = 3.0;

        // First: collect per-(page, leaf) the distinct Y-lines and the
        // Section-bearing Y-line (if any) plus its hierarchy level.
        struct LeafInfo {
            y_lines: Vec<f32>,
            section_y: Option<(f32, u32)>,
        }
        let mut leaves: HashMap<(u32, String), LeafInfo> = HashMap::new();

        for el in out.iter() {
            let Some(p) = el.placement.as_ref() else {
                continue;
            };
            let Some(label) = p.region_label.as_deref() else {
                continue;
            };
            let key = (p.page_number, label.to_string());
            let info = leaves.entry(key).or_insert(LeafInfo {
                y_lines: Vec::new(),
                section_y: None,
            });
            let y = p.bounding_box.y;
            if !info.y_lines.iter().any(|&yy| (yy - y).abs() < Y_TOL) {
                info.y_lines.push(y);
            }
            if el.element_type == ParsedElementType::Section && info.section_y.is_none() {
                info.section_y = Some((y, el.hierarchy_level));
            }
        }

        // Second pass: promote within single-Y-line leaves that contain a
        // Section. The single-Y constraint is the structural-atom check.
        for el in out.iter_mut() {
            if el.element_type == ParsedElementType::Section {
                continue;
            }
            let Some(p) = el.placement.as_ref() else {
                continue;
            };
            let Some(label) = p.region_label.as_deref() else {
                continue;
            };
            let key = (p.page_number, label.to_string());
            let Some(info) = leaves.get(&key) else {
                continue;
            };
            if info.y_lines.len() != 1 {
                continue;
            }
            let Some((sy, level)) = info.section_y else {
                continue;
            };
            let y = p.bounding_box.y;
            if (sy - y).abs() < Y_TOL {
                el.element_type = ParsedElementType::Section;
                el.hierarchy_level = level;
            }
        }
    }

    /// CR-67 Part B — Promote section-fragment neighbors.
    ///
    /// For each Section, examine source-order ±1 neighbors. Promote a
    /// non-Section neighbor to Section iff:
    ///   (a) `neighbor.style_info.class_name == section.style_info.class_name`
    ///   (b) `|neighbor.bbox.height - section.bbox.height| < HEIGHT_TOL` (= 2.0)
    ///
    /// NodeTypeClustering downstream fuses adjacent same-type elements —
    /// a promoted `"**1.1.**"` Paragraph plus its existing
    /// `"**Document Structure**"` Section neighbor coalesce into a single
    /// `"**1.1. Document Structure**"` Section node.
    ///
    /// The height gate is load-bearing. A naive same-class rule would
    /// promote cluster-bloated Paragraphs (e.g. an `h=256` merged-title-
    /// and-body blob next to an `h=10.5` true Section). After
    /// NodeTypeClustering fused those, CR-65 (`graph_sanity` height
    /// bound) would demote the resulting 266pt-tall "Section" back to
    /// Paragraph — losing both the true Section AND any other Section
    /// that got fused with the bloat. The 2pt tolerance keeps promotion
    /// to true single-line same-class fragments only.
    pub fn promote_section_fragments(elements: &mut [ParsedPdfElement]) {
        const HEIGHT_TOL: f32 = 2.0;

        // Collect indices of Section elements (snapshot before mutation so
        // we don't cascade-promote through a chain in a single pass).
        let section_indices: Vec<usize> = elements
            .iter()
            .enumerate()
            .filter(|(_, el)| el.element_type == ParsedElementType::Section)
            .map(|(i, _)| i)
            .collect();

        // Collect (neighbor_idx, hierarchy_level) to promote. Defer the
        // mutation until after the scan so a section at index N can't
        // promote its left neighbor and then have that newly-Section
        // neighbor re-trigger promotion of index N-2 in the same pass.
        let mut to_promote: Vec<(usize, u32)> = Vec::new();

        for &si in &section_indices {
            let section = &elements[si];
            let Some(s_place) = section.placement.as_ref() else {
                continue;
            };
            let s_class = &section.style_info.class_name;
            let s_height = s_place.bounding_box.height;
            let s_level = section.hierarchy_level;

            for &offset in &[-1i32, 1i32] {
                let ni = match (si as i32).checked_add(offset) {
                    Some(n) if n >= 0 && (n as usize) < elements.len() => n as usize,
                    _ => continue,
                };
                let neighbor = &elements[ni];
                if neighbor.element_type == ParsedElementType::Section {
                    continue;
                }
                // Header / Footer / Margin are pre-classified from region
                // labels — never overwrite them. (Aligns with the
                // `classify` early-out at the top of this module.)
                if matches!(
                    neighbor.element_type,
                    ParsedElementType::Header
                        | ParsedElementType::Footer
                        | ParsedElementType::Margin
                ) {
                    continue;
                }
                if &neighbor.style_info.class_name != s_class {
                    continue;
                }
                let Some(n_place) = neighbor.placement.as_ref() else {
                    continue;
                };
                let n_height = n_place.bounding_box.height;
                if (n_height - s_height).abs() >= HEIGHT_TOL {
                    continue;
                }
                to_promote.push((ni, s_level));
            }
        }

        for (idx, level) in to_promote {
            // Re-check: a neighbor adjacent to two sections gets queued
            // twice; the first promotion makes the second a no-op. Also
            // honor the snapshot: don't overwrite a Section that was
            // already there (defensive — section_indices excluded these).
            if elements[idx].element_type != ParsedElementType::Section {
                elements[idx].element_type = ParsedElementType::Section;
                elements[idx].hierarchy_level = level;
            }
        }
    }

    /// Sb8 — Numbered subsection seed promotion.
    ///
    /// Deep RFC-style subsection headers ("3.5.2. Reset Generation") are
    /// typeset at *body* font size, distinguished only by bold weight, and
    /// Tika splits the number into its own span. They fail the size tiers
    /// (delta ≈ 0 → R3), R3's isolation gate (the one-line header shares an
    /// XY-cut leaf with the body paragraph below it), and bookmark-match (the
    /// number-only span doesn't equal the full outline title). Result: depth-3+
    /// subsections detect at < 20% while depth-1/2 detect at ~100%.
    ///
    /// This seeds them off the one signal the leaf can't corrupt: a **bold,
    /// line-leading multi-level-numbering segment** ("N.M[.K…]"). Every bold
    /// segment on such a line is promoted to Section at the numbering depth;
    /// `promote_section_fragments` + NodeTypeClustering then fuse the number and
    /// title into one node. Anchoring on the visual line `(page, Y)` rather than
    /// the leaf sidesteps the isolation failure by construction — the body
    /// paragraph is a different Y-line and is never swept in.
    ///
    /// Guards keep it strictly additive and precise:
    /// - fires only on a line with **no** existing Section (never disturbs a
    ///   size/bookmark-detected header or its assigned depth);
    /// - the leading segment must be **bold** — excludes TOC entries (normal
    ///   weight) and inline cross-references (not line-leading);
    /// - the line must carry an alphabetic title (a bare number is not a header).
    fn promote_numbered_line_seeds(out: &mut [ParsedPdfElement]) {
        use std::collections::HashMap;
        const Y_TOL: f32 = 3.0;

        // Group element indices by visual line: (page, quantized-Y), and learn
        // each page's content box (left/right text edge) for the trailing-
        // whitespace filter. Skip furniture (Header/Footer/Margin).
        let mut lines: HashMap<(u32, i64), Vec<usize>> = HashMap::new();
        let mut page_right: HashMap<u32, f32> = HashMap::new();
        let mut page_left: HashMap<u32, f32> = HashMap::new();
        for (i, el) in out.iter().enumerate() {
            if matches!(
                el.element_type,
                ParsedElementType::Header | ParsedElementType::Footer | ParsedElementType::Margin
            ) {
                continue;
            }
            let Some(p) = el.placement.as_ref() else {
                continue;
            };
            let right = p.bounding_box.x + p.bounding_box.width;
            page_right
                .entry(p.page_number)
                .and_modify(|v| *v = v.max(right))
                .or_insert(right);
            page_left
                .entry(p.page_number)
                .and_modify(|v| *v = v.min(p.bounding_box.x))
                .or_insert(p.bounding_box.x);
            let ybucket = (p.bounding_box.y / Y_TOL).round() as i64;
            lines.entry((p.page_number, ybucket)).or_default().push(i);
        }

        let mut to_promote: Vec<(usize, u32)> = Vec::new();
        for idxs in lines.values() {
            // Purely additive: never touch a line that already has a Section.
            if idxs
                .iter()
                .any(|&i| out[i].element_type == ParsedElementType::Section)
            {
                continue;
            }
            // Leftmost segment on the line.
            let Some(&lead) = idxs.iter().min_by(|&&a, &&b| {
                let xa = out[a].placement.as_ref().map_or(f32::MAX, |p| p.bounding_box.x);
                let xb = out[b].placement.as_ref().map_or(f32::MAX, |p| p.bounding_box.x);
                xa.partial_cmp(&xb).unwrap_or(std::cmp::Ordering::Equal)
            }) else {
                continue;
            };

            // Leading segment must be bold and start with a multi-level number.
            if !font_is_bold(&out[lead].style_info) {
                continue;
            }
            let Some(m) = NUMBERED_SEED_REGEX.find(out[lead].text.trim_start()) else {
                continue;
            };
            // Numbering depth = component count ("3.5.2" → 3). Provisional —
            // CR-70 rebalance recomputes depth from numbering; this only needs
            // to be consistent across the line so the fragments share a bucket
            // in the `same_depth`-keyed Section clustering merge.
            let level = m.as_str().matches('.').count() as u32 + 1;

            // The line must carry a *bold* alphabetic title, not just a bare
            // number. Only bold segments get promoted, so a bold number beside a
            // non-bold title (NIST AI-RMF subcategory rows: bold "1.4:" + normal
            // description) would otherwise leave a titleless "1.4:" section.
            let has_bold_title = idxs.iter().any(|&i| {
                font_is_bold(&out[i].style_info)
                    && out[i].text.chars().filter(|c| c.is_alphabetic()).count() >= 2
            });
            if !has_bold_title {
                continue;
            }

            // Geometric TOC/body filter: a real header leaves whitespace to the
            // right of the column; a TOC entry (leader + page number) or a
            // body/table line fills it. Require a minimum trailing-whitespace
            // fraction of the column width.
            let page = out[lead].placement.as_ref().map_or(0, |p| p.page_number);
            let line_right = idxs
                .iter()
                .map(|&i| {
                    out[i]
                        .placement
                        .as_ref()
                        .map_or(f32::MIN, |p| p.bounding_box.x + p.bounding_box.width)
                })
                .fold(f32::MIN, f32::max);
            let (Some(&right), Some(&left)) = (page_right.get(&page), page_left.get(&page)) else {
                continue;
            };
            let width = right - left;
            if width <= 0.0 || (right - line_right) / width < MIN_HEADER_TRAILING_RATIO {
                continue;
            }

            // Promote every bold segment on the line (number + title fragments).
            for &i in idxs {
                if font_is_bold(&out[i].style_info) {
                    to_promote.push((i, level));
                }
            }
        }

        for (i, level) in to_promote {
            if out[i].element_type != ParsedElementType::Section {
                out[i].element_type = ParsedElementType::Section;
                out[i].hierarchy_level = level;
            }
        }
    }
}

impl<'a> ParseRule for SectionDetectionV2Rule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        println!(
            "📝 [V2] Applying section detection V2 to {} elements...",
            elements.len()
        );

        // If no elements provided, bootstrap from text_elements (same as V1)
        let input_elements = if elements.is_empty() {
            println!("   📋 [V2] No input elements — bootstrapping from text_elements");
            self.text_elements
                .iter()
                .enumerate()
                .map(|(i, te)| ParsedPdfElement {
                    element_type: ParsedElementType::from_region_label(
                        te.placement.region_label.as_deref(),
                    ),
                    text: te.text.clone(),
                    hierarchy_level: 3,
                    position: i,
                    style_info: te.style_info.clone(),
                    placement: Some(te.placement.clone()),
                    reading_order: te.reading_order,
                    bookmark_match: te.bookmark_match.clone(),
                    token_count: te.token_count,
                    links: vec![],
                })
                .collect()
        } else {
            elements
        };

        let mut hierarchy_context = HierarchyContext::new();
        let mut out = Vec::with_capacity(input_elements.len());

        for element in input_elements {
            if let Some(_te) = self.text_elements.get(element.position) {
                let (new_type, new_level) =
                    self.classify(element.position, &mut hierarchy_context, &element);
                out.push(ParsedPdfElement {
                    element_type: new_type,
                    hierarchy_level: new_level,
                    ..element
                });
            } else {
                // No corresponding text element — preserve with content level
                let content_level = hierarchy_context.get_content_level();
                out.push(ParsedPdfElement {
                    hierarchy_level: content_level,
                    ..element
                });
            }
        }

        // Sb8 — seed deep numbered subsections (RFC X.Y.Z) before the fragment
        // passes, so the same-line / source-adjacent promotion + clustering fuse
        // the number and title that the size/isolation/bookmark gates all miss.
        if self.config.section_detection_v2.numbered_seed_promotion {
            Self::promote_numbered_line_seeds(&mut out);
        }

        // Same-Y same-(page, leaf) section promotion. Tika fragments a
        // single visual line into multiple PdfTextElements (e.g., the
        // section heading "3.1 Encoder and Decoder Stacks" emits two
        // fragments at the same Y: "3.1" and "Encoder and Decoder Stacks").
        // Per-element gates can reject one fragment for contingent reasons
        // (the number prefix "3.1" fails `passes_alpha_ratio`) while
        // accepting its sibling. The fragments share a structural identity:
        // if one is a Section, all of them are. Promote them together.
        Self::promote_same_line_section_fragments(&mut out);

        // CR-67 Part B — source-order ±1 fragment promotion. The same-Y
        // pass above only fires for fragments sharing a single Y line
        // within one leaf; numbered RFC headers fragmented across spans
        // (e.g. `**1.1.**` at one Y followed by `**Document Structure**`
        // at a slightly different Y) miss it. This walk uses source-order
        // adjacency + same font-class + matching bbox-height as the
        // signature, then lets NodeTypeClustering downstream fuse the
        // adjacent same-type elements into a single Section node.
        Self::promote_section_fragments(&mut out);

        let sections = out
            .iter()
            .filter(|e| e.element_type == ParsedElementType::Section)
            .count();
        println!(
            "   ✅ [V2] Detected {} sections across {} elements",
            sections,
            out.len()
        );
        Ok(out)
    }

    fn name(&self) -> &str {
        "SectionDetectionV2"
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Unit tests — CR-19 four-region piecewise classification
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::DocumentAnalysis;
    use crate::config::{InclusionPattern, ParsingConfig, SectionDetectionV2Config};
    use crate::rules::engine::FontSizeAnalysis;
    use crate::types::{BoundingBox, FontClass, PdfTextElement, Placement, StyleData};
    use std::collections::{BTreeMap, HashMap};

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn make_placement(line_number: u32, rotation: i32) -> Placement {
        Placement {
            page_number: 1,
            bounding_box: BoundingBox {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 12.0,
            },
            line_number,
            segment_number: 0,
            rotation,
            paragraph_number: 0,
            // Default fixture leaf — body content. Tests that exercise
            // bold-in-paragraph or multi-leaf isolation override this.
            region_label: Some("1".to_string()),
            page_width: 0.0,
            page_height: 0.0,
        }
    }

    fn make_element(
        font_size: f32,
        bold: bool,
        class_name: &str,
        line_number: u32,
    ) -> PdfTextElement {
        let font_weight = if bold {
            "bold".to_string()
        } else {
            "normal".to_string()
        };
        PdfTextElement {
            text: "Introduction".to_string(),
            style_info: FontClass {
                class_name: class_name.to_string(),
                font_family: "TestFont".to_string(),
                font_size,
                font_style: "normal".to_string(),
                font_weight,
                color: "#000000".to_string(),
            },
            placement: make_placement(line_number, 0),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,            raw_tags: vec![],
            link: None,
        }
    }

    /// Non-bold neighbour element sharing the same Y, same leaf as index 0.
    /// V3 isolation reads "different bold-ness on the same Y in the same leaf
    /// → not isolated", so a non-bold neighbour next to a bold candidate is
    /// the canonical bold-in-paragraph reject case.
    fn make_neighbour(line_number: u32) -> PdfTextElement {
        PdfTextElement {
            text: "neighbour".to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x: 110.0,
                    y: 0.0,
                    width: 50.0,
                    height: 12.0,
                },
                line_number,
                segment_number: 1,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 1,
            bookmark_match: None,
            token_count: 1,            raw_tags: vec![],
            link: None,
        }
    }

    fn make_font_analysis(body_size: f32, class_counts: Vec<(&str, usize)>) -> FontSizeAnalysis {
        let mut class_usage_counts = HashMap::new();
        for (name, count) in &class_counts {
            class_usage_counts.insert(name.to_string(), *count);
        }
        FontSizeAnalysis {
            body_text_size: body_size,
            class_usage_counts,
            ..FontSizeAnalysis::default()
        }
    }

    fn make_config(tolerance: f32, margin: f32, ratio: Option<f32>) -> ParsingConfig {
        ParsingConfig {
            section_detection_v2: SectionDetectionV2Config {
                font_size_tolerance: tolerance,
                structural_size_margin: margin,
                structural_size_ratio: ratio,
                line_height_tolerance: 3.0,
                ..SectionDetectionV2Config::default()
            },
            ..ParsingConfig::default()
        }
    }

    fn make_style_data() -> StyleData {
        StyleData {
            font_classes: BTreeMap::new(),
        }
    }

    fn make_document_analysis() -> DocumentAnalysis {
        // Rules do not read DocumentAnalysis fields in tests (the parameter is
        // threaded through but unused by V2 — see `_document_analysis` on the
        // rule struct). Default is sufficient.
        DocumentAnalysis::default()
    }

    /// Build a `SectionDetectionV2Rule` and call `classify_pass1` on index 0.
    fn classify(
        elements: &[PdfTextElement],
        font_analysis: &FontSizeAnalysis,
        config: &ParsingConfig,
    ) -> bool {
        let engine = RuleEngine::new().expect("engine");
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let rule = SectionDetectionV2Rule::new(
            &engine,
            elements,
            config,
            &document_analysis,
            font_analysis,
            &style_data,
        );
        rule.classify_pass1(0)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    /// Test 1 — Region 1: auto-promotes without any confirming signals.
    /// delta = 16 - 10 = 6 > margin (5) → Region 1 → true
    #[test]
    fn test_region1_auto_promotes_without_signals() {
        let body = 10.0;
        // element: 16pt, not bold, common font, isolated (no neighbours → line_number 0 unique)
        let elements = vec![make_element(16.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 2 — R1 boundary: delta == margin is R2 (not R1).
    /// delta = 15 - 10 = 5 = margin → `font_size > body + margin` is `15 > 15` = false → R2.
    /// To make R2 reject we need `bold OR isolated_in_leaf` to be false. Use a
    /// non-bold candidate whose same-Y same-leaf neighbour IS bold → leaf
    /// isolation flips to false (different bold-ness on the same line). Both
    /// signals false → R2 rejects.
    #[test]
    fn test_region1_boundary_is_region2() {
        let body = 10.0;
        let mut neighbour = make_neighbour(5);
        neighbour.style_info.font_weight = "bold".to_string();
        let elements = vec![make_element(15.0, false, "body", 5), neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 3 — Region 2: bold alone promotes.
    /// delta = 13 - 10 = 3, tolerance = 1, margin = 5 → 1 < 3 < 5 → Region 2
    /// bold = true → promoted
    #[test]
    fn test_region2_bold_alone_promotes() {
        let body = 10.0;
        let elements = vec![make_element(13.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 4 — R2: isolated alone promotes (V3 OR semantics).
    /// delta = 13 - 10 = 3 → R2; non-bold, alone in leaf → isolated → promoted.
    /// Captures the V3 relaxation (`bold OR isolated`) that lets non-bold but
    /// structurally-isolated medium-tier headers (academic italic / Computer-
    /// Modern section titles) classify cleanly.
    #[test]
    fn test_region2_isolated_alone_promotes() {
        let body = 10.0;
        let elements = vec![make_element(13.0, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// Test 5 — R2: neither bold nor isolated rejects.
    /// delta = 3 → R2; non-bold candidate with a bold same-Y same-leaf
    /// neighbour → leaf isolation flips to false → both signals false → reject.
    /// Captures the bold-anchor-with-non-bold-body case (e.g. bold "Article 5"
    /// with non-bold continuation "of this Regulation").
    #[test]
    fn test_region2_neither_signal_rejects() {
        let body = 10.0;
        let mut neighbour = make_neighbour(7);
        neighbour.style_info.font_weight = "bold".to_string();
        let elements = vec![make_element(13.0, false, "body", 7), neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 6 — Region 3: isolated AND bold promotes.
    /// |delta| = |10.5 - 10| = 0.5 ≤ tolerance 1 → Region 3
    /// isolated (unique line), bold → promoted
    #[test]
    fn test_region3_isolated_and_bold_promotes() {
        let body = 10.0;
        let elements = vec![make_element(10.5, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    // (V2 Test 7 — `isolated AND rare` — removed in V3: rarity gate dropped.)

    /// Test 8 — R3: isolated alone does NOT promote.
    /// |delta| ≤ tolerance, isolated, not bold, common font → NOT promoted
    #[test]
    fn test_region3_isolated_alone_does_not_promote() {
        let body = 10.0;
        // "body" = 90/100 = 90% → not rare
        let elements = vec![make_element(10.5, false, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 90), ("other", 10)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 9 — Region 3: non-isolated bold does NOT promote.
    /// |delta| ≤ tolerance, bold, with a same-Y X-clustered neighbour whose
    /// joined extent fills the page → ratio = 1.0 ≥ isolation_threshold →
    /// NOT isolated → NOT promoted.
    ///
    /// Neighbour is placed at gap=4pt (well below `X_CLUSTER_GAP_THRESHOLD_PT=6`)
    /// so the two elements form a single visual cluster, which is what the
    /// "non-isolated" semantic requires post-strip.
    #[test]
    fn test_region3_non_isolated_bold_does_not_promote() {
        let body = 10.0;
        let mut element = make_element(10.5, true, "body", 3);
        element.placement.bounding_box.width = 160.0;
        let mut neighbour = make_neighbour(3);
        neighbour.placement.bounding_box.x = 164.0; // gap = 4pt → same cluster
        neighbour.placement.bounding_box.width = 36.0;
        let elements = vec![element, neighbour];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 10 — Reject: below body − tolerance.
    /// delta = 8 - 10 = -2 < -tolerance (-1) → REJECT regardless of signals
    #[test]
    fn test_reject_below_body_minus_tolerance() {
        let body = 10.0;
        let elements = vec![make_element(8.0, true, "body", 0)];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// CR-64 — Zero-width-bbox elements rejected at pass-1.
    ///
    /// Tika emits rotated text on non-first pages as per-glyph zero-width
    /// spans (e.g. a vertical "Parameters" y-axis label arrives as ten
    /// single-letter spans at width=0.0, font_size 26pt — the projected
    /// vertical extent). Without the guard those would be auto-promoted as
    /// Region 1 candidates because 26 > body+margin.
    #[test]
    fn test_cr64_zero_width_bbox_is_rejected() {
        let body = 10.0;
        // Element that WOULD be Region 1 (big font) — but width=0 disqualifies.
        let mut element = make_element(26.0, false, "body", 0);
        element.placement.bounding_box.width = 0.0;
        let elements = vec![element];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    /// Test 11 — Proportional mode: structural_size_ratio overrides margin.
    /// ratio = Some(1.5), body = 10 → threshold = 15.
    /// 12pt: delta = 2 > tolerance 1, < threshold 15 → Region 2; bold + isolated → promoted.
    /// 16pt: font_size 16 > threshold 15 → Region 1 → promoted.
    #[test]
    fn test_proportional_ratio_overrides_margin() {
        let body = 10.0;
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        // margin=0.1 so body+margin=10.1; ratio=Some(1.5) → threshold=15
        // isolation_neighbor_gap = 20; elements at unique lines → isolated
        let config = make_config(1.0, 0.1, Some(1.5));

        // 12pt: delta=2 > tolerance 1 → Region 2, isolated, bold → promoted (both signals)
        let elements_12 = vec![make_element(12.0, true, "body", 0)];
        assert!(classify(&elements_12, &font_analysis, &config));

        // 16pt: font_size 16 > threshold 15 → Region 1 → promoted
        let elements_16 = vec![make_element(16.0, false, "body", 0)];
        assert!(classify(&elements_16, &font_analysis, &config));
    }

    /// Test 12 — arXiv watermark regression (V3 path).
    /// In production, arxiv-style sidebar / watermark content is rotated and
    /// excluded from `body_element_indices` by `tag_and_resort` → its
    /// `region_label` stays `None` → element_type becomes `Margin` at the
    /// conversion boundary → the Block 07 classify guard skips section
    /// detection entirely. As a defense, `classify_pass1` itself rejects
    /// rotated elements upfront. This test pins that defense.
    #[test]
    fn test_arxiv_watermark_regression() {
        let body = 7.0;
        let mut element = make_element(11.0, true, "body", 0);
        element.placement.rotation = 90;
        let elements = vec![element];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    // ── CR-43 tests — bookmark_match bypasses alpha_ratio ─────────────────────

    /// CR-43 — `bookmark_match` overrides the alpha-ratio safety filter.
    ///
    /// Text `'10.1. Idle Timeout'` is 18 chars / 11 alpha → ratio 0.611, which
    /// fails `min_alpha_ratio = 0.7`. Pre-CR-43, `classify_pass1` rejects at
    /// the alpha-ratio gate and CR-41's R3 disjunction never gets to fire.
    /// With CR-43 the bookmark substrate bypasses the gate and R3
    /// (`bold AND (isolated OR bookmark_match)`) admits it.
    ///
    /// Reproduces the rfc-quic page-57 miss class: short numbered titles like
    /// `'10.1. Idle Timeout'`, `'19.2. PING Frames'`, `'17.2.4. Handshake
    /// Packet'` whose digit-and-dot prefix tanks alpha ratio below 0.7.
    #[test]
    fn test_cr43_bookmark_match_bypasses_alpha_ratio() {
        let body = 12.0;
        let mut element = make_element(body, true, "body", 0);
        element.text = "10.1. Idle Timeout".to_string();
        element.bookmark_match = Some(BookmarkSection {
            title: "10.1. Idle Timeout".to_string(),
            order: 75,
            level: 3,
        });

        let elements = vec![element];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let mut config = make_config(0.1, 4.0, None);
        // Mirror production threshold (config.yaml). Default in code is 0.5,
        // which would not exercise the bypass.
        config.section_detection_v2.min_alpha_ratio = 0.7;

        // Sanity: alpha ratio is below the gate (test fixture invariant).
        let alpha = "10.1. Idle Timeout"
            .chars()
            .filter(|c| c.is_alphabetic())
            .count();
        let total = "10.1. Idle Timeout".chars().count();
        assert!(
            (alpha as f32) / (total as f32) < 0.7,
            "fixture must fail alpha-ratio gate"
        );

        assert!(classify(&elements, &font_analysis, &config));
    }

    /// CR-43 paired control — without `bookmark_match`, the alpha-ratio gate
    /// still rejects. Confirms the bypass keys on the bookmark substrate, not
    /// on something else (font size, bold, isolation).
    #[test]
    fn test_cr43_alpha_ratio_still_rejects_without_bookmark() {
        let body = 12.0;
        let mut element = make_element(body, true, "body", 0);
        element.text = "10.1. Idle Timeout".to_string();
        // bookmark_match: None (default from make_element)

        let elements = vec![element];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let mut config = make_config(0.1, 4.0, None);
        config.section_detection_v2.min_alpha_ratio = 0.7;

        assert!(!classify(&elements, &font_analysis, &config));
    }

    // ── CR-75 tests — bookmark bypasses the same-Y bold-mismatch veto ──────────

    /// CR-75: a bold + bookmark-matched header sharing its Y-line in the same
    /// leaf with a non-bold neighbour (the line-number gutter of a line-numbered
    /// draft) still promotes. Without the bypass, `has_same_y_bold_mismatch`
    /// hard-rejects it before R3 — the FDA-guidance failure mode.
    #[test]
    fn test_cr75_bookmark_bypasses_same_y_bold_mismatch() {
        let body = 12.0;
        // Body-size bold header (delta 0 → R3) with an authoritative bookmark.
        let mut header = make_element(body, true, "body", 0);
        header.bookmark_match = Some(BookmarkSection {
            title: "Introduction".to_string(),
            order: 0,
            level: 1,
        });
        // Non-bold gutter line-number sharing the header's Y in the same leaf.
        let gutter = make_neighbour(0);

        let elements = vec![header, gutter];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 4.0, None);
        assert!(classify(&elements, &font_analysis, &config));
    }

    /// CR-75 paired control — without `bookmark_match`, the same-Y bold-mismatch
    /// veto still rejects. Confirms the bypass keys on the bookmark substrate,
    /// preserving the original bold-in-paragraph guard.
    #[test]
    fn test_cr75_same_y_mismatch_still_rejects_without_bookmark() {
        let body = 12.0;
        let header = make_element(body, true, "body", 0); // bold, no bookmark
        let gutter = make_neighbour(0);

        let elements = vec![header, gutter];
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 4.0, None);
        assert!(!classify(&elements, &font_analysis, &config));
    }

    // ── CR-20 tests — bold detection font-family fallback ─────────────────────

    /// Helper: build a PdfTextElement with explicit font_weight and font_family strings.
    fn make_element_with_font(
        font_size: f32,
        font_weight: &str,
        font_family: &str,
        class_name: &str,
        line_number: u32,
    ) -> PdfTextElement {
        PdfTextElement {
            text: "Introduction".to_string(),
            style_info: FontClass {
                class_name: class_name.to_string(),
                font_family: font_family.to_string(),
                font_size,
                font_style: "normal".to_string(),
                font_weight: font_weight.to_string(),
                color: "#000000".to_string(),
            },
            placement: make_placement(line_number, 0),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,            raw_tags: vec![],
            link: None,
        }
    }

    /// CR-20 Test 1 — Regression: font_weight="bold" still detected as bold.
    #[test]
    fn test_bold_detected_via_font_weight() {
        let element = make_element_with_font(12.0, "bold", "ArialMT", "h1", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 2 — LaTeX "Medi" family detected as bold (NimbusRomNo9L-Medi).
    #[test]
    fn test_bold_detected_via_medi_family() {
        let element = make_element_with_font(12.0, "normal", "NimbusRomNo9L-Medi", "latex-medi", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 3 — LaTeX "bx" family detected as bold (CMBX10).
    #[test]
    fn test_bold_detected_via_bx_family() {
        let element = make_element_with_font(12.0, "normal", "CMBX10", "latex-bx", 0);
        assert!(SectionDetectionV2Rule::is_bold(&element));
    }

    /// CR-20 Test 4 — Regular fonts are not bold: NimbusRomNo9L-Regu and CMR10.
    #[test]
    fn test_regular_font_not_bold() {
        let regu = make_element_with_font(10.0, "normal", "NimbusRomNo9L-Regu", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&regu));

        let cmr = make_element_with_font(10.0, "normal", "CMR10", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&cmr));
    }

    /// CR-75 — Linux Libertine/Biolinum bold cut (`…TB`) detected as bold even
    /// though Tika reports `font-weight: normal` for it (the crispr-review /
    /// arXiv-libertine failure mode).
    #[test]
    fn test_cr75_libertine_biolinum_tb_is_bold() {
        let biolinum = make_element_with_font(14.0, "normal", "LinBiolinumTB", "h1", 0);
        assert!(SectionDetectionV2Rule::is_bold(&biolinum));

        let libertine = make_element_with_font(9.0, "normal", "LinLibertineTB", "h2", 0);
        assert!(SectionDetectionV2Rule::is_bold(&libertine));
    }

    /// CR-75 paired control — the regular (`…T`) and italic (`…TI`) cuts of the
    /// same families are NOT bold, so the `TB` suffix rule stays surgical.
    #[test]
    fn test_cr75_libertine_regular_and_italic_not_bold() {
        let regular = make_element_with_font(9.0, "normal", "LinLibertineT", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&regular));

        let italic = make_element_with_font(9.0, "normal", "LinLibertineTI", "body", 0);
        assert!(!SectionDetectionV2Rule::is_bold(&italic));
    }

    // ── V3 leaf-based isolation tests ─────────────────────────────────────────

    /// Build a leaf-tagged element at the given (x, y, leaf, bold). Width
    /// fixed at 80pt for predictable Y-line geometry across the leaf-isolation
    /// fixtures.
    fn make_leaf_element(x: f32, y: f32, leaf: &str, bold: bool) -> PdfTextElement {
        let weight = if bold { "bold" } else { "normal" };
        PdfTextElement {
            text: "fragment".to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: weight.to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x,
                    y,
                    width: 80.0,
                    height: 12.0,
                },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some(leaf.to_string()),
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,            raw_tags: vec![],
            link: None,
        }
    }

    fn make_leaf_rule<'a>(
        engine: &'a RuleEngine,
        elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
    ) -> SectionDetectionV2Rule<'a> {
        SectionDetectionV2Rule::new(
            engine,
            elements,
            config,
            document_analysis,
            font_analysis,
            style_data,
        )
    }

    /// V3 Test 1 — single element in a leaf has no same-Y neighbors → isolated.
    #[test]
    fn block09_leaf_singleton_is_isolated() {
        let elements = vec![make_leaf_element(0.0, 0.0, "1", true)];
        let fa = make_font_analysis(10.0, vec![("body", 1)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 2 — bold candidate + non-bold same-Y same-leaf neighbor →
    /// not isolated. Canonical bold-emphasis-in-paragraph case.
    #[test]
    fn block09_leaf_bold_with_nonbold_same_line_not_isolated() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),   // bold candidate
            make_leaf_element(90.0, 0.0, "1", false), // non-bold body neighbor
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 3 — multi-line leaf rejects isolation regardless of
    /// fontsize. The (page, leaf) substrate identifies a flowing content
    /// block; the candidate is part of the flow. Real bold headers in
    /// big multi-line leaves still classify via R2's bold disjunct or R1
    /// auto-promotion. Inline cross-references (e.g. `Section 6 of [...]`)
    /// in body leaves get correctly rejected too — same gate.
    #[test]
    fn block09_leaf_multi_line_rejects_isolation() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", false),
            make_leaf_element(0.0, 50.0, "1", false),
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 3b — multi-line leaf with mixed fontsizes rejects
    /// isolation. Body content is the canonical case: a math-prose mix
    /// where a fragment at +1pt above body is alone at its size on its
    /// line, but the leaf has body content at body size on other lines.
    #[test]
    fn block09_leaf_multi_line_mixed_size_rejects() {
        let mut larger = make_leaf_element(0.0, 0.0, "1", false);
        larger.style_info.font_size = 11.0;
        let body = make_leaf_element(0.0, 50.0, "1", false);
        let elements = vec![larger, body];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 4 — multi-fragment all-bold same-Y same-leaf → isolated.
    /// A bold title that Tika emits as multiple spans on the same line is
    /// still one structural unit.
    #[test]
    fn block09_leaf_all_bold_multi_fragment_is_isolated() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),
            make_leaf_element(90.0, 0.0, "1", true),
            make_leaf_element(180.0, 0.0, "1", true),
        ];
        let fa = make_font_analysis(10.0, vec![("body", 3)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 5 — different leaves on the same Y → isolated. The XY-cut
    /// produced two separate leaves at this Y (e.g., left and right column
    /// of a multi-column page). Cross-leaf neighbors don't disqualify.
    #[test]
    fn block09_leaf_cross_leaf_same_y_does_not_disqualify() {
        let elements = vec![
            make_leaf_element(0.0, 0.0, "1", true),    // leaf 1
            make_leaf_element(300.0, 0.0, "2", false), // leaf 2, same Y
        ];
        let fa = make_font_analysis(10.0, vec![("body", 2)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(rule.is_isolated_in_leaf(0));
    }

    /// V3 Test 6 — element with no `region_label` (orphan) → not isolated.
    /// In production, Margin elements are skipped at `classify` entry by
    /// the Block 07 guard, so this code path doesn't fire there. Defensive
    /// behavior: orphans never claim isolation.
    #[test]
    fn block09_leaf_missing_label_returns_false() {
        let mut e = make_leaf_element(0.0, 0.0, "1", true);
        e.placement.region_label = None;
        let elements = vec![e];
        let fa = make_font_analysis(10.0, vec![("body", 1)]);
        let cfg = make_config(0.1, 4.0, None);
        let da = make_document_analysis();
        let sd = make_style_data();
        let eng = RuleEngine::new().expect("engine");
        let rule = make_leaf_rule(&eng, &elements, &cfg, &da, &fa, &sd);
        assert!(!rule.is_isolated_in_leaf(0));
    }

    // ── CR-26 tests — isolation-gated inclusion patterns ──────────────────────

    /// Build a body-size, non-bold, non-rotated element at the given x/width on
    /// page=1 / band=0 / col=0 / y=0. Used to construct isolation scenarios where
    /// the inclusion-pattern path is the only route to promotion.
    fn make_inclusion_element(text: &str, x: f32, width: f32) -> PdfTextElement {
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "body".to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x,
                    y: 0.0,
                    width,
                    height: 12.0,
                },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                // Body leaf — the inclusion-pattern fixtures probe pattern + isolation
                // semantics; co-leaf neighbours of differing bold-ness are how
                // `is_isolated_in_leaf` distinguishes structural labels from inline.
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,            raw_tags: vec![],
            link: None,
        }
    }

    /// Build a configured rule with custom inclusion/exclusion pattern lists.
    /// All other config values match `make_config(1.0, 5.0, None)`.
    ///
    /// Inclusion patterns wrap with CR-26-era gates (isolation-only, no bold
    /// requirement) so existing CR-26 tests using non-bold fixtures keep
    /// passing. CR-42 tests that exercise per-pattern gates use
    /// `build_pattern_rule_test_with_gates` instead.
    fn build_pattern_rule_test(
        elements: &[PdfTextElement],
        inclusion: Vec<&str>,
        exclusion: Vec<&str>,
    ) -> (
        ParsingConfig,
        FontSizeAnalysis,
        DocumentAnalysis,
        StyleData,
        RuleEngine,
    ) {
        let inclusion_patterns: Vec<InclusionPattern> = inclusion
            .into_iter()
            .map(|p| InclusionPattern {
                pattern: p.to_string(),
                require_bold: false,
                require_isolation: true,
            })
            .collect();
        build_pattern_rule_test_with_gates(elements, inclusion_patterns, exclusion)
    }

    /// CR-42 — variant that takes pre-built `InclusionPattern`s so per-pattern
    /// `require_bold` / `require_isolation` gates can be exercised in tests.
    fn build_pattern_rule_test_with_gates(
        elements: &[PdfTextElement],
        inclusion_patterns: Vec<InclusionPattern>,
        exclusion: Vec<&str>,
    ) -> (
        ParsingConfig,
        FontSizeAnalysis,
        DocumentAnalysis,
        StyleData,
        RuleEngine,
    ) {
        let mut config = make_config(1.0, 5.0, None);
        config.section_detection_v2.inclusion_patterns = inclusion_patterns;
        config.section_detection_v2.exclusion_patterns =
            exclusion.into_iter().map(String::from).collect();
        let font_analysis = make_font_analysis(10.0, vec![("body", elements.len().max(1))]);
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let engine = RuleEngine::new().expect("engine");
        (config, font_analysis, document_analysis, style_data, engine)
    }

    /// CR-26 Test 1 — Inclusion match on an isolated element promotes.
    /// Element alone on its visual line (no same-Y neighbours) → isolated=true.
    #[test]
    fn test_cr26_inclusion_isolated_promotes() {
        let elements = vec![make_inclusion_element("Article 5", 100.0, 80.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(rule.apply_pattern_refinement(false, 0, "Article 5"));
    }

    /// CR-26 Test 2 — Inclusion match next to a different-bold-ness same-Y
    /// same-leaf neighbour does NOT promote. Under V3 leaf isolation, the
    /// presence of any same-line same-leaf neighbour with mismatched bold-ness
    /// flips `is_isolated_in_leaf` to false — which gates the inclusion-pattern
    /// path. Captures the bold-anchor-with-non-bold-body case (e.g., bold
    /// "Article 5" anchor + non-bold continuation "of this Regulation").
    #[test]
    fn test_cr26_inclusion_paragraph_internal_does_not_promote() {
        let mut anchor = make_inclusion_element("Article 5", 100.0, 80.0);
        anchor.style_info.font_weight = "bold".to_string();
        let continuation =
            make_inclusion_element("of this Regulation, the framework", 185.0, 195.0);
        let elements = vec![anchor, continuation];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // line_extent ≈ 400, column_width ≈ 400 → ratio ≈ 1.0 ≥ 0.80 → not isolated
        assert!(!rule.apply_pattern_refinement(false, 0, "as described in Article 5 of"));
    }

    /// CR-26 Test 3 — `(?i)` flag honoured per-pattern.
    /// `(?i)chapter` matches `CHAPTER II`; bare `chapter` does not.
    #[test]
    fn test_cr26_case_insensitive_flag_honoured() {
        let elements = vec![make_inclusion_element("CHAPTER II", 100.0, 80.0)];

        // (?i)chapter → matches CHAPTER II → isolated → promote
        let (config_ci, fa_ci, da_ci, sd_ci, eng_ci) =
            build_pattern_rule_test(&elements, vec!["(?i)chapter"], vec![]);
        let rule_ci =
            SectionDetectionV2Rule::new(&eng_ci, &elements, &config_ci, &da_ci, &fa_ci, &sd_ci);
        assert!(rule_ci.apply_pattern_refinement(false, 0, "CHAPTER II"));

        // bare lowercase `chapter` does NOT match CHAPTER II → no promote
        let (config_cs, fa_cs, da_cs, sd_cs, eng_cs) =
            build_pattern_rule_test(&elements, vec!["chapter"], vec![]);
        let rule_cs =
            SectionDetectionV2Rule::new(&eng_cs, &elements, &config_cs, &da_cs, &fa_cs, &sd_cs);
        assert!(!rule_cs.apply_pattern_refinement(false, 0, "CHAPTER II"));
    }

    /// CR-26 Test 4 — Exclusion still demotes Pass 1-promoted candidates.
    /// Verifies the exclusion path is unaffected by the isolation-gate change on
    /// the inclusion path.
    #[test]
    fn test_cr26_exclusion_unaffected_by_isolation_gate() {
        let elements = vec![make_inclusion_element(
            "Figure 3: Architecture",
            100.0,
            80.0,
        )];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec![], vec!["^Figure\\s"]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // Pass 1 = true (simulating Region 1 promotion); exclusion match → demoted
        assert!(!rule.apply_pattern_refinement(true, 0, "Figure 3: Architecture"));
    }

    /// CR-73 follow-up — tightened caption exclusion. `^Table\s+[A-Z0-9]`
    /// requires a label token after "Table", so real captions ("Table 1:",
    /// "Table B-1:") demote while "Table of Contents" (lowercase "of", a genuine
    /// outline heading and a bookmark TP on RFCs) survives.
    #[test]
    fn caption_exclusion_spares_table_of_contents() {
        let elements = vec![make_inclusion_element("Table of Contents", 100.0, 80.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec![], vec!["^Table\\s+[A-Z0-9]"]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(!rule.apply_pattern_refinement(true, 0, "Table 1: Results"), "numbered caption demoted");
        assert!(!rule.apply_pattern_refinement(true, 0, "Table B-1: Summary"), "lettered caption demoted");
        assert!(rule.apply_pattern_refinement(true, 0, "Table of Contents"), "TOC heading survives");
    }

    /// CR-26 Test 5 — Length cap rejects long body wrap-lines.
    /// Body wrap-line begins with `Article` and is structurally isolated (single
    /// span, no same-Y neighbours), but its length exceeds `inclusion_max_length`.
    /// The synthetic length gate is what protects Pass 2 on documents like CELEX
    /// where Tika reports degenerate font sizes and Pass 1 cannot disambiguate.
    #[test]
    fn test_cr26_length_cap_rejects_body_wrap() {
        let long_wrap = "Article 17, including ICT network performance issues and ICT-related";
        assert!(long_wrap.chars().count() > 30, "fixture must exceed cap");
        let elements = vec![make_inclusion_element(long_wrap, 100.0, 400.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)^article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // Pattern matches, element is isolated, but length > 30 → not promoted
        assert!(!rule.apply_pattern_refinement(false, 0, long_wrap));
    }

    /// CR-26 Test 6 — Length cap admits short structural labels.
    /// Companion to Test 5: confirms the cap doesn't accidentally reject the
    /// real labels we're trying to catch.
    #[test]
    fn test_cr26_length_cap_admits_short_label() {
        let label = "Article 64";
        assert!(label.chars().count() <= 30, "fixture must be within cap");
        let elements = vec![make_inclusion_element(label, 100.0, 80.0)];
        let (config, fa, da, sd, eng) =
            build_pattern_rule_test(&elements, vec!["(?i)^article"], vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(rule.apply_pattern_refinement(false, 0, label));
    }

    // ── CR-42 tests — per-pattern bold/isolation gating ──────────────────────

    /// CR-42 Test — `require_bold: true` rejects the inline-hyperlink-span FP.
    ///
    /// Reproduces the rfc-quic page-38 false positive: an inline hyperlink span
    /// `<span class="f4" style="color:#2222ee">Section 18</span>` (normal weight)
    /// matches the `^section\s+\d+` inclusion pattern, sits in its own tiny
    /// XY-cut leaf (so `is_isolated_in_leaf` returns true), and has length 10
    /// (well under `inclusion_max_length: 20`). Pre-CR-42 this promoted, then
    /// same-line clustering joined the surrounding spans into a Section node.
    /// With `require_bold: true` on the pattern, the gate rejects.
    ///
    /// Paired with the next test which confirms `require_bold: true` still
    /// admits a real bold label of the same shape (e.g., a UK-Acts "Section 12").
    #[test]
    fn test_cr42_require_bold_rejects_hyperlink_span() {
        // Normal-weight hyperlink span — matches pattern, is isolated, fails bold gate
        let mut hyperlink = make_inclusion_element("Section 18", 200.0, 60.0);
        hyperlink.style_info.color = "#2222ee".to_string();
        let elements = vec![hyperlink];

        let inclusion = vec![InclusionPattern {
            pattern: r"(?i)^section\s+\d+[a-z]?\s*$".to_string(),
            require_bold: true,
            require_isolation: true,
        }];

        let (config, fa, da, sd, eng) =
            build_pattern_rule_test_with_gates(&elements, inclusion, vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(!rule.apply_pattern_refinement(false, 0, "Section 18"));
    }

    /// CR-42 Test (paired) — `require_bold: true` still admits a bold label.
    ///
    /// Same shape as the FP test but `font_weight: "bold"` — confirms the gate
    /// doesn't accidentally reject real UK-Acts-of-Parliament bold "Section 12"
    /// labels (the canonical use case for this pattern).
    #[test]
    fn test_cr42_require_bold_admits_bold_label() {
        let mut label = make_inclusion_element("Section 12", 100.0, 60.0);
        label.style_info.font_weight = "bold".to_string();
        let elements = vec![label];

        let inclusion = vec![InclusionPattern {
            pattern: r"(?i)^section\s+\d+[a-z]?\s*$".to_string(),
            require_bold: true,
            require_isolation: true,
        }];

        let (config, fa, da, sd, eng) =
            build_pattern_rule_test_with_gates(&elements, inclusion, vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        assert!(rule.apply_pattern_refinement(false, 0, "Section 12"));
    }

    /// CR-42 Test — `require_isolation: false` admits a non-isolated label.
    ///
    /// Confirms per-pattern gates compose orthogonally: a pattern that opts out
    /// of isolation can still promote a same-line-as-other-text label as long
    /// as the bold gate (or any future gate) is satisfied. This is the
    /// generality lever — the shape that lets future non-canonical-label
    /// patterns ride on this infra without retrofitting code.
    #[test]
    fn test_cr42_require_isolation_false_admits_non_isolated() {
        // Two same-Y bold elements in the same leaf — `is_isolated_in_leaf` returns
        // false for either alone, so a `require_isolation: true` pattern would reject.
        let mut anchor = make_inclusion_element("Article 5", 100.0, 80.0);
        anchor.style_info.font_weight = "bold".to_string();
        let mut neighbour = make_inclusion_element("of this Regulation", 200.0, 200.0);
        neighbour.style_info.font_weight = "bold".to_string();
        let elements = vec![anchor, neighbour];

        let inclusion = vec![InclusionPattern {
            pattern: r"(?i)^article\s+\d+[a-z]?\s*$".to_string(),
            require_bold: true,
            require_isolation: false,
        }];

        let (config, fa, da, sd, eng) =
            build_pattern_rule_test_with_gates(&elements, inclusion, vec![]);
        let rule = SectionDetectionV2Rule::new(&eng, &elements, &config, &da, &fa, &sd);
        // is_isolated_in_leaf would be false here, but the pattern doesn't require it.
        assert!(rule.apply_pattern_refinement(false, 0, "Article 5"));
    }

    // ── CR-27 tests — keyword tiebreaker hierarchy ───────────────────────────

    /// Default config for hierarchy unit tests: tolerance=0.1, margin=5.0,
    /// no proportional ratio, isolation_threshold default.
    fn cr27_config() -> SectionDetectionV2Config {
        SectionDetectionV2Config {
            font_size_tolerance: 0.1,
            structural_size_margin: 5.0,
            structural_size_ratio: None,
            max_depth: 6,
            enforce_max_depth: true,
            starting_section_level: 1,
            ..SectionDetectionV2Config::default()
        }
    }

    /// CR-27 Test 1 — same keyword at equal font is sibling.
    #[test]
    fn test_cr27_same_keyword_is_sibling() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d1 = ctx.update_for_section(12.0, Some("part"), &cfg);
        let d2 = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d1, d2, "two PARTs at equal font must be siblings");
    }

    /// CR-27 Test 2 — different keyword at equal font fires tiebreaker (deeper).
    #[test]
    fn test_cr27_different_keyword_fires_tiebreaker() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        let d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        assert_eq!(d_chap, d_part + 1, "CHAPTER must be one deeper than PART");
    }

    /// CR-27 Test 3 — keyword reappearance steps back up.
    #[test]
    fn test_cr27_keyword_reappearance_steps_back_up() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_part1 = ctx.update_for_section(12.0, Some("part"), &cfg);
        let _d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        let _d_num = ctx.update_for_section(12.0, Some("numbered"), &cfg);
        let d_part2 = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d_part2, d_part1, "PART 2 must return to PART 1's depth");
    }

    /// CR-27 Test 4 — `None` ↔ `None` is sibling.
    #[test]
    fn test_cr27_none_is_sibling_with_none() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d1 = ctx.update_for_section(12.0, None, &cfg);
        let d2 = ctx.update_for_section(12.0, None, &cfg);
        assert_eq!(
            d1, d2,
            "two None-keyword sections at equal font must be siblings"
        );
    }

    /// CR-27 Test 5 — `None` after a keyword fires tiebreaker (deeper).
    /// Pass-1 subtitle following a CHAPTER becomes a child of the CHAPTER.
    #[test]
    fn test_cr27_none_after_keyword_goes_deeper() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_chap = ctx.update_for_section(12.0, Some("chapter"), &cfg);
        let d_subtitle = ctx.update_for_section(12.0, None, &cfg);
        assert_eq!(
            d_subtitle,
            d_chap + 1,
            "subtitle must be one deeper than CHAPTER"
        );
    }

    /// CR-27 Test 6 — font-delta still wins when decisive (skips the tiebreaker).
    #[test]
    fn test_cr27_font_delta_wins_when_decisive() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();
        let d_title = ctx.update_for_section(24.0, None, &cfg);
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(
            d_part,
            d_title + 1,
            "decisive font-delta still pushes deeper"
        );
    }

    /// CR-27 Test 7 — full Police Act PART/CHAPTER/numbered sequence.
    /// Title → PART 1 → numbered → numbered → PART 2 → CHAPTER 1 → numbered.
    /// Expected depths: 1, 2, 3, 3, 2, 3, 4.
    #[test]
    fn test_cr27_police_act_sequence() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();

        // Title at 24pt
        assert_eq!(ctx.update_for_section(24.0, None, &cfg), 1);
        // PART 1 at 12pt — font-delta decisive (smaller), pushed deeper to 2
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 2);
        // numbered at 12pt — tie, different keyword, not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 3);
        // sibling numbered → still 3
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 3);
        // PART 2 at 12pt — tie, keyword 'part' in stack at depth 2 → step back → 2
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 2);
        // CHAPTER 1 at 12pt — tie, 'chapter' not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(12.0, Some("chapter"), &cfg), 3);
        // numbered at 12pt — tie, 'numbered' was popped during PART 2 step-back
        //   → not currently in stack → tiebreaker → 4
        assert_eq!(ctx.update_for_section(12.0, Some("numbered"), &cfg), 4);
    }

    /// CR-27 Test 8 — CELEX cascade with all-equal-font sections.
    /// Title (None) → CHAPTER → subtitle (None) → Article → article-title (None).
    /// Expected: 1, 2, 3, 3, 4. The subtitle is a None layer that gets skipped
    /// past when Article arrives, so Article sits at chapter+1 = 3 alongside
    /// (not under) the subtitle.
    #[test]
    fn test_cr27_celex_cascade() {
        let mut cfg = cr27_config();
        // CELEX has font_size = 1.0 everywhere; lower margin so Title→CHAPTER
        // doesn't trigger the decisive-delta path.
        cfg.structural_size_margin = 5.0;
        let mut ctx = HierarchyContext::new();

        // Title — first section, no prior, depth = starting_level
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 1);
        // CHAPTER I — tie (delta=0), keyword chapter ≠ None → tiebreaker → 2
        assert_eq!(ctx.update_for_section(1.0, Some("chapter"), &cfg), 2);
        // "General provisions" subtitle — tie, None ≠ chapter → tiebreaker → 3
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 3);
        // Article 1 — tie, top is None subtitle → skip past it; effective top
        //   is chapter; article ≠ chapter, not in stack → tiebreaker → 3
        assert_eq!(ctx.update_for_section(1.0, Some("article"), &cfg), 3);
        // "Subject matter" — tie, None ≠ article → tiebreaker → 4
        assert_eq!(ctx.update_for_section(1.0, None, &cfg), 4);
    }

    /// CR-27 Test 9 — depth cap respected when enforce_max_depth=true.
    #[test]
    fn test_cr27_depth_cap_respected() {
        let mut cfg = cr27_config();
        cfg.max_depth = 3;
        cfg.enforce_max_depth = true;
        let mut ctx = HierarchyContext::new();
        // Each section adds a tier at the same font: 1, 2, 3, then capped at 3
        assert_eq!(ctx.update_for_section(12.0, Some("part"), &cfg), 1);
        assert_eq!(ctx.update_for_section(12.0, Some("chapter"), &cfg), 2);
        assert_eq!(ctx.update_for_section(12.0, Some("article"), &cfg), 3);
        assert_eq!(ctx.update_for_section(12.0, Some("section"), &cfg), 3);
    }

    /// CR-27 Test 10 — REGRESSION REPRO: title-wrap drift on Police Act §86.
    ///
    /// Reproduces the depth-5 paragraph drift observed under section 86
    /// "Causing death by dangerous driving or careless driving when under the
    /// influence of drink or drugs: increased penalties". The section title
    /// is split across two Tika lines, same font class (f9), same band:
    ///
    ///   Line 0: "86 Causing death by dangerous driving..."  → matches `numbered`
    ///   Line 1: "influence of drink or drugs: increased..." → matches no keyword
    ///
    /// Both pass Pass 1 (bold + isolated). Line 0 promotes at the correct
    /// numbered tier. Line 1 arrives with `None` keyword at the same font,
    /// triggering the CR-27 tiebreaker and pushing (None, depth+1) on the stack.
    /// Subsequent body paragraphs then read `current_depth + 1`, landing one
    /// level too deep.
    ///
    /// This test currently documents the buggy behaviour. See discussion: this
    /// is arguably an upstream Pass-1 issue (over-promotion of title wrap-lines)
    /// surfaced by CR-27's now-correct tier discrimination.
    #[test]
    fn test_cr27_title_wrap_drifts_deeper_repro() {
        let cfg = cr27_config();
        let mut ctx = HierarchyContext::new();

        // Set up the Police Act §86 surrounding context.
        // Title (24pt) → PART 5 (12pt) → "86 Causing death..." line 0 (12pt, numbered)
        ctx.update_for_section(24.0, None, &cfg); // Title at d=1
        let d_part = ctx.update_for_section(12.0, Some("part"), &cfg);
        assert_eq!(d_part, 2, "PART 5 expected at depth 2");

        let d_section = ctx.update_for_section(12.0, Some("numbered"), &cfg);
        assert_eq!(d_section, 3, "numbered section expected at depth 3");

        // Now the title wrap-line: same font, no keyword match.
        let d_wrap = ctx.update_for_section(12.0, None, &cfg);

        // Body paragraphs after the section read get_content_level().
        let body_depth = ctx.get_content_level();

        // Demonstrate the bug: wrap-line is treated as a NEW deeper section,
        // and body content is pushed one level too deep.
        assert_eq!(
            d_wrap, 4,
            "BUG: wrap-line classified as deeper section instead of sibling/continuation"
        );
        assert_eq!(
            body_depth, 5,
            "BUG: body paragraphs land at section_depth + 2 instead of + 1"
        );

        // Expected behaviour (commented out — these are the assertions we'd
        // want after a fix):
        //   assert_eq!(d_wrap, 3, "wrap-line should remain at section depth (continuation)");
        //   assert_eq!(body_depth, 4, "body should be one level under the section");
    }

    // ── Block 07 — Header / Footer / Margin classification skip ───────────────

    /// Build a `PdfTextElement` whose font size and isolation would otherwise
    /// promote it to Section under the Region 1 path (`test_region1_auto_promotes`
    /// is the canonical positive case). Caller chooses the `region_label` to
    /// drive the Block 07 boundary classifier.
    fn make_section_sized_element(region_label: Option<&str>) -> PdfTextElement {
        let mut el = make_element(16.0, false, "body", 0);
        el.placement.region_label = region_label.map(str::to_string);
        el
    }

    /// Apply `SectionDetectionV2Rule` over a freshly-bootstrapped element list
    /// (the bootstrap path picks `element_type` from `region_label`) and return
    /// the resulting element types in order.
    fn run_apply_bootstrap(text_elements: &[PdfTextElement]) -> Vec<ParsedElementType> {
        let body = 10.0;
        let font_analysis = make_font_analysis(body, vec![("body", 100)]);
        let config = make_config(1.0, 5.0, None);
        let document_analysis = make_document_analysis();
        let style_data = make_style_data();
        let engine = RuleEngine::new().expect("engine");
        let rule = SectionDetectionV2Rule::new(
            &engine,
            text_elements,
            &config,
            &document_analysis,
            &font_analysis,
            &style_data,
        );
        rule.apply(Vec::new())
            .expect("apply succeeds")
            .into_iter()
            .map(|e| e.element_type)
            .collect()
    }

    /// Block 07 — `H-*` region label is preserved through section detection.
    /// The element has section-promoting geometry (16pt vs 10pt body, isolated)
    /// so without the skip guard it would land as `Section`.
    #[test]
    fn block07_header_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(Some("H-1"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Header]);
    }

    /// Block 07 — `F-*` region label is preserved through section detection.
    #[test]
    fn block07_footer_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(Some("F-2"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Footer]);
    }

    /// Block 07 — orphan element (region_label = None) classifies as Margin and
    /// is skipped by section detection. Sidebar marginalia / rotated content /
    /// elements outside any Region tree leaf land here.
    #[test]
    fn block07_orphan_label_skips_section_promotion() {
        let text_elements = vec![make_section_sized_element(None)];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Margin]);
    }

    /// Block 07 — body leaf labels (`"1"`, `"2-1"`, …) follow the normal
    /// section-detection path. This is the regression check for the guard:
    /// the `H-` / `F-` prefix gate must not catch body labels.
    #[test]
    fn block07_body_leaf_label_still_promotes_to_section() {
        let text_elements = vec![make_section_sized_element(Some("2-1"))];
        let types = run_apply_bootstrap(&text_elements);
        assert_eq!(types, vec![ParsedElementType::Section]);
    }

    // ── CR-67 Part B — source-order ±1 section-fragment promotion ───────

    /// Build a `ParsedPdfElement` with the fields the promotion routine
    /// reads: element_type, font class name, bbox height, hierarchy
    /// level. Other fields get harmless defaults.
    fn make_parsed_for_promote(
        element_type: ParsedElementType,
        text: &str,
        class_name: &str,
        height: f32,
        hierarchy_level: u32,
    ) -> ParsedPdfElement {
        ParsedPdfElement {
            element_type,
            text: text.to_string(),
            hierarchy_level,
            position: 0,
            style_info: FontClass {
                class_name: class_name.to_string(),
                font_family: "TestFont".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "bold".to_string(),
                color: "#000000".to_string(),
            },
            placement: Some(Placement {
                page_number: 1,
                bounding_box: BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    width: 100.0,
                    height,
                },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
            }),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            links: vec![],
        }
    }

    /// Sb8 helper — a bold/normal text segment at explicit `(x, y, width)`.
    fn seed_el_w(text: &str, x: f32, y: f32, bold: bool, width: f32) -> ParsedPdfElement {
        ParsedPdfElement {
            element_type: ParsedElementType::Paragraph,
            text: text.to_string(),
            hierarchy_level: 3,
            position: 0,
            style_info: FontClass {
                class_name: "f13".to_string(),
                font_family: if bold { "Noto-Serif-Bold" } else { "Noto-Serif" }.to_string(),
                font_size: 13.0,
                font_style: "normal".to_string(),
                font_weight: if bold { "bold" } else { "normal" }.to_string(),
                color: "#000000".to_string(),
            },
            placement: Some(Placement {
                page_number: 1,
                bounding_box: BoundingBox { x, y, width, height: 19.1 },
                line_number: 0,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 0,
                region_label: Some("1".to_string()),
                page_width: 0.0,
                page_height: 0.0,
            }),
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            links: vec![],
        }
    }

    /// Short header segment (default width) that leaves trailing whitespace —
    /// paired with a full-width body element in tests so the trailing-ratio
    /// filter sees the column edge.
    fn seed_el(text: &str, x: f32, y: f32, bold: bool) -> ParsedPdfElement {
        seed_el_w(text, x, y, bold, 20.0)
    }

    /// A full-width body line so the trailing-whitespace filter knows the
    /// column's right edge (66 → 518). Real headers stop well short of it.
    fn body_line() -> ParsedPdfElement {
        seed_el_w("Body text spanning the full column width to the margin", 66.0, 40.0, false, 452.0)
    }

    /// Sb8 — the canonical RFC depth-3 miss. A bold "3.5.2. " number and a bold
    /// "Reset Generation" title share a Y-line; the body sits 19pt below. The
    /// seed promotes both header segments to Section (depth = numbering
    /// component count) and leaves the body as Paragraph.
    #[test]
    fn sb8_numbered_seed_promotes_split_header() {
        let mut out = vec![
            body_line(),
            seed_el("3.5.2. ", 66.0, 468.0, true),
            seed_el("Reset Generation", 99.0, 468.0, true),
            seed_el("A TCP user or application can issue a reset…", 66.0, 487.0, false),
        ];

        SectionDetectionV2Rule::promote_numbered_line_seeds(&mut out);

        assert_eq!(out[1].element_type, ParsedElementType::Section, "number segment seeds");
        assert_eq!(out[2].element_type, ParsedElementType::Section, "title segment seeds");
        assert_eq!(out[1].hierarchy_level, 3, "depth = numbering component count");
        assert_eq!(out[2].hierarchy_level, 3);
        assert_eq!(
            out[3].element_type,
            ParsedElementType::Paragraph,
            "body on a different Y-line must not be swept in",
        );
    }

    /// Sb8 — a TOC entry has the same text shape but NORMAL weight. The bold
    /// gate must keep it from promoting.
    #[test]
    fn sb8_numbered_seed_skips_non_bold_toc() {
        let mut out = vec![
            seed_el("3.5.2", 90.0, 66.0, false),
            seed_el("Reset Generation", 120.0, 66.0, false),
        ];
        SectionDetectionV2Rule::promote_numbered_line_seeds(&mut out);
        assert_eq!(out[0].element_type, ParsedElementType::Paragraph);
        assert_eq!(out[1].element_type, ParsedElementType::Paragraph);
    }

    /// Sb8 — a bold number+colon with a NON-bold title (NIST AI-RMF subcategory
    /// shape) must not seed: only bold segments promote, so a non-bold title
    /// would leave a bare "1.4:" section.
    #[test]
    fn sb8_numbered_seed_requires_bold_title() {
        let mut out = vec![
            seed_el("1.4:", 66.0, 100.0, true),
            seed_el("The risk management process and its outcomes", 90.0, 100.0, false),
        ];
        SectionDetectionV2Rule::promote_numbered_line_seeds(&mut out);
        assert_eq!(out[0].element_type, ParsedElementType::Paragraph);
        assert_eq!(out[1].element_type, ParsedElementType::Paragraph);
    }

    /// Sb8 — a full-width numbered line (TOC entry or body/table line) is
    /// filtered geometrically: it fills the column to the right margin, leaving
    /// no trailing whitespace, unlike a real header. The NIST Privacy Framework
    /// TOC ("1.0 Introduction ........ 5") is the canonical case.
    #[test]
    fn sb8_numbered_seed_skips_full_width_line() {
        let mut out = vec![
            body_line(), // column right edge = 518
            seed_el_w("1.0 Privacy Framework Introduction ......... 5", 66.0, 100.0, true, 452.0),
            seed_el("3.5.2.", 66.0, 200.0, true),
            seed_el("Reset Generation", 99.0, 200.0, true),
        ];
        SectionDetectionV2Rule::promote_numbered_line_seeds(&mut out);
        assert_eq!(
            out[1].element_type,
            ParsedElementType::Paragraph,
            "full-width numbered line (TOC/body) leaves no trailing whitespace → filtered",
        );
        assert_eq!(out[2].element_type, ParsedElementType::Section, "short header kept");
        assert_eq!(out[3].element_type, ParsedElementType::Section);
    }

    /// Sb8 — a single-level list marker ("1.") is not multi-level numbering and
    /// must not seed (would over-promote numbered list items).
    #[test]
    fn sb8_numbered_seed_skips_single_level() {
        let mut out = vec![
            seed_el("1.", 66.0, 100.0, true),
            seed_el("First do this thing", 80.0, 100.0, true),
        ];
        SectionDetectionV2Rule::promote_numbered_line_seeds(&mut out);
        assert_eq!(out[0].element_type, ParsedElementType::Paragraph);
        assert_eq!(out[1].element_type, ParsedElementType::Paragraph);
    }

    /// CR-67 Part B — canonical RFC fragmentation. A `**1.1.**` Paragraph
    /// (rejected by alpha-ratio) sits immediately before a
    /// `**Document Structure**` Section, sharing font class `f7` and
    /// bbox height 10.5. Same-class same-height → promotion fires; both
    /// neighbors classify as Section (NodeTypeClustering then fuses
    /// them into one Section node downstream).
    #[test]
    fn cr67_promote_same_class_same_height_fires() {
        let mut elements = vec![
            make_parsed_for_promote(
                ParsedElementType::Paragraph,
                "**1.1.**",
                "f7",
                10.5,
                3,
            ),
            make_parsed_for_promote(
                ParsedElementType::Section,
                "**Document Structure**",
                "f7",
                10.5,
                2,
            ),
        ];

        SectionDetectionV2Rule::promote_section_fragments(&mut elements);

        assert_eq!(
            elements[0].element_type,
            ParsedElementType::Section,
            "left neighbor with same font class + matching height must promote"
        );
        assert_eq!(
            elements[0].hierarchy_level, 2,
            "promoted fragment inherits the section's hierarchy level"
        );
        assert_eq!(elements[1].element_type, ParsedElementType::Section);
    }

    /// CR-67 Part B — same-class but height mismatch outside HEIGHT_TOL
    /// (2.0). 10.5 vs 12.6 → diff 2.1 ≥ 2.0 → no promotion. Defends
    /// against picking up adjacent inline emphasis that happens to share
    /// a font class but is visually a different glyph run.
    #[test]
    fn cr67_promote_height_mismatch_no_promotion() {
        let mut elements = vec![
            make_parsed_for_promote(
                ParsedElementType::Paragraph,
                "**1.1.**",
                "f7",
                10.5,
                3,
            ),
            make_parsed_for_promote(
                ParsedElementType::Section,
                "**Title**",
                "f7",
                12.6,
                2,
            ),
        ];

        SectionDetectionV2Rule::promote_section_fragments(&mut elements);

        assert_eq!(
            elements[0].element_type,
            ParsedElementType::Paragraph,
            "height-mismatch (Δ=2.1 ≥ HEIGHT_TOL) must NOT promote",
        );
        assert_eq!(elements[1].element_type, ParsedElementType::Section);
    }

    /// CR-67 Part B — alphafold 3.1-bloat defense. A merged-title-and-body
    /// blob (Paragraph, h=256) sits source-order adjacent to a true
    /// single-line Section (h=10.5) sharing font class f22. Without the
    /// height gate, the blob would promote → NodeTypeClustering fuses →
    /// CR-65 demotes the 266-tall result back to Paragraph → both
    /// nodes lost. The height gate must reject promotion.
    #[test]
    fn cr67_promote_bloated_paragraph_height_gate_rejects() {
        let mut elements = vec![
            make_parsed_for_promote(
                ParsedElementType::Paragraph,
                "3.1. In our first... [bloated body merge]",
                "f22",
                256.0,
                3,
            ),
            make_parsed_for_promote(
                ParsedElementType::Section,
                "3.2 Section title",
                "f22",
                10.5,
                2,
            ),
        ];

        SectionDetectionV2Rule::promote_section_fragments(&mut elements);

        assert_eq!(
            elements[0].element_type,
            ParsedElementType::Paragraph,
            "bloated paragraph (h=256 vs section h=10.5) must NOT promote — \
             the height gate is load-bearing against CR-65 cascade demotion",
        );
        assert_eq!(elements[1].element_type, ParsedElementType::Section);
    }
}
