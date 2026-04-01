//! Blazegraph XHTML Parser
//!
//! Parses the Blazegraph XHTML intermediate format produced by PDF backends
//! into PreprocessorOutput. This parser is shared across all PDF backends.
//!
//! The Blazegraph XHTML format includes:
//! - Page divs with data-page attributes
//! - Spans with data-bbox, data-line, data-segment attributes
//! - CSS font classes in <style> block
//! - Document metadata in <meta> tags
//! - Bookmarks/TOC in <ul> structure

use crate::types::*;
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

// Pre-compiled regexes for XHTML parsing performance
static PAGE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<div class="page"[^>]*>(.*?)</div>"#).unwrap());

static PARAGRAPH_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<p[^>]*>(.*?)</p>").unwrap());

static SPAN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<span[^>]*class="([^"]*)"[^>]*data-bbox="([^"]*)"[^>]*data-line="([^"]*)"[^>]*data-segment="([^"]*)"[^>]*>([^<]*)</span>"#).unwrap()
});

static META_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"<meta\s+name="([^"]*)"[^>]*content="([^"]*)"[^>]*/?>"#).unwrap()
});

static STYLE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<style[^>]*>(.*?)</style>").unwrap());

static FONT_CLASS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(\w+)\s*\{\s*font-family:\s*([^;]+);\s*font-size:\s*([^;]+);\s*font-style:\s*([^;]+);\s*font-weight:\s*([^;]+);\s*color:\s*([^;]+);\s*\}").unwrap()
});



/// Parse Blazegraph XHTML into PreprocessorOutput
///
/// This is the main entry point for XHTML parsing. It extracts:
/// - Text elements with positioning and styling
/// - Document metadata
/// - Style data (font classes)
/// - Bookmark data (if present)
pub fn parse_xhtml(xhtml: &str) -> Result<PreprocessorOutput> {
    let (text_elements, metadata, style_data, bookmark_data) = parse_xhtml_content(xhtml)?;

    Ok(PreprocessorOutput {
        text_elements,
        metadata,
        style_data,
        bookmark_data,
    })
}

/// Parse XHTML content into structured components
fn parse_xhtml_content(
    xhtml: &str,
) -> Result<(
    Vec<PdfTextElement>,
    DocumentMetadata,
    StyleData,
    Option<BookmarkData>,
)> {
    // Extract enhanced metadata from <meta> tags
    let metadata = extract_enhanced_metadata(xhtml)?;

    // Extract style data from CSS
    let style_data = extract_style_data(xhtml)?;

    // Extract bookmark data
    let bookmark_data = extract_bookmark_data(xhtml)?;

    // Extract text elements with full resolution (needs style and bookmark data)
    let text_elements = extract_text_elements(xhtml, &style_data, &bookmark_data)?;

    println!(
        "✅ XHTML parsing complete: {} text elements, {} font classes, {} bookmarks",
        text_elements.len(),
        style_data.font_classes.len(),
        bookmark_data
            .as_ref()
            .map(|b| b.sections.len())
            .unwrap_or(0)
    );

    Ok((text_elements, metadata, style_data, bookmark_data))
}

/// Extract text elements with hierarchical parsing: pages → paragraphs → spans
fn extract_text_elements(
    xhtml: &str,
    style_data: &StyleData,
    bookmark_data: &Option<BookmarkData>,
) -> Result<Vec<PdfTextElement>> {
    // Pre-allocate capacity based on estimated element count
    let estimated_elements = xhtml.matches("<span").count();
    let mut text_elements = Vec::with_capacity(estimated_elements);
    let mut global_paragraph_number = 0u32;
    let mut global_reading_order = 0u32;

    // Create bookmark lookup
    let bookmark_sections: Vec<BookmarkSection> = bookmark_data
        .as_ref()
        .map(|bd| bd.sections.clone())
        .unwrap_or_default();

    let mut total_pages = 0;
    for (page_index, page_cap) in PAGE_REGEX.captures_iter(xhtml).enumerate() {
        let page_number = (page_index + 1) as u32;
        total_pages = page_number;
        let mut page_elements = Vec::new();

        if let Some(page_content) = page_cap.get(1) {
            let page_html = page_content.as_str();

            for p_cap in PARAGRAPH_REGEX.captures_iter(page_html) {
                if let Some(p_content) = p_cap.get(1) {
                    let paragraph_html = p_content.as_str();

                    extract_spans_from_paragraph(
                        paragraph_html,
                        page_number,
                        global_paragraph_number,
                        style_data,
                        &bookmark_sections,
                        &mut page_elements,
                    )?;

                    global_paragraph_number += 1;
                }
            }

            // Sort page elements by spatial position: Y first (top to bottom), then X (left to right)
            page_elements.sort_unstable_by(|a, b| {
                a.bounding_box
                    .y
                    .total_cmp(&b.bounding_box.y)
                    .then_with(|| a.bounding_box.x.total_cmp(&b.bounding_box.x))
            });

            // Assign global reading order to sorted elements
            for element in &mut page_elements {
                element.reading_order = global_reading_order;
                global_reading_order += 1;
            }

            text_elements.extend(page_elements);
        }
    }

    println!(
        "📊 Total extraction: {} text elements from {} paragraphs across {} pages",
        text_elements.len(),
        global_paragraph_number,
        total_pages
    );

    Ok(text_elements)
}

