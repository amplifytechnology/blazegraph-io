//! DOCX body channel — `.docx` zip bytes → `DocumentGraph`.
//!
//! This is the DOCX counterpart to [`super::super::md::generic_md::parse`].
//! Where the generic-markdown channel walks pulldown-cmark events, this
//! module unzips the OOXML container, reads `word/document.xml` +
//! `word/styles.xml`, and walks `<w:body>` with [`quick_xml`] into the same
//! `Vec<SemanticTreeElement>` → [`GraphBuilder::build_graph_deterministic`]
//! shape.
//!
//! ## Mental model
//!
//! A `.docx` is a ZIP. `word/document.xml`'s `<w:body>` is a flat,
//! reading-ordered sequence of two block primitives:
//!
//! - `<w:p>` — a paragraph (`<w:pPr>` properties + a run of `<w:r>`).
//! - `<w:tbl>` — a table (`<w:tr>` rows → `<w:tc>` cells → paragraphs).
//!
//! Almost all semantics come from **one field**: the paragraph's resolved
//! **outline level**. DOCX is *light inference* — no geometry, no detection,
//! no `graph_sanity`. The heading style **is** the authoritative outline.
//!
//! ## Algorithm
//!
//! 1. Unzip; read `word/document.xml` and (best-effort) `word/styles.xml`.
//! 2. Build a `styleId → EffectiveStyle { name, outline_lvl }` map, resolving
//!    `outline_lvl` through the `<w:basedOn>` chain.
//! 3. Single streaming `quick-xml` pass over `<w:body>`: maintain a small
//!    block stack so each top-level `<w:p>` / `<w:tbl>` accumulates its props
//!    plus run text and flushes to one [`SemanticTreeElement`] on close, with
//!    `text_order = 0..N`.
//! 4. The vec feeds [`GraphBuilder::build_graph_deterministic`].
//! 5. `flow_type = Free`, `physical_location = None` (DOCX is reflowable).
//! 6. `compute_structural_profile` then `compute_breadcrumbs` — same
//!    post-build sequence as the PDF and markdown channels.
//!
//! ## Scope (C1 + C2)
//!
//! Body structure: Section / Paragraph / Table / Blockquote, depth from the
//! `outlineLvl` gate, run-text concatenation, and emphasis projection
//! (`<w:b/>` → `**…**`, `<w:i/>` → `*…*`) (C1). Plus hyperlink ref extraction
//! (C2): each `<w:hyperlink w:anchor>` becomes an `InternalRef` and each
//! `<w:hyperlink r:id>` with `TargetMode="External"` (resolved via
//! `word/_rels/document.xml.rels`) becomes an `ExternalRef` on the element
//! that contains it; the link's visible run text reuses the same `<w:t>`
//! concatenation (the ref is *additional*, not a replacement). **Out of
//! scope** (later handoffs): metadata population (C3), CLI dispatch (C4),
//! header/footer (C5).
//!
//! ## ParseIdentity
//!
//! Returns [`ParseIdentity::Verified`] — a "we parsed successfully" signal,
//! same as the generic-markdown path (there's no self-describing hash to
//! verify against).

use std::collections::HashMap;
use std::io::Read;

use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use sha2::{Digest, Sha256};

use crate::graphs::builder::GraphBuilder;
use crate::graphs::node_id::NodeIdGenerator;
use crate::tokens::estimate_token_count;
use crate::types::*;

use super::super::md::types::{ParseError, ParseIdentity, ParseOptions, ParseResult};
use super::rels::HyperlinkRel;

/// Resolved style information for one `styleId`, after walking the
/// `<w:basedOn>` chain. `outline_lvl` is the field that drives the Section
/// gate; `name` distinguishes `Quote`/`IntenseQuote` styles whose node-type
/// mapping is name-based rather than outline-based.
#[derive(Debug, Clone, Default)]
struct EffectiveStyle {
    /// `<w:name w:val>` — the human-facing style name (e.g. `"Quote"`).
    name: Option<String>,
    /// Resolved `<w:outlineLvl w:val>` (0..=8 for heading levels 1..=9),
    /// inherited through `<w:basedOn>` unless the style sets its own.
    outline_lvl: Option<u8>,
}

/// Parse a `.docx` byte buffer into a `DocumentGraph`.
///
/// Cracks the OOXML ZIP container, reads `word/document.xml` +
/// `word/styles.xml`, and projects `<w:body>` to a faithful
/// Section/Paragraph/Table/Blockquote tree with emphasis canonicalized
/// (`<w:b/>` → `**…**`, `<w:i/>` → `*…*`).
///
/// `opts` is currently unused — the DOCX path has no strict-vs-drift
/// distinction (no embedded `graph_sha256` to verify against). The argument
/// is present for API symmetry with [`super::super::md::parse_markdown`].
///
/// Returns [`ParseError::MalformedDocx`] if the bytes are not a valid ZIP or
/// `word/document.xml` is absent (i.e. not a WordprocessingML document).
pub fn parse_docx(bytes: &[u8], _opts: ParseOptions) -> Result<ParseResult, ParseError> {
    // 1. Container read.
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| ParseError::MalformedDocx(format!("not a valid zip container: {e}")))?;

    let document_xml = read_zip_entry(&mut archive, "word/document.xml").ok_or_else(|| {
        ParseError::MalformedDocx(
            "word/document.xml absent — not a WordprocessingML document".to_string(),
        )
    })?;
    // styles.xml is best-effort: a document with only inline outline levels
    // still parses. Absence yields an empty style map.
    let styles_xml = read_zip_entry(&mut archive, "word/styles.xml").unwrap_or_default();
    // document.xml.rels resolves `<w:hyperlink r:id>` → URL + TargetMode
    // (C2). Best-effort: a document with no `r:id` hyperlinks (or none at
    // all) has no rels part to read; absence yields an empty map.
    let rels_xml = read_zip_entry(&mut archive, "word/_rels/document.xml.rels").unwrap_or_default();

    // 2. Styles resolution map + hyperlink relationships map.
    let styles = build_styles_map(&styles_xml)?;
    let hyperlink_rels = super::rels::parse_hyperlink_rels(&rels_xml)?;

    // 3. Body walk → elements.
    let elements = walk_body(&document_xml, &styles, &hyperlink_rels)?;

    // 4. Provenance — DOCX has no source PDF, so
    //    `source_sha256 = sha256(zip bytes)`. `config_hash = "none"`
    //    (the channel has no parsing config).
    let source_sha256 = sha256_hex(bytes);
    let config_hash = "none".to_string();
    let provenance = ParseProvenance {
        blazegraph_version: env!("CARGO_PKG_VERSION").to_string(),
        source_format: "docx".to_string(),
        source_filename: String::new(), // CLI may overwrite; the lib doesn't know
        source_sha256: source_sha256.clone(),
        config_hash: config_hash.clone(),
    };
    let id_gen = NodeIdGenerator::new(&provenance.source_sha256, &provenance.config_hash);

    // 5. Build the graph. The builder asserts `text_order == vec position`;
    //    we satisfied that in `walk_body` by pushing in order.
    let mut graph = GraphBuilder::new()
        .build_graph_deterministic(elements, &id_gen, provenance)
        .map_err(|e| ParseError::MalformedDocx(format!("graph build failed: {e}")))?;

    // 6. Populate fields the builder doesn't. DOCX is reflowable — no
    //    per-element bbox exists, so `flow_type = Free`. Metadata is left at
    //    the builder default; C3 populates it from `docProps/*.xml`.
    graph.structural_profile.flow_type = FlowType::Free;

    // 7. Canonical post-build sequence (mirrors processor.rs / the MD path).
    graph.compute_structural_profile();
    graph.compute_breadcrumbs();

    Ok(ParseResult {
        graph,
        identity: ParseIdentity::Verified,
    })
}

