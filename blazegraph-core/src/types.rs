use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use uuid::Uuid;

pub type NodeId = Uuid;

// ===== NODE LOCATION TYPES =====
// These types implement the location model from 001-document-model.
// SemanticLocation is always present (computed by GraphBuilder from tree structure).
// PhysicalLocation is only present for fixed-flow formats (PDF).

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeLocation {
    /// Always present — computed by GraphBuilder from final tree structure
    pub semantic: SemanticLocation,
    /// Only for fixed-flow formats (PDF) — passed through from channel
    pub physical: Option<PhysicalLocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticLocation {
    /// Hierarchical position in the document tree (e.g. "2.3.4")
    pub path: String,
    /// Tree depth (0 = root level)
    pub depth: u32,
    /// Human-readable trail (e.g. ["Chapter 2", "Methods", "Overview"])
    pub breadcrumbs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhysicalLocation {
    /// Page number (1-indexed)
    pub page: u32,
    /// Bounding box on the page
    pub bounding_box: BoundingBox,
}

/// Signals whether physical location data is meaningful for this document
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum FlowType {
    /// PDF — has physical layout, physical_location is present
    #[default]
    Fixed,
    /// Markdown, DOCX — reflows, physical_location is None
    Free,
}

/// Aggregated document-level information computed during parsing.
/// This is NOT a node in the tree — it is information *about* the document.
/// Has proto-L1 character: one per document, invariant to tree structure.
/// See 006-document-info-separation.md for design rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentInfo {
    /// References the Document node in nodes[] (the tree root)
    pub root_id: NodeId,
    /// Metadata extracted from the source format (title, author, page count, etc.)
    pub document_metadata: DocumentMetadata,
    /// PDF bookmarks/table of contents (if available in the source PDF)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmark_data: Option<BookmarkData>,

    /// Origin of this graph — the (version, source, config) triple that
    /// reproduces it. Persisted on the graph so the bgraph.md emitter
    /// (and any future round-trip consumer) has the load-bearing
    /// identity inputs without re-deriving them.
    ///
    /// `None` for legacy graphs loaded from pre-0.6.0 graph.json files
    /// and for graphs built via the legacy `GraphBuilder::build_graph`
    /// path (random UUIDv4 IDs, no hash inputs anyway).
    /// `Some(_)` for any graph built fresh through
    /// `GraphBuilder::build_graph_deterministic`.
    /// Schema 0.6.0 (B2 of MD+DOCX flow).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parse_provenance: Option<ParseProvenance>,

    /// Parsing-semantics signal. Bgraph.md v2.1.0+ (CR-49):
    ///
    /// - `"tree"` — spacelike content (documents, notes, derived corpus.md
    ///   files). Default semantically; absence means tree.
    /// - `"stream"` — timelike content (conversations, message logs).
    ///
    /// Lives on the doc-level `bgraph` block. The bgraph.md emitter writes
    /// this field when populated and skips it when `None`. URD's
    /// topology-aware storage uses this signal to choose dedup strategy
    /// (cross-pack global vs pack-scoped) at write time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topology: Option<String>,

    /// Logical-identity namespace for lineage tracking. Bgraph.md v2.1.0+
    /// (CR-49). Distinct from `source.sha256` (byte-identity): this carries
    /// stable identifiers (filesystem path, application-assigned stable ID)
    /// that survive content edits.
    ///
    /// - Tree, curated artifacts (PDFs): typically absent (`None`).
    /// - Tree, mutable files (notes): `path` SHOULD be populated;
    ///   `stable_id` SHOULD be populated when the source format supports it
    ///   (e.g., markdown frontmatter `id:` field).
    /// - Stream (conversations): `stable_id` MUST carry the conversation_id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_identity: Option<SourceIdentity>,

    /// URD address of the prior revision of this logical artifact.
    /// Bgraph.md v2.1.0+ (CR-49). 1:1 chain pointer — leave absent to model
    /// DAG-style forks (lineage links in URD's link drawer enumerate
    /// siblings via `source.sha256` when `supersedes` is absent).
    ///
    /// Use cases: note re-emit, re-emit under newer blazegraph, replacing
    /// config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
}

/// Lineage-identity namespace carried on `DocumentInfo`. Bgraph.md v2.1.0+
/// (CR-49). All sub-fields optional; emit any subset.
///
/// `content_hash` is **not** stored here — it would be a redundant copy of
/// `source.sha256`. Consumers reading content-hash lineage read it from
/// `source.sha256` directly.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SourceIdentity {
    /// Absolute filesystem path of the source artifact. Stable until
    /// rename. Populate for mutable files (notes, drafts); usually `None`
    /// for curated artifacts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    /// Application- or user-assigned stable ID. For notes: markdown
    /// frontmatter `id:` field. For conversations: the conversation_id.
    /// Forever-stable when assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_id: Option<String>,
}

