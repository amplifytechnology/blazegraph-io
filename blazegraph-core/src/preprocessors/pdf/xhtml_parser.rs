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

static META_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta\s+name="([^"]*)"[^>]*content="([^"]*)"[^>]*/?>"#).unwrap()
});

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
    let t0 = Instant::now();
    let metadata = extract_enhanced_metadata(xhtml)?;
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

/// Sort page elements spatially (Y then X) and assign global reading_order.
fn finalize_page_elements(
    page_elements: &mut Vec<PdfTextElement>,
    all_elements: &mut Vec<PdfTextElement>,
    global_reading_order: &mut u32,
) {
    page_elements.sort_unstable_by(|a, b| {
        a.placement
            .bounding_box
            .y
            .total_cmp(&b.placement.bounding_box.y)
            .then_with(|| {
                a.placement
                    .bounding_box
                    .x
                    .total_cmp(&b.placement.bounding_box.x)
            })
    });
    for el in page_elements.iter_mut() {
        el.reading_order = *global_reading_order;
        *global_reading_order += 1;
    }
    all_elements.append(page_elements);
}

/// NFKC + whitespace fold for cross-source string equivalence. Used to align
/// PDF-extracted glyph runs (which can contain ligatures like ﬀ ﬁ) with
/// bookmark titles (which usually decompose to plain ASCII). Also collapses
/// any internal whitespace runs to a single space so layout-driven word
/// breaks don't defeat matching.
fn normalize_for_match(s: &str) -> String {
    let nfkc: String = s.nfkc().collect();
    nfkc.split_whitespace().collect::<Vec<_>>().join(" ")
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
    style_data: &StyleData,
    bookmark_lookup: &HashMap<String, BookmarkSection>,
) -> Option<PdfTextElement> {
    // Parse bounding box: "x,y,width,height"
    let bbox_parts: Vec<&str> = span_bbox.split(',').collect();
    if bbox_parts.len() != 4 {
        return None;
    }
    let (x, y, width, height) = match (
        bbox_parts[0].trim().parse::<f32>(),
        bbox_parts[1].trim().parse::<f32>(),
        bbox_parts[2].trim().parse::<f32>(),
        bbox_parts[3].trim().parse::<f32>(),
    ) {
        (Ok(x), Ok(y), Ok(w), Ok(h)) => (x, y, w, h),
        _ => return None,
    };

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
// Metadata extraction (regex, stable)
// ============================================================================

/// Extract enhanced metadata from <meta> tags.
fn extract_enhanced_metadata(xhtml: &str) -> Result<DocumentMetadata> {
    let mut metadata = DocumentMetadata::default();

    for cap in META_REGEX.captures_iter(xhtml) {
        if let (Some(name), Some(content)) = (cap.get(1), cap.get(2)) {
            let name_str = name.as_str();
            let content_str = content.as_str().to_string();

            match name_str {
                "dc:title" => metadata.title = Some(content_str),
                "dc:creator" => metadata.author = Some(content_str),
                "dc:language" => metadata.language = Some(content_str),
                "xmp:dc:publisher" | "dc:publisher" => metadata.publisher = Some(content_str),
                "xmp:CreatorTool" => metadata.creator_tool = Some(content_str),
                "pdf:producer" => metadata.producer = Some(content_str),
                "pdf:PDFVersion" => metadata.pdf_version = Some(content_str),
                "dcterms:created" => metadata.created = Some(content_str),
                "dcterms:modified" => metadata.modified = Some(content_str),
                "dc:description" => metadata.description = Some(content_str),
                "pdf:encrypted" => metadata.encrypted = Some(content_str == "true"),
                "pdf:hasMarkedContent" => metadata.has_marked_content = Some(content_str == "true"),
                "xmpTPg:NPages" => {
                    if let Ok(pages) = content_str.parse::<u32>() {
                        metadata.page_count = pages;
                    }
                }
                _ => {}
            }
        }
    }

    Ok(metadata)
}

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
}