/// Read one ZIP entry to a UTF-8 `String`. Returns `None` if the entry is
/// absent or not valid UTF-8 (OOXML parts are always UTF-8 XML, so a
/// non-UTF-8 part is treated as absent rather than a hard error).
fn read_zip_entry<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<String> {
    let mut file = archive.by_name(name).ok()?;
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    Some(buf)
}

// =============================================================================
// Styles resolution
// =============================================================================

/// A style's own (pre-inheritance) name / basedOn / outlineLvl, as read
/// directly from its `<w:style>` block.
#[derive(Debug, Default)]
struct RawStyle {
    name: Option<String>,
    based_on: Option<String>,
    own_outline_lvl: Option<u8>,
}

/// Parse `word/styles.xml` into a `styleId → EffectiveStyle` map, resolving
/// `outline_lvl` through the `<w:basedOn>` chain.
///
/// A style inherits its parent's `outlineLvl` unless it sets its own — so
/// custom heading styles that merely `basedOn="Heading1"` are caught by the
/// Section gate automatically. The two-pass shape (collect raw styles, then
/// resolve inheritance) keeps the chain-walk simple and cycle-safe.
fn build_styles_map(styles_xml: &str) -> Result<HashMap<String, EffectiveStyle>, ParseError> {
    let raw = collect_raw_styles(styles_xml)?;

    let mut resolved: HashMap<String, EffectiveStyle> = HashMap::with_capacity(raw.len());
    for (id, style) in &raw {
        resolved.insert(
            id.clone(),
            EffectiveStyle {
                name: style.name.clone(),
                outline_lvl: resolve_outline_lvl(id, &raw),
            },
        );
    }
    Ok(resolved)
}

