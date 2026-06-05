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
//! ## Scope (C1)
//!
//! Body structure only: Section / Paragraph / Table / Blockquote, depth from
//! the `outlineLvl` gate, run-text concatenation, and emphasis projection
//! (`<w:b/>` → `**…**`, `<w:i/>` → `*…*`). **Out of scope** (later handoffs):
//! hyperlink ref extraction (C2 — but hyperlink run *text* IS included
//! inline), metadata population (C3), CLI dispatch (C4), header/footer (C5).
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

    // 2. Styles resolution map.
    let styles = build_styles_map(&styles_xml)?;

    // 3. Body walk → elements.
    let elements = walk_body(&document_xml, &styles)?;

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
}

/// Walk `<w:body>`'s top-level children (`<w:p>` and `<w:tbl>`) in document
/// order, projecting each to one [`SemanticTreeElement`]. Paragraphs nested
/// inside table cells are consumed by the table-text assembly, not emitted
/// as standalone nodes.
fn walk_body(
    document_xml: &str,
    styles: &HashMap<String, EffectiveStyle>,
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
                            para_on_end(acc, name);
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
                            table_on_end(acc, name);
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

fn para_on_end(acc: &mut ParagraphAccumulator, name: &[u8]) {
    match name {
        b"pPr" => {
            acc.in_ppr = false;
            acc.seen_ppr_end = true;
        }
        b"t" => acc.runs.in_t = false,
        b"rPr" => acc.runs.in_rpr = false,
        b"r" => acc.runs.on_run_end(),
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

    let mut text = acc.runs.finish();
    // Trim Section heading text (heading-marker semantics, decision 5);
    // leave Paragraph/Blockquote text as-assembled.
    if element_type == SemanticElementType::Section {
        text = text.trim().to_string();
    }
    // Skip empty paragraphs (decision 6b) — also keeps `.validate()` from
    // rejecting an empty-body Paragraph/Section.
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
            internal_refs: vec![],
            external_refs: vec![],
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

fn table_on_end(acc: &mut TableAccumulator, name: &[u8]) {
    match name {
        b"tc" => {
            if let Some(cell) = acc.cell_runs.take() {
                acc.current_row.push(cell.finish());
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
        internal_refs: vec![],
        external_refs: vec![],
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
}

impl RunAssembler {
    fn on_run_start(&mut self) {
        self.in_run = true;
        self.in_rpr = false;
        self.bold = false;
        self.italic = false;
        self.run_text.clear();
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

    fn finish(self) -> String {
        self.out
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
