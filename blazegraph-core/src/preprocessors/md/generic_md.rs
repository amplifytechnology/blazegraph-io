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
//! 7. `compute_breadcrumbs` — same
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

use super::super::canonical::{format_pipe_table, wrap_emphasis};
use super::frontmatter::extract_frontmatter;
use super::types::{ParseError, ParseIdentity, ParseOptions, ParseResult};

/// Active inline-collection context. Set when we enter a top-level
/// Heading or Paragraph; cleared on matching End. Used to gate the
/// inline-event-to-buffer accumulation that builds C-7b canonical-form
/// text (v2.2.0+ / CR-61).
#[derive(Debug)]
enum InlineMode {
    Heading(HeadingLevel),
    Paragraph,
}

/// Flush the pending inline text run into `out`, wrapped in the canonical
/// emphasis delimiters for the current (bold, italic) state. No-op when the
/// run is empty. Called at every inline boundary (Emphasis/Strong/Link/Code
/// edges + block close) so each maximal same-formatting run is emitted as one
/// `wrap_emphasis` unit — keeping whitespace outside the markers and matching
/// the DOCX channel byte-for-byte.
fn flush_inline_run(out: &mut String, run: &mut String, bold: bool, italic: bool) {
    if !run.is_empty() {
        out.push_str(&wrap_emphasis(run, bold, italic));
        run.clear();
    }
}

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
    // for Section headings and Paragraphs inside their Start/End pair
    // (reconstructing inline content with C-7b canonical delimiters
    // per CR-61 / v2.2.0+), and we slice the source range for other
    // block-cluster tags' Start event when we're at the top level
    // (CodeBlock / List / Blockquote / Table — literal /
    // literal-with-markers domains per C-7a, source preserved verbatim).
    let mut nesting: u32 = 0;
    // Active inline-collection mode: None outside a top-level Heading
    // / Paragraph; Some(...) while collecting inline events between
    // their Start and End. Heading carries its level so we can
    // assign the right depth on End.
    let mut inline_mode: Option<InlineMode> = None;
    let mut inline_buf = String::new();
    // Stack of link destination URLs — needed because TagEnd::Link
    // doesn't carry the URL; we record it on Start and consume on End.
    let mut link_url_stack: Vec<String> = Vec::new();
    // Inline emphasis state. Rather than push `*`/`**` at each
    // Emphasis/Strong boundary (which preserves pulldown's nesting and
    // can leave whitespace *inside* the markers, e.g. `*italic, **bi,***`),
    // we accumulate each maximal same-formatting text run in `run_buf`
    // and emit it through the shared `wrap_emphasis` helper — the SAME
    // canonical form the DOCX channel produces. Strong/Emphasis nesting
    // is collapsed to per-run (bold, italic) flags, so a run that is both
    // becomes `***run***` and adjacent runs split cleanly
    // (`*italic,* ***bi,***`) with whitespace kept outside the delimiters.
    let mut bold_depth: u32 = 0;
    let mut italic_depth: u32 = 0;
    let mut run_buf = String::new();
    // Tracks the depth of the most recent Section we've emitted, so
    // non-Section leaves (Paragraph, CodeBlock, List, Blockquote,
    // Table) can carry a hierarchy_level that makes
    // GraphBuilder::find_parent attach them under that Section
    // rather than directly under Document. Sentinel 0 means "no
    // Section seen yet — orphan prose at Document depth."
    let mut current_section_depth: u32 = 0;

    for (event, range) in parser {
        match event {
            // ---------- Section / heading boundaries ----------
            Event::Start(Tag::Heading { level, .. }) => {
                nesting += 1;
                if nesting == 1 {
                    // Top-level heading — enter inline-collection mode.
                    inline_mode = Some(InlineMode::Heading(level));
                    inline_buf.clear();
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if nesting == 1 {
                    let level = match inline_mode.take() {
                        Some(InlineMode::Heading(l)) => l,
                        _ => panic!("heading end without matching start (parser invariant)"),
                    };
                    // Flush the final text run (emphasis is balanced/closed by
                    // here, so bold/italic are 0 — a plain push).
                    flush_inline_run(
                        &mut inline_buf,
                        &mut run_buf,
                        bold_depth > 0,
                        italic_depth > 0,
                    );
                    let heading_text = std::mem::take(&mut inline_buf).trim().to_string();
                    // Skip content-free headings (e.g. a `##` spacer or an
                    // image-only heading). An empty Section has no rendered form
                    // and violates the C-7a non-empty-body convention; following
                    // content stays under the last real section.
                    if !heading_text.is_empty() {
                        let text_order = elements.len() as u32;
                        let depth = heading_level_to_depth(level);
                        elements.push(
                            SemanticTreeElement {
                                text: heading_text.clone(),
                                element_type: SemanticElementType::Section,
                                hierarchy_level: depth,
                                text_order,
                                physical_location: None,
                                style: None,
                                token_count: estimate_token_count(&heading_text),
                                internal_refs: vec![],
                                external_refs: vec![],
                                confidence: 0,
                            }
                            .validate(),
                        );
                        current_section_depth = depth;
                    }
                }
                nesting -= 1;
            }

            // ---------- Paragraph boundaries (now event-reconstructed) ----------
            Event::Start(Tag::Paragraph) => {
                if nesting == 0 {
                    inline_mode = Some(InlineMode::Paragraph);
                    inline_buf.clear();
                }
                nesting += 1;
            }
            Event::End(TagEnd::Paragraph) => {
                if nesting == 1 && matches!(inline_mode, Some(InlineMode::Paragraph)) {
                    inline_mode = None;
                    // Flush the final text run (emphasis is balanced/closed by
                    // here, so bold/italic are 0 — a plain push).
                    flush_inline_run(
                        &mut inline_buf,
                        &mut run_buf,
                        bold_depth > 0,
                        italic_depth > 0,
                    );
                    let para_text = std::mem::take(&mut inline_buf).trim().to_string();
                    // Skip content-free paragraphs (e.g. an image-only paragraph
                    // with empty alt text). An empty body has no rendered form and
                    // would violate the C-7a non-empty-body wire convention — fix
                    // the producer rather than emit an invalid Paragraph.
                    if !para_text.is_empty() {
                        let text_order = elements.len() as u32;
                        let leaf_level = current_section_depth + 1;
                        elements.push(
                            SemanticTreeElement {
                                text: para_text.clone(),
                                element_type: SemanticElementType::Paragraph,
                                hierarchy_level: leaf_level,
                                text_order,
                                physical_location: None,
                                style: None,
                                token_count: estimate_token_count(&para_text),
                                internal_refs: vec![],
                                external_refs: vec![],
                                confidence: 0,
                            }
                            .validate(),
                        );
                    }
                }
                nesting = nesting.saturating_sub(1);
            }

            // ---------- Inline emphasis / code / link events ----------
            // C-7b canonical delimiters: `*italic*` / `**bold**` /
            // `` `code` `` / `[label](url)`. Source-side variant
            // (`_italic_`, `__bold__`, `[label][ref]`) gets normalized
            // on parse per C-7c — the pulldown-cmark event stream
            // already resolved the source-side variant; we just emit
            // the canonical form.
            Event::Start(Tag::Emphasis) => {
                flush_inline_run(
                    &mut inline_buf,
                    &mut run_buf,
                    bold_depth > 0,
                    italic_depth > 0,
                );
                italic_depth += 1;
                nesting += 1;
            }
            Event::End(TagEnd::Emphasis) => {
                flush_inline_run(
                    &mut inline_buf,
                    &mut run_buf,
                    bold_depth > 0,
                    italic_depth > 0,
                );
                italic_depth = italic_depth.saturating_sub(1);
                nesting = nesting.saturating_sub(1);
            }
            Event::Start(Tag::Strong) => {
                flush_inline_run(
                    &mut inline_buf,
                    &mut run_buf,
                    bold_depth > 0,
                    italic_depth > 0,
                );
                bold_depth += 1;
                nesting += 1;
            }
            Event::End(TagEnd::Strong) => {
                flush_inline_run(
                    &mut inline_buf,
                    &mut run_buf,
                    bold_depth > 0,
                    italic_depth > 0,
                );
                bold_depth = bold_depth.saturating_sub(1);
                nesting = nesting.saturating_sub(1);
            }
            Event::Start(Tag::Link { dest_url, .. }) => {
                // Track URL for matched End event. Always push onto
                // stack so nesting balance is preserved even if we're
                // not in inline_mode (defensive — pulldown shouldn't
                // emit Link outside a block, but cheaper to be safe).
                link_url_stack.push(dest_url.to_string());
                if inline_mode.is_some() {
                    flush_inline_run(
                        &mut inline_buf,
                        &mut run_buf,
                        bold_depth > 0,
                        italic_depth > 0,
                    );
                    inline_buf.push('[');
                }
                nesting += 1;
            }
            Event::End(TagEnd::Link) => {
                let url = link_url_stack.pop().unwrap_or_default();
                if inline_mode.is_some() {
                    // Flush the label run (wrapped per current emphasis state)
                    // before the closing bracket + destination.
                    flush_inline_run(
                        &mut inline_buf,
                        &mut run_buf,
                        bold_depth > 0,
                        italic_depth > 0,
                    );
                    inline_buf.push_str(&format!("]({url})"));
                }
                nesting = nesting.saturating_sub(1);
            }

            // ---------- Top-level non-heading non-paragraph blocks ----------
            Event::Start(tag) => {
                if nesting == 0 {
                    if let Some(element_type) = project_top_level_tag(&tag) {
                        let mut source = slice_verbatim(body, range);
                        // Tables re-canonicalize to the shared pipe-table form
                        // (CR-80 #2) so the MD channel converges with DOCX/PDF;
                        // other literal-with-markers blocks stay verbatim (C-7a).
                        if element_type == SemanticElementType::Table {
                            source = format_pipe_table(&source);
                        }
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
                        elements.push(
                            SemanticTreeElement {
                                text: source.clone(),
                                element_type,
                                hierarchy_level: leaf_level,
                                text_order,
                                physical_location: None,
                                style: None,
                                token_count: estimate_token_count(&source),
                                internal_refs: vec![],
                                external_refs: vec![],
                                confidence: 0,
                            }
                            .validate(),
                        );
                    }
                }
                nesting += 1;
            }
            Event::End(_) => {
                nesting = nesting.saturating_sub(1);
            }

            // ---------- Inline content collection (text / code / breaks) ----------
            // When inline_mode is Some, accumulate raw text / code-span
            // content into inline_buf. Code spans wrap in backticks for
            // C-7b canonical form (` `code` `).
            Event::Text(s) => {
                if inline_mode.is_some() {
                    run_buf.push_str(&s);
                }
            }
            Event::Code(s) => {
                if inline_mode.is_some() {
                    // A code span ends the current text run (its backticks
                    // sit outside any emphasis markers); flush, then emit
                    // the ` `code` ` C-7b form.
                    flush_inline_run(
                        &mut inline_buf,
                        &mut run_buf,
                        bold_depth > 0,
                        italic_depth > 0,
                    );
                    inline_buf.push('`');
                    inline_buf.push_str(&s);
                    inline_buf.push('`');
                }
            }
            Event::SoftBreak => {
                // Breaks stay *inside* the current run so an emphasis span
                // that wraps a soft break remains a single `wrap_emphasis`
                // unit (whitespace at a run edge is pulled outside the markers).
                if matches!(inline_mode, Some(InlineMode::Paragraph)) {
                    // Paragraphs preserve soft-wrap as newline. Headings
                    // collapse to space (a heading wrapping mid-line is
                    // typographically a continuation, not a line break).
                    run_buf.push('\n');
                } else if matches!(inline_mode, Some(InlineMode::Heading(_))) {
                    run_buf.push(' ');
                }
            }
            Event::HardBreak => {
                if inline_mode.is_some() {
                    // CommonMark hard break = two-space + newline.
                    run_buf.push_str("  \n");
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
                    elements.push(
                        SemanticTreeElement {
                            text: source.clone(),
                            element_type: SemanticElementType::Paragraph,
                            hierarchy_level: leaf_level,
                            text_order,
                            physical_location: None,
                            style: None,
                            token_count: estimate_token_count(&source),
                            internal_refs: vec![],
                            external_refs: vec![],
                            confidence: 0,
                        }
                        .validate(),
                    );
                }
            }
            Event::Html(_html) => {
                if nesting == 0 {
                    let text_order = elements.len() as u32;
                    let source = slice_verbatim(body, range);
                    let leaf_level = current_section_depth + 1;
                    elements.push(
                        SemanticTreeElement {
                            text: source.clone(),
                            element_type: SemanticElementType::Paragraph,
                            hierarchy_level: leaf_level,
                            text_order,
                            physical_location: None,
                            style: None,
                            token_count: estimate_token_count(&source),
                            internal_refs: vec![],
                            external_refs: vec![],
                            confidence: 0,
                        }
                        .validate(),
                    );
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
        blazegraph_version: crate::VERSION.to_string(),
        source_format: "markdown".to_string(),
        source_filename: String::new(), // CLI may overwrite; the lib doesn't know
        source_sha256: source_sha256.clone(),
        config_hash: config_hash.clone(),
    };
    let id_gen = NodeIdGenerator::new(); // CR-83: content+breadcrumb-derived; no doc namespace

    // 4. Build the graph. The builder asserts `text_order == vec
    //    position`; we satisfied that above by pushing in order.
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen)
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
    graph.document_info.flow_type = FlowType::Free;

    // 6. Canonical post-build sequence (mirrors processor.rs).
    graph.compute_breadcrumbs();

    Ok(ParseResult {
        graph,
        identity: ParseIdentity::Verified,
        // Block A / Amendment M: provenance rides beside the graph,
        // never on it.
        provenance,
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
    fn parse_empty_paragraph_is_skipped_not_emitted() {
        // An image-only paragraph with empty alt text yields a content-free
        // paragraph in the event stream. It must be skipped, not emitted as an
        // empty-body Paragraph (which would panic validate() + violate C-7a).
        let graph = parse_ok("Real prose.\n\n![](image.png)\n\nMore prose.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes.len(),
            2,
            "the content-free image paragraph must be skipped, leaving two prose paragraphs"
        );
        assert!(
            nodes.iter().all(|n| !n.content.text.trim().is_empty()),
            "no emitted node may have an empty body"
        );
    }

    #[test]
    fn parse_empty_heading_is_skipped_not_emitted() {
        // A content-free heading (image-only here; `##` spacers behave the same)
        // must be skipped, not emitted as an empty-body Section.
        let graph = parse_ok("# Real\n\n## ![](image.png)\n\nBody.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes.len(),
            2,
            "the content-free heading must be skipped, leaving one Section + one Paragraph"
        );
        assert!(
            nodes.iter().all(|n| !n.content.text.trim().is_empty()),
            "no emitted node may have an empty body"
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
        // Re-canonicalized to the shared pipe-table form (CR-80 #2): outer
        // pipes, a delimiter row, padded columns. Content left-aligned.
        assert_eq!(
            nodes[0].content.text,
            "| a   | b   |\n|-----|-----|\n| 1   | 2   |",
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
        // `date` migrates to canonical `created` (CR-57 / design doc § Notes on `created`).
        assert_eq!(metadata.created.as_deref(), Some("2026-05-12"));
        let md_ns = metadata.md.as_ref().expect("md namespace populated");
        assert_eq!(md_ns.tags, vec!["rust".to_string(), "b6".to_string()]);
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
            matches!(graph.document_info.flow_type, FlowType::Free),
            "generic markdown is reflowable; flow_type must be Free",
        );
    }

    #[test]
    fn parse_sets_parse_provenance_with_source_sha256() {
        let input = "# Hi\n";
        // Block A: provenance rides on the ParseResult, not the graph.
        let result = parse(input, ParseOptions::default()).expect("parses");
        let prov = &result.provenance;
        assert_eq!(prov.source_format, "markdown");
        assert_eq!(prov.config_hash, "none");
        // sha256 of "# Hi\n" — deterministic.
        let expected = sha256_hex(input.as_bytes());
        assert_eq!(prov.source_sha256, expected);
    }

    // ---------- CR-61 / v2.2.0+: C-7c canonical-form normalization ----------
    //
    // Paragraph + Section heading text is reconstructed from inline events
    // with C-7b canonical delimiters. Non-canonical source forms get
    // normalized on parse. Block-cluster types (CodeBlock / List /
    // Blockquote / Table) preserve raw source verbatim per the
    // literal / literal-with-markers domains in C-7a.

    #[test]
    fn paragraph_normalizes_underscore_italic_to_asterisk() {
        let graph = parse_ok("Hello _italic_ world.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "Paragraph");
        assert_eq!(nodes[0].content.text, "Hello *italic* world.");
    }

    #[test]
    fn paragraph_normalizes_double_underscore_bold_to_double_asterisk() {
        let graph = parse_ok("Hello __bold__ world.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].content.text, "Hello **bold** world.");
    }

    #[test]
    fn paragraph_preserves_already_canonical_asterisk_emphasis() {
        let graph = parse_ok("Hello *italic* and **bold**.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].content.text, "Hello *italic* and **bold**.");
    }

    #[test]
    fn paragraph_preserves_inline_code_with_backticks() {
        let graph = parse_ok("Use `foo` here.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].content.text, "Use `foo` here.");
    }

    #[test]
    fn paragraph_preserves_inline_link_in_canonical_form() {
        let graph = parse_ok("See [docs](https://example.com/x) for details.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes[0].content.text,
            "See [docs](https://example.com/x) for details."
        );
    }

    #[test]
    fn paragraph_normalizes_reference_link_to_inline_canonical() {
        // Source uses reference-link syntax `[label][ref]` with a
        // separate `[ref]: url` definition. Per C-7b, the canonical form
        // in body text is `[label](url)`. pulldown-cmark resolves the
        // reference's dest_url at parse time; the event-walk emits the
        // resolved canonical form.
        let graph = parse_ok("See [docs][d] for details.\n\n[d]: https://example.com/x\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes[0].content.text,
            "See [docs](https://example.com/x) for details."
        );
    }

    #[test]
    fn heading_with_emphasis_normalizes_to_canonical_asterisk_form() {
        // Section heading text picks up the same canonical-form rule —
        // the inline-parser layer is the same as paragraphs per C-7b.
        let graph = parse_ok("# Hello _italic_ world\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].node_type, "Section");
        assert_eq!(nodes[0].content.text, "Hello *italic* world");
    }

    #[test]
    fn heading_with_inline_code_preserves_backticks() {
        // Previously the heading event-walk dropped backticks on Code
        // events (Section text became "Use foo here" for source
        // "# Use `foo` here"). CR-61 fixes the incidental bug as part
        // of canonical-form reconstruction.
        let graph = parse_ok("# Use `foo` here\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].content.text, "Use `foo` here");
    }

    #[test]
    fn codeblock_preserves_double_underscore_verbatim() {
        // CodeBlock body is in the literal domain (C-7a). The inline
        // parser does not apply inside; `__init__` stays as literal
        // underscores regardless of any emphasis-normalization rule.
        let input = "```python\ndef __init__(self):\n    pass\n```\n";
        let graph = parse_ok(input);
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_type, "CodeBlock");
        assert!(
            nodes[0].content.text.contains("__init__"),
            "CodeBlock should preserve `__init__` verbatim; got: {:?}",
            nodes[0].content.text,
        );
    }

    #[test]
    fn blockquote_preserves_underscore_emphasis_verbatim() {
        // Blockquote body is literal-with-markers (C-7a). `>` markers
        // AND inline emphasis sequences in the body are preserved as
        // raw source — the inline parser doesn't normalize across the
        // block-cluster boundary.
        let graph = parse_ok("> This is _italic_-styled quotation.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].node_type, "Blockquote");
        assert!(
            nodes[0].content.text.contains("_italic_"),
            "Blockquote should preserve raw `_italic_` underscores; got: {:?}",
            nodes[0].content.text,
        );
    }

    #[test]
    fn paragraph_emphasis_whitespace_kept_outside_delimiters() {
        // CR-80 #1: pandoc emits a fused nested span with whitespace inside
        // the markers (`*italic, **bold-italic,***`). The MD channel must
        // re-canonicalize to per-run split spans with whitespace outside —
        // byte-identical to the DOCX channel's `wrap_emphasis` output.
        let graph = parse_ok("Here is some **bold,** *italic, **bold-italic,*** rest.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes[0].content.text,
            "Here is some **bold,** *italic,* ***bold-italic,*** rest.",
        );
    }

    #[test]
    fn paragraph_emphasis_spanning_soft_break_stays_one_run() {
        // A soft break inside an emphasis span must not split it into two
        // `*..*` spans; it stays a single wrapped run with the newline kept.
        let graph = parse_ok("*italic\nspanning* a break.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(nodes[0].content.text, "*italic\nspanning* a break.");
    }

    #[test]
    fn paragraph_combined_bold_italic_in_source_round_trips_canonical() {
        // `***word***` in source = Strong containing Emphasis (or vice
        // versa, depending on parse). Reconstructs to the same canonical
        // form. Source `**_word_**` (mixed delimiters) also collapses
        // to `***word***`.
        let graph = parse_ok("This is **_combined_** styling.\n");
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes[0].content.text, "This is ***combined*** styling.",
            "Combined bold-italic should normalize to ***triple-asterisk*** form",
        );
    }
}