/// Pass 1: collect each `<w:style>`'s own name / basedOn / outlineLvl.
fn collect_raw_styles(styles_xml: &str) -> Result<HashMap<String, RawStyle>, ParseError> {
    let mut raw: HashMap<String, RawStyle> = HashMap::new();
    let mut reader = Reader::from_str(styles_xml);
    let mut current_id: Option<String> = None;
    let mut current = RawStyle::default();

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ParseError::MalformedDocx(format!("styles.xml: {e}")))?;
        match event {
            Event::Start(e) if local_name(e.name().as_ref()) == b"style" => {
                current_id = attr_value(&e, b"w:styleId");
                current = RawStyle::default();
            }
            // `<w:name>`, `<w:basedOn>`, `<w:outlineLvl>` are usually empty
            // elements (`<w:name w:val="…"/>`); tolerate the Start form too.
            Event::Empty(e) | Event::Start(e) => {
                if current_id.is_some() {
                    read_style_child(&e, &mut current);
                }
            }
            Event::End(e) if local_name(e.name().as_ref()) == b"style" => {
                if let Some(id) = current_id.take() {
                    raw.insert(id, std::mem::take(&mut current));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(raw)
}

/// Read one child element inside a `<w:style>` block into the accumulator.
fn read_style_child(e: &BytesStart, current: &mut RawStyle) {
    match local_name(e.name().as_ref()) {
        b"name" => {
            if current.name.is_none() {
                current.name = attr_value(e, b"w:val");
            }
        }
        b"basedOn" => {
            if current.based_on.is_none() {
                current.based_on = attr_value(e, b"w:val");
            }
        }
        b"outlineLvl" => {
            if current.own_outline_lvl.is_none() {
                current.own_outline_lvl =
                    attr_value(e, b"w:val").and_then(|v| v.parse::<u8>().ok());
            }
        }
        _ => {}
    }
}

/// Pass 2: walk the `basedOn` chain from `start_id`, returning the first
/// `own_outline_lvl` found. Cycle-safe via a hard cap (OOXML style
/// inheritance is shallow; 32 bounds any real chain and a malformed cycle).
fn resolve_outline_lvl(start_id: &str, raw: &HashMap<String, RawStyle>) -> Option<u8> {
    let mut cursor = start_id;
    for _ in 0..32 {
        let node = raw.get(cursor)?;
        if let Some(lvl) = node.own_outline_lvl {
            return Some(lvl);
        }
        match node.based_on.as_deref() {
            Some(parent) if parent != cursor => cursor = parent,
            _ => return None,
        }
    }
    None
}

// =============================================================================
// Body walk (single streaming pass)
// =============================================================================

/// Which block we're currently accumulating, if any.
enum Block {
    /// A top-level `<w:p>` — collect its props (until first run) + run text.
    Paragraph(ParagraphAccumulator),
    /// A top-level `<w:tbl>` — collect rows / cells.
    Table(TableAccumulator),
}

/// Accumulates a paragraph's outline/style props and assembled run text.
#[derive(Default)]
struct ParagraphAccumulator {
    inline_outline_lvl: Option<u8>,
    style_id: Option<String>,
    runs: RunAssembler,
    /// True once we've left `<w:pPr>` (any further `<w:rPr>` belongs to runs,
    /// not paragraph props).
    seen_ppr_end: bool,
    in_ppr: bool,
}

/// Accumulates a table's rows. Each cell is itself a run-assembled string.
#[derive(Default)]
struct TableAccumulator {
    rows: Vec<Vec<String>>,
    /// Stack of cell run-assemblers; depth > 1 means nested table (we still
    /// accumulate into the innermost open cell — v1 flattens nesting).
    cell_runs: Option<RunAssembler>,
    /// Nesting depth of `<w:tbl>` (>1 inside a nested table). Cells of a
    /// nested table flatten into the same row text in v1.
    tbl_depth: u32,
    /// Current row being built.
    current_row: Vec<String>,
    /// Hyperlink refs from all cells, bubbled up at cell finish. The table
    /// flattens to one node (decision 6), so cell refs attach to that one
    /// Table element (the visible text is the cell text, already in `rows`).
    refs: PendingRefs,
}

/// Walk `<w:body>`'s top-level children (`<w:p>` and `<w:tbl>`) in document
/// order, projecting each to one [`SemanticTreeElement`]. Paragraphs nested
/// inside table cells are consumed by the table-text assembly, not emitted
/// as standalone nodes.
fn walk_body(
    document_xml: &str,
    styles: &HashMap<String, EffectiveStyle>,
    hyperlink_rels: &HashMap<String, HyperlinkRel>,
) -> Result<Vec<SemanticTreeElement>, ParseError> {
    let mut reader = Reader::from_str(document_xml);
    let mut elements: Vec<SemanticTreeElement> = Vec::new();

    let mut in_body = false;
    // The block currently being accumulated (a top-level `<w:p>` or
    // `<w:tbl>`). `None` between blocks.
    let mut block: Option<Block> = None;
    // Depth of the most recently emitted Section. Non-Section leaves
    // (Paragraph / Blockquote / Table) carry `current_section_depth + 1` as
    // their `hierarchy_level` so `GraphBuilder::find_parent` attaches them
    // under that open Section (and crucially does NOT reset the section
    // stack — a literal `hierarchy_level = 0` leaf would, via
    // `find_parent`'s `level <= 1` truncate, detach later sibling Sections
    // from their parent). Sentinel 0 = "no Section seen yet" → leaf attaches
    // to the Document root at depth 1. This mirrors `generic_md::parse`'s
    // `current_section_depth` exactly; the contract's mapping-table value of
    // `0` for non-Section types is the conceptual sentinel, while the
    // builder contract requires the open-section-relative level here.
    let mut current_section_depth: u32 = 0;

    loop {
        let event = reader
            .read_event()
            .map_err(|e| ParseError::MalformedDocx(format!("document.xml: {e}")))?;

        match event {
            Event::Start(e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if !in_body {
                    if name == b"body" {
                        in_body = true;
                    }
                    continue;
                }
                match &mut block {
                    // Not inside a block yet — a top-level child opens one.
                    None => match name {
                        b"p" => block = Some(Block::Paragraph(ParagraphAccumulator::default())),
                        b"tbl" => {
                            block = Some(Block::Table(TableAccumulator {
                                tbl_depth: 1,
                                ..Default::default()
                            }));
                        }
                        _ => {} // <w:sectPr> etc. — ignored.
                    },
                    Some(Block::Paragraph(acc)) => para_on_start(acc, &e, &mut reader)?,
                    Some(Block::Table(acc)) => table_on_start(acc, &e, &mut reader)?,
                }
            }
            Event::Empty(e) => {
                if !in_body {
                    continue;
                }
                match &mut block {
                    Some(Block::Paragraph(acc)) => para_on_empty(acc, &e),
                    Some(Block::Table(acc)) => {
                        if let Some(cell) = acc.cell_runs.as_mut() {
                            cell.on_empty(&e);
                        }
                    }
                    None => {}
                }
            }
            Event::Text(t) => {
                let text = t
                    .unescape()
                    .map(|c| c.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(t.as_ref()).into_owned());
                match &mut block {
                    Some(Block::Paragraph(acc)) => acc.runs.on_text(&text),
                    Some(Block::Table(acc)) => {
                        if let Some(cell) = acc.cell_runs.as_mut() {
                            cell.on_text(&text);
                        }
                    }
                    None => {}
                }
            }
            Event::End(e) => {
                if !in_body {
                    continue;
                }
                let qname = e.name();
                let name = local_name(qname.as_ref());
                match &mut block {
                    None => {
                        if name == b"body" {
                            in_body = false;
                        }
                    }
                    Some(Block::Paragraph(_)) => {
                        if name == b"p" {
                            if let Some(Block::Paragraph(acc)) = block.take() {
                                if let Some(el) = finish_paragraph(
                                    acc,
                                    styles,
                                    elements.len() as u32,
                                    current_section_depth,
                                ) {
                                    if el.element_type == SemanticElementType::Section {
                                        current_section_depth = el.hierarchy_level;
                                    }
                                    elements.push(el);
                                }
                            }
                        } else if let Some(Block::Paragraph(acc)) = &mut block {
                            para_on_end(acc, name, hyperlink_rels);
                        }
                    }
                    Some(Block::Table(_)) => {
                        if name == b"tbl" {
                            if let Some(Block::Table(acc)) = &mut block {
                                acc.tbl_depth = acc.tbl_depth.saturating_sub(1);
                                if acc.tbl_depth == 0 {
                                    if let Some(Block::Table(acc)) = block.take() {
                                        elements.push(finish_table(
                                            acc,
                                            elements.len() as u32,
                                            current_section_depth,
                                        ));
                                    }
                                }
                            }
                        } else if let Some(Block::Table(acc)) = &mut block {
                            table_on_end(acc, name, hyperlink_rels);
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    Ok(elements)
}

// ---- Paragraph accumulation ------------------------------------------------

fn para_on_start(
    acc: &mut ParagraphAccumulator,
    e: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<(), ParseError> {
    match local_name(e.name().as_ref()) {
        b"pPr" if !acc.seen_ppr_end => acc.in_ppr = true,
        // Inside <w:pPr>: pStyle / outlineLvl carried as Start (rare) too.
        b"pStyle" if acc.in_ppr => {
            if acc.style_id.is_none() {
                acc.style_id = attr_value(e, b"w:val");
            }
        }
        b"outlineLvl" if acc.in_ppr => {
            if acc.inline_outline_lvl.is_none() {
                acc.inline_outline_lvl = attr_value(e, b"w:val").and_then(|v| v.parse::<u8>().ok());
            }
        }
        // Run formatting / text — delegate to the run assembler.
        b"hyperlink" => acc.runs.on_hyperlink_start(e),
        b"r" => acc.runs.on_run_start(),
        b"rPr" => acc.runs.in_rpr = true,
        b"b" if acc.runs.in_rpr => acc.runs.bold = run_bool_on(e),
        b"i" if acc.runs.in_rpr => acc.runs.italic = run_bool_on(e),
        b"t" => acc.runs.in_t = true,
        b"drawing" => {
            // Skip the whole drawing subtree — no Figure node in scope.
            reader
                .read_to_end(e.name())
                .map_err(|err| ParseError::MalformedDocx(format!("drawing: {err}")))?;
        }
        _ => {}
    }
    Ok(())
}

fn para_on_empty(acc: &mut ParagraphAccumulator, e: &BytesStart) {
    match local_name(e.name().as_ref()) {
        b"pStyle" if acc.in_ppr => {
            if acc.style_id.is_none() {
                acc.style_id = attr_value(e, b"w:val");
            }
        }
        b"outlineLvl" if acc.in_ppr => {
            if acc.inline_outline_lvl.is_none() {
                acc.inline_outline_lvl = attr_value(e, b"w:val").and_then(|v| v.parse::<u8>().ok());
            }
        }
        _ => acc.runs.on_empty(e),
    }
}

fn para_on_end(
    acc: &mut ParagraphAccumulator,
    name: &[u8],
    hyperlink_rels: &HashMap<String, HyperlinkRel>,
) {
    match name {
        b"pPr" => {
            acc.in_ppr = false;
            acc.seen_ppr_end = true;
        }
        b"t" => acc.runs.in_t = false,
        b"rPr" => acc.runs.in_rpr = false,
        b"r" => acc.runs.on_run_end(),
        b"hyperlink" => acc.runs.on_hyperlink_end(hyperlink_rels),
        _ => {}
    }
}

fn finish_paragraph(
    acc: ParagraphAccumulator,
    styles: &HashMap<String, EffectiveStyle>,
    text_order: u32,
    current_section_depth: u32,
) -> Option<SemanticTreeElement> {
    let (element_type, mapped_level) =
        classify_paragraph(acc.inline_outline_lvl, acc.style_id.as_deref(), styles);

    // Sections carry their own depth; non-Section leaves carry
    // `current_section_depth + 1` so they attach under the open Section
    // (see the `current_section_depth` note in `walk_body`).
    let hierarchy_level = if element_type == SemanticElementType::Section {
        mapped_level
    } else {
        current_section_depth + 1
    };

    let (mut text, refs) = acc.runs.finish_with_refs();
    // Trim Section heading text (heading-marker semantics, decision 5);
    // leave Paragraph/Blockquote text as-assembled.
    if element_type == SemanticElementType::Section {
        text = text.trim().to_string();
    }
    // Skip empty paragraphs (decision 6b) — also keeps `.validate()` from
    // rejecting an empty-body Paragraph/Section. (A hyperlink-only paragraph
    // with no visible text would have empty refs too — nothing to attach.)
    if text.trim().is_empty() {
        return None;
    }

    Some(
        SemanticTreeElement {
            text: text.clone(),
            element_type,
            hierarchy_level,
            text_order,
            physical_location: None,
            style: None,
            token_count: estimate_token_count(&text),
            internal_refs: refs.internal,
            external_refs: refs.external,
            confidence: 0,
        }
        .validate(),
    )
}

// ---- Table accumulation ----------------------------------------------------

fn table_on_start(
    acc: &mut TableAccumulator,
    e: &BytesStart,
    reader: &mut Reader<&[u8]>,
) -> Result<(), ParseError> {
    match local_name(e.name().as_ref()) {
        b"tbl" => acc.tbl_depth += 1, // nested table — v1 flattens.
        b"tr" if acc.cell_runs.is_none() => acc.current_row = Vec::new(),
        b"tc" => acc.cell_runs = Some(RunAssembler::default()),
        // Inside a cell, accumulate runs. Cell text spans multiple `<w:p>`;
        // we insert a `\n` between paragraphs via on_paragraph_break.
        b"p" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.on_paragraph_break();
            }
        }
        b"hyperlink" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.on_hyperlink_start(e);
            }
        }
        b"r" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.on_run_start();
            }
        }
        b"rPr" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.in_rpr = true;
            }
        }
        b"b" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                if cell.in_rpr {
                    cell.bold = run_bool_on(e);
                }
            }
        }
        b"i" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                if cell.in_rpr {
                    cell.italic = run_bool_on(e);
                }
            }
        }
        b"t" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.in_t = true;
            }
        }
        b"drawing" => {
            reader
                .read_to_end(e.name())
                .map_err(|err| ParseError::MalformedDocx(format!("drawing: {err}")))?;
        }
        _ => {}
    }
    Ok(())
}

