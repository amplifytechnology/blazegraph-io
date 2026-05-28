//! Blazegraph XHTML Parser
//!
//! Parses the Blazegraph XHTML intermediate format produced by PDF backends
//! into PreprocessorOutput. This parser is shared across all PDF backends.
//!
//! The Blazegraph XHTML format includes:
//! - Page divs with data-page attributes
//! - Per-page meta: `<div class="page-meta" data-page data-width data-height />`
//! - Asides: `<aside data-rotation="N">` — rotated content (e.g., arxiv sidebars)
//! - Paragraphs (`<p>`) wrapping body lines as a parsing convenience
//! - Spans with data-bbox, data-line, data-segment attributes
//! - CSS font classes in <style> block
//! - Document metadata in <meta> tags
//! - Bookmarks/TOC in <ul> structure
//!
//! Note: `<div class="band">` and `data-column` were emitted by Tika prior to
//! the layout-reasoning consolidation flow (2026-05-03). Tika no longer emits
//! either; structural reasoning lives on the Rust side. The legacy Placement
//! fields `band`, `column`, `nr_band_columns` were dropped in schema 0.5.0
//! (Block 06b — reading-order resort); region tagging is now produced post-
//! analytics via `analytics::reading_order::tag_and_resort` and lives on
//! `Placement.region_label`.
//!
//! Parser approach: quick-xml pull-parser in event mode. A lightweight state
//! machine tracks the current page and aside/rotation context as the parser
//! walks the event stream. This avoids the fragility of regex on nested XHTML
//! and naturally handles context propagation (aside → span) without building
//! a full DOM.
//!
//! quick-xml was chosen because it is already a workspace dependency, is
//! widely used in the Rust ecosystem, and supports pull-mode parsing with
//! attribute access — exactly what context tracking needs.

use crate::tokens::estimate_token_count;
use crate::types::*;
use anyhow::Result;
use quick_xml::escape::unescape;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::Instant;
use unicode_normalization::UnicodeNormalization;

// Pre-compiled regexes

/// Extracts raw tag fragments from a span's inner content.
/// Matches self-closing tags (`<br/>`) and paired open/close tags (`<a href="...">text</a>`).
/// Valid XHTML guarantees text content is entity-escaped, so any literal `<` is a real tag.
static RAW_TAG_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)(?:<[^>]+/>|<[a-zA-Z][^>]*>.*?</[a-zA-Z][^>]*>)").unwrap());

// CR-57 Phase B: the META regex + flat tag dispatch moved to
// `crate::preprocessors::pdf::metadata::PdfMetadataExtractor`. The
// metadata-extraction entry point in `parse_xhtml` now drives the trait,
// so unrecognized XMP tags surface in `pdf.extras` rather than being
// silently dropped.

static STYLE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<style[^>]*>(.*?)</style>").unwrap());

static FONT_CLASS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(\w+)\s*\{\s*font-family:\s*([^;]+);\s*font-size:\s*([^;]+);\s*font-style:\s*([^;]+);\s*font-weight:\s*([^;]+);\s*color:\s*([^;]+);\s*\}")
        .unwrap()
});

// ============================================================================
// Public entry point
// ============================================================================

/// Parse Blazegraph XHTML into PreprocessorOutput.
///
/// Main entry point. Extracts text elements (with full structural context),
/// document metadata, style data, and bookmark data.
pub fn parse_xhtml(xhtml: &str) -> Result<PreprocessorOutput> {
    use crate::preprocessors::metadata::extract_document_metadata;
    use crate::preprocessors::pdf::metadata::PdfMetadataExtractor;

    let t0 = Instant::now();
    let metadata = extract_document_metadata(&PdfMetadataExtractor::new(xhtml), &());
    let t1 = Instant::now();
    let style_data = extract_style_data(xhtml)?;
    let t2 = Instant::now();
    let bookmark_data = extract_bookmark_data(xhtml)?;
    let t3 = Instant::now();
    let text_elements = extract_text_elements(xhtml, &style_data, &bookmark_data)?;
    let t4 = Instant::now();

    println!(
        "XHTML parsing complete: {} text elements, {} font classes, {} bookmarks (meta={}ms, style={}ms, bookmark={}ms, text={}ms)",
        text_elements.len(),
        style_data.font_classes.len(),
        bookmark_data
            .as_ref()
            .map(|b| b.sections.len())
            .unwrap_or(0),
        (t1 - t0).as_millis(),
        (t2 - t1).as_millis(),
        (t3 - t2).as_millis(),
        (t4 - t3).as_millis(),
    );

    Ok(PreprocessorOutput {
        text_elements,
        metadata,
        style_data,
        bookmark_data,
    })
}

// ============================================================================
// Text element extraction — regex-driven flat scanner (CR-40)
// ============================================================================
//
// Post-Block-1 strip (`ccabf4291`, 2026-05-03), the XHTML structure is
// essentially flat: <div class="page"> contains <p> contains <span>. The only
// optional structural element is <aside data-rotation="..."> for rotated
// content. Spans are leaf elements with plain text plus optional one-level
// inner tags (the future-<link> extension point, captured via raw_tags).
//
// We scan with a single alternation regex (`EVENT_REGEX`) plus a `find_at(pos)`
// advancement loop. After matching a span open, we advance pos past `</span>`
// so any inner tags (e.g. <link>) don't trigger spurious top-level matches.
//
// Coupling: the patterns are tied to the emission style of BlazePDF2XHTML.java.
// New optional `data-*` attrs on existing tags are tolerated (we capture the
// whole tag and parse attrs by name within); new structural tags require
// updating this regex.

/// Top-level event regex. Each alternation is "the next interesting boundary".
/// Whichever matches first at `pos` is the next event. Span dispatch advances
/// past `</span>`, so inner tags inside a span are not matched as events.
static EVENT_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r#"<div class="page">"#,
        r#"|<div class="page-meta"[^/]*/>"#,
        r#"|<aside\s[^>]*>"#,
        r#"|</aside>"#,
        r#"|<p>"#,
        r#"|</p>"#,
        r#"|<span\s[^>]*>"#,
    ))
    .unwrap()
});

