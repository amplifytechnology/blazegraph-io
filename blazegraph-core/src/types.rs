use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FlowType {
    /// PDF — has physical layout, physical_location is present
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
pub const SCHEMA_VERSION: &str = "0.5.0";

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
    pub nodes: Vec<DocumentNode>,
    pub document_info: DocumentInfo,
    pub structural_profile: StructuralProfile,
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

impl NodeContent {
    pub fn new(text: String) -> Self {
        Self {
            text: text.trim().to_string(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleMetadata {
    pub font_class: String,
    pub font_size: Option<f32>,
    pub is_bold: bool,
    pub is_italic: bool,
    pub font_family: Option<String>,
    pub color: Option<String>, // CSS color value (e.g., "#FF0000" or "rgb(255,0,0)")
}

/// Quantitative measurement of graph shape — deterministic, mechanically computed from structure.
/// Travels with graph.json. Describes the L0 tree's statistical properties.
/// See AmplifyNotes/09-Profile-Types.md for design rationale.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralProfile {
    pub created_at: DateTime<Utc>,
    pub document_type: DocumentType,
    pub flow_type: FlowType,
    pub total_nodes: usize,

    // Analytics fields
    pub total_tokens: usize,
    pub token_distribution: TokenDistribution,
    pub node_type_distribution: NodeTypeDistribution,
    pub depth_distribution: DepthDistribution,
}

impl Default for StructuralProfile {
    fn default() -> Self {
        Self {
            created_at: Utc::now(),
            document_type: DocumentType::Unknown,
            flow_type: FlowType::Fixed,
            total_nodes: 0,
            total_tokens: 0,
            token_distribution: TokenDistribution::default(),
            node_type_distribution: NodeTypeDistribution::default(),
            depth_distribution: DepthDistribution::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DocumentType {
    LegalContract,
    AcademicPaper,
    TechnicalManual,
    BusinessReport,
    Generic,
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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DocumentMetadata {
    // Current fields
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
    pub page_count: u32,

    // Enhanced flat fields from <meta> tags
    pub publisher: Option<String>,        // xmp:dc:publisher
    pub creator_tool: Option<String>,     // xmp:CreatorTool
    pub producer: Option<String>,         // pdf:producer
    pub pdf_version: Option<String>,      // pdf:PDFVersion
    pub created: Option<String>,          // dcterms:created
    pub modified: Option<String>,         // dcterms:modified
    pub description: Option<String>,      // dc:description
    pub encrypted: Option<bool>,          // pdf:encrypted
    pub has_marked_content: Option<bool>, // pdf:hasMarkedContent
}

impl DocumentMetadata {
    /// Merge extracted metadata on top of current values.
    /// Non-None fields from `extracted` overwrite; None fields preserve existing.
    /// page_count overwrites if > 0.
    pub fn merge_extracted(&mut self, extracted: DocumentMetadata) {
        if extracted.title.is_some() {
            self.title = extracted.title;
        }
        if extracted.author.is_some() {
            self.author = extracted.author;
        }
        if extracted.language.is_some() {
            self.language = extracted.language;
        }
        if extracted.page_count > 0 {
            self.page_count = extracted.page_count;
        }
        if extracted.publisher.is_some() {
            self.publisher = extracted.publisher;
        }
        if extracted.creator_tool.is_some() {
            self.creator_tool = extracted.creator_tool;
        }
        if extracted.producer.is_some() {
            self.producer = extracted.producer;
        }
        if extracted.pdf_version.is_some() {
            self.pdf_version = extracted.pdf_version;
        }
        if extracted.created.is_some() {
            self.created = extracted.created;
        }
        if extracted.modified.is_some() {
            self.modified = extracted.modified;
        }
        if extracted.description.is_some() {
            self.description = extracted.description;
        }
        if extracted.encrypted.is_some() {
            self.encrypted = extracted.encrypted;
        }
        if extracted.has_marked_content.is_some() {
            self.has_marked_content = extracted.has_marked_content;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StyleData {
    pub font_classes: std::collections::HashMap<String, FontClass>,
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

// Graph builder structs
#[derive(Debug, Clone)]
pub struct ElementGroup {
    pub elements: Vec<ParsedPdfElement>,
    pub group_type: GroupType,
    pub hierarchy_level: u32,
    pub combined_text: String,
}

#[derive(Debug, Clone)]
pub enum GroupType {
    Section,
    Paragraph,
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
}