/// Orphan struct reserved for the future stream-topology design slice.
///
/// CR-49 introduced this as the variant-specific metadata carrier for
/// `SemanticElementType::Message` on the shared `DocumentNode` /
/// `SemanticTreeElement` shapes. CR-59 retracted the wire format and
/// removed the field — the struct survives only as a placeholder for
/// the future stream-topology pipeline.
///
/// The shape (`speaker: Option<String>`, `timestamp: Option<String>`,
/// `turn_number: Option<u32>`) is provisional — the real stream-topology
/// design will likely promote `speaker` to a richer `{role, identifier?}`
/// object, and may add fields not anticipated here. Treat this as a
/// placeholder, not a contract.
///
/// Not constructed anywhere in v2.1.0.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageMetadata {
    pub speaker: Option<String>,
    pub timestamp: Option<String>,
    pub turn_number: Option<u32>,
}

/// Provenance record for a specific parse run. Persisted on
/// `DocumentInfo` so consumers can reproduce the graph deterministically
/// and emit round-trippable bgraph.md.
///
/// `(source_sha256, config_hash)` is the identity pair that feeds
/// `NodeIdGenerator::new`. Per CR-47, `blazegraph_version` rides
/// along as provenance documentation only — it no longer enters the
/// node-ID namespace, so node IDs survive parser version bumps for
/// the same `(source, config)`. See
/// `docs/P2/core/architecture/08-bgraph-md-format.md` (v2.0.0 wire
/// format) for the consumer contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseProvenance {
    /// Parser version that produced this graph (e.g. `"0.2.2"`).
    /// Sourced from `env!("CARGO_PKG_VERSION")` at build time.
    pub blazegraph_version: String,

    /// Source format identifier (`"pdf"`, `"markdown"`, `"docx"`, …).
    pub source_format: String,

    /// Original source filename (basename, e.g. `"rfc-quic.pdf"`).
    pub source_filename: String,

    /// SHA-256 of the source bytes (hex-encoded).
    pub source_sha256: String,

    /// Hash of the parsing config that produced this graph
    /// (hex-encoded).
    pub config_hash: String,
}
/// The schema version stamped on every graph output.
/// Bump this when the output shape changes.
///
/// 0.4.0 — Block 05 of the document-analytics flow: dropped `document_info.document_analysis`.
/// Analytics live in pipeline memory + sidecar dumps under `cache/stat/<name>/<hash>.json`,
/// not in the graph output schema.
///
/// 0.5.0 — Block 06b reading-order resort: dropped legacy `Placement.band`,
/// `Placement.column`, `Placement.nr_band_columns` (zeroed out since the
/// Block 1 layout-reasoning consolidation); added `Placement.region_label`
/// for region tree leaf annotation produced by `analytics::reading_order::tag_and_resort`.
///
/// 0.5.1 — Block 07 header/footer/margin classification: added `Header`,
/// `Footer`, `Margin` variants to `ParsedElementType` (and matching
/// `GroupType` variants). Element type is assigned at the
/// `PdfTextElement` → `ParsedPdfElement` boundary from `region_label`
/// (`H-*` → Header, `F-*` → Footer, `None` → Margin, body leaf labels →
/// Paragraph). Section detection skips Header / Footer / Margin.
///
/// 0.6.0 — B2 of MD+DOCX flow:
///   1. Added `DocumentInfo.parse_provenance: Option<ParseProvenance>`
///      so the bgraph.md emitter can produce real (not placeholder)
///      doc-level identity fields and downstream round-trip consumers
///      can re-derive deterministic node IDs without re-reading the
///      source bytes.
///   2. Moved `created_at` from `StructuralProfile` to
///      `SortedDocumentGraph` (the on-disk wrapper). `DocumentGraph`
///      is now time-free, which is the canonical-input invariant
///      required for `canonical_json(&DocumentGraph)` to be byte-
///      deterministic across runs of the same logical graph.
///
/// Backwards-compatible: existing 0.5.1 graphs deserialize cleanly
/// with `parse_provenance = None` and `SortedDocumentGraph.created_at`
/// defaulting to the Unix epoch (clearly "no real value").
///
/// 0.7.0 — B6 of MD+DOCX flow:
///   1. Added `CodeBlock`, `List`, `Blockquote`, `Table` variants to
///      `SemanticElementType` (and their string counterparts in
///      `DocumentNode.node_type`). These are produced by the markdown
///      channel; the PDF channel never produces them. The union schema
///      absorbs this asymmetry by design — graphs from one channel are
///      a subset of `SemanticElementType` and that is a feature.
///   2. Added canonical frontmatter fields to `DocumentMetadata`:
///      `date: Option<String>`, `tags: Vec<String>` (default empty),
///      `draft: Option<bool>`. These are the starting point for
///      normalizing metadata across MD / PDF / DOCX.
///   3. Added `DocumentMetadata.extras: BTreeMap<String,
///      serde_json::Value>` — an opaque pass-through bucket for
///      non-canonical frontmatter keys. YAML values are converted to
///      JSON values at the frontmatter-parse boundary so the schema
///      does not depend on the YAML library.
///
/// Backwards-compatible: existing 0.6.0 graphs deserialize cleanly
/// because all new fields default (`Option::None` / `Vec::new` /
/// `BTreeMap::new`), and the new enum variants are additive.
pub const SCHEMA_VERSION: &str = "0.7.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentGraph {
    pub nodes: HashMap<NodeId, DocumentNode>,
    pub document_info: DocumentInfo,
    pub structural_profile: StructuralProfile,
}