/// State carried across the regex scan, mutated as alternations match.
struct ParseState {
    page_number: u32,
    /// Dimensions of the current page in PDF points, sourced from the
    /// `<div class="page-meta" data-width data-height />` self-closing tag.
    /// Reset to 0.0 on each new page; stays 0.0 if page-meta is absent
    /// (older Tika output). Downstream analytics treats 0.0 as "unknown".
    current_page_width: f32,
    current_page_height: f32,
    /// Set on `<aside data-rotation="...">`, cleared on `</aside>`.
    /// Spans emitted inside an aside inherit the rotation.
    current_rotation: i32,
    /// Global, 0-indexed paragraph counter, incremented at each `</p>`.
    /// (Behavior preserved from the prior pull-parser implementation.)
    paragraph_number: u32,
    page_elements: Vec<PdfTextElement>,
}

impl ParseState {
    fn new() -> Self {
        ParseState {
            page_number: 0,
            current_page_width: 0.0,
            current_page_height: 0.0,
            current_rotation: 0,
            paragraph_number: 0,
            page_elements: Vec::new(),
        }
    }
}

/// Read a quoted attribute value from a tag string.
/// Returns `Some("…")` if `name="…"` is found; `None` otherwise.
/// Order-independent — new optional attrs don't break this.
fn parse_attr_str<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", name);
    let start = tag.find(&needle)?;
    let value_start = start + needle.len();
    let rest = &tag[value_start..];
    let end = rest.find('"')?;
    Some(&rest[..end])
}

fn parse_attr_f32(tag: &str, name: &str) -> Option<f32> {
    parse_attr_str(tag, name).and_then(|s| s.parse().ok())
}

fn parse_attr_i32(tag: &str, name: &str) -> Option<i32> {
    parse_attr_str(tag, name).and_then(|s| s.parse().ok())
}

fn parse_attr_u32(tag: &str, name: &str) -> Option<u32> {
    parse_attr_str(tag, name).and_then(|s| s.parse().ok())
}

/// Parse a comma-separated `x,y,w,h` bbox string into a BoundingBox.
/// Returns None on malformed input (wrong arity or unparseable floats).
fn parse_bbox_string(s: &str) -> Option<BoundingBox> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let x = parts[0].trim().parse::<f32>().ok()?;
    let y = parts[1].trim().parse::<f32>().ok()?;
    let width = parts[2].trim().parse::<f32>().ok()?;
    let height = parts[3].trim().parse::<f32>().ok()?;
    Some(BoundingBox {
        x,
        y,
        width,
        height,
    })
}

/// CR-62: Parse the `data-link-*` attribute family on a `<span>` tag into a
/// typed PdfLinkAnnotation. Returns `None` when `data-link-kind` is absent
/// (the vast majority of spans) or when required per-kind fields are missing.
///
/// Attribute schema (defined by Tika-side `addLinkAttributes` in
/// `BlazePDF2XHTML.java`):
///
/// - `data-link-bbox="x,y,w,h"` — required when kind present (annotation rect,
///   top-origin coords matching `data-bbox`).
/// - `data-link-kind` — one of `internal-named`, `internal-page`, `external-uri`.
/// - Per kind:
///   - `internal-named`: `data-link-target-name` (required), optional
///     `data-link-target-page` / `-x` / `-y` (resolved by name tree).
///   - `internal-page`: `data-link-target-page` (required), optional `-x` / `-y`.
///   - `external-uri`: `data-link-target-url` (required).
fn parse_link_annotation(tag: &str) -> Option<PdfLinkAnnotation> {
    let kind_str = parse_attr_str(tag, "data-link-kind")?;
    let bbox_str = parse_attr_str(tag, "data-link-bbox")?;
    let source_bbox = parse_bbox_string(bbox_str)?;

    let target_page = parse_attr_u32(tag, "data-link-target-page");
    let target_x = parse_attr_str(tag, "data-link-target-x").and_then(|s| s.parse::<f32>().ok());
    let target_y = parse_attr_str(tag, "data-link-target-y").and_then(|s| s.parse::<f32>().ok());

    let kind = match kind_str {
        "internal-named" => {
            let name = parse_attr_str(tag, "data-link-target-name")?.to_string();
            PdfLinkKind::InternalNamed {
                name,
                target_page,
                target_x,
                target_y,
            }
        }
        "internal-page" => {
            let target_page = target_page?;
            PdfLinkKind::InternalPage {
                target_page,
                target_x,
                target_y,
            }
        }
        "external-uri" => {
            let url = parse_attr_str(tag, "data-link-target-url")?.to_string();
            PdfLinkKind::ExternalUri { url }
        }
        _ => return None, // Unrecognized kind — degrade gracefully.
    };

    Some(PdfLinkAnnotation { source_bbox, kind })
}

/// Walk a span's inner content and accumulate text outside any inner tag
/// region. Equivalent to the prior pull-parser semantics where Event::Text
/// was captured only when span_nesting == 0 — i.e., text inside
/// `<link>...</link>` is skipped (the link itself is captured via raw_tags).
/// Self-closing tags (`<br/>`) don't change nesting.
fn extract_text_skipping_inner_tags(inner: &str) -> String {
    let mut out = String::with_capacity(inner.len());
    let mut nesting: u32 = 0;
    let mut pos = 0;
    while pos < inner.len() {
        match inner[pos..].find('<') {
            Some(rel) => {
                let lt = pos + rel;
                if nesting == 0 {
                    out.push_str(&inner[pos..lt]);
                }
                let Some(rel_gt) = inner[lt..].find('>') else {
                    if nesting == 0 {
                        out.push_str(&inner[lt..]);
                    }
                    break;
                };
                let gt = lt + rel_gt + 1;
                let tag = &inner[lt..gt];
                let is_close = tag.starts_with("</");
                let is_self = tag.ends_with("/>");
                if is_close {
                    nesting = nesting.saturating_sub(1);
                } else if !is_self {
                    nesting += 1;
                }
                pos = gt;
            }
            None => {
                if nesting == 0 {
                    out.push_str(&inner[pos..]);
                }
                break;
            }
        }
    }
    out
}

