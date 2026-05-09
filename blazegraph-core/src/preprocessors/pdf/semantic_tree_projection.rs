//! PDF channel exit point — projects `Vec<ParsedPdfElement>` (post-rules,
//! post-promote_same_line, stable) onto the channel-agnostic
//! `Vec<SemanticTreeElement>`. Everything downstream (`GraphBuilder`)
//! operates on the universal type.
//!
//! Drops PDF-channel-internal fields (`bookmark_match`, `reading_order`,
//! `position`, raw `FontClass`). Replaces them with best-effort
//! projections (FontClass → `StyleInfo`) or omits them entirely. Assigns
//! `text_order = vec_index` so the deterministic ID generator can salt
//! IDs from a stable, sortable scalar.
//!
//! See `docs/P2/core/handoffs/2026-05-09-A1-A2-semantic-tree-element.md`
//! for the design rationale.

use crate::types::{
    ParsedElementType, ParsedPdfElement, PhysicalLocation, SemanticElementType,
    SemanticTreeElement, StyleInfo,
};

/// Project the rule-engine output onto the channel-agnostic
/// `SemanticTreeElement` type. This is the PDF channel's exit point —
/// everything downstream (`GraphBuilder`) operates on the universal type.
///
/// See module docs for what's preserved vs dropped.
pub fn project_to_semantic_tree(elements: Vec<ParsedPdfElement>) -> Vec<SemanticTreeElement> {
    elements
        .into_iter()
        .enumerate()
        .map(|(index, parsed)| {
            let element_type = project_element_type(&parsed.element_type);
            let hierarchy_level = normalize_hierarchy_level(element_type, parsed.hierarchy_level);
            let physical_location = parsed.placement.as_ref().map(|p| PhysicalLocation {
                page: p.page_number,
                bounding_box: p.bounding_box.clone(),
            });
            let style = Some(project_style(&parsed));

            SemanticTreeElement {
                text: parsed.text,
                element_type,
                hierarchy_level,
                text_order: index as u32,
                physical_location,
                style,
                token_count: parsed.token_count,
            }
        })
        .collect()
}

/// Map `ParsedElementType` (rule-engine output, PDF-channel-internal) to
/// the channel-agnostic `SemanticElementType`.
///
/// `List` and `ListItem` collapse to `Paragraph` for v1 — the design flow
/// defers list semantics until the round-trip arc lands. Future Track-B
/// work will lift them to first-class variants.
fn project_element_type(input: &ParsedElementType) -> SemanticElementType {
    match input {
        ParsedElementType::Section => SemanticElementType::Section,
        ParsedElementType::Paragraph => SemanticElementType::Paragraph,
        // Deferred — collapses to Paragraph in v1. See design flow.
        ParsedElementType::List => SemanticElementType::Paragraph,
        ParsedElementType::ListItem => SemanticElementType::Paragraph,
        ParsedElementType::Header => SemanticElementType::Header,
        ParsedElementType::Footer => SemanticElementType::Footer,
        ParsedElementType::Margin => SemanticElementType::Margin,
    }
}

/// Sentinel-level rule: only `Section` carries a meaningful hierarchy
/// level (1 = top-level, 2 = nested, ...). For `Paragraph` / `Header` /
/// `Footer` / `Margin`, normalize to `0` — these are leaves attached to
/// the current open Section. See `SemanticTreeElement::hierarchy_level`.
fn normalize_hierarchy_level(element_type: SemanticElementType, input_level: u32) -> u32 {
    match element_type {
        SemanticElementType::Section => input_level,
        SemanticElementType::Paragraph
        | SemanticElementType::Header
        | SemanticElementType::Footer
        | SemanticElementType::Margin => 0,
    }
}