fn table_on_end(
    acc: &mut TableAccumulator,
    name: &[u8],
    hyperlink_rels: &HashMap<String, HyperlinkRel>,
) {
    match name {
        b"tc" => {
            if let Some(cell) = acc.cell_runs.take() {
                // Bubble the cell's hyperlink refs up to the table (one node,
                // decision 6) along with its assembled text.
                let (text, refs) = cell.finish_with_refs();
                acc.current_row.push(text);
                acc.refs.internal.extend(refs.internal);
                acc.refs.external.extend(refs.external);
            }
        }
        b"tr" => {
            if acc.cell_runs.is_none() {
                acc.rows.push(std::mem::take(&mut acc.current_row));
            }
        }
        b"t" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.in_t = false;
            }
        }
        b"rPr" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.in_rpr = false;
            }
        }
        b"r" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.on_run_end();
            }
        }
        b"hyperlink" => {
            if let Some(cell) = acc.cell_runs.as_mut() {
                cell.on_hyperlink_end(hyperlink_rels);
            }
        }
        _ => {}
    }
}

fn finish_table(
    acc: TableAccumulator,
    text_order: u32,
    current_section_depth: u32,
) -> SemanticTreeElement {
    // Rows joined by `\n`, cells within a row joined by ` | ` (decision 6).
    let text = acc
        .rows
        .iter()
        .map(|row| row.join(" | "))
        .collect::<Vec<_>>()
        .join("\n");

    SemanticTreeElement {
        text: text.clone(),
        element_type: SemanticElementType::Table,
        // Attach under the open Section (see `walk_body`'s note); a literal
        // `0` would reset the section stack in `find_parent`.
        hierarchy_level: current_section_depth + 1,
        text_order,
        physical_location: None,
        style: None,
        token_count: estimate_token_count(&text),
        // Hyperlink refs from any cell attach to the one Table node (decision 6).
        internal_refs: acc.refs.internal,
        external_refs: acc.refs.external,
        confidence: 0,
    }
    .validate()
}

// =============================================================================
// Run-text assembly + emphasis projection (decisions 5 / 5b)
// =============================================================================

/// Accumulates a sequence of `<w:r>` runs into canonical inline text.
///
/// Per contract decision 5:
/// - `<w:t>` → its text, honoring `xml:space="preserve"` (we never trim
///   mid-assembly).
/// - `<w:tab/>` → `\t`, `<w:br/>` / `<w:cr/>` → `\n`.
/// - `<w:drawing>` / images → skipped by the walker (no Figure node).
/// - `<w:hyperlink>` run text included inline (the ref is C2's job — we don't
///   special-case `<w:hyperlink>`, so its child `<w:r>` runs flow through).
///
/// Per decision 5b, each run's text is wrapped by its `<w:rPr>` formatting
/// **before** concatenation: `<w:b/>` → `**text**`, `<w:i/>` → `*text*`,
/// both → `***text***`. Literal markdown metacharacters in raw run text are
/// backslash-escaped (the inline analog of the C-6 self-reference escape).
#[derive(Default)]
struct RunAssembler {
    out: String,
    // Current run state.
    in_run: bool,
    in_rpr: bool,
    in_t: bool,
    bold: bool,
    italic: bool,
    run_text: String,
    // Hyperlink state (C2). A `<w:hyperlink>` wraps runs inside a `<w:p>` /
    // cell; we record its attributes + the `out`-buffer offset at open, then
    // slice the visible text (the wrapped run concatenation) at close to build
    // a ref. Refs accumulate here and are drained by the owning accumulator at
    // `finish`. `<w:hyperlink>` does not nest in practice; if it did, only the
    // innermost open link is tracked (the outer reopens on the next start).
    open_link: Option<OpenHyperlink>,
    refs: PendingRefs,
}

/// An in-flight `<w:hyperlink>`: its attributes + the `out`-buffer length at
/// the moment it opened. The visible link text is `out[text_start..]` once the
/// link closes.
struct OpenHyperlink {
    /// `w:anchor` — an internal link to a `<w:bookmarkStart w:name>`.
    anchor: Option<String>,
    /// `r:id` — a relationship id resolved via `document.xml.rels`.
    rel_id: Option<String>,
    /// `out.len()` when the link opened (byte offset of the visible text).
    text_start: usize,
}

/// Hyperlink-derived refs collected while assembling a paragraph / cell, drained
/// into the element's `internal_refs` / `external_refs` at finish.
#[derive(Default)]
struct PendingRefs {
    internal: Vec<InternalRef>,
    external: Vec<ExternalRef>,
}

impl RunAssembler {
    fn on_run_start(&mut self) {
        self.in_run = true;
        self.in_rpr = false;
        self.bold = false;
        self.italic = false;
        self.run_text.clear();
    }

    /// Open a `<w:hyperlink>`: record its `w:anchor` / `r:id` and the current
    /// output offset so the visible text can be sliced when it closes.
    fn on_hyperlink_start(&mut self, e: &BytesStart) {
        self.open_link = Some(OpenHyperlink {
            anchor: attr_value(e, b"w:anchor"),
            rel_id: attr_value(e, b"r:id"),
            text_start: self.out.len(),
        });
    }

    /// Close a `<w:hyperlink>`: build the ref from its attributes + the visible
    /// text accumulated since it opened, resolving `r:id` via `rels`.
    ///
    /// - `w:anchor="name"` → `InternalRef` targeting the bookmark name.
    /// - `r:id` whose rels target is `TargetMode="External"` → `ExternalRef`.
    /// - `r:id` that is internal-part / unresolved, and a hyperlink with
    ///   neither attribute → skipped (out of scope per contract decision 7).
    ///
    /// `w:anchor` takes precedence when both are present (a same-document
    /// anchor is the internal target; the contract maps anchor → internal).
    fn on_hyperlink_end(&mut self, rels: &HashMap<String, HyperlinkRel>) {
        let Some(link) = self.open_link.take() else {
            return;
        };
        // Visible text = the run concatenation produced inside the hyperlink.
        let text = self.out[link.text_start..].to_string();

        if let Some(name) = link.anchor {
            self.refs.internal.push(InternalRef {
                text,
                source_page: None,
                source_bbox: None,
                target: InternalRefTarget::Named {
                    name,
                    page: None,
                    point: None,
                },
            });
        } else if let Some(rel) = link.rel_id.as_deref().and_then(|id| rels.get(id)) {
            if rel.is_external() {
                self.refs.external.push(ExternalRef {
                    text,
                    source_page: None,
                    source_bbox: None,
                    target: ExternalRefTarget::Uri {
                        url: rel.target.clone(),
                    },
                });
            }
            // Non-External r:id (internal-part link) → skip (decision 7).
        }
        // Hyperlink with neither attribute → nothing to attribute; skip.
    }

    fn on_run_end(&mut self) {
        if self.in_run {
            self.out
                .push_str(&wrap_emphasis(&self.run_text, self.bold, self.italic));
            self.in_run = false;
        }
    }

    fn on_text(&mut self, raw: &str) {
        if self.in_t {
            self.run_text.push_str(&escape_inline_markdown(raw));
        }
    }

    fn on_empty(&mut self, e: &BytesStart) {
        match local_name(e.name().as_ref()) {
            b"b" if self.in_rpr => self.bold = run_bool_on(e),
            b"i" if self.in_rpr => self.italic = run_bool_on(e),
            b"tab" => self.run_text.push('\t'),
            b"br" | b"cr" => self.run_text.push('\n'),
            _ => {}
        }
    }

    /// A paragraph boundary inside a multi-paragraph table cell: insert a
    /// newline between paragraphs (but not before the first).
    fn on_paragraph_break(&mut self) {
        if !self.out.is_empty() {
            self.out.push('\n');
        }
    }