/// Extract text elements via a flat regex-driven scan.
///
/// Replaces the previous quick_xml pull-parser state machine. EVENT_REGEX
/// finds the next structural event; each match is dispatched. Span events
/// use `str::find("</span>")` to locate the close, then advance past it
/// so inner tags don't trigger spurious matches.
fn extract_text_elements(
    xhtml: &str,
    style_data: &StyleData,
    bookmark_data: &Option<BookmarkData>,
) -> Result<Vec<PdfTextElement>> {
    // Precompute bookmark lookup keyed by NFKC+whitespace-folded title. Without
    // this, build_element ran `normalize_match` on every bookmark title for every
    // text element — O(N×B) with NFKC inside the inner loop. For docs like
    // rfc-quic (9469 elements × 219 bookmarks) that was ~2M normalize calls per
    // parse. Now: normalize each title once here, O(1) lookup per element.
    //
    // `or_insert_with` preserves "first occurrence wins" semantics matching
    // the prior `bookmark_sections.iter().find(...)` behavior, regardless of
    // HashMap insertion order.
    let bookmark_lookup: HashMap<String, BookmarkSection> = match bookmark_data.as_ref() {
        Some(bd) => {
            let mut lookup = HashMap::with_capacity(bd.sections.len());
            for section in &bd.sections {
                lookup
                    .entry(normalize_for_match(&section.title))
                    .or_insert_with(|| section.clone());
            }
            lookup
        }
        None => HashMap::new(),
    };

    let mut state = ParseState::new();
    let mut all_elements: Vec<PdfTextElement> = Vec::new();
    let mut global_reading_order: u32 = 0;

    let mut pos = 0;
    while let Some(m) = EVENT_REGEX.find_at(xhtml, pos) {
        let matched = &xhtml[m.start()..m.end()];

        if matched == r#"<div class="page">"# {
            // New page: flush in-flight page_elements, reset page-scoped state.
            if !state.page_elements.is_empty() {
                finalize_page_elements(
                    &mut state.page_elements,
                    &mut all_elements,
                    &mut global_reading_order,
                );
            }
            state.page_number += 1;
            state.current_rotation = 0;
            state.current_page_width = 0.0;
            state.current_page_height = 0.0;
            pos = m.end();
        } else if matched.starts_with(r#"<div class="page-meta""#) {
            state.current_page_width = parse_attr_f32(matched, "data-width").unwrap_or(0.0);
            state.current_page_height = parse_attr_f32(matched, "data-height").unwrap_or(0.0);
            pos = m.end();
        } else if matched.starts_with("<aside") {
            state.current_rotation = parse_attr_i32(matched, "data-rotation").unwrap_or(0);
            pos = m.end();
        } else if matched == "</aside>" {
            state.current_rotation = 0;
            pos = m.end();
        } else if matched == "<p>" {
            // Paragraph open: no state change; counter increments at </p>.
            pos = m.end();
        } else if matched == "</p>" {
            state.paragraph_number += 1;
            pos = m.end();
        } else if matched.starts_with("<span") {
            // Span: parse attrs, find </span>, slice inner, build element.
            let class = parse_attr_str(matched, "class").unwrap_or("").to_string();
            let bbox = parse_attr_str(matched, "data-bbox")
                .unwrap_or("")
                .to_string();
            let line = parse_attr_u32(matched, "data-line").unwrap_or(0);
            let segment = parse_attr_u32(matched, "data-segment").unwrap_or(0);
            // CR-62: data-link-* attrs are present only on link-bearing spans.
            let link = parse_link_annotation(matched);

            let inner_start = m.end();
            let close_offset = match xhtml[inner_start..].find("</span>") {
                Some(off) => off,
                None => break, // Graceful: malformed XHTML, stop.
            };
            let inner = &xhtml[inner_start..inner_start + close_offset];

            // Inner content: text + optional inner tags (e.g. future <link>).
            // raw_tags collects tag fragments; the text portion is everything else.
            let raw_tags = extract_raw_tags(inner);
            let inner_text_raw = extract_text_skipping_inner_tags(inner);
            let text_content = match unescape(&inner_text_raw) {
                Ok(cow) => normalize_segment_text(&cow),
                Err(_) => normalize_segment_text(&inner_text_raw),
            };

            if !text_content.trim().is_empty() {
                if let Some(element) = build_element(
                    &state,
                    &class,
                    &bbox,
                    line,
                    segment,
                    text_content,
                    raw_tags,
                    link,
                    style_data,
                    &bookmark_lookup,
                ) {
                    state.page_elements.push(element);
                }
            }

            // Advance past </span> so inner tags can't trigger spurious matches.
            pos = inner_start + close_offset + "</span>".len();
        } else {
            // Defensive: unmatched alternation case. Advance to avoid stalling.
            pos = m.end();
        }
    }

    // Flush the final page.
    if !state.page_elements.is_empty() {
        finalize_page_elements(
            &mut state.page_elements,
            &mut all_elements,
            &mut global_reading_order,
        );
    }

    println!(
        "Total extraction: {} text elements across {} pages",
        all_elements.len(),
        state.page_number
    );

    Ok(all_elements)
}

