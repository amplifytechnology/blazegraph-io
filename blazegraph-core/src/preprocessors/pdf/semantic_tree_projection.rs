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
    ExternalRef, ExternalRefTarget, InternalRef, InternalRefTarget, ParsedElementType,
    ParsedPdfElement, PdfLinkKind, PhysicalLocation, SemanticElementType, SemanticTreeElement,
    StyleInfo, TargetPoint,
};

/// Project the rule-engine output onto the channel-agnostic
/// `SemanticTreeElement` type. This is the PDF channel's exit point —
/// everything downstream (`GraphBuilder`) operates on the universal type.
///
/// See module docs for what's preserved vs dropped.
///
/// **Canonical-form projection (v2.2.0+, CR-61).** Before constructing the
/// `SemanticTreeElement`, body text in the markdown-inline domain
/// (Section / Paragraph / Header / Footer / Margin per C-7a) is wrapped in
/// canonical emphasis tokens based on element-level style flags:
/// `is_bold && is_italic` → `***text***`, `is_bold` → `**text**`,
/// `is_italic` → `*text*`. This makes the in-graph `text` already
/// canonical (C-7b); the emitter stays dumb. Element-level only —
/// span-level emphasis (per-segment style preservation through merge)
/// is future work, and equation typography is explicitly out of scope
/// (see DT-05). The `style` field on the element is still populated
/// verbatim from Tika (DT-03) — the body-text emphasis tokens are the
/// canonical-form projection, the `style` JSON is the verbatim record;
/// they are not redundant.
pub fn project_to_semantic_tree(elements: Vec<ParsedPdfElement>) -> Vec<SemanticTreeElement> {
    elements
        .into_iter()
        .enumerate()
        .map(|(index, parsed)| {
            let element_type = project_element_type(&parsed.element_type);
            let physical_location = parsed.placement.as_ref().map(|p| PhysicalLocation {
                page: p.page_number,
                bounding_box: p.bounding_box.clone(),
            });
            let style = project_style(&parsed);

            // C-7b canonical emphasis projection. Applies only to elements
            // whose body is in the markdown-inline domain (C-7a). For
            // literal / literal-with-markers domains (CodeBlock / List /
            // Blockquote / Table) we skip wrapping — those preserve raw
            // source verbatim. Today's PDF channel never produces those
            // variants (rule engine maps List/ListItem → Paragraph; no
            // CodeBlock detection yet); the guard makes the rule explicit
            // for when classification improves.
            let text = if element_type.body_is_markdown_inline() {
                apply_canonical_emphasis(parsed.text.clone(), &style)
            } else {
                parsed.text.clone()
            };

            // CR-62: split the rule-engine's unified `links: Vec<PdfLinkAnnotation>`
            // into channel-agnostic `internal_refs[]` / `external_refs[]` at the
            // projection boundary. `text` field on each ref is the visible link
            // text (substring-anchor lookup key per CR-61); for the PDF channel
            // we extract it from the parsed element's text by intersecting the
            // annotation's source bbox with the element's glyph extent — but
            // because each pre-merge ParsedPdfElement corresponds to a single
            // span (and thus a single link), the text-of-the-ref equals the
            // text-of-the-span before merge. After merge the link's source bbox
            // is preserved, so we still know which substring it covers.
            //
            // The simplest correct shape: project each link's source bbox +
            // kind directly, and use the parsed element's `text` as the ref's
            // text (this is exact pre-merge, and post-merge it's the merged
            // body text — consumers find the substring via nth-anchor lookup).
            //
            // **Per-merge text honesty:** pre-merge the parsed element IS the
            // single span carrying the link, so `text` is exact. The merged
            // element's `links` carries the original per-span source_bbox of
            // each link, which is invariant to the merge. The ref's `text`
            // field is best-set to the per-link span text — but the
            // ParsedPdfElement merge has already lost the per-span text
            // attribution. As a pragmatic v1, set `text` to empty for the
            // PDF channel and rely on consumers using `source_bbox` directly
            // for spatial lookup; the substring-anchor convention is the
            // MD-channel's contract. Mark this as a known gap for a future
            // refinement slice if a consumer needs per-ref text directly.
            let (internal_refs, external_refs) = split_links_into_refs(&parsed);

            SemanticTreeElement {
                text,
                element_type,
                hierarchy_level: parsed.hierarchy_level,
                text_order: index as u32,
                physical_location,
                style: Some(style),
                token_count: parsed.token_count,
                internal_refs,
                external_refs,
                // CR-78 (v2.4.0): carry the rule engine's detection confidence
                // through to the SemanticTreeElement. `0` for non-Section
                // elements (the rule engine leaves them unscored).
                confidence: parsed.confidence,
            }
            .validate()
        })
        .collect()
}