/// The serialization-ready output format. Carries a schema version
/// so consumers can detect and handle shape changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortedDocumentGraph {
    pub schema_version: String,
    /// Wall-clock time at which this graph was serialized to disk.
    /// Lives on the wrapper (not on `DocumentGraph`) so `canonical_json`
    /// is deterministic across runs of the same logical graph — see
    /// the canonical-input invariant in
    /// `docs/P2/core/architecture/08-bgraph-md-format.md`.
    /// `#[serde(default)]` keeps pre-0.6.0 graph.json fixtures (which
    /// carried `created_at` on `StructuralProfile` instead) loadable;
    /// the default is the Unix epoch — a clearly "no real value"
    /// sentinel rather than the misleading `Utc::now()`.
    #[serde(default = "default_created_at")]
    pub created_at: DateTime<Utc>,
    pub nodes: Vec<DocumentNode>,
    pub document_info: DocumentInfo,
    pub structural_profile: StructuralProfile,
}

/// Sentinel default for `SortedDocumentGraph.created_at` when loading
/// pre-0.6.0 fixtures that did not carry the field. Returns the Unix
/// epoch (1970-01-01T00:00:00Z) — clearly "no real value", in contrast
/// to `Utc::now()` which would silently lie about emission time.
fn default_created_at() -> DateTime<Utc> {
    DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is always valid")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentNode {
    pub id: NodeId,
    pub node_type: String,
    pub location: NodeLocation,
    pub text_order: Option<u32>,
    pub content: NodeContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style_info: Option<StyleMetadata>,
    pub token_count: usize,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

impl DocumentNode {
    /// Create a node with a random UUIDv4 ID (legacy, non-deterministic).
    pub fn new(node_type: &str, text: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            node_type: node_type.to_string(),
            location: NodeLocation {
                semantic: SemanticLocation {
                    path: String::new(),
                    depth: 0,
                    breadcrumbs: Vec::new(),
                },
                physical: None,
            },
            text_order: Some(0),
            content: NodeContent::new(text),
            style_info: None,
            token_count: 0,
            parent: None,
            children: Vec::new(),
        }
    }

    /// Create a node with a specific ID (for deterministic UUIDv5 generation).
    pub fn new_with_id(id: NodeId, node_type: &str, text: String) -> Self {
        Self {
            id,
            node_type: node_type.to_string(),
            location: NodeLocation {
                semantic: SemanticLocation {
                    path: String::new(),
                    depth: 0,
                    breadcrumbs: Vec::new(),
                },
                physical: None,
            },
            text_order: Some(0),
            content: NodeContent::new(text),
            style_info: None,
            token_count: 0,
            parent: None,
            children: Vec::new(),
        }
    }

    pub fn new_with_physical(
        node_type: &str,
        text: String,
        page: Option<u32>,
        bounding_box: Option<BoundingBox>,
    ) -> Self {
        let mut node = Self::new(node_type, text);
        if let Some(page) = page {
            node.location.physical = Some(PhysicalLocation {
                page,
                bounding_box: bounding_box.unwrap_or(BoundingBox {
                    x: 0.0,
                    y: 0.0,
                    width: 0.0,
                    height: 0.0,
                }),
            });
        }
        node
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeContent {
    pub text: String,
    // Future: can add node-type-specific fields here
    // pub heading_level: Option<u32>, // for sections
    // pub image_path: Option<String>, // for images
    // pub table_data: Option<TableData>, // for tables
}

/// Reserved line-prefixes that must never appear unescaped in
/// `NodeContent.text`. Lines starting (at column zero) with any of these
/// get a leading `\` prepended at construction. The escape uses
/// CommonMark's `\\`` backslash-escape semantics — invisible in
/// rendered markdown but breaks our line scanner's fence detection.
///
/// Initial member: `"```bgraph"` — the reserved fence prefix per the
/// bgraph.md spec § Reserved fence prefix. The bare-prefix match
/// captures every suffix variant in one rule (`bgraph`, `bgraph-section`,
/// `bgraph-anything-future`). See § Reserved-prefix escape contract
/// in `docs/P2/core/architecture/08-bgraph-md-format.md` for the
/// authoritative definition.
const RESERVED_LINE_PREFIXES: &[&str] = &["```bgraph"];

impl NodeContent {
    pub fn new(text: String) -> Self {
        Self {
            text: escape_reserved_prefixes(text).trim().to_string(),
        }
    }
}

/// Idempotently escape lines matching any [`RESERVED_LINE_PREFIXES`]
/// entry at column zero by prepending `\`. See `NodeContent::new`.
fn escape_reserved_prefixes(text: String) -> String {
    if !RESERVED_LINE_PREFIXES.iter().any(|seq| text.contains(seq)) {
        return text;
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut first = true;
    for line in text.split('\n') {
        if !first {
            out.push('\n');
        }
        first = false;
        if RESERVED_LINE_PREFIXES
            .iter()
            .any(|seq| line.starts_with(seq))
        {
            out.push('\\');
        }
        out.push_str(line);
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum NodeType {
    Document,
    Section { level: u32, title: String },
    Paragraph,
    List,
    ListItem,
    Table,
    Figure,
    Header,
    Footer,
}

/// Verbatim Tika style projection — see DT-03 for why this is the right shape now.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleMetadata {
    pub font_class: String,
    pub font_size: Option<f32>,
    pub is_bold: bool,
    pub is_italic: bool,
    pub font_family: Option<String>,
    /// CSS foreground color value (e.g., "#FF0000" or "rgb(255,0,0)").
    /// Renamed from `color` in CR-45 — this field has always been the
    /// foreground color; the rename clarifies that against the new
    /// `background_color` slot.
    pub foreground_color: Option<String>,
    /// CSS background color value (e.g., "#FFFFFF" or "rgb(255,255,255)").
    /// PDF channel: populated only when Tika surfaces `background-color` on
    /// the wrapping element. Most body-text spans have no explicit
    /// background, so `None` is the dominant case — see DT-03 for why we
    /// project verbatim rather than synthesize.
    pub background_color: Option<String>,
}

/// Channel-agnostic style information attached to `SemanticTreeElement`.
///
/// Reuses `StyleMetadata` (the shape DocumentNode already serializes) so
/// the projection boundary can carry style data through without lossy
/// reshaping. The v1 shape may not be final — channels populate as
/// best-effort or `None`. See `SemanticTreeElement::style` for usage.
pub type StyleInfo = StyleMetadata;

/// Quantitative measurement of graph shape — deterministic, mechanically computed from structure.
/// Travels with graph.json. Describes the L0 tree's statistical properties.
/// See AmplifyNotes/09-Profile-Types.md for design rationale.
///
/// **Canonical-input invariant (B2 of MD+DOCX flow):** this struct must
/// contain no time- or environment-dependent fields. The previous
/// `created_at: DateTime<Utc>` field moved to
/// `SortedDocumentGraph.created_at` (the on-disk wrapper) in schema
/// 0.6.0 so `canonical_json(&DocumentGraph)` is byte-deterministic
/// across runs of the same logical graph.
/// `#[serde(default)]` on the struct lets older fixtures (which had
/// `created_at` here) still deserialize cleanly — serde silently drops
/// the unknown field.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct StructuralProfile {
    pub document_type: DocumentType,
    pub flow_type: FlowType,
    pub total_nodes: usize,

    // Analytics fields
    pub total_tokens: usize,
    pub token_distribution: TokenDistribution,
    pub node_type_distribution: NodeTypeDistribution,
    pub depth_distribution: DepthDistribution,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum DocumentType {
    LegalContract,
    AcademicPaper,
    TechnicalManual,
    BusinessReport,
    Generic,
    #[default]
    Unknown,
}

// ===== ENHANCED GRAPH ANALYTICS STRUCTURES =====

/// Histogram-based token distribution for comprehensive statistical analysis
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenDistribution {
    pub by_node_type: HashMap<String, TokenHistogram>,
    pub overall: TokenHistogram,
}

/// Histogram representation enabling statistical calculations (mean, median, mode, variance)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenHistogram {
    pub bins: Vec<HistogramBin>,
    pub total_count: usize,
    pub total_tokens: usize,
    // Cached statistics for performance
    pub mean: f32,
    pub median: f32,
    pub mode: Option<u32>, // Bin with highest frequency
    pub variance: f32,
}

impl Default for TokenHistogram {
    fn default() -> Self {
        Self {
            bins: Vec::new(),
            total_count: 0,
            total_tokens: 0,
            mean: 0.0,
            median: 0.0,
            mode: None,
            variance: 0.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistogramBin {
    pub range_start: u32, // Inclusive
    pub range_end: u32,   // Exclusive
    pub count: usize,     // Number of nodes in this range
    pub token_sum: usize, // Total tokens in this range
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NodeTypeDistribution {
    pub counts: HashMap<String, usize>,
    pub percentages: HashMap<String, f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DepthDistribution {
    pub max_depth: u32,
    pub depth_counts: HashMap<u32, usize>,
    pub avg_depth: f32,
}

impl Default for DepthDistribution {
    fn default() -> Self {
        Self {
            max_depth: 0,
            depth_counts: HashMap::new(),
            avg_depth: 0.0,
        }
    }
}

// Note: StructuralHealth (variance/balance/richness heuristics) removed.
// Health assessment requires document-type context and belongs downstream
// of the L0 parser. See AmplifyNotes/09-Profile-Types.md.

// TikaOutput struct removed in CR-11 (cache architecture refactor).
// Raw XHTML is now cached as .xhtml files at cache point C1.
// Parsed elements live in PreprocessorOutput at cache point C2.

/// Spatial and structural metadata about where a text element lives in its source.
/// Populated for PDF-sourced elements. Future HTML/Markdown preprocessors may produce
/// elements with `placement: None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    /// 1-indexed page number on the source PDF.
    pub page_number: u32,

    /// Bounding box on the PDF page.
    pub bounding_box: BoundingBox,

    /// 0-indexed line number within the paragraph. From `data-line` on span.
    pub line_number: u32,

    /// 0-indexed segment number within the line. From `data-segment` on span.
    pub segment_number: u32,

    /// Text rotation in degrees: 0, 90, 180, 270. Non-zero when inside `<aside data-rotation="N">` (CR-10).
    pub rotation: i32,

    /// Tika's paragraph number within the page (per-page counter, reset each page).
    /// This is a Y-gap heuristic — reliable for clean prose, less so for math/list content.
    pub paragraph_number: u32,

    /// Region tree leaf label this element belongs to. Set by Block 06b
    /// (`analytics::reading_order::tag_and_resort`) when the analytics
    /// pre-pass runs. Values:
    ///   - `Some("1")`, `Some("2-1")`, etc. — body element in the named
    ///     Region tree leaf (depth-first reading-order path from
    ///     `RegionStats.per_page[…].root`).
    ///   - `Some("H-1")`, `Some("H-2")`, … — header element (y-asc within
    ///     the page's headers above `geometry.header_y`).
    ///   - `Some("F-1")`, `Some("F-2")`, … — footer element (y-asc within
    ///     the page's footers below `geometry.doc_footer_y`).
    ///   - `None` — orphan element (within body Y range but outside the
    ///     body X range — marginalia, sidebar). Placed at the end of the
    ///     page's reading order in original Tika sequence.
    ///
    /// Internal pipeline state — not a public schema field. Optional via
    /// `#[serde(default)]` so caches that predate this field deserialize
    /// cleanly with `region_label = None`.
    #[serde(default)]
    pub region_label: Option<String>,

    /// Width of the source PDF page in points. Sourced from Tika's
    /// `<div class="page-meta" data-width=…>` (added by Tika as part of
    /// the layout-reasoning consolidation flow). Carried on every
    /// element so analytics can size per-page heatmaps without a
    /// per-page side-channel.
    /// `#[serde(default)]` keeps c2-preprocessor caches written before
    /// this field existed deserializable; the value is 0.0 in that case
    /// and consumers should treat 0.0 as "unknown".
    #[serde(default)]
    pub page_width: f32,

    /// Height of the source PDF page in points. See `page_width`.
    #[serde(default)]
    pub page_height: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfTextElement {
    pub text: String,
    pub style_info: FontClass,
    pub placement: Placement, // REPLACES: bounding_box, page_number,
    //   paragraph_number, line_number,
    //   segment_number, rotation
    pub reading_order: u32,
    pub bookmark_match: Option<BookmarkSection>,
    pub token_count: usize,
    /// Unrecognized XHTML tag fragments that are descendants of this span (any depth).
    /// Each entry is the full unparsed fragment including content text.
    /// Empty for all spans in current core Tika output. A future corpus-tier Tika JAR
    /// will emit <a href>, <annotation>, etc. here.
    pub raw_tags: Vec<String>,
}

impl PdfTextElement {
    /// Bounding box of this element on the source PDF page.
    pub fn bounding_box(&self) -> &BoundingBox {
        &self.placement.bounding_box
    }

    /// 1-indexed page number on the source PDF.
    pub fn page_number(&self) -> u32 {
        self.placement.page_number
    }

    /// Text rotation in degrees.
    pub fn rotation(&self) -> i32 {
        self.placement.rotation
    }

    /// 0-indexed line number within the paragraph.
    pub fn line_number(&self) -> u32 {
        self.placement.line_number
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoundingBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    // page moved to DocumentNode level
}

/// Document-extracted metadata. Universal canonical fields at the top
/// (cross-channel consistency contract); channel-specific format-native
/// fields under named namespaces.
///
/// Wire-format home: the `bgraph-metadata` doc-level fence
/// (`docs/P2/core/architecture/08-bgraph-md-format.md` § Amendment I.3).
/// One per document; emitted whether or not any field is populated.
///
/// Canonical fields read **source-native only** — no body-side fallback
/// (per `09-metadata-first-class.md` § F-02 deferred to composition layer).
/// Title / author / language / description / created return `None` when the
/// source's metadata does not carry them; cleanup is a composition-layer
/// concern (URD or above), not extraction.
///
/// Schema 0.7.0 (B6): canonical fields + opaque `extras` were flat.
/// Schema 0.7.1+ (CR-57 / v2.1.0): namespaced channel-specific sub-objects
/// (`pdf` / `md` / `docx`) replace the flat per-channel fields.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentMetadata {
    // Universal extracted (canonical)
    pub title: Option<String>,
    pub author: Option<String>,
    pub description: Option<String>,
    pub language: Option<String>,
    pub created: Option<String>,

    // Channel-specific namespaces (typically only one populated per document).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pdf: Option<PdfMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub md: Option<MdMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docx: Option<DocxMetadata>,
}

/// PDF-channel metadata: strong-convention typed fields + `extras`
/// passthrough for any non-canonical XMP / `<meta>` tag the source carries.
///
/// `extras` closes the asymmetry from pre-CR-57 where unrecognized XMP tags
/// were silently dropped at the `_ => {}` arm in the flat extractor —
/// per `09-metadata-first-class.md` § Channel-specific (pdf).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PdfMetadata {
    pub version: Option<String>,           // pdf:PDFVersion
    pub producer: Option<String>,          // pdf:producer
    pub creator_tool: Option<String>,      // xmp:CreatorTool
    pub publisher: Option<String>,         // xmp:dc:publisher | dc:publisher
    pub page_count: Option<u32>,           // xmpTPg:NPages
    pub encrypted: Option<bool>,           // pdf:encrypted
    pub has_marked_content: Option<bool>,  // pdf:hasMarkedContent
    pub modified: Option<String>,          // dcterms:modified
    #[serde(default)]
    pub extras: BTreeMap<String, serde_json::Value>,
}

/// MD-channel metadata: strong-convention frontmatter slots (`draft`,
/// `tags`, `categories`) plus `extras` for everything else (Hugo `slug` /
/// `layout`, Astro `pubDate`, Obsidian aliases, …).
///
/// `BTreeMap` (not `HashMap`) so canonical JSON serialization is
/// deterministic — same input must produce the same `graph_sha256`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MdMetadata {
    pub draft: Option<bool>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub extras: BTreeMap<String, serde_json::Value>,
}

/// DOCX-channel metadata: strong-convention typed fields drawn from
/// `docProps/core.xml` + `docProps/app.xml`; `extras` passthrough for
/// `docProps/custom.xml` and any non-canonical OOXML properties.
///
/// Stub-ready (CR-57): the type lands in v2.1.0 so the schema is
/// complete and Track C (S10) does not need a follow-on schema bump.
/// The DOCX channel extractor is a stub returning `Default` until S10
/// wires the real OOXML probe.
/// Tagged union for channel-specific metadata, produced by a channel's
/// [`crate::preprocessors::metadata::MetadataExtractor`] impl and routed
/// into the corresponding namespace slot on [`DocumentMetadata`].
///
/// Adding a new channel adds a variant here; the type system carries the
/// dispatch — every match site updates under the compiler's eye.
#[derive(Debug, Clone)]
pub enum ChannelMetadata {
    Pdf(PdfMetadata),
    Md(MdMetadata),
    Docx(DocxMetadata),
    // Future: Html(HtmlMetadata), Epub(EpubMetadata), …
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocxMetadata {
    pub application: Option<String>,
    pub app_version: Option<String>,
    pub pages: Option<u32>,
    pub words: Option<u32>,
    pub characters: Option<u32>,
    pub lines: Option<u32>,
    pub paragraphs: Option<u32>,
    pub company: Option<String>,
    pub manager: Option<String>,
    pub template: Option<String>,
    pub total_time: Option<u32>,
    pub doc_security: Option<u32>,
    pub last_modified_by: Option<String>,
    pub revision: Option<String>,
    pub modified: Option<String>,
    #[serde(default)]
    pub extras: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleData {
    /// Sorted by key (font class name) so JSON serialization is deterministic.
    /// Required for content-addressed cache stability — see CR-40.
    pub font_classes: std::collections::BTreeMap<String, FontClass>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FontClass {
    pub class_name: String,  // "f1", "f2", "f3", etc. (kept for convenience)
    pub font_family: String, // "LiberationSerif-Italic"
    pub font_size: f32,      // 20.0
    pub font_style: String,  // "italic", "normal"
    pub font_weight: String, // "bold", "normal"
    pub color: String,       // "#000000"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkData {
    pub sections: Vec<BookmarkSection>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BookmarkSection {
    pub title: String,
    pub order: u32,
    /// Hierarchy level (1 = top-level, 2 = subsection, etc.)
    #[serde(default = "default_bookmark_level")]
    pub level: u32,
}

fn default_bookmark_level() -> u32 {
    1
}

#[derive(Debug, Clone)]
pub struct ClassificationResult {
    pub document_type: DocumentType,
    pub _confidence: f32,
}

// New output format structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequentialDocument {
    pub format: String,
    pub segments: Vec<SequentialSegment>,
    pub structural_profile: StructuralProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequentialSegment {
    pub id: usize,
    pub node_type: String,
    pub text: String,
    pub location: NodeLocation,
    pub style: Option<StyleMetadata>,
    pub tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlatDocument {
    pub format: String,
    pub chunks: Vec<String>,
}

// Enhanced List Detection - Two-Phase Processing
#[derive(Debug, Clone)]
pub struct ListSequence {
    pub start_index: usize,
    pub end_index: usize,
    pub marker_indices: Vec<usize>, // Positions of actual markers within sequence
}

// ===== TITLE INFERENCE =====

/// Infer a best-guess document title from parsed elements.
/// Used as a fallback when Tika metadata doesn't provide a title.
/// Current strategy: first Section element's text.
/// Future candidates: largest font on page 1, first bold text, etc.
pub fn infer_title(elements: &[ParsedPdfElement]) -> Option<String> {
    // Strategy 1: First section element
    elements
        .iter()
        .find(|e| e.element_type == ParsedElementType::Section)
        .map(|e| e.text.trim().to_string())
        .filter(|t| !t.is_empty())
}

// ===== GRAPH ANALYTICS IMPLEMENTATION =====

/// Result of analytics computation for any subset of nodes
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphAnalyticsResult {
    pub token_distribution: TokenDistribution,
    pub node_type_distribution: NodeTypeDistribution,
    pub depth_distribution: DepthDistribution,
}

/// Analytics computer that can analyze any subset of nodes in the graph
pub struct GraphAnalytics;

// ===== SEMANTIC TREE ELEMENT (channel boundary) =====
//
// `SemanticTreeElement` is the convergence type — what every input channel
// (PDF, MD, DOCX) projects to before the universal `GraphBuilder` consumes
// it. Format-specific quirks live in the channel projection; downstream
// (GraphBuilder) is channel-agnostic.
//
// See `preprocessors/pdf/semantic_tree_projection.rs` for the PDF
// channel's projection function, and `graphs/builder.rs` for the
// downstream consumer.

/// The convergence type — what every input channel (PDF, MD, DOCX) projects
/// to before the universal `GraphBuilder` consumes it.
///
/// Channel contract: when a `Vec<SemanticTreeElement>` is produced, all
/// format-specific transforms are complete. `GraphBuilder` walks-and-zips;
/// it does not merge, reorder, or post-process. `text_order` equals
/// projection-time vec position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticTreeElement {
    /// The text content of this element.
    pub text: String,

    /// What kind of element this is.
    pub element_type: SemanticElementType,

    /// Hierarchy level hint. Meaningful for `Section` (1 = top-level,
    /// 2 = nested, ...); for `Paragraph` / `Header` / `Footer` / `Margin`
    /// it is `0` (sentinel — these are leaves attached to the current
    /// open Section).
    pub hierarchy_level: u32,

    /// Position in the channel's projection-output vec. Zero-based,
    /// strictly sequential, no gaps. `GraphBuilder` uses this as the
    /// salt for deterministic ID generation; it also asserts
    /// `text_order == vec_index` as a sanity check.
    pub text_order: u32,

    /// Physical location, if the source format has it (PDF only).
    /// `None` for free-flow formats (Markdown, DOCX).
    pub physical_location: Option<PhysicalLocation>,

    /// Style information, if the source format has it. v1 shape
    /// may not be final — channels populate as best-effort or `None`.
    pub style: Option<StyleInfo>,

    /// Pre-computed token count.
    pub token_count: usize,
}

/// Element kinds carried at the SemanticTreeElement boundary.
///
/// Two clusters:
/// - **PDF-cluster:** `Section`, `Paragraph`, `Header`, `Footer`,
///   `Margin`. Produced by the PDF channel. `Header` / `Footer` /
///   `Margin` exist because they're PDF "running noise" — page numbers,
///   chapter headers, sidebars — that the markdown channel never
///   produces.
/// - **Markdown-cluster:** `CodeBlock`, `List`, `Blockquote`, `Table`.
///   Produced by the markdown channel. Each is one node holding the
///   verbatim raw markdown source for the block (with delimiters
///   preserved — fence + language tag, `>` markers, list bullets, table
///   pipes). The PDF channel never produces these; PDFs render to
///   prose `Paragraph` nodes regardless of source structure.
///
/// The asymmetry is a feature of the union schema, not a gap. Each
/// channel produces a subset; `SemanticElementType` is the full union.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SemanticElementType {
    Section,
    Paragraph,
    Header,
    Footer,
    Margin,
    // Schema 0.7.0+ (B6): markdown-channel block types.
    CodeBlock,
    List,
    Blockquote,
    Table,
    /// Orphan variant reserved for the future stream-topology design slice.
    ///
    /// CR-49 added the variant + wire-format support; CR-59 retracted the
    /// wire format (the variant had no in-memory production path —
    /// PDF and generic-MD are both tree-topology channels and never emit
    /// Message nodes — and the field-on-shared-types coupling forced
    /// tree-topology code to know about it). The variant survives as a
    /// sentinel marking the intended future shape: a separate
    /// stream-topology pipeline that will live alongside (not inside)
    /// the current tree-topology code path, with its own emitter, parser,
    /// and carrier type.
    ///
    /// Do not add a `Message` arm to any tree-topology code path. The
    /// builder maps it to `"Message"` only so `match` exhaustiveness
    /// compiles; emitter / parser have no arm for it (and reject it at
    /// the wire-format boundary).
    Message,
}
/// Complete output from document preprocessing
///
/// Contains all the data extracted from document parsing, including
/// text elements, metadata, styling information, and document structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessorOutput {
    /// Extracted text elements with positioning and styling
    pub text_elements: Vec<PdfTextElement>,
    /// Document metadata (title, author, creation date, etc.)
    pub metadata: DocumentMetadata,
    /// Style information (fonts, colors, formatting)
    pub style_data: StyleData,
    /// Document bookmarks/table of contents (if available)
    pub bookmark_data: Option<BookmarkData>,
}

// Rule engine structs

// New struct for enhanced TextElement processing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedPdfElement {
    pub element_type: ParsedElementType,
    pub text: String,
    pub hierarchy_level: u32,
    pub position: usize,
    pub style_info: FontClass,
    pub placement: Option<Placement>, // REPLACES: bounding_box, page_number, paragraph_number
    // None for non-PDF sources (future HTML/Markdown).
    pub reading_order: u32,
    pub bookmark_match: Option<BookmarkSection>,
    pub token_count: usize,
}

impl ParsedPdfElement {
    /// Return the placement for PDF-sourced elements.
    /// Panics with a clear message if called on a non-PDF element (placement is None).
    pub fn pdf_placement(&self) -> &Placement {
        self.placement
            .as_ref()
            .expect("PDF-sourced ParsedPdfElement must have placement")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ParsedElementType {
    Section,
    Paragraph,
    List,
    ListItem,
    Header,
    Footer,
    Margin,
}

impl ParsedElementType {
    /// Initial element type derived from a region label produced by
    /// `analytics::reading_order::tag_and_resort`.
    ///
    /// - `Some("H-*")` → `Header` (running header element)
    /// - `Some("F-*")` → `Footer` (running footer element)
    /// - `None` → `Margin` (orphan: sidebar / rotated / out-of-region content)
    /// - any other label (body leaf path like `"1"`, `"2-1"`, `"2-4-1"`) → `Paragraph`
    pub fn from_region_label(label: Option<&str>) -> Self {
        match label {
            Some(l) if l.starts_with("H-") => Self::Header,
            Some(l) if l.starts_with("F-") => Self::Footer,
            Some(_) => Self::Paragraph,
            None => Self::Margin,
        }
    }
}

#[cfg(test)]
mod parsed_element_type_tests {
    use super::ParsedElementType;

    #[test]
    fn header_label_maps_to_header() {
        assert_eq!(
            ParsedElementType::from_region_label(Some("H-1")),
            ParsedElementType::Header
        );
        assert_eq!(
            ParsedElementType::from_region_label(Some("H-12")),
            ParsedElementType::Header
        );
    }

    #[test]
    fn footer_label_maps_to_footer() {
        assert_eq!(
            ParsedElementType::from_region_label(Some("F-1")),
            ParsedElementType::Footer
        );
        assert_eq!(
            ParsedElementType::from_region_label(Some("F-3")),
            ParsedElementType::Footer
        );
    }

    #[test]
    fn body_leaf_label_maps_to_paragraph() {
        // Region tree leaf paths from `analytics::reading_order` —
        // depth-first DF order on the per-page Region tree.
        for label in ["1", "2-1", "2-4-1", "10"] {
            assert_eq!(
                ParsedElementType::from_region_label(Some(label)),
                ParsedElementType::Paragraph,
                "label {label:?} should map to Paragraph",
            );
        }
    }

    #[test]
    fn missing_label_maps_to_margin() {
        // `region_label = None` covers orphans: rotated content, sidebar
        // marginalia, elements outside any region tree leaf, pages with
        // no Region tree (e.g., NonBody pages where xy_cut bailed).
        assert_eq!(
            ParsedElementType::from_region_label(None),
            ParsedElementType::Margin
        );
    }

    #[test]
    fn header_footer_prefix_is_strict() {
        // A label that merely contains "H-" or "F-" mid-string is a body
        // leaf path, not a header / footer. Only the leading prefix counts.
        assert_eq!(
            ParsedElementType::from_region_label(Some("1-H-2")),
            ParsedElementType::Paragraph
        );
        assert_eq!(
            ParsedElementType::from_region_label(Some("Hello")),
            ParsedElementType::Paragraph
        );
    }
}

// =========================================================================
// v2.0.0 reserved-prefix escape contract tests (CR-48 / Amendment H).
// =========================================================================

#[cfg(test)]
mod node_content_escape_tests {
    use super::NodeContent;

    #[test]
    fn escapes_bare_reserved_prefix_at_column_zero() {
        let nc = NodeContent::new("```bgraph".to_string());
        assert_eq!(nc.text, "\\```bgraph");
    }

    #[test]
    fn escapes_suffixed_reserved_prefix_at_column_zero() {
        for suffix in &["-section", "-paragraph", "-bookmarks", "-anything-future"] {
            let raw = format!("```bgraph{suffix}");
            let nc = NodeContent::new(raw.clone());
            assert_eq!(
                nc.text,
                format!("\\{raw}"),
                "bare-prefix match should capture suffix {suffix:?}"
            );
        }
    }

    #[test]
    fn does_not_escape_reserved_prefix_mid_line() {
        // The line scanner only matches at column zero, so mid-line
        // mentions of the reserved prefix don't need escaping.
        let prose = "The reserved prefix is ```bgraph at column zero.".to_string();
        let nc = NodeContent::new(prose.clone());
        assert_eq!(nc.text, prose);
    }

    #[test]
    fn does_not_escape_other_code_fence_languages() {
        for line in &["```rust", "```python", "```", "```bash"] {
            let nc = NodeContent::new((*line).to_string());
            assert_eq!(
                nc.text, *line,
                "non-bgraph code fence {line:?} must pass through unchanged"
            );
        }
    }

    #[test]
    fn escape_is_idempotent() {
        // Applying NodeContent::new twice must equal applying it once.
        // This is the round-trip-stability property: parse reads
        // already-escaped text and constructing a NodeContent on it
        // does not double-escape.
        let raw = "```bgraph-section\nmore text".to_string();
        let once = NodeContent::new(raw);
        let twice = NodeContent::new(once.text.clone());
        assert_eq!(once.text, twice.text);
    }

    #[test]
    fn escapes_only_lines_starting_with_reserved_prefix() {
        // Multi-line body with a mix: only the reserved-prefix line
        // should be escaped, others pass through.
        let raw = "First line.\n```bgraph-paragraph\nThird line.".to_string();
        let nc = NodeContent::new(raw);
        assert_eq!(nc.text, "First line.\n\\```bgraph-paragraph\nThird line.");
    }

    #[test]
    fn trim_still_applies_after_escape() {
        // C-5 trim semantics survive — leading/trailing whitespace of
        // the whole text is stripped, but internal newlines preserve.
        let nc = NodeContent::new("  \n```bgraph\nbody\n  ".to_string());
        assert_eq!(nc.text, "\\```bgraph\nbody");
    }
}