/// Sort page elements by Tika's structural reading-order anchor and assign
/// global reading_order.
///
/// CR-51 (initial): replaced the previous Y-then-X spatial sort that
/// mis-clustered same-line spans with different font baselines (Shannon's
/// small-caps section header, attention's superscript citations).
///
/// CR-51 (revision after rfc-quic regression): the within-line key is
/// `bounding_box.x` — NOT `segment_number`. Tika walks PDF content streams in
/// source order, which is monotonic-in-X for Shannon/attention but not for
/// rfc-quic (where inline-emphasized words like `MAY`/`SHOULD` and URL
/// hyperlinks are emitted as a later `data-segment` than their visual
/// neighbors). Sorting by `bounding_box.x` within a `(paragraph, line)` group
/// is the lossless reading-order reconstruction: Tika's `data-line` grouping
/// is reliable, and geometric X within line gives the correct order for
/// inline-styled spans regardless of stream-emission order.
///
/// `segment_number` is retained as a tiebreaker for the (rare) case where two
/// spans share an X coordinate within epsilon. `bounding_box.y` is the final
/// fallback for fully-degenerate inputs.
///
/// The full lex hierarchy is `(page, region_rank, paragraph, line,
/// bounding_box.x, segment_number, bounding_box.y)`. `region_rank` is a no-op
/// at this point in the pipeline because `region_label` is `None` until
/// `analytics::reading_order::tag_and_resort` runs; it is included here so
/// this sort remains the right "Tika-document-order" anchor if the parser is
/// ever consumed without the analytics resort.
fn finalize_page_elements(
    page_elements: &mut Vec<PdfTextElement>,
    all_elements: &mut Vec<PdfTextElement>,
    global_reading_order: &mut u32,
) {
    page_elements.sort_by(|a, b| {
        let pa = &a.placement;
        let pb = &b.placement;
        pa.page_number
            .cmp(&pb.page_number)
            .then_with(|| {
                region_rank(pa.region_label.as_deref())
                    .cmp(&region_rank(pb.region_label.as_deref()))
            })
            .then(pa.paragraph_number.cmp(&pb.paragraph_number))
            .then(pa.line_number.cmp(&pb.line_number))
            // Within line: X drives reading order. This is the load-bearing
            // bit of CR-51 — see the docstring above for the rfc-quic
            // motivation.
            .then(pa.bounding_box.x.total_cmp(&pb.bounding_box.x))
            // Tiebreakers — only fire when X is degenerate (two glyphs at
            // identical X within a line, which is unusual visually but can
            // happen for stacked annotations).
            .then(pa.segment_number.cmp(&pb.segment_number))
            .then(pa.bounding_box.y.total_cmp(&pb.bounding_box.y))
    });
    for el in page_elements.iter_mut() {
        el.reading_order = *global_reading_order;
        *global_reading_order += 1;
    }
    all_elements.append(page_elements);
}

/// Stable rank for a region label so the sort hierarchy can disambiguate
/// columns when Tika's `paragraph_number` collides across regions.
///
/// Lexicographic on the `Option<String>` — `None` sorts FIRST (treated as
/// "no region claim yet"), then string labels in `.cmp` order. At parser
/// time every element has `region_label == None`, so this is a no-op; it
/// becomes load-bearing only if a future consumer re-sorts elements after
/// `analytics::reading_order::tag_and_resort` has tagged them.
fn region_rank(label: Option<&str>) -> (u8, Option<&str>) {
    match label {
        None => (0, None),
        Some(s) => (1, Some(s)),
    }
}

/// CR-67 Part A — strips the dot from a leading section-numbering prefix
/// (e.g. `"3.1. Approach 1"` → `"3.1 Approach 1"`). Conservative pattern:
/// only fires when the dot immediately follows a numeric (`\d+(?:\.\d+)*`)
/// or letter-then-optional-digits (`[A-Z]\d*`) prefix and is itself
/// followed by whitespace. Sentence-ending periods (`"...ends in a
/// period."`) are not section-numbering prefixes and pass through
/// unchanged.
static SECTION_NUMBER_PREFIX_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\d+(?:\.\d+)*|[A-Z]\d*)\.\s").unwrap());

/// NFKC + whitespace fold for cross-source string equivalence. Used to align
/// PDF-extracted glyph runs (which can contain ligatures like ﬀ ﬁ) with
/// bookmark titles (which usually decompose to plain ASCII). Also collapses
/// any internal whitespace runs to a single space so layout-driven word
/// breaks don't defeat matching.
///
/// CR-67 Part A: additionally strips the trailing dot that follows a
/// leading section-numbering token (`"3.1. Approach 1"` → `"3.1 Approach
/// 1"`, `"5. Conclusion"` → `"5 Conclusion"`). This aligns body-text
/// emissions like `"3.1. Title"` with outline entries written as
/// `"3.1 Title"`, restoring the bookmark-match bypass for elements that
/// fail the `isolated_in_leaf` XY-cut gate.
pub(crate) fn normalize_for_match(s: &str) -> String {
    let nfkc: String = s.nfkc().collect();
    let folded = nfkc.split_whitespace().collect::<Vec<_>>().join(" ");
    SECTION_NUMBER_PREFIX_REGEX
        .replace(&folded, "$1 ")
        .into_owned()
}

/// Build a `PdfTextElement` from the current parse state and span attrs.
#[allow(clippy::too_many_arguments)]
fn build_element(
    state: &ParseState,
    span_class: &str,
    span_bbox: &str,
    line_number: u32,
    segment_number: u32,
    text_content: String,
    raw_tags: Vec<String>,
    link: Option<PdfLinkAnnotation>,
    style_data: &StyleData,
    bookmark_lookup: &HashMap<String, BookmarkSection>,
) -> Option<PdfTextElement> {
    // Parse bounding box: "x,y,width,height"
    let bbox = parse_bbox_string(span_bbox)?;
    let BoundingBox {
        x,
        y,
        width,
        height,
    } = bbox;

    // Resolve font class
    let font_class = if let Some(fc) = style_data.font_classes.get(span_class) {
        fc.clone()
    } else {
        fallback_font(span_class)
    };

    // Bookmark match — O(1) via precomputed lookup keyed on normalized title.
    let bookmark_match = bookmark_lookup
        .get(&normalize_for_match(&text_content))
        .cloned();

    Some(PdfTextElement {
        text: text_content.clone(),
        style_info: font_class,
        placement: Placement {
            page_number: state.page_number,
            bounding_box: BoundingBox {
                x,
                y,
                width,
                height,
            },
            line_number,
            segment_number,
            rotation: state.current_rotation,
            paragraph_number: state.paragraph_number,
            region_label: None,
            page_width: state.current_page_width,
            page_height: state.current_page_height,
        },
        reading_order: 0, // Assigned later in finalize_page_elements
        bookmark_match,
        token_count: estimate_token_count(&text_content),
        raw_tags,
        link,
    })
}