    /// Consume the assembler, returning the assembled text and the hyperlink
    /// refs collected along the way. Used where refs must be attributed to the
    /// owning element (paragraphs, and — bubbled up — table cells).
    fn finish_with_refs(self) -> (String, PendingRefs) {
        (self.out, self.refs)
    }
}

/// Map a paragraph's resolved props to `(node type, hierarchy_level)` per the
/// contract mapping table (decision 4).
///
/// Resolution order for the effective outline level: the paragraph's own
/// inline `<w:outlineLvl>` wins; else the style-map lookup for its
/// `<w:pStyle>`. Then:
/// - `outline_lvl ∈ 0..=8` → Section, `hierarchy_level = outline_lvl + 1`.
/// - styleId == `"Title"` → Section, level 1.
/// - style name ∈ {`Quote`, `IntenseQuote`} → Blockquote, level 0.
/// - else → Paragraph, level 0.
fn classify_paragraph(
    inline_outline_lvl: Option<u8>,
    style_id: Option<&str>,
    styles: &HashMap<String, EffectiveStyle>,
) -> (SemanticElementType, u32) {
    let style = style_id.and_then(|id| styles.get(id));
    let effective_outline = inline_outline_lvl.or_else(|| style.and_then(|s| s.outline_lvl));

    if let Some(lvl) = effective_outline {
        if lvl <= 8 {
            return (SemanticElementType::Section, (lvl as u32) + 1);
        }
        // outlineLvl == 9 is the OOXML "body text" sentinel (e.g.
        // TOCHeading) — not a heading level; fall through.
    }

    // styleId == "Title" → Section level 1 (mirrors CR-74 title-nesting).
    if style_id == Some("Title") {
        return (SemanticElementType::Section, 1);
    }

    // style name ∈ {Quote, IntenseQuote} → Blockquote.
    if let Some(name) = style.and_then(|s| s.name.as_deref()) {
        if name == "Quote" || name == "IntenseQuote" {
            return (SemanticElementType::Blockquote, 0);
        }
    }

    (SemanticElementType::Paragraph, 0)
}

/// Interpret a `<w:b>` / `<w:i>` toggle. OOXML toggles are *on* unless
/// `w:val` explicitly disables them (`"0"` / `"false"` / `"off"`).
fn run_bool_on(e: &BytesStart) -> bool {
    match attr_value(e, b"w:val") {
        Some(v) => !matches!(v.as_str(), "0" | "false" | "off"),
        None => true,
    }
}

/// Wrap run text in canonical emphasis marks per decision 5b. Whitespace-only
/// or empty text is returned unwrapped (no `****` on a blank run, and we
/// don't wrap pure-whitespace runs so `**bold** word` stays clean).
fn wrap_emphasis(text: &str, bold: bool, italic: bool) -> String {
    if text.is_empty() || text.trim().is_empty() {
        return text.to_string();
    }
    match (bold, italic) {
        (true, true) => format!("***{text}***"),
        (true, false) => format!("**{text}**"),
        (false, true) => format!("*{text}*"),
        (false, false) => text.to_string(),
    }
}