/// Extract spans from a single paragraph with proper page and paragraph context
fn extract_spans_from_paragraph(
    paragraph_html: &str,
    page_number: u32,
    paragraph_number: u32,
    style_data: &StyleData,
    bookmark_sections: &[BookmarkSection],
    text_elements: &mut Vec<PdfTextElement>,
) -> Result<()> {
    for cap in SPAN_REGEX.captures_iter(paragraph_html) {
        if let (Some(class), Some(bbox_str), Some(line_str), Some(segment_str), Some(text)) =
            (cap.get(1), cap.get(2), cap.get(3), cap.get(4), cap.get(5))
        {
            let text_content = text.as_str().trim();
            if text_content.is_empty() {
                continue;
            }

            // Parse bounding box: "x,y,width,height"
            let bbox_parts: Vec<&str> = bbox_str.as_str().split(',').collect();
            if bbox_parts.len() == 4 {
                if let (Ok(x), Ok(y), Ok(width), Ok(height)) = (
                    bbox_parts[0].parse::<f32>(),
                    bbox_parts[1].parse::<f32>(),
                    bbox_parts[2].parse::<f32>(),
                    bbox_parts[3].parse::<f32>(),
                ) {
                    let line_number = line_str.as_str().parse::<u32>().unwrap_or(0);
                    let segment_number = segment_str.as_str().parse::<u32>().unwrap_or(0);

                    // Resolve font class from style_data
                    let font_class_name = class.as_str();
                    let resolved_font_class =
                        if let Some(font_class) = style_data.font_classes.get(font_class_name) {
                            font_class.clone()
                        } else {
                            fallback_font(font_class_name)
                        };

                    // Check for bookmark match (normalize whitespace for fuzzy matching)
                    let normalize_ws = |s: &str| -> String {
                        s.split_whitespace().collect::<Vec<_>>().join(" ")
                    };
                    let text_normalized = normalize_ws(text_content);
                    let bookmark_match = bookmark_sections
                        .iter()
                        .find(|section| normalize_ws(&section.title) == text_normalized)
                        .cloned();

                    text_elements.push(PdfTextElement {
                        text: text_content.to_string(),
                        style_info: resolved_font_class,
                        bounding_box: BoundingBox {
                            x,
                            y,
                            width,
                            height,
                        },
                        page_number,
                        paragraph_number,
                        line_number,
                        segment_number,
                        reading_order: 0, // Will be assigned during spatial sorting
                        bookmark_match,
                        token_count: estimate_token_count(text_content),
                    });
                }
            }
        }
    }

    Ok(())
}

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

/// Extract enhanced metadata from <meta> tags
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

/// Extract style data from CSS <style> block
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

    println!("⚠️  No CSS styles found in XHTML - returning empty StyleData");
    Ok(StyleData {
        font_classes: HashMap::new(),
    })
}

/// Extract bookmark data from nested <ul><li> structure emitted by Tika.
///
/// Tika emits the PDF outline (document bookmarks) as nested <ul>/<li> elements
/// after the </style> block. The nesting depth corresponds to the outline hierarchy:
///   <ul>           ← level 1
///     <li>Title</li>
///     <ul>         ← level 2
///       <li>Sub</li>
///     </ul>
///   </ul>
///
/// Previous implementation used `rfind("<ul>")` which found only the innermost
/// (last) <ul> block, dropping 99% of outline entries. See CR-XX.
fn extract_bookmark_data(xhtml: &str) -> Result<Option<BookmarkData>> {
    // Find the bookmark region: first <ul> after </style> (Tika emits bookmarks
    // at the end of the document, after the CSS style block)
    let search_start = xhtml.rfind("</style>").unwrap_or(0);
    let bookmark_region = &xhtml[search_start..];

    // Find the first <ul> in the bookmark region
    let Some(first_ul) = bookmark_region.find("<ul>") else {
        return Ok(None);
    };

    let bookmark_html = &bookmark_region[first_ul..];

    // Parse the nested structure by walking through tags, tracking depth
    let mut sections = Vec::new();
    let mut depth: u32 = 0;

    // Simple state machine: scan for <ul>, </ul>, <li>...</li>
    let mut pos = 0;
    let bytes = bookmark_html.as_bytes();
    let len = bytes.len();

    while pos < len {
        if bytes[pos] != b'<' {
            pos += 1;
            continue;
        }

        let tag_start = pos;
        // Find end of tag
        let Some(tag_end_offset) = bookmark_html[pos..].find('>') else {
            break;
        };
        let tag_end = pos + tag_end_offset + 1;
        let tag = &bookmark_html[tag_start..tag_end];

        if tag == "<ul>" {
            depth += 1;
        } else if tag == "</ul>" {
            if depth == 0 {
                break; // Malformed, stop
            }
            depth -= 1;
            if depth == 0 {
                break; // Closed the outermost <ul>, we're done
            }
        } else if tag == "<li>" {
            // Extract content until </li>
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