// ============================================================================
// Segment text normalization
// ============================================================================

/// Normalize a span's raw text for emission.
///
/// Two operations: (a) Unicode NFKC fold so ligature glyphs (ﬀ ﬁ ﬂ ﬃ ﬄ),
/// non-breaking space, and other compat-decomposable chars are reduced to
/// their canonical forms — embeddings/search/AI then see what readers see;
/// (b) whitespace cleanup that collapses internal runs to a single space
/// while preserving a single leading/trailing space if the original had
/// boundary whitespace. The boundary signal is what lets the downstream
/// same-line concatenation know whether Tika emitted a separator.
fn normalize_segment_text(s: &str) -> String {
    let nfkc: String = s.nfkc().collect();
    let leading = nfkc.starts_with(|c: char| c.is_whitespace());
    let trailing = nfkc.ends_with(|c: char| c.is_whitespace());
    let body: String = nfkc.split_whitespace().collect::<Vec<_>>().join(" ");
    if body.is_empty() {
        return String::new();
    }
    let mut out = String::with_capacity(body.len() + leading as usize + trailing as usize);
    if leading {
        out.push(' ');
    }
    out.push_str(&body);
    if trailing {
        out.push(' ');
    }
    out
}

// ============================================================================
// raw_tags extraction
// ============================================================================

/// Extract raw tag fragments from the inner content of a primary `<span>`.
///
/// XHTML guarantees text content is entity-escaped (`<` → `&lt;`), so any
/// literal `<...>` in the inner slice is definitionally a real tag — not user
/// text masquerading as markup. The regex is therefore unambiguous.
fn extract_raw_tags(inner: &str) -> Vec<String> {
    RAW_TAG_REGEX
        .find_iter(inner)
        .map(|m| m.as_str().to_string())
        .collect()
}

// ============================================================================
// Font helpers
// ============================================================================

fn fallback_font(font_class_name: &str) -> FontClass {
    FontClass {
        class_name: font_class_name.to_string(),
        font_family: "unknown".to_string(),
        font_size: 12.0,
        font_style: "normal".to_string(),
        font_weight: "normal".to_string(),
        color: "#000000".to_string(),
    }
}

// ============================================================================
// Metadata extraction
// ============================================================================
//
// The flat `extract_enhanced_metadata` from pre-CR-57 was replaced by
// `crate::preprocessors::pdf::metadata::PdfMetadataExtractor` — the
// trait-driven extractor that lives next to the trait it implements.
// `parse_xhtml` (above) drives that extractor directly.

// ============================================================================
// Style data extraction (regex, stable)
// ============================================================================

/// Extract style data from CSS <style> block.
fn extract_style_data(xhtml: &str) -> Result<StyleData> {
    if let Some(style_start) = xhtml.rfind("<style") {
        if let Some(style_end) = xhtml[style_start..].find("</style>") {
            let style_block = &xhtml[style_start..style_start + style_end + 8];

            if let Some(style_cap) = STYLE_REGEX.captures(style_block) {
                if let Some(css_content) = style_cap.get(1) {
                    let css = css_content.as_str();
                    let mut font_classes = BTreeMap::new();

                    for cap in FONT_CLASS_REGEX.captures_iter(css) {
                        if let (
                            Some(class_name),
                            Some(family),
                            Some(size_str),
                            Some(style),
                            Some(weight),
                            Some(color),
                        ) = (
                            cap.get(1),
                            cap.get(2),
                            cap.get(3),
                            cap.get(4),
                            cap.get(5),
                            cap.get(6),
                        ) {
                            let class_name_str = class_name.as_str().to_string();
                            let size_text = size_str.as_str().trim();
                            let size = size_text
                                .trim_end_matches("px")
                                .parse::<f32>()
                                .unwrap_or(12.0);

                            let font_class = FontClass {
                                class_name: class_name_str.clone(),
                                font_family: family.as_str().trim().to_string(),
                                font_size: size,
                                font_style: style.as_str().trim().to_string(),
                                font_weight: weight.as_str().trim().to_string(),
                                color: color.as_str().trim().to_string(),
                            };
                            font_classes.insert(class_name_str, font_class);
                        }
                    }

                    if !font_classes.is_empty() {
                        return Ok(StyleData { font_classes });
                    }
                }
            }
        }
    }

    println!("No CSS styles found in XHTML — returning empty StyleData");
    Ok(StyleData {
        font_classes: BTreeMap::new(),
    })
}

// ============================================================================
// Bookmark extraction (manual state machine, stable)
// ============================================================================

