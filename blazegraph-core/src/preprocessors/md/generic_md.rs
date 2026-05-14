//! Generic markdown channel — `.md` string → `DocumentGraph`.
//!
//! This is the markdown counterpart to the PDF channel. Where the
//! bgraph.md parser at [`super::bgraph_md`] reconstructs a graph from
//! a self-describing wire format, this module parses *generic*
//! markdown (no bgraph fences) into the same `DocumentGraph` shape via
//! [`pulldown_cmark`] events.
//!
//! ## Algorithm
//!
//! 1. Pre-pass: strip and parse YAML frontmatter via
//!    [`super::frontmatter::extract_frontmatter`]. Populates the
//!    canonical fields of [`DocumentMetadata`]; non-canonical keys go
//!    to `extras`. Malformed YAML is silent — the input passes through
//!    unchanged.
//! 2. Pulldown-cmark walk on the post-frontmatter body using
//!    [`pulldown_cmark::Parser::into_offset_iter`] so each top-level
//!    block carries its source byte range. The range gives us the
//!    *verbatim* markdown text for non-Section nodes — bullets, fence
//!    markers, table pipes, `>` quote markers, all preserved.
//! 3. Each top-level block projects to one [`SemanticTreeElement`]:
//!    - `Heading` → `Section` with `text = stripped heading text`,
//!      `hierarchy_level = N` (the heading level).
//!    - `Paragraph` → `Paragraph` with `text = source slice`.
//!    - `CodeBlock` → `CodeBlock` with `text = source slice` (fence,
//!      language tag, body, closing fence — all preserved).
//!    - `List` → `List` with `text = source slice` (entire list,
//!      including nested sublists).
//!    - `BlockQuote` → `Blockquote` with `text = source slice`
//!      including `>` markers.
//!    - `Table` (GFM) → `Table` with `text = source slice` including
//!      pipes + alignment row.
//! 4. `text_order` is assigned in vec position (0..N).
//! 5. The vec feeds [`GraphBuilder::build_graph_deterministic`].
//! 6. Title falls back to the first Section's text if frontmatter
//!    didn't carry one (the filename-stem fallback is the CLI's job).
//! 7. `compute_structural_profile` then `compute_breadcrumbs` — same
//!    post-build sequence as the PDF channel and the bgraph.md parser.
//!
//! ## ParseIdentity
//!
//! Returns [`ParseIdentity::Verified`]. The generic-markdown path
//! doesn't have a self-describing hash to verify against — the
//! `ParseIdentity::Verified` here is a "we parsed successfully"
//! signal, distinct from the bgraph.md path's hash-match semantics.
//!
//! ## Open calls (documented in the B6 AAR)
//!
//! - `Tag::Rule` (horizontal rule) → emit as a Paragraph with
//!   `text = "---"`. Cheap to preserve byte-identically and useful for
//!   `<hr>` rendering in downstream consumers.
//! - `Tag::HtmlBlock` → emit as a Paragraph with the raw HTML in
//!   `text`. v1 has no HTMLBlock variant; the raw HTML round-trips as
//!   ordinary text.

use crate::graphs::builder::GraphBuilder;
use crate::graphs::node_id::NodeIdGenerator;
use crate::tokens::estimate_token_count;
use crate::types::*;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};

use super::frontmatter::extract_frontmatter;
use super::types::{ParseError, ParseIdentity, ParseOptions, ParseResult};