/// CR-62: Split `ParsedPdfElement.links` (the unified `Vec<PdfLinkAnnotation>`
/// carried through clustering) into channel-agnostic `internal_refs[]` /
/// `external_refs[]`. Each link contributes exactly one entry to one of the
/// two output vecs, preserving source order.
fn split_links_into_refs(
    parsed: &ParsedPdfElement,
) -> (Vec<InternalRef>, Vec<ExternalRef>) {
    let source_page = parsed.placement.as_ref().map(|p| p.page_number);
    let mut internal_refs = Vec::new();
    let mut external_refs = Vec::new();
    for link in &parsed.links {
        match &link.kind {
            PdfLinkKind::InternalNamed {
                name,
                target_page,
                target_x,
                target_y,
            } => {
                let point = target_point(*target_x, *target_y);
                internal_refs.push(InternalRef {
                    text: String::new(),
                    source_page,
                    source_bbox: Some(link.source_bbox.clone()),
                    target: InternalRefTarget::Named {
                        name: name.clone(),
                        page: *target_page,
                        point,
                    },
                });
            }
            PdfLinkKind::InternalPage {
                target_page,
                target_x,
                target_y,
            } => {
                let point = target_point(*target_x, *target_y);
                internal_refs.push(InternalRef {
                    text: String::new(),
                    source_page,
                    source_bbox: Some(link.source_bbox.clone()),
                    target: InternalRefTarget::Page {
                        page: *target_page,
                        point,
                    },
                });
            }
            PdfLinkKind::ExternalUri { url } => {
                external_refs.push(ExternalRef {
                    text: String::new(),
                    source_page,
                    source_bbox: Some(link.source_bbox.clone()),
                    target: ExternalRefTarget::Uri { url: url.clone() },
                });
            }
        }
    }
    (internal_refs, external_refs)
}

fn target_point(x: Option<f32>, y: Option<f32>) -> Option<TargetPoint> {
    if x.is_none() && y.is_none() {
        None
    } else {
        Some(TargetPoint { x, y })
    }
}

/// Apply C-7b canonical inline emphasis tokens (`**bold**` / `*italic*` /
/// `***bold-italic***`) to body text based on element-level style flags.
/// Empty / whitespace-only text is returned unchanged — C-7a empty-body
/// validation (Part D) rejects those at construction; this function does
/// not duplicate that check.
fn apply_canonical_emphasis(text: String, style: &StyleInfo) -> String {
    if !style.is_bold && !style.is_italic {
        return text;
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return text;
    }
    match (style.is_bold, style.is_italic) {
        (true, true) => format!("***{trimmed}***"),
        (true, false) => format!("**{trimmed}**"),
        (false, true) => format!("*{trimmed}*"),
        (false, false) => unreachable!("guarded by early return above"),
    }
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
        // CR-79 (Tier 1): a region leaf tagged Table by the TableDetectionRule
        // projects to the channel-agnostic Table node type.
        ParsedElementType::Table => SemanticElementType::Table,
    }
}