/// Extract bookmark data from nested <ul><li> structure emitted by Tika.
///
/// Tika emits the PDF outline (document bookmarks) as nested <ul>/<li> elements
/// after the </style> block. The nesting depth corresponds to the outline hierarchy.
///
/// Previous implementation used `rfind("<ul>")` which found only the innermost
/// (last) <ul> block, dropping most outline entries. This version finds the first
/// <ul> after </style> and walks the full nested structure.
fn extract_bookmark_data(xhtml: &str) -> Result<Option<BookmarkData>> {
    let search_start = xhtml.rfind("</style>").unwrap_or(0);
    let bookmark_region = &xhtml[search_start..];

    let Some(first_ul) = bookmark_region.find("<ul>") else {
        return Ok(None);
    };

    let bookmark_html = &bookmark_region[first_ul..];

    let mut sections = Vec::new();
    let mut depth: u32 = 0;
    let mut pos = 0;
    let bytes = bookmark_html.as_bytes();
    let len = bytes.len();

    while pos < len {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }

        let tag_start = pos;
        let Some(tag_end_offset) = bookmark_html[pos..].find('>') else {
            break;
        };
        let tag_end = pos + tag_end_offset + 1;
        let tag = &bookmark_html[tag_start..tag_end];

        if tag == "<ul>" {
            depth += 1;
        } else if tag == "</ul>" {
            if depth == 0 {
                break;
            }
            depth -= 1;
            if depth == 0 {
                break;
            }
        } else if tag == "<li>" {
            if let Some(li_end_offset) = bookmark_html[tag_end..].find("</li>") {
                let title = bookmark_html[tag_end..tag_end + li_end_offset].trim();
                if !title.is_empty() {
                    let order = sections.len() as u32;
                    sections.push(BookmarkSection {
                        title: title.to_string(),
                        order,
                        level: depth,
                    });
                }
            }
        }

        pos = tag_end;
    }

    if sections.is_empty() {
        Ok(None)
    } else {
        Ok(Some(BookmarkData { sections }))
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Wrap a body fragment in a minimal valid Blazegraph XHTML document.
    fn xhtml_with(body: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head>
<meta name="dc:title" content="Test" />
<style type="text/css">.f1 {{ font-family: Test; font-size: 10px; font-style: normal; font-weight: normal; color: #000000; }}
</style>
<title>Test</title>
</head>
<body>
{body}
</body>
</html>"#
        )
    }

    /// CR-40 acceptance: spans containing inner tags (e.g. future PDF goto
    /// `<link>` elements) get their text content correctly extracted (link
    /// inner text excluded) and the full tag fragment captured in raw_tags.
    #[test]
    fn span_with_inner_link_captures_raw_tag_and_excludes_link_text() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="100.0" data-height="100.0" />
<p>
<span class="f1" data-bbox="10,10,50,12" data-line="0" data-segment="0">See <link href="page:42">section 4.2</link> for details</span>
</p>
</div>"#,
        );
        let output = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(output.text_elements.len(), 1, "exactly one span emits");
        let el = &output.text_elements[0];

        // Text excludes the link's inner text (matches span_nesting==0 semantics
        // from the prior pull-parser implementation).
        assert!(
            el.text.contains("See") && el.text.contains("for details"),
            "text content should contain prefix and suffix; got: {:?}",
            el.text
        );
        assert!(
            !el.text.contains("section 4.2"),
            "text should not include link inner content; got: {:?}",
            el.text
        );

        // raw_tags captures the full <link>...</link> fragment for downstream
        // consumers (e.g. corpus connection graph in closed-source modes).
        assert!(
            el.raw_tags
                .iter()
                .any(|t| t.contains("<link") && t.contains("section 4.2")),
            "raw_tags should contain the link fragment; got: {:?}",
            el.raw_tags
        );
    }

    // ── CR-62: data-link-* attribute parsing ─────────────────────────────────

    /// CR-62: span carrying `data-link-kind="internal-named"` attrs lands on
    /// `PdfTextElement.link` as the typed `InternalNamed` variant with the
    /// Tika-resolved target page + coords carried through. Matches what
    /// BlazePDF2XHTML emits for academic-paper citations (PDActionGoTo →
    /// PDNamedDestination → PDPageXYZDestination).
    #[test]
    fn cr62_span_with_internal_named_link_lands_on_element() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="612" data-height="792" />
<p>
<span class="f5" data-bbox="327.0,99.9,10.0,8.7" data-line="0" data-segment="1" data-link-bbox="326.0,98.9,12.0,8.8" data-link-kind="internal-named" data-link-target-name="cite.hochreiter1997" data-link-target-page="10" data-link-target-x="108.0" data-link-target-y="339.0">13</span>
</p>
</div>"#,
        );
        let out = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(out.text_elements.len(), 1);
        let el = &out.text_elements[0];
        let link = el.link.as_ref().expect("link should be Some");
        assert_eq!(link.source_bbox.x, 326.0);
        assert_eq!(link.source_bbox.y, 98.9);
        assert_eq!(link.source_bbox.width, 12.0);
        assert_eq!(link.source_bbox.height, 8.8);
        match &link.kind {
            PdfLinkKind::InternalNamed {
                name,
                target_page,
                target_x,
                target_y,
            } => {
                assert_eq!(name, "cite.hochreiter1997");
                assert_eq!(*target_page, Some(10));
                assert_eq!(*target_x, Some(108.0));
                assert_eq!(*target_y, Some(339.0));
            }
            other => panic!("expected InternalNamed, got {:?}", other),
        }
    }

    /// CR-62: span carrying `data-link-kind="external-uri"` attrs lands on
    /// `PdfTextElement.link` as `ExternalUri`. Matches what BlazePDF2XHTML
    /// emits for PDActionURI link annotations.
    #[test]
    fn cr62_span_with_external_uri_link_lands_on_element() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="612" data-height="792" />
<p>
<span class="f6" data-bbox="405.7,527.1,99.4,8.0" data-line="11" data-segment="1" data-link-bbox="404.7,525.2,100.3,11.1" data-link-kind="external-uri" data-link-target-url="https://github.com/tensorflow/tensor2tensor">https://github.com/</span>
</p>
</div>"#,
        );
        let out = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(out.text_elements.len(), 1);
        let link = out.text_elements[0].link.as_ref().expect("link Some");
        match &link.kind {
            PdfLinkKind::ExternalUri { url } => {
                assert_eq!(url, "https://github.com/tensorflow/tensor2tensor");
            }
            other => panic!("expected ExternalUri, got {:?}", other),
        }
    }

    /// CR-62: backward-compat — spans without any `data-link-*` attrs have
    /// `link == None`. Verifies the parser stays a no-op on pre-CR-62 XHTML.
    #[test]
    fn cr62_span_without_link_attrs_has_no_link_field() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="100.0" data-height="100.0" />