/// Project `FontClass` (PDF-channel-internal style metadata) into
/// channel-agnostic `StyleInfo` (alias for `StyleMetadata`). Lossless on
/// the fields the downstream consumer (`DocumentNode.style_info`)
/// historically populates: font_family, font_size, is_bold, is_italic,
/// color, font_class.
fn project_style(parsed: &ParsedPdfElement) -> StyleInfo {
    let font = &parsed.style_info;
    StyleInfo {
        font_class: font.class_name.clone(),
        font_size: Some(font.font_size),
        font_family: Some(font.font_family.clone()),
        color: Some(font.color.clone()),
        is_bold: font.font_weight.to_lowercase().contains("bold"),
        is_italic: font.font_style.to_lowercase().contains("italic"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        BookmarkSection, BoundingBox, FontClass, ParsedElementType, ParsedPdfElement, Placement,
    };

    /// Build a `ParsedPdfElement` with every field populated. Tests that
    /// care about specific field shapes mutate the result.
    fn fixture(element_type: ParsedElementType, hierarchy_level: u32) -> ParsedPdfElement {
        ParsedPdfElement {
            element_type,
            text: "sample text".to_string(),
            hierarchy_level,
            position: 7,
            style_info: FontClass {
                class_name: "f1".to_string(),
                font_family: "LiberationSerif".to_string(),
                font_size: 12.0,
                font_style: "italic".to_string(),
                font_weight: "bold".to_string(),
                color: "#112233".to_string(),
            },
            placement: Some(Placement {
                page_number: 3,
                bounding_box: BoundingBox {
                    x: 1.0,
                    y: 2.0,
                    width: 10.0,
                    height: 12.0,
                },
                line_number: 5,
                segment_number: 0,
                rotation: 0,
                paragraph_number: 4,
                region_label: Some("1".to_string()),
                page_width: 612.0,
                page_height: 792.0,
            }),
            reading_order: 99,
            bookmark_match: Some(BookmarkSection {
                title: "Some Title".to_string(),
                order: 0,
                level: 1,
            }),
            token_count: 42,
        }
    }

    #[test]
    fn projects_all_fields_happy_path() {
        // Section so hierarchy_level survives normalization (level=2 round-trips).
        let input = fixture(ParsedElementType::Section, 2);
        let projected = project_to_semantic_tree(vec![input]);

        assert_eq!(projected.len(), 1);
        let element = &projected[0];

        // Universal fields preserved.
        assert_eq!(element.text, "sample text");
        assert_eq!(element.element_type, SemanticElementType::Section);
        assert_eq!(element.hierarchy_level, 2);
        assert_eq!(element.text_order, 0);
        assert_eq!(element.token_count, 42);

        // PhysicalLocation projected from Placement.
        let physical = element
            .physical_location
            .as_ref()
            .expect("physical_location should be Some for PDF source");
        assert_eq!(physical.page, 3);
        assert_eq!(physical.bounding_box.x, 1.0);
        assert_eq!(physical.bounding_box.y, 2.0);
        assert_eq!(physical.bounding_box.width, 10.0);
        assert_eq!(physical.bounding_box.height, 12.0);

        // FontClass projected to StyleInfo (best-effort — see AAR).
        let style = element.style.as_ref().expect("style should be Some");
        assert_eq!(style.font_class, "f1");
        assert_eq!(style.font_size, Some(12.0));
        assert_eq!(style.font_family.as_deref(), Some("LiberationSerif"));
        assert_eq!(style.color.as_deref(), Some("#112233"));
        assert!(style.is_bold);
        assert!(style.is_italic);
    }

    #[test]
    fn text_order_is_sequential_zero_indexed() {
        let inputs: Vec<_> = (0..5)
            .map(|_| fixture(ParsedElementType::Paragraph, 1))
            .collect();
        let projected = project_to_semantic_tree(inputs);

        assert_eq!(projected.len(), 5);
        for (i, element) in projected.iter().enumerate() {
            assert_eq!(
                element.text_order, i as u32,
                "text_order at index {i} should equal {i}, got {}",
                element.text_order,
            );
        }
    }

    #[test]
    fn element_type_mapping_covers_all_variants() {
        let cases = [
            (ParsedElementType::Section, SemanticElementType::Section),
            (ParsedElementType::Paragraph, SemanticElementType::Paragraph),
            // List/ListItem collapse to Paragraph (deferred — design flow scope).
            (ParsedElementType::List, SemanticElementType::Paragraph),
            (ParsedElementType::ListItem, SemanticElementType::Paragraph),
            (ParsedElementType::Header, SemanticElementType::Header),
            (ParsedElementType::Footer, SemanticElementType::Footer),
            (ParsedElementType::Margin, SemanticElementType::Margin),
        ];

        for (input_type, expected_output) in cases {
            let projected = project_to_semantic_tree(vec![fixture(input_type.clone(), 1)]);
            assert_eq!(
                projected[0].element_type, expected_output,
                "{input_type:?} should map to {expected_output:?}",
            );
        }
    }

    #[test]
    fn placement_none_projects_to_physical_location_none() {
        // Edge case: a non-PDF source (synthetic, future MD/DOCX channels)
        // builds elements with `placement: None`. Projection must propagate
        // that as `physical_location: None`.
        let mut input = fixture(ParsedElementType::Paragraph, 1);
        input.placement = None;

        let projected = project_to_semantic_tree(vec![input]);
        assert!(
            projected[0].physical_location.is_none(),
            "placement=None should project to physical_location=None",
        );
    }

    #[test]
    fn hierarchy_level_normalized_to_zero_for_non_section_types() {
        // Even if the input has hierarchy_level = 1 (the rule-engine
        // default for non-Section types), the projection must normalize
        // to 0 — the sentinel value for "leaf attached to current
        // open Section".
        for non_section in [
            ParsedElementType::Paragraph,
            ParsedElementType::Header,
            ParsedElementType::Footer,
            ParsedElementType::Margin,
            ParsedElementType::List,
            ParsedElementType::ListItem,
        ] {
            let input = fixture(non_section.clone(), 5); // arbitrary nonzero
            let projected = project_to_semantic_tree(vec![input]);
            assert_eq!(
                projected[0].hierarchy_level, 0,
                "{non_section:?} should normalize hierarchy_level to 0, got {}",
                projected[0].hierarchy_level,
            );
        }

        // Sanity-check: Section preserves its hierarchy_level.
        let section = fixture(ParsedElementType::Section, 3);
        let projected = project_to_semantic_tree(vec![section]);
        assert_eq!(
            projected[0].hierarchy_level, 3,
            "Section must preserve hierarchy_level (it carries real depth info)",
        );
    }

    #[test]
    fn style_projects_from_fontclass_best_effort() {
        // Decision (see AAR): FontClass projects to Some(StyleInfo) with a
        // direct field-by-field map. is_bold/is_italic come from
        // case-insensitive substring match on font_weight / font_style —
        // matches the legacy GraphBuilder::apply_style_info behavior so
        // post-projection graph output stays structurally equivalent.
        let mut input = fixture(ParsedElementType::Paragraph, 1);
        input.style_info.font_weight = "Bold".to_string();
        input.style_info.font_style = "Italic".to_string();

        let projected = project_to_semantic_tree(vec![input]);
        let style = projected[0]
            .style
            .as_ref()
            .expect("style should be Some — FontClass always projects best-effort");
        assert!(
            style.is_bold,
            "Bold (capital) should match case-insensitive"
        );
        assert!(
            style.is_italic,
            "Italic (capital) should match case-insensitive",
        );

        // Normal style case.
        let mut normal = fixture(ParsedElementType::Paragraph, 1);
        normal.style_info.font_weight = "normal".to_string();
        normal.style_info.font_style = "normal".to_string();
        let projected_normal = project_to_semantic_tree(vec![normal]);
        let style_normal = projected_normal[0]
            .style
            .as_ref()
            .expect("style should be Some");
        assert!(!style_normal.is_bold);
        assert!(!style_normal.is_italic);
    }
}
