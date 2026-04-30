//! Blazegraph XHTML Parser
//!
//! Parses the Blazegraph XHTML intermediate format produced by PDF backends
//! into PreprocessorOutput. This parser is shared across all PDF backends.
//!
//! The Blazegraph XHTML format includes:
//! - Page divs with data-page attributes
//! - Bands: `<div class="band" data-band="N" data-columns="K">` — Y-band containers
//! - Asides: `<aside data-rotation="N">` — rotated content (e.g., arxiv sidebars)
//! - Spans with data-bbox, data-line, data-segment, data-column attributes
//! - CSS font classes in <style> block
//! - Document metadata in <meta> tags
//! - Bookmarks/TOC in <ul> structure
//!
//! Parser approach: quick-xml pull-parser in event mode. A lightweight state
//! machine tracks the current page, band context, and aside/rotation context
//! as the parser walks the event stream. This avoids the fragility of regex
//! on nested XHTML and naturally handles context propagation (band → span,
//! aside → span) without building a full DOM.
//!
//! quick-xml was chosen because it is already a workspace dependency, is
//! widely used in the Rust ecosystem, and supports pull-mode parsing with
//! attribute access — exactly what context tracking needs.

use crate::types::*;
use anyhow::Result;
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;
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
    let metadata = extract_enhanced_metadata(xhtml)?;
    let style_data = extract_style_data(xhtml)?;
    let bookmark_data = extract_bookmark_data(xhtml)?;
    let text_elements = extract_text_elements(xhtml, &style_data, &bookmark_data)?;

    println!(
        "XHTML parsing complete: {} text elements, {} font classes, {} bookmarks",
        text_elements.len(),
        style_data.font_classes.len(),
        bookmark_data
            .as_ref()
            .map(|b| b.sections.len())
            .unwrap_or(0)
    );

    Ok(PreprocessorOutput {
        text_elements,
        metadata,
        style_data,
        bookmark_data,
    })
}

// ============================================================================
// Text element extraction — pull-parser state machine
// ============================================================================

/// Parser context: which structural container are we currently inside?
#[derive(Debug, Clone, PartialEq)]
enum Container {
    None,
    Aside,
    Band,
}

/// State machine context threaded through the event loop.
struct ParseContext {
    // Page-level
    in_page: bool,
    page_number: u32,
    // div nesting depth: 1 = page div open, 2 = band div open inside page, etc.
    // Used to correctly identify which </div> closes which container.
    div_depth: usize,

    // Structural container (mutually exclusive at the immediate child-of-page level)
    container: Container,
    // div_depth at which the current band opened — so we know which </div> closes it
    band_div_depth: usize,

    // Band state (valid when container == Band)
    current_band: u32,
    current_nr_band_columns: u32,

    // Aside/rotation state (valid when container == Aside)
    current_rotation: i32,

    // Paragraph tracking
    in_paragraph: bool,
    paragraph_number: u32,

    // Span state
    in_span: bool,
    span_nesting: u32, // nested <span> depth inside the primary span (0 when in primary)
    span_start_pos: usize, // byte offset in the source XHTML right after the primary span's `>`
    span_class: String,
    span_bbox: String,
    span_line: u32,
    span_segment: u32,
    span_column: u32,
    span_text: String,
    span_raw_tags: Vec<String>,
}

impl ParseContext {
    fn new() -> Self {
        ParseContext {
            in_page: false,
            page_number: 0,
            div_depth: 0,
            container: Container::None,
            band_div_depth: 0,
            current_band: 0,
            current_nr_band_columns: 1,
            current_rotation: 0,
            in_paragraph: false,
            paragraph_number: 0,
            in_span: false,
            span_nesting: 0,
            span_start_pos: 0,
            span_class: String::new(),
            span_bbox: String::new(),
            span_line: 0,
            span_segment: 0,
            span_column: 0,
            span_text: String::new(),
            span_raw_tags: vec![],
        }
    }

    fn reset_span(&mut self) {
        self.in_span = false;
        self.span_nesting = 0;
        self.span_start_pos = 0;
        self.span_class.clear();
        self.span_bbox.clear();
        self.span_line = 0;
        self.span_segment = 0;
        self.span_column = 0;
        self.span_text.clear();
        self.span_raw_tags.clear();
    }
}

/// Get the string value of a named attribute from a quick-xml Attributes iterator.
fn get_attr(attrs: &quick_xml::events::attributes::Attributes, name: &[u8]) -> Option<String> {
    for a in attrs.clone().filter_map(Result::ok) {
        if a.key.as_ref() == name {
            return String::from_utf8(a.value.to_vec()).ok();
        }
    }
    None
}