/// Project `FontClass` (PDF-channel-internal style metadata) into
/// channel-agnostic `StyleInfo` (alias for `StyleMetadata`). Lossless on
/// the fields the downstream consumer (`DocumentNode.style_info`)
/// historically populates: font_family, font_size, is_bold, is_italic,
/// foreground_color, font_class.
///
/// CR-45: `foreground_color` is sourced from Tika's CSS `color:` (which
/// is the foreground per CSS spec). `background_color` stays `None` here
/// — Tika's current `FontClass` regex captures `color:` only and we
/// don't see `background-color:` on the CSS spans across the corpus. The
/// `None` is the honest answer; DT-03 covers why we project verbatim
/// rather than synthesize.
fn project_style(parsed: &ParsedPdfElement) -> StyleInfo {
    let font = &parsed.style_info;
    StyleInfo {
        font_class: font.class_name.clone(),
        font_size: Some(font.font_size),
        font_family: Some(font.font_family.clone()),
        foreground_color: Some(font.color.clone()),
        background_color: None,
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
            links: vec![],
            confidence: 0,
        }
    }

    #[test]
    fn projects_all_fields_happy_path() {
        // Section so hierarchy_level survives normalization (level=2 round-trips).
        let input = fixture(ParsedElementType::Section, 2);
        let projected = project_to_semantic_tree(vec![input]);

        assert_eq!(projected.len(), 1);
        let element = &projected[0];

        // Universal fields preserved. v2.2.0+ (CR-61): the fixture has
        // font_style="italic" + font_weight="bold", so canonical-form
        // projection wraps the body in `***...***` (C-7b bold-italic).
        // See `applies_canonical_emphasis_per_c7b` below for the dedicated
        // projection-behavior tests.
        assert_eq!(element.text, "***sample text***");
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
        assert_eq!(style.foreground_color.as_deref(), Some("#112233"));
        assert_eq!(
            style.background_color, None,
            "background_color stays None — Tika CSS regex captures color only (CR-45)",
        );
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
    fn hierarchy_level_passes_through_unchanged() {
        // The rule engine assigns hierarchy_level for every element such
        // that the existing GraphBuilder algorithm (find_parent + section
        // stack) attaches each element to the right parent. The projection
        // is a pure carrier — we pass hierarchy_level through unchanged.
        // Whether to reshape this convention (sentinel-zero for non-Section
        // types + matching find_parent change) is tracked as CR-44.
        for element_type in [
            ParsedElementType::Section,
            ParsedElementType::Paragraph,
            ParsedElementType::Header,
            ParsedElementType::Footer,
            ParsedElementType::Margin,
            ParsedElementType::List,
            ParsedElementType::ListItem,
        ] {
            let input = fixture(element_type.clone(), 5);
            let projected = project_to_semantic_tree(vec![input]);
            assert_eq!(
                projected[0].hierarchy_level, 5,
                "{element_type:?} should pass hierarchy_level through unchanged, got {}",
                projected[0].hierarchy_level,
            );
        }
    }

    // ---------- CR-61 / v2.2.0+: canonical-form emphasis projection ----------

    /// Build a fixture with explicit bold/italic flags. Helper for the
    /// C-7b projection tests below.
    fn fixture_with_emphasis(
        element_type: ParsedElementType,
        text: &str,
        bold: bool,
        italic: bool,
    ) -> ParsedPdfElement {
        let mut fx = fixture(element_type, 1);
        fx.text = text.to_string();
        fx.style_info.font_weight = if bold { "bold" } else { "normal" }.to_string();
        fx.style_info.font_style = if italic { "italic" } else { "normal" }.to_string();
        fx
    }

    #[test]
    fn applies_canonical_emphasis_italic_only_wraps_text_in_asterisks() {
        let input = fixture_with_emphasis(ParsedElementType::Paragraph, "lorem ipsum", false, true);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "*lorem ipsum*");
        // Style flag also preserved verbatim per DT-03 — the body wrap
        // and the JSON `style` field carry the same signal.
        assert!(projected[0].style.as_ref().unwrap().is_italic);
        assert!(!projected[0].style.as_ref().unwrap().is_bold);
    }

    #[test]
    fn applies_canonical_emphasis_bold_only_wraps_text_in_double_asterisks() {
        let input = fixture_with_emphasis(ParsedElementType::Paragraph, "lorem ipsum", true, false);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "**lorem ipsum**");
    }

    #[test]
    fn applies_canonical_emphasis_bold_and_italic_wraps_in_triple_asterisks() {
        let input = fixture_with_emphasis(ParsedElementType::Paragraph, "lorem ipsum", true, true);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "***lorem ipsum***");
    }

    #[test]
    fn applies_canonical_emphasis_no_flags_leaves_text_unchanged() {
        let input =
            fixture_with_emphasis(ParsedElementType::Paragraph, "lorem ipsum", false, false);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "lorem ipsum");
    }

    #[test]
    fn applies_canonical_emphasis_trims_surrounding_whitespace_before_wrap() {
        // Whitespace at the edges would break CommonMark emphasis (the
        // delimiters must be adjacent to non-whitespace inside). Trim
        // before wrap; NodeContent::new will trim again downstream as a
        // no-op.
        let input =
            fixture_with_emphasis(ParsedElementType::Paragraph, "  lorem  ", false, true);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "*lorem*");
    }

    #[test]
    fn applies_canonical_emphasis_to_section_heading_text() {
        // PDF section headings with bold style get the canonical wrap
        // too — this is the "redundant-but-canonical" case (the # prefix
        // already implies emphasis, but the rule is uniform across
        // markdown-inline-domain types per C-7a).
        let input = fixture_with_emphasis(ParsedElementType::Section, "Introduction", true, false);
        let projected = project_to_semantic_tree(vec![input]);
        assert_eq!(projected[0].text, "**Introduction**");
        assert_eq!(projected[0].element_type, SemanticElementType::Section);
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