/// Parse a plain markdown string (no bgraph fences) into a
/// `DocumentGraph`.
///
/// Frontmatter (YAML, fenced by `---`) is parsed leniently and
/// populates `graph.document_info.document_metadata`. The remainder
/// is fed through pulldown-cmark and projected to
/// `Vec<SemanticTreeElement>`, then through
/// [`GraphBuilder::build_graph_deterministic`].
///
/// `opts` is currently unused — the generic-markdown path has no
/// strict-vs-drift distinction to honor (there's no embedded
/// `graph_sha256` to verify against). The argument is present for
/// API symmetry with [`super::bgraph_md::parse`].
pub fn parse(input: &str, _opts: ParseOptions) -> Result<ParseResult, ParseError> {
    // 1. Frontmatter pre-pass — leaves `body` as a `&str` slice into
    //    `input` so we can directly map pulldown's byte ranges into
    //    body bytes without re-numbering.
    let (frontmatter_metadata, body) = extract_frontmatter(input);

    // 2. pulldown-cmark walk. ENABLE_TABLES for GFM tables; nothing
    //    else (no smart punctuation, no strikethrough rewriting) —
    //    the byte ranges we extract include the raw source.
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);

    let parser = Parser::new_ext(body, options).into_offset_iter();
    let mut elements: Vec<SemanticTreeElement> = Vec::new();

    // We only act on **top-level** blocks. Nesting depth tracks how
    // many `Tag::*` Start events are currently open; we collect text
    // for Section headings inside a `Heading` start/end pair, and
    // we slice the source range for other tags' Start event when
    // we're at the top level.
    let mut nesting: u32 = 0;
    let mut heading_text_buf = String::new();
    let mut current_heading_level: Option<HeadingLevel> = None;
    // Tracks the depth of the most recent Section we've emitted, so
    // non-Section leaves (Paragraph, CodeBlock, List, Blockquote,
    // Table) can carry a hierarchy_level that makes
    // GraphBuilder::find_parent attach them under that Section
    // rather than directly under Document. Sentinel 0 means "no
    // Section seen yet — orphan prose at Document depth."
    let mut current_section_depth: u32 = 0;

    for (event, range) in parser {
        match event {
            // ---------- Section starts ----------
            Event::Start(Tag::Heading { level, .. }) => {
                nesting += 1;
                if nesting == 1 {
                    // Top-level heading — start collecting heading
                    // text from subsequent Text / Code / etc. events.
                    current_heading_level = Some(level);
                    heading_text_buf.clear();
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if nesting == 1 {
                    // Close out the heading.
                    let heading_text = std::mem::take(&mut heading_text_buf).trim().to_string();
                    let level = current_heading_level
                        .take()
                        .expect("heading end without start (parser invariant)");
                    let text_order = elements.len() as u32;
                    let depth = heading_level_to_depth(level);
                    elements.push(SemanticTreeElement {
                        text: heading_text.clone(),
                        element_type: SemanticElementType::Section,
                        hierarchy_level: depth,
                        text_order,
                        physical_location: None,
                        style: None,
                        token_count: estimate_token_count(&heading_text),
                    });
                    current_section_depth = depth;
                }
                nesting -= 1;
            }

            // ---------- Top-level non-heading blocks ----------
            Event::Start(tag) => {
                if nesting == 0 {
                    if let Some(element_type) = project_top_level_tag(&tag) {
                        let source = slice_verbatim(body, range);
                        let text_order = elements.len() as u32;
                        // Non-Section leaves use a hierarchy_level
                        // that GraphBuilder::find_parent interprets
                        // as "attach to the most recent open
                        // Section." Setting `level = current_section_depth + 1`
                        // means the find_parent stack walk stops at
                        // the current section (whose depth is
                        // `current_section_depth`) — see
                        // `graphs/builder.rs::find_parent`. Before
                        // any Section is seen, `current_section_depth = 0`
                        // and the leaf attaches to Document at depth 1.
                        let leaf_level = current_section_depth + 1;
                        elements.push(SemanticTreeElement {
                            text: source.clone(),
                            element_type,
                            hierarchy_level: leaf_level,
                            text_order,
                            physical_location: None,
                            style: None,
                            token_count: estimate_token_count(&source),
                        });
                    }
                }
                nesting += 1;
            }
            Event::End(_) => {
                nesting = nesting.saturating_sub(1);
            }

            // ---------- Heading-text collection ----------
            // Inside a top-level heading, capture text / code-span /
            // softbreak content so we can build a clean `text` field.
            Event::Text(s) => {
                if nesting == 1 && current_heading_level.is_some() {
                    heading_text_buf.push_str(&s);
                }
            }
            Event::Code(s) => {
                if nesting == 1 && current_heading_level.is_some() {
                    heading_text_buf.push_str(&s);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if nesting == 1 && current_heading_level.is_some() {
                    heading_text_buf.push(' ');
                }
            }

            // ---------- Top-level Rule / HtmlBlock ----------
            // These appear as standalone events (no Start/End pair).
            // Rule → render as Paragraph("---"); cheap and round-trip
            // friendly. HtmlBlock → emit as Paragraph with raw HTML in
            // text (no HTMLBlock variant in v1; the bytes survive).
            Event::Rule => {
                if nesting == 0 {
                    let text_order = elements.len() as u32;
                    let source = slice_verbatim(body, range);
                    let leaf_level = current_section_depth + 1;
                    elements.push(SemanticTreeElement {
                        text: source.clone(),
                        element_type: SemanticElementType::Paragraph,
                        hierarchy_level: leaf_level,
                        text_order,
                        physical_location: None,
                        style: None,
                        token_count: estimate_token_count(&source),
                    });
                }
            }
            Event::Html(_html) => {
                if nesting == 0 {
                    let text_order = elements.len() as u32;
                    let source = slice_verbatim(body, range);
                    let leaf_level = current_section_depth + 1;
                    elements.push(SemanticTreeElement {
                        text: source.clone(),
                        element_type: SemanticElementType::Paragraph,
                        hierarchy_level: leaf_level,
                        text_order,
                        physical_location: None,
                        style: None,
                        token_count: estimate_token_count(&source),
                    });
                }
            }

            // Other events we don't act on at the top level.
            _ => {}
        }
    }

    // 3. Provenance — generic markdown has no source PDF, so
    //    `source_sha256 = sha256(input_bytes)`. `config_hash = "none"`
    //    (placeholder; the channel has no parsing config today).
    let source_sha256 = sha256_hex(input.as_bytes());
    let config_hash = "none".to_string();
    let provenance = ParseProvenance {
        blazegraph_version: env!("CARGO_PKG_VERSION").to_string(),
        source_format: "markdown".to_string(),
        source_filename: String::new(), // CLI may overwrite; the lib doesn't know
        source_sha256: source_sha256.clone(),
        config_hash: config_hash.clone(),
    };
    let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);

    // 4. Build the graph. The builder asserts `text_order == vec
    //    position`; we satisfied that above by pushing in order.
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen, provenance)
        .map_err(|e| ParseError::MalformedFence(format!("graph build failed: {e}")))?;

    // 5. Populate fields the builder doesn't.
    //
    // Title fallback is deliberately NOT done here — if the
    // frontmatter didn't carry a title, we leave it as `None` so
    // the emitter knows there was no `title: ...` line in the
    // input. Otherwise the emitter would synthesize a frontmatter
    // block for inputs that had none, breaking byte-identical
    // round-trip. Filename-stem fallback is the CLI's job (it has
    // access to the input filename; the lib does not).
    graph.document_info.document_metadata = frontmatter_metadata;
    graph.structural_profile.flow_type = FlowType::Free;

    // 6. Canonical post-build sequence (mirrors processor.rs).
    graph.compute_structural_profile();
    graph.compute_breadcrumbs();

    Ok(ParseResult {
        graph,
        identity: ParseIdentity::Verified,
    })
}