/// Extract text elements using a pull-parser state machine.
///
/// The state machine tracks:
///   current page → current container (aside/band/none) → paragraph → span
///
/// Band and aside attributes are propagated to every span they contain.
fn extract_text_elements(
    xhtml: &str,
    style_data: &StyleData,
    bookmark_data: &Option<BookmarkData>,
) -> Result<Vec<PdfTextElement>> {
    let bookmark_sections: Vec<BookmarkSection> = bookmark_data
        .as_ref()
        .map(|bd| bd.sections.clone())
        .unwrap_or_default();

    let mut reader = Reader::from_str(xhtml);
    // Default trim_text is false in quick-xml — text is returned verbatim.
    // We trim at emit time (span_text.trim()) to handle whitespace from pretty-printed XHTML.

    let mut ctx = ParseContext::new();
    let mut page_elements: Vec<PdfTextElement> = Vec::new();
    let mut all_elements: Vec<PdfTextElement> = Vec::new();
    let mut global_reading_order: u32 = 0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let tag_name = e.name();
                let tag_str = std::str::from_utf8(tag_name.as_ref()).unwrap_or("");

                match tag_str {
                    "div" => {
                        // Track structural div depth only when not inside a span
                        if !ctx.in_span {
                            ctx.div_depth += 1;
                        }
                        let class = get_attr(&e.attributes(), b"class").unwrap_or_default();
                        if class == "page" {
                            // Flush previous page elements
                            if !page_elements.is_empty() {
                                finalize_page_elements(
                                    &mut page_elements,
                                    &mut all_elements,
                                    &mut global_reading_order,
                                );
                            }
                            ctx.in_page = true;
                            ctx.page_number += 1;
                            ctx.container = Container::None;
                            ctx.band_div_depth = 0;
                            ctx.current_band = 0;
                            ctx.current_nr_band_columns = 1;
                            ctx.current_rotation = 0;
                        } else if class == "band" && ctx.in_page {
                            ctx.container = Container::Band;
                            ctx.band_div_depth = ctx.div_depth;
                            ctx.current_band = get_attr(&e.attributes(), b"data-band")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            ctx.current_nr_band_columns =
                                get_attr(&e.attributes(), b"data-columns")
                                    .and_then(|v| v.parse().ok())
                                    .unwrap_or(1);
                            // Rotation resets when we enter a band (aside is sibling, not ancestor)
                            ctx.current_rotation = 0;
                        }
                        // other divs (e.g., inside a band) are tracked by div_depth but
                        // do not change container state.
                    }
                    "aside" if ctx.in_page => {
                        ctx.container = Container::Aside;
                        ctx.current_rotation = get_attr(&e.attributes(), b"data-rotation")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        ctx.current_band = 0;
                        ctx.current_nr_band_columns = 1;
                    }
                    "p" if ctx.in_page => {
                        ctx.in_paragraph = true;
                    }
                    "span" if ctx.in_page && ctx.in_paragraph => {
                        if ctx.in_span {
                            // Nested span inside the primary span: track depth only.
                            // Its bytes are captured via buffer_position slicing at close.
                            ctx.span_nesting += 1;
                        } else {
                            // Opening the primary span
                            ctx.in_span = true;
                            ctx.span_nesting = 0;
                            ctx.span_start_pos = reader.buffer_position();
                            ctx.span_class =
                                get_attr(&e.attributes(), b"class").unwrap_or_default();
                            ctx.span_bbox =
                                get_attr(&e.attributes(), b"data-bbox").unwrap_or_default();
                            ctx.span_line = get_attr(&e.attributes(), b"data-line")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            ctx.span_segment = get_attr(&e.attributes(), b"data-segment")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            ctx.span_column = get_attr(&e.attributes(), b"data-column")
                                .and_then(|v| v.parse().ok())
                                .unwrap_or(0);
                            ctx.span_text.clear();
                            ctx.span_raw_tags.clear();
                        }
                    }
                    _ => {}
                }
            }

            Ok(Event::Empty(_)) => {}

            Ok(Event::Text(ref e)) => {
                // Capture direct text of the primary span (not text inside nested tags).
                if ctx.in_span && ctx.span_nesting == 0 {
                    if let Ok(text) = e.unescape() {
                        ctx.span_text.push_str(&text);
                    }
                }
            }

            Ok(Event::End(ref e)) => {
                let tag_name = e.name();
                let tag_str = std::str::from_utf8(tag_name.as_ref()).unwrap_or("");

                match tag_str {
                    "span" if ctx.in_span => {
                        if ctx.span_nesting > 0 {
                            ctx.span_nesting -= 1;
                        } else {
                            // Closing the primary span: slice inner content, extract raw_tags, emit
                            let end_pos = reader.buffer_position() - 7; // len("</span>") == 7
                            ctx.span_raw_tags = extract_raw_tags(
                                xhtml.get(ctx.span_start_pos..end_pos).unwrap_or(""),
                            );
                            // Preserve boundary whitespace (collapse internal runs, keep at most one
                            // leading/trailing space) so downstream same-line concatenation can see
                            // whether Tika emitted a separator at the segment join.
                            let text_content = normalize_segment_text(&ctx.span_text);
                            if !text_content.trim().is_empty() {
                                if let Some(element) = build_element(
                                    &ctx,
                                    text_content,
                                    style_data,
                                    &bookmark_sections,
                                ) {
                                    page_elements.push(element);
                                }
                            }
                            ctx.reset_span();
                        }
                    }
                    "p" if ctx.in_paragraph => {
                        ctx.in_paragraph = false;
                        ctx.paragraph_number += 1;
                    }
                    "aside" if ctx.container == Container::Aside => {
                        ctx.container = Container::None;
                        ctx.current_rotation = 0;
                    }
                    "div" if !ctx.in_span => {
                        // Use div_depth to distinguish which </div> closes which container.
                        // Band div was opened at band_div_depth; page div was opened at depth 1.
                        // (When inside a span, </div> is caught by the raw_tags wildcard arm above.)
                        if ctx.container == Container::Band && ctx.div_depth == ctx.band_div_depth {
                            // Closing the band div
                            ctx.container = Container::None;
                            ctx.current_band = 0;
                            ctx.current_nr_band_columns = 1;
                            ctx.band_div_depth = 0;
                        } else if ctx.in_page && ctx.div_depth == 1 {
                            // Closing the page div (outermost page div is at depth 1)
                            ctx.in_page = false;
                        }
                        // Decrement after checks (depth was at current value during checks)
                        if ctx.div_depth > 0 {
                            ctx.div_depth -= 1;
                        }
                    }
                    _ => {}
                }
            }

            Ok(Event::Eof) => break,
            Err(_) => break, // Graceful: stop on malformed XML, return what we have
            _ => {}
        }
    }

    // Flush the last page
    if !page_elements.is_empty() {
        finalize_page_elements(
            &mut page_elements,
            &mut all_elements,
            &mut global_reading_order,
        );
    }

    println!(
        "Total extraction: {} text elements across {} pages",
        all_elements.len(),
        ctx.page_number
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

/// Build a `PdfTextElement` from the current span context.
fn build_element(
    ctx: &ParseContext,
    text_content: String,
    style_data: &StyleData,
    bookmark_sections: &[BookmarkSection],
) -> Option<PdfTextElement> {
    // Parse bounding box: "x,y,width,height"
    let bbox_parts: Vec<&str> = ctx.span_bbox.split(',').collect();
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
    let font_class = if let Some(fc) = style_data.font_classes.get(&ctx.span_class) {
        fc.clone()
    } else {
        fallback_font(&ctx.span_class)
    };

    // Check for bookmark match — NFKC + whitespace fold both sides so a
    // segment containing "oﬀer" matches a bookmark title with "offer".
    let normalize_match = |s: &str| -> String {
        let nfkc: String = s.nfkc().collect();
        nfkc.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    let text_normalized = normalize_match(&text_content);
    let bookmark_match = bookmark_sections
        .iter()
        .find(|s| normalize_match(&s.title) == text_normalized)
        .cloned();

    Some(PdfTextElement {
        text: text_content.clone(),
        style_info: font_class,
        placement: Placement {
            page_number: ctx.page_number,
            bounding_box: BoundingBox {
                x,
                y,
                width,
                height,
            },
            band: ctx.current_band,
            column: ctx.span_column,
            nr_band_columns: ctx.current_nr_band_columns,
            line_number: ctx.span_line,
            segment_number: ctx.span_segment,
            rotation: ctx.current_rotation,
            paragraph_number: ctx.paragraph_number,
        },
        reading_order: 0, // Assigned later in finalize_page_elements
        bookmark_match,
        token_count: estimate_token_count(&text_content),
        raw_tags: ctx.span_raw_tags.clone(),
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

fn estimate_token_count(text: &str) -> usize {
    text.len() / 4 // Rough estimation: ~4 characters per token
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
                    let mut font_classes = HashMap::new();

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
        font_classes: HashMap::new(),
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