<p>
<span class="f1" data-bbox="10,10,50,12" data-line="0" data-segment="0">just text</span>
</p>
</div>"#,
        );
        let out = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(out.text_elements.len(), 1);
        assert!(out.text_elements[0].link.is_none(),
            "spans without data-link-* should produce link: None");
    }

    /// CR-40 acceptance: spans inside `<aside data-rotation="...">` inherit
    /// the rotation; spans outside the aside have rotation=0.
    #[test]
    fn aside_propagates_rotation_to_inner_spans_only() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="100.0" data-height="100.0" />
<aside data-rotation="90">
<p>
<span class="f1" data-bbox="10,10,50,12" data-line="0" data-segment="0">Rotated text</span>
</p>
</aside>
<p>
<span class="f1" data-bbox="20,20,50,12" data-line="0" data-segment="0">Normal text</span>
</p>
</div>"#,
        );
        let output = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(output.text_elements.len(), 2);

        let rotated = output
            .text_elements
            .iter()
            .find(|e| e.text.trim() == "Rotated text")
            .expect("rotated element");
        let normal = output
            .text_elements
            .iter()
            .find(|e| e.text.trim() == "Normal text")
            .expect("normal element");

        assert_eq!(
            rotated.placement.rotation, 90,
            "span inside <aside data-rotation=\"90\"> should inherit rotation"
        );
        assert_eq!(
            normal.placement.rotation, 0,
            "span outside aside should have rotation=0"
        );
    }

    // ── CR-51: data-segment as reading-order anchor ──────────────────────────

    /// CR-51: Shannon-shape — alternating f1/f4 spans on the same `data-line`
    /// have different Y baselines (small-caps font drops a pixel). Pre-fix,
    /// the Y-first spatial sort flattened them into `T D N C HE ISCRETE …`.
    /// Post-fix, within-line X drives reading_order (which here happens to
    /// equal segment order 0..7, since Tika emitted Shannon's section header
    /// in left-to-right source order).
    #[test]
    fn cr51_within_line_x_overrides_y_baseline_for_shannon_header() {
        let xhtml = xhtml_with(
            r#"<div class="page"><div class="page-meta" data-page="0" data-width="612.0" data-height="792.0" />
<p>
<span class="f4" data-bbox="219.2,249.8,19.5,9.7" data-line="0" data-segment="0">1. T</span>
<span class="f1" data-bbox="239.3,251.3,11.1,7.7" data-line="0" data-segment="1">HE</span>
<span class="f4" data-bbox="253.3,249.8,7.2,9.7" data-line="0" data-segment="2">D</span>
<span class="f1" data-bbox="261.0,251.3,35.2,7.7" data-line="0" data-segment="3">ISCRETE</span>
<span class="f4" data-bbox="299.3,249.8,7.2,9.7" data-line="0" data-segment="4">N</span>
<span class="f1" data-bbox="307.0,251.3,39.8,7.7" data-line="0" data-segment="5">OISELESS</span>
<span class="f4" data-bbox="349.7,249.8,6.7,9.7" data-line="0" data-segment="6">C</span>
<span class="f1" data-bbox="356.8,251.3,35.3,7.7" data-line="0" data-segment="7">HANNEL</span>
</p>
</div>"#,
        );
        let output = parse_xhtml(&xhtml).expect("parse should succeed");
        assert_eq!(output.text_elements.len(), 8);

        // After the fix, reading_order matches the within-line X order
        // (which here equals segment order 0..7 because Tika emitted these
        // spans in left-to-right source order). The text stream reads
        // `1. T`, `HE`, `D`, `ISCRETE`, `N`, `OISELESS`, `C`, `HANNEL`.
        let mut by_order: Vec<&PdfTextElement> = output.text_elements.iter().collect();
        by_order.sort_by_key(|e| e.reading_order);
        let texts: Vec<&str> = by_order.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["1. T", "HE", "D", "ISCRETE", "N", "OISELESS", "C", "HANNEL"],
            "spans must read in within-line X order (Y baseline must not split)"
        );

        // And the segment_number field on each element matches its position.
        for (i, el) in by_order.iter().enumerate() {
            assert_eq!(
                el.placement.segment_number, i as u32,
                "element {} should have segment_number={}, got {}",
                i, i, el.placement.segment_number
            );
        }
    }

    /// CR-51: Multi-column synth — two elements with identical
    /// `(paragraph, line, segment)` but different `region_label` must be
    /// disambiguated by `region_rank`. All "1" before all "2", regardless of
    /// the order the parser saw them (Tika in a two-column doc may
    /// interleave). We test the sort directly here since `region_label` is
    /// always `None` after `parse_xhtml` — it's set later by
    /// `analytics::reading_order::tag_and_resort`. The sort itself, though,
    /// must respect the field when present.
    #[test]
    fn cr51_region_rank_disambiguates_multi_column() {
        // Build four elements: two columns, two lines each. Tika emitted
        // interleaved (L1, R1, L2, R2). After the sort, region "1" (left)
        // must come before region "2" (right).
        let mut elements: Vec<PdfTextElement> = vec![
            mk_test_element(1, "L1", 110.0, 100.0, 0, 0, 0, Some("1")),
            mk_test_element(1, "R1", 400.0, 100.0, 0, 0, 0, Some("2")),
            mk_test_element(1, "L2", 110.0, 120.0, 0, 1, 0, Some("1")),
            mk_test_element(1, "R2", 400.0, 120.0, 0, 1, 0, Some("2")),
        ];

        let mut all_elements: Vec<PdfTextElement> = Vec::new();
        let mut global_reading_order: u32 = 0;
        finalize_page_elements(&mut elements, &mut all_elements, &mut global_reading_order);

        let texts: Vec<&str> = all_elements.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["L1", "L2", "R1", "R2"],
            "region_rank must drive all of column 1 before all of column 2"
        );
    }

    /// CR-51: Within-line X is the primary key — when two elements share
    /// `(page, paragraph, line)`, the one with the smaller X wins regardless
    /// of which arrived first in document order.
    #[test]
    fn cr51_within_line_x_drives_order() {
        // Same (paragraph, line); different X. Left (110) must come before
        // right (400) in reading order.
        let mut elements: Vec<PdfTextElement> = vec![
            mk_test_element(1, "right", 400.0, 100.0, 0, 0, 0, None),
            mk_test_element(1, "left", 110.0, 100.0, 0, 0, 0, None),
        ];

        let mut all_elements: Vec<PdfTextElement> = Vec::new();
        let mut global_reading_order: u32 = 0;
        finalize_page_elements(&mut elements, &mut all_elements, &mut global_reading_order);

        let texts: Vec<&str> = all_elements.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec!["left", "right"],
            "within-line X drives reading order"
        );

        // Reading-order also assigned monotonically.
        assert_eq!(all_elements[0].reading_order, 0);
        assert_eq!(all_elements[1].reading_order, 1);
    }

    /// CR-51 (revision): rfc-quic-shape — Tika emits an inline-emphasized
    /// span (`MAY` in f9-class) with a LATER `data-segment` than its visual
    /// neighbors. The X coordinate puts it between segments 0 and 1, but the
    /// PDF content-stream order is 0, 1, 2. The fix asserts X-within-line
    /// drives reading order, so the styled span lands at its visual position
    /// regardless of segment number.
    #[test]
    fn cr51_within_line_x_overrides_segment_for_rfc_quic_emphasis() {
        // X positions reproduce the rfc-quic RESET_STREAM line: body 0 ends
        // at ~x=207, MAY starts at x=207.1, body 1 starts at x=228.0.
        // Segments are 0, 1, 2 in Tika emission order, but visual order is
        // body0 → MAY → body1.
        let mut elements: Vec<PdfTextElement> = vec![
            mk_test_element(1, "received. An implementation ", 65.9, 457.0, 0, 2, 0, None),
            mk_test_element(1, " interrupt delivery of stream data", 228.0, 457.0, 0, 2, 1, None),
            mk_test_element(1, "MAY", 207.1, 457.0, 0, 2, 2, None),
        ];

        let mut all_elements: Vec<PdfTextElement> = Vec::new();
        let mut global_reading_order: u32 = 0;
        finalize_page_elements(&mut elements, &mut all_elements, &mut global_reading_order);

        let texts: Vec<&str> = all_elements.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "received. An implementation ",
                "MAY",
                " interrupt delivery of stream data",
            ],
            "within-line X order must beat data-segment order — the rfc-quic regression"
        );
    }

    // Helper for CR-51 sort tests — builds a minimal PdfTextElement with the
    // sort-relevant fields set; everything else gets defaults.
    #[allow(clippy::too_many_arguments)]
    fn mk_test_element(
        page: u32,
        text: &str,
        x: f32,
        y: f32,
        paragraph: u32,
        line: u32,
        segment: u32,
        region: Option<&str>,
    ) -> PdfTextElement {
        PdfTextElement {
            text: text.to_string(),
            style_info: FontClass {
                class_name: "f1".to_string(),
                font_family: "Times".to_string(),
                font_size: 10.0,
                font_style: "normal".to_string(),
                font_weight: "normal".to_string(),
                color: "#000000".to_string(),
            },
            placement: Placement {
                page_number: page,
                bounding_box: BoundingBox {
                    x,
                    y,
                    width: 50.0,
                    height: 10.0,
                },
                line_number: line,
                segment_number: segment,
                rotation: 0,
                paragraph_number: paragraph,
                region_label: region.map(|s| s.to_string()),
                page_width: 612.0,
                page_height: 792.0,
            },
            reading_order: 0,
            bookmark_match: None,
            token_count: 1,
            raw_tags: vec![],
            link: None,
        }
    }

    // ── CR-67 Part A: section-numbering trailing-dot strip ──────────────────

    /// CR-67 Part A: `normalize_for_match` strips the dot that follows a
    /// leading section-numbering prefix so body-text emissions like
    /// `"3.1. Approach 1"` align with outline entries `"3.1 Approach 1"`.
    /// Covers numeric (`3.1.`, `5.`) and letter-then-digit (`A1.`, `B.`)
    /// prefix variants. The rescue is what lets alphafold's 3.1 header
    /// reach the bookmark-match disjunct (CR-41) after `isolated_in_leaf`
    /// rejects it.
    #[test]
    fn test_normalize_for_match_strips_section_numbering_trailing_dot() {
        // Numeric multi-level prefix — the alphafold case.
        assert_eq!(
            normalize_for_match("3.1. Approach 1: Fix model sizes"),
            "3.1 Approach 1: Fix model sizes",
        );
        // Single-level numeric prefix.
        assert_eq!(
            normalize_for_match("5. Conclusion"),
            "5 Conclusion",
        );
        // Letter-then-optional-digit prefix (appendix style).
        assert_eq!(
            normalize_for_match("A. Appendix"),
            "A Appendix",
        );
        assert_eq!(
            normalize_for_match("B1. Detailed Notes"),
            "B1 Detailed Notes",
        );
    }

    /// CR-67 Part A: the strip is tightly scoped to leading section
    /// prefixes; a trailing period at the end of a normal sentence must
    /// pass through unchanged. Same for unrelated leading text that
    /// happens to contain a dot.
    #[test]
    fn test_normalize_for_match_preserves_sentence_period() {
        // Plain sentence — trailing period is not a section prefix.
        assert_eq!(
            normalize_for_match("This sentence ends in a period."),
            "This sentence ends in a period.",
        );
        // No whitespace after the prefix-shape dot → not a prefix
        // either (this is a numbered identifier, not a header).
        assert_eq!(
            normalize_for_match("3.14is pi"),
            "3.14is pi",
        );
        // Lowercase-letter prefix is not in the allowed grammar.
        assert_eq!(
            normalize_for_match("a. lowercase header-ish"),
            "a. lowercase header-ish",
        );
    }
}