/// Map a `pulldown_cmark::HeadingLevel` to our 1-based depth integer.
fn heading_level_to_depth(level: HeadingLevel) -> u32 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

/// Map a top-level pulldown-cmark `Tag` (Start) to a
/// `SemanticElementType` we want to emit as a node. Returns `None`
/// for tags we don't project (inline tags inside a paragraph land
/// here too, but the outer `nesting == 0` check above gates that).
fn project_top_level_tag(tag: &Tag) -> Option<SemanticElementType> {
    match tag {
        Tag::Paragraph => Some(SemanticElementType::Paragraph),
        Tag::CodeBlock(_) => Some(SemanticElementType::CodeBlock),
        Tag::List(_) => Some(SemanticElementType::List),
        Tag::BlockQuote => Some(SemanticElementType::Blockquote),
        Tag::Table(_) => Some(SemanticElementType::Table),
        // Headings are handled separately (text-extracting path).
        Tag::Heading { .. } => None,
        // FootnoteDefinition, Item, TableHead/Row/Cell, Emphasis,
        // Strong, Link, Image, MetadataBlock — all sub-block or
        // inline and never reach top level when our nesting tracker
        // is at 0.
        _ => None,
    }
}

/// Slice the body string by the byte range pulldown-cmark gives us
/// for a block. Trims the trailing `\n` that pulldown includes in the
/// range so consecutive blocks don't double-up newlines on emit
/// (the emitter rejoins with `\n\n`).
fn slice_verbatim(body: &str, range: std::ops::Range<usize>) -> String {
    let raw = &body[range];
    raw.trim_end_matches('\n').to_string()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(input: &str) -> DocumentGraph {
        parse(input, ParseOptions::default())
            .expect("parse should succeed")
            .graph
    }

    fn nodes_in_order(graph: &DocumentGraph) -> Vec<&DocumentNode> {
        let mut nodes: Vec<&DocumentNode> = graph
            .nodes
            .values()
            .filter(|n| n.text_order.is_some())
            .collect();
        nodes.sort_by_key(|n| n.text_order.expect("filtered above"));
        nodes
    }

    #[test]
    fn parse_single_h1_produces_one_section() {
        let graph = parse_ok("# Hello\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1, "expected exactly one body node");
        assert_eq!(nodes[0].node_type, "Section");
        assert_eq!(nodes[0].content.text, "Hello");
        assert_eq!(nodes[0].location.semantic.depth, 1);
    }

    #[test]
    fn parse_orphan_paragraph_attaches_to_document_root() {
        let graph = parse_ok("Just a paragraph, no heading.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "Paragraph");
        // Orphan prose lives at the Document root (depth 0 sentinel,
        // attached to graph.document_info.root_id).
        assert_eq!(
            nodes[0].parent,
            Some(graph.document_info.root_id),
            "orphan paragraph must attach to Document root"
        );
    }

    #[test]
    fn parse_nested_headings_produce_nested_sections() {
        let graph = parse_ok("# Top\n\n## Sub\n\nBody.\n");
        let nodes = nodes_in_order(&graph);
        // 3 nodes: Section(Top), Section(Sub), Paragraph(Body.)
        assert_eq!(nodes.len(), 3, "expected 3 nodes; got: {:?}", nodes);
        assert_eq!(nodes[0].node_type, "Section");
        assert_eq!(nodes[0].location.semantic.depth, 1);
        assert_eq!(nodes[1].node_type, "Section");
        assert_eq!(nodes[1].location.semantic.depth, 2);
        // Section(Sub) is a child of Section(Top).
        assert_eq!(nodes[1].parent, Some(nodes[0].id));
        // Paragraph(Body.) is a child of Section(Sub).
        assert_eq!(nodes[2].parent, Some(nodes[1].id));
    }

    #[test]
    fn parse_level_skip_h1_to_h3_preserves_depth_3() {
        let graph = parse_ok("# Top\n\n### Skip\n\nBody.\n");
        let nodes = nodes_in_order(&graph);
        // Markdown-literal: ### means depth 3 regardless of skip.
        assert_eq!(nodes[0].location.semantic.depth, 1);
        assert_eq!(nodes[1].location.semantic.depth, 3);
    }

    #[test]
    fn parse_codeblock_text_includes_fence_and_language() {
        let input = "```rust\nfn main() {}\n```\n";
        let graph = parse_ok(input);
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "CodeBlock");
        assert!(
            nodes[0].content.text.contains("```rust"),
            "CodeBlock text should include opening fence + language tag; got: {:?}",
            nodes[0].content.text
        );
        assert!(
            nodes[0].content.text.contains("fn main() {}"),
            "CodeBlock text should include body; got: {:?}",
            nodes[0].content.text
        );
        assert!(
            nodes[0].content.text.contains("```"),
            "CodeBlock text should include closing fence; got: {:?}",
            nodes[0].content.text
        );
    }

    #[test]
    fn parse_list_text_includes_markers() {
        let input = "- one\n- two\n- three\n";
        let graph = parse_ok(input);
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "List");
        let text = &nodes[0].content.text;
        assert!(
            text.contains("- one") && text.contains("- two") && text.contains("- three"),
            "List text should include bullet markers + items; got: {:?}",
            text
        );
    }

    #[test]
    fn parse_blockquote_text_includes_gt_markers() {
        let input = "> Quoted.\n> Still quoted.\n";
        let graph = parse_ok(input);
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "Blockquote");
        let text = &nodes[0].content.text;
        assert!(
            text.contains("> Quoted.") && text.contains("> Still quoted."),
            "Blockquote text should include `>` markers; got: {:?}",
            text
        );
    }

    #[test]
    fn parse_table_text_includes_pipes() {
        let input = "| a | b |\n|---|---|\n| 1 | 2 |\n";
        let graph = parse_ok(input);
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "Table");
        let text = &nodes[0].content.text;
        assert!(
            text.contains("| a | b |") && text.contains("|---|---|"),
            "Table text should include pipes + alignment row; got: {:?}",
            text
        );
    }

    #[test]
    fn parse_frontmatter_populates_document_metadata() {
        let input = "---\n\
                     title: From Frontmatter\n\
                     author: Marcus\n\
                     date: 2026-05-12\n\
                     tags: [rust, b6]\n\
                     ---\n\
                     # Body Heading\n\
                     \n\
                     Some prose.\n";
        let graph = parse_ok(input);
        let metadata = &graph.document_info.document_metadata;
        assert_eq!(metadata.title.as_deref(), Some("From Frontmatter"));
        assert_eq!(metadata.author.as_deref(), Some("Marcus"));
        assert_eq!(metadata.date.as_deref(), Some("2026-05-12"));
        assert_eq!(metadata.tags, vec!["rust".to_string(), "b6".to_string()]);
    }

    #[test]
    fn parse_leaves_title_unset_when_frontmatter_lacks_title() {
        // The lib path deliberately does NOT synthesize a title
        // fallback — leaving `title = None` preserves byte-identical
        // round-trip for inputs that had no frontmatter. Filename-
        // stem fallback is the CLI's job; downstream consumers can
        // pull the first Section's text themselves if they want.
        let input = "# Body Heading\n\nProse.\n";
        let graph = parse_ok(input);
        assert!(
            graph.document_info.document_metadata.title.is_none(),
            "title should be None when no frontmatter; got: {:?}",
            graph.document_info.document_metadata.title
        );
    }

    #[test]
    fn parse_sets_flow_type_to_free() {
        let graph = parse_ok("# Hi\n");
        assert!(
            matches!(graph.structural_profile.flow_type, FlowType::Free),
            "generic markdown is reflowable; flow_type must be Free",
        );
    }

    #[test]
    fn parse_sets_parse_provenance_with_source_sha256() {
        let input = "# Hi\n";
        let graph = parse_ok(input);
        let prov = graph
            .document_info
            .parse_provenance
            .as_ref()
            .expect("provenance present");
        assert_eq!(prov.source_format, "markdown");
        assert_eq!(prov.config_hash, "none");
        // sha256 of "# Hi\n" — deterministic.
        let expected = sha256_hex(input.as_bytes());
        assert_eq!(prov.source_sha256, expected);
    }
}