/// Escape the inline markdown metacharacters that would otherwise be
/// misparsed as emphasis / code in raw run text — the inline analog of the
/// C-6 self-reference escape. We escape the emphasis + code delimiters
/// (`\`, `*`, `_`, `` ` ``) so a run that literally contains `*` does not
/// fuse with an adjacent projected `**bold**` into an ambiguous sequence.
/// Brackets are left literal (per CR-61: literal `[text]` is correct
/// CommonMark and needs no escape).
fn escape_inline_markdown(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        if matches!(ch, '\\' | '*' | '_' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

// =============================================================================
// XML helpers
// =============================================================================

/// Strip the namespace prefix (`w:p` → `p`) from a qualified element name.
fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// Read an attribute value by its (possibly prefixed) name, matching on the
/// local name so `w:val` and a bare `val` both resolve.
fn attr_value(e: &BytesStart, name: &[u8]) -> Option<String> {
    let want = local_name(name);
    e.attributes().flatten().find_map(|a| {
        if local_name(a.key.as_ref()) == want {
            a.unescape_value()
                .ok()
                .map(|c| c.into_owned())
                .or_else(|| Some(String::from_utf8_lossy(&a.value).into_owned()))
        } else {
            None
        }
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ---------------------------------------------------------------------
    // Fixture helpers — run against `test_fixtures/docx/structured.docx`, a
    // clean, purpose-built fixture authored by us (no third-party provenance).
    // Regenerate with `scripts/generate_docx_fixtures.py` (parent repo venv).
    // ---------------------------------------------------------------------

    fn fixture_bytes(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_fixtures/docx")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {path:?}: {e}"))
    }

    fn parse_fixture(name: &str) -> DocumentGraph {
        parse_docx(&fixture_bytes(name), ParseOptions::default())
            .expect("docx fixture should parse")
            .graph
    }

    /// Body nodes (text_order set), sorted by reading order.
    fn nodes_in_order(graph: &DocumentGraph) -> Vec<&DocumentNode> {
        let mut nodes: Vec<&DocumentNode> = graph
            .nodes
            .values()
            .filter(|n| n.text_order.is_some())
            .collect();
        nodes.sort_by_key(|n| n.text_order.expect("filtered above"));
        nodes
    }

    fn count_by_type(graph: &DocumentGraph, ty: &str) -> usize {
        nodes_in_order(graph)
            .iter()
            .filter(|n| n.node_type == ty)
            .count()
    }

    fn sections_by_depth(graph: &DocumentGraph) -> HashMap<u32, usize> {
        let mut m: HashMap<u32, usize> = HashMap::new();
        for n in nodes_in_order(graph) {
            if n.node_type == "Section" {
                *m.entry(n.location.semantic.depth).or_default() += 1;
            }
        }
        m
    }

    // =====================================================================
    // End-to-end fixture tests — `structured.docx`, a clean, purpose-built
    // fixture (generated by `scripts/generate_docx_fixtures.py` in the parent
    // repo; authored by us, generic metadata, no third-party provenance).
    // Reading order: Title, H1 "Introduction", para(bold+italic),
    // H2 "Methodology", para, H3 "Data Collection", para, Quote,
    // H1 "Results", 2×2 Table, H2 "Discussion", TOC1, TOC2, closing para.
    // =====================================================================

    #[test]
    fn structured_heading_tree() {
        // outlineLvl gate → depth = outlineLvl + 1. Title + 2×H1 at depth 1,
        // 2×H2 at depth 2, 1×H3 at depth 3.
        let graph = parse_fixture("structured.docx");
        let by_depth = sections_by_depth(&graph);
        assert_eq!(
            by_depth.get(&1).copied(),
            Some(3),
            "Title + 2×Heading1 at depth 1"
        );
        assert_eq!(by_depth.get(&2).copied(), Some(2), "2×Heading2 at depth 2");
        assert_eq!(by_depth.get(&3).copied(), Some(1), "1×Heading3 at depth 3");
        assert_eq!(count_by_type(&graph, "Section"), 6, "6 Sections total");

        // Nesting: an H2 attaches under a depth-1 Section, and the H3 under a
        // depth-2 Section.
        let nodes = nodes_in_order(&graph);
        let h2 = nodes
            .iter()
            .find(|n| n.node_type == "Section" && n.location.semantic.depth == 2)
            .expect("an H2 Section");
        let h2_parent = h2
            .parent
            .and_then(|p| graph.nodes.get(&p))
            .expect("H2 has a parent node");
        assert_eq!(
            h2_parent.location.semantic.depth, 1,
            "H2 nests under a depth-1 Section"
        );
        let h3 = nodes
            .iter()
            .find(|n| n.node_type == "Section" && n.location.semantic.depth == 3)
            .expect("an H3 Section");
        let h3_parent = h3
            .parent
            .and_then(|p| graph.nodes.get(&p))
            .expect("H3 has a parent node");
        assert_eq!(
            h3_parent.location.semantic.depth, 2,
            "H3 nests under a depth-2 Section"
        );
    }

    #[test]
    fn structured_table() {
        // One `<w:tbl>` → one Table node; cells joined ` | `, rows by newline.
        let graph = parse_fixture("structured.docx");
        assert_eq!(count_by_type(&graph, "Table"), 1, "one Table node");
        let table = nodes_in_order(&graph)
            .into_iter()
            .find(|n| n.node_type == "Table")
            .expect("a Table node");
        assert!(
            table.content.text.contains("Name | Value"),
            "header row cells joined by ` | `; got: {:?}",
            table.content.text
        );
        assert!(
            table.content.text.contains("Alpha | 42"),
            "data row cells joined by ` | `; got: {:?}",
            table.content.text
        );
    }

    #[test]
    fn structured_toc_title_and_blockquote() {
        // The outlineLvl gate keeps the two TOC-styled paragraphs OUT of the
        // Section tree — they're Paragraphs. The `Title` paragraph is a
        // depth-1 Section, and the `Quote`-styled paragraph is a Blockquote.
        let graph = parse_fixture("structured.docx");

        // The first body node is the Title, a depth-1 Section.
        let nodes = nodes_in_order(&graph);
        assert_eq!(
            nodes[0].node_type, "Section",
            "first node (Title) is a Section"
        );
        assert_eq!(nodes[0].location.semantic.depth, 1, "Title is depth 1");

        // The two TOC entries are NOT Sections (total stays 6).
        assert_eq!(
            count_by_type(&graph, "Section"),
            6,
            "TOC entries are not Sections"
        );

        // The Quote-styled paragraph → one Blockquote.
        assert_eq!(
            count_by_type(&graph, "Blockquote"),
            1,
            "Quote style → one Blockquote"
        );

        // A TOC entry's listing text survives as a Paragraph (faithful).
        let has_toc_para = nodes
            .iter()
            .any(|n| n.node_type == "Paragraph" && n.content.text.starts_with("Introduction\t"));
        assert!(has_toc_para, "TOC listing survives as a Paragraph");
    }

    #[test]
    fn structured_emphasis_projection() {
        // The Introduction paragraph mixes a bold run and an italic run:
        // "This document mixes **bold** and *italic* text."
        let graph = parse_fixture("structured.docx");
        let para = nodes_in_order(&graph)
            .into_iter()
            .find(|n| n.node_type == "Paragraph" && n.content.text.contains("mixes"))
            .expect("the emphasis paragraph");
        let t = &para.content.text;
        assert!(t.contains("**bold**"), "bold run → `**…**`; got: {t:?}");
        assert!(t.contains("*italic*"), "italic run → `*…*`; got: {t:?}");
    }

    #[test]
    fn parse_sets_flow_type_to_free_and_docx_provenance() {
        // DOCX is reflowable: flow_type = Free, no per-element bbox.
        let graph = parse_fixture("structured.docx");
        assert!(
            matches!(graph.structural_profile.flow_type, FlowType::Free),
            "DOCX is reflowable; flow_type must be Free"
        );
        // physical_location is None on every node (no geometry).
        for n in nodes_in_order(&graph) {
            assert!(
                n.location.physical.is_none(),
                "DOCX nodes carry no physical location; node {:?} did",
                n.id
            );
        }
        let prov = graph
            .document_info
            .parse_provenance
            .as_ref()
            .expect("provenance present");
        assert_eq!(prov.source_format, "docx");
        assert_eq!(prov.config_hash, "none");
        let expected = sha256_hex(&fixture_bytes("structured.docx"));
        assert_eq!(
            prov.source_sha256, expected,
            "source_sha256 = sha256(zip bytes)"
        );
    }

    #[test]
    fn metadata_left_at_builder_default() {
        // C1 does NOT populate metadata (that's C3). Even though
        // structured.docx has a core:title, C1 leaves the canonical title
        // unset because it never reads docProps.
        let graph = parse_fixture("structured.docx");
        assert!(
            graph.document_info.document_metadata.title.is_none(),
            "C1 leaves title unset (C3 populates from docProps/core.xml)"
        );
    }

    #[test]
    fn structured_has_no_refs() {
        // `structured.docx` carries no hyperlinks, so C2 emits no refs on it
        // (the ref machinery only fires on `<w:hyperlink>` elements).
        let graph = parse_fixture("structured.docx");
        let total_refs: usize = graph
            .nodes
            .values()
            .map(|n| n.internal_refs.len() + n.external_refs.len())
            .sum();
        assert_eq!(total_refs, 0, "no hyperlinks in structured.docx → no refs");
    }

    // =====================================================================
    // End-to-end ref extraction — `with_links.docx`, a clean fixture with
    // real hyperlinks (generated by `scripts/generate_docx_fixtures.py`):
    // one external (URL via r:id → rels TargetMode=External) and one internal
    // (w:anchor → a heading bookmark). Exercises the full zip → rels → walk →
    // ref path against a real OOXML container.
    // =====================================================================

    #[test]
    fn with_links_external_ref_resolved() {
        // The external hyperlink (r:id → https://blazegraph.io/) resolves to
        // exactly one ExternalRef::Uri with the visible link text.
        let graph = parse_fixture("with_links.docx");
        let externals: Vec<&ExternalRef> = graph
            .nodes
            .values()
            .flat_map(|n| &n.external_refs)
            .collect();
        assert_eq!(
            externals.len(),
            1,
            "one external hyperlink → one ExternalRef"
        );
        let r = externals[0];
        assert_eq!(r.text, "the project site", "visible link text");
        match &r.target {
            ExternalRefTarget::Uri { url } => {
                assert_eq!(url, "https://blazegraph.io/", "URL from rels Target")
            }
        }
    }

    #[test]
    fn with_links_internal_anchor_ref() {
        // The internal hyperlink (w:anchor="conclusion") resolves to exactly
        // one InternalRef::Named targeting the bookmark name.
        let graph = parse_fixture("with_links.docx");
        let internals: Vec<&InternalRef> = graph
            .nodes
            .values()
            .flat_map(|n| &n.internal_refs)
            .collect();
        assert_eq!(internals.len(), 1, "one anchor hyperlink → one InternalRef");
        let r = internals[0];
        assert_eq!(r.text, "Conclusion section", "visible link text");
        match &r.target {
            InternalRefTarget::Named { name, page, point } => {
                assert_eq!(name, "conclusion", "target = bookmark name");
                assert!(
                    page.is_none() && point.is_none(),
                    "Free flow, no page/point"
                );
            }
            other => panic!("expected Named target, got {other:?}"),
        }
    }

    #[test]
    fn with_links_total_ref_count() {
        // Exactly two refs total across the document: 1 internal + 1 external.
        let graph = parse_fixture("with_links.docx");
        let (mut internal, mut external) = (0usize, 0usize);
        for n in graph.nodes.values() {
            internal += n.internal_refs.len();
            external += n.external_refs.len();
        }
        assert_eq!((internal, external), (1, 1), "1 internal + 1 external ref");
    }

    // =====================================================================
    // Container / error-path unit tests.
    // =====================================================================

    #[test]
    fn non_zip_bytes_error() {
        let err = parse_docx(b"this is not a zip", ParseOptions::default())
            .expect_err("garbage bytes should not parse");
        assert!(matches!(err, ParseError::MalformedDocx(_)));
    }

    #[test]
    fn zip_without_document_xml_errors() {
        // A valid zip that lacks word/document.xml is not a docx.
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::FileOptions<()> = zip::write::FileOptions::default();
            w.start_file("hello.txt", opts).unwrap();
            use std::io::Write;
            w.write_all(b"hi").unwrap();
            w.finish().unwrap();
        }
        let err = parse_docx(&buf, ParseOptions::default())
            .expect_err("zip without document.xml should error");
        match err {
            ParseError::MalformedDocx(msg) => {
                assert!(
                    msg.contains("document.xml"),
                    "message should name the missing part: {msg}"
                )
            }
            other => panic!("expected MalformedDocx, got {other:?}"),
        }
    }

    // =====================================================================
    // Helper unit tests — styles resolution, classification, emphasis,
    // run assembly, escaping.
    // =====================================================================

    #[test]
    fn styles_resolve_outline_via_based_on_chain() {
        // CustomHeading basedOn Heading1 (outlineLvl 0) but sets no level of
        // its own → inherits 0. SetsOwn overrides its parent.
        let xml = r#"<w:styles>
            <w:style w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style>
            <w:style w:styleId="CustomHeading"><w:name w:val="My Heading"/><w:basedOn w:val="Heading1"/></w:style>
            <w:style w:styleId="SetsOwn"><w:name w:val="Sets Own"/><w:basedOn w:val="Heading1"/><w:pPr><w:outlineLvl w:val="3"/></w:pPr></w:style>
            <w:style w:styleId="Normal"><w:name w:val="Normal"/></w:style>
        </w:styles>"#;
        let map = build_styles_map(xml).unwrap();
        assert_eq!(map["Heading1"].outline_lvl, Some(0));
        assert_eq!(
            map["CustomHeading"].outline_lvl,
            Some(0),
            "inherits via basedOn"
        );
        assert_eq!(map["SetsOwn"].outline_lvl, Some(3), "own outlineLvl wins");
        assert_eq!(map["Normal"].outline_lvl, None);
    }

    #[test]
    fn styles_cyclic_based_on_is_safe() {
        // Pathological cycle A→B→A with no outlineLvl — must terminate.
        let xml = r#"<w:styles>
            <w:style w:styleId="A"><w:name w:val="A"/><w:basedOn w:val="B"/></w:style>
            <w:style w:styleId="B"><w:name w:val="B"/><w:basedOn w:val="A"/></w:style>
        </w:styles>"#;
        let map = build_styles_map(xml).unwrap();
        assert_eq!(map["A"].outline_lvl, None);
        assert_eq!(map["B"].outline_lvl, None);
    }

    #[test]
    fn classify_inline_outline_beats_style() {
        // Inline outlineLvl on the paragraph wins over the style lookup.
        let mut styles = HashMap::new();
        styles.insert(
            "Normal".to_string(),
            EffectiveStyle {
                name: Some("Normal".into()),
                outline_lvl: None,
            },
        );
        let (ty, lvl) = classify_paragraph(Some(2), Some("Normal"), &styles);
        assert_eq!(ty, SemanticElementType::Section);
        assert_eq!(lvl, 3, "outlineLvl 2 → depth 3");
    }

    #[test]
    fn classify_title_is_section_level_1() {
        let styles = HashMap::new();
        let (ty, lvl) = classify_paragraph(None, Some("Title"), &styles);
        assert_eq!(ty, SemanticElementType::Section);
        assert_eq!(lvl, 1);
    }

    #[test]
    fn classify_quote_is_blockquote() {
        let mut styles = HashMap::new();
        styles.insert(
            "Quote".to_string(),
            EffectiveStyle {
                name: Some("Quote".into()),
                outline_lvl: None,
            },
        );
        let (ty, lvl) = classify_paragraph(None, Some("Quote"), &styles);
        assert_eq!(ty, SemanticElementType::Blockquote);
        assert_eq!(lvl, 0);
    }

    #[test]
    fn classify_toc_and_normal_are_paragraph() {
        let mut styles = HashMap::new();
        styles.insert(
            "TOC1".to_string(),
            EffectiveStyle {
                name: Some("toc 1".into()),
                outline_lvl: None,
            },
        );
        assert_eq!(
            classify_paragraph(None, Some("TOC1"), &styles).0,
            SemanticElementType::Paragraph,
            "TOC entries have no outlineLvl → Paragraph (the TOC trap solves itself)"
        );
        assert_eq!(
            classify_paragraph(None, None, &HashMap::new()).0,
            SemanticElementType::Paragraph
        );
    }

    #[test]
    fn classify_outline_lvl_9_is_not_a_section() {
        // outlineLvl 9 is the OOXML "body text" sentinel (e.g. TOCHeading) —
        // not a heading. Falls through to Paragraph.
        let styles = HashMap::new();
        assert_eq!(
            classify_paragraph(Some(9), None, &styles).0,
            SemanticElementType::Paragraph
        );
    }

    #[test]
    fn wrap_emphasis_forms() {
        assert_eq!(wrap_emphasis("x", false, false), "x");
        assert_eq!(wrap_emphasis("x", true, false), "**x**");
        assert_eq!(wrap_emphasis("x", false, true), "*x*");
        assert_eq!(wrap_emphasis("x", true, true), "***x***");
        // Whitespace-only runs are never wrapped (no `** **`).
        assert_eq!(wrap_emphasis(" ", true, false), " ");
        assert_eq!(wrap_emphasis("", true, true), "");
    }

    #[test]
    fn escape_inline_markdown_escapes_emphasis_chars() {
        assert_eq!(escape_inline_markdown("a*b_c`d"), "a\\*b\\_c\\`d");
        assert_eq!(escape_inline_markdown("back\\slash"), "back\\\\slash");
        // Brackets are left literal (CR-61: literal `[text]` is fine).
        assert_eq!(escape_inline_markdown("[link]"), "[link]");
        assert_eq!(escape_inline_markdown("plain text"), "plain text");
    }

    #[test]
    fn run_bool_toggle_semantics() {
        let on = BytesStart::new("w:b");
        assert!(run_bool_on(&on), "bare <w:b/> is on");
        let off = BytesStart::from_content("w:b w:val=\"0\"", 3);
        assert!(!run_bool_on(&off), "<w:b w:val=\"0\"/> is off");
        let off2 = BytesStart::from_content("w:b w:val=\"false\"", 3);
        assert!(!run_bool_on(&off2));
    }

    #[test]
    fn local_name_strips_prefix() {
        assert_eq!(local_name(b"w:pStyle"), b"pStyle");
        assert_eq!(local_name(b"pStyle"), b"pStyle");
    }

    #[test]
    fn tab_and_break_become_whitespace() {
        // A minimal inline document: one body paragraph with a tab + break.
        let doc = r#"<w:document><w:body>
            <w:p><w:r><w:t>a</w:t><w:tab/><w:t>b</w:t><w:br/><w:t>c</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, "a\tb\nc");
        assert_eq!(els[0].element_type, SemanticElementType::Paragraph);
    }

    #[test]
    fn empty_paragraph_is_skipped() {
        let doc = r#"<w:document><w:body>
            <w:p><w:pPr><w:spacing w:after="0"/></w:pPr></w:p>
            <w:p><w:r><w:t>real</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1, "spacing-only paragraph is skipped");
        assert_eq!(els[0].text, "real");
        assert_eq!(els[0].text_order, 0, "text_order has no gap from the skip");
    }

    #[test]
    fn xml_space_preserve_keeps_leading_space() {
        let doc = "<w:document><w:body>\
            <w:p><w:r><w:t xml:space=\"preserve\"> leading</w:t></w:r></w:p>\
        </w:body></w:document>";
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1);
        // The preserved leading space survives in a Paragraph (not trimmed).
        assert_eq!(els[0].text, " leading");
    }

    #[test]
    fn hyperlink_run_text_is_inline() {
        // <w:hyperlink> wrapping a run: the run text flows inline AND produces
        // a ref (C2). The text is included inline regardless of the ref.
        let doc = r#"<w:document><w:body>
            <w:p><w:r><w:t>See </w:t></w:r><w:hyperlink w:anchor="x"><w:r><w:t>here</w:t></w:r></w:hyperlink><w:r><w:t> now</w:t></w:r></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(
            els[0].text, "See here now",
            "hyperlink run text included inline"
        );
    }

    // =====================================================================
    // C2 — hyperlink → ref mapping (synthetic XML, contract decision 7).
    // The rels parser itself is unit-tested in `rels.rs`; these exercise the
    // mapping a `<w:hyperlink>` attribute → InternalRef / ExternalRef, the
    // visible-text capture, and the skip cases.
    // =====================================================================

    /// Build a hyperlink rels map from `(id, target, external)` triples.
    fn rels_map(entries: &[(&str, &str, bool)]) -> HashMap<String, HyperlinkRel> {
        entries
            .iter()
            .map(|(id, target, external)| {
                let xml = format!(
                    r#"<Relationships><Relationship Id="{id}" Type="http://x/relationships/hyperlink" Target="{target}"{mode}/></Relationships>"#,
                    mode = if *external { r#" TargetMode="External""# } else { "" },
                );
                let m = super::super::rels::parse_hyperlink_rels(&xml).unwrap();
                (id.to_string(), m[*id].clone())
            })
            .collect()
    }

    #[test]
    fn hyperlink_anchor_becomes_internal_ref() {
        // `<w:hyperlink w:anchor="sec1">` → InternalRef::Named { name: "sec1" }
        // with the visible run text and no source page/bbox (Free flow).
        let doc = r#"<w:document><w:body>
            <w:p><w:r><w:t>Jump to </w:t></w:r><w:hyperlink w:anchor="sec1"><w:r><w:t>Section One</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, "Jump to Section One");
        assert_eq!(els[0].internal_refs.len(), 1, "one InternalRef");
        assert!(els[0].external_refs.is_empty());
        let r = &els[0].internal_refs[0];
        assert_eq!(r.text, "Section One", "visible link text captured");
        assert!(
            r.source_page.is_none() && r.source_bbox.is_none(),
            "Free flow"
        );
        match &r.target {
            InternalRefTarget::Named { name, page, point } => {
                assert_eq!(name, "sec1", "target = bookmark name");
                assert!(page.is_none() && point.is_none(), "no page/point in DOCX");
            }
            other => panic!("expected Named target, got {other:?}"),
        }
    }

    #[test]
    fn hyperlink_external_rid_becomes_external_ref() {
        // `<w:hyperlink r:id="rId7">` resolved via rels (TargetMode=External)
        // → ExternalRef::Uri { url } with the visible text.
        let doc = r#"<w:document><w:body>
            <w:p><w:r><w:t>Visit </w:t></w:r><w:hyperlink r:id="rId7"><w:r><w:t>our site</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let rels = rels_map(&[("rId7", "https://example.com/", true)]);
        let els = walk_body(doc, &HashMap::new(), &rels).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, "Visit our site");
        assert_eq!(els[0].external_refs.len(), 1, "one ExternalRef");
        assert!(els[0].internal_refs.is_empty());
        let r = &els[0].external_refs[0];
        assert_eq!(r.text, "our site");
        assert!(r.source_page.is_none() && r.source_bbox.is_none());
        match &r.target {
            ExternalRefTarget::Uri { url } => assert_eq!(url, "https://example.com/"),
        }
    }

    #[test]
    fn hyperlink_non_external_rid_is_skipped() {
        // An `r:id` whose rels target is NOT external (internal-part link) →
        // out of scope; no ref emitted, but the run text stays inline.
        let doc = r#"<w:document><w:body>
            <w:p><w:hyperlink r:id="rId3"><w:r><w:t>internal part</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let rels = rels_map(&[("rId3", "media/image1.png", false)]);
        let els = walk_body(doc, &HashMap::new(), &rels).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].text, "internal part", "text stays inline");
        assert!(
            els[0].internal_refs.is_empty() && els[0].external_refs.is_empty(),
            "non-External r:id emits no ref (decision 7)"
        );
    }

    #[test]
    fn hyperlink_unresolved_rid_is_skipped() {
        // An `r:id` absent from rels (dangling) → no ref, text preserved.
        let doc = r#"<w:document><w:body>
            <w:p><w:hyperlink r:id="rIdMissing"><w:r><w:t>dangling</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els[0].text, "dangling");
        assert!(els[0].internal_refs.is_empty() && els[0].external_refs.is_empty());
    }

    #[test]
    fn hyperlink_anchor_wins_when_both_present() {
        // A hyperlink carrying both w:anchor and r:id → InternalRef (anchor is
        // the same-document target; contract maps anchor → internal).
        let doc = r#"<w:document><w:body>
            <w:p><w:hyperlink w:anchor="here" r:id="rId7"><w:r><w:t>both</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let rels = rels_map(&[("rId7", "https://example.com/", true)]);
        let els = walk_body(doc, &HashMap::new(), &rels).unwrap();
        assert_eq!(els[0].internal_refs.len(), 1, "anchor takes precedence");
        assert!(els[0].external_refs.is_empty());
    }

    #[test]
    fn hyperlink_emphasis_in_visible_text_is_captured() {
        // The captured visible text is the *wrapped* run concatenation, so an
        // emphasized link text round-trips its markdown marks into ref.text.
        let doc = r#"<w:document><w:body>
            <w:p><w:hyperlink w:anchor="x"><w:r><w:rPr><w:b/></w:rPr><w:t>Bold Link</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els[0].internal_refs[0].text, "**Bold Link**");
    }

    #[test]
    fn multiple_hyperlinks_in_one_paragraph() {
        // Two links in one paragraph → two refs, each with its own slice of
        // visible text (offset-based capture keeps them separate).
        let doc = r#"<w:document><w:body>
            <w:p><w:hyperlink w:anchor="a"><w:r><w:t>first</w:t></w:r></w:hyperlink><w:r><w:t> and </w:t></w:r><w:hyperlink r:id="rId9"><w:r><w:t>second</w:t></w:r></w:hyperlink></w:p>
        </w:body></w:document>"#;
        let rels = rels_map(&[("rId9", "https://ex/", true)]);
        let els = walk_body(doc, &HashMap::new(), &rels).unwrap();
        assert_eq!(els[0].text, "first and second");
        assert_eq!(els[0].internal_refs.len(), 1);
        assert_eq!(els[0].internal_refs[0].text, "first");
        assert_eq!(els[0].external_refs.len(), 1);
        assert_eq!(els[0].external_refs[0].text, "second");
    }

    #[test]
    fn hyperlink_in_table_cell_attaches_to_table_node() {
        // A hyperlink inside a table cell → its ref bubbles up to the one
        // Table node (decision 6 flattens the table to a single node).
        let doc = r#"<w:document><w:body>
            <w:tbl><w:tr>
                <w:tc><w:p><w:hyperlink r:id="rId5"><w:r><w:t>cell link</w:t></w:r></w:hyperlink></w:p></w:tc>
                <w:tc><w:p><w:hyperlink w:anchor="bm"><w:r><w:t>cell anchor</w:t></w:r></w:hyperlink></w:p></w:tc>
            </w:tr></w:tbl>
        </w:body></w:document>"#;
        let rels = rels_map(&[("rId5", "https://cell/", true)]);
        let els = walk_body(doc, &HashMap::new(), &rels).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].element_type, SemanticElementType::Table);
        assert_eq!(els[0].external_refs.len(), 1, "external cell link → Table");
        assert_eq!(els[0].external_refs[0].text, "cell link");
        assert_eq!(els[0].internal_refs.len(), 1, "anchor cell link → Table");
        assert_eq!(els[0].internal_refs[0].text, "cell anchor");
    }

    #[test]
    fn table_flatten_rows_and_cells() {
        let doc = r#"<w:document><w:body>
            <w:tbl>
                <w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>
                <w:tr><w:tc><w:p><w:r><w:t>1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>2</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els.len(), 1);
        assert_eq!(els[0].element_type, SemanticElementType::Table);
        assert_eq!(els[0].text, "a | b\n1 | 2");
    }

    #[test]
    fn table_cell_emphasis_projected() {
        let doc = r#"<w:document><w:body>
            <w:tbl><w:tr><w:tc><w:p><w:r><w:rPr><w:b/></w:rPr><w:t>Head</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
        </w:body></w:document>"#;
        let els = walk_body(doc, &HashMap::new(), &HashMap::new()).unwrap();
        assert_eq!(els[0].text, "**Head**", "bold cell projects to `**…**`");
    }
}
