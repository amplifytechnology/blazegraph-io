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
    /// Analysis computed from text elements (font distributions, style stats)
    pub document_analysis: DocumentAnalysis,
}
/// The schema version stamped on every graph output.
/// Bump this when the output shape changes.
pub const SCHEMA_VERSION: &str = "0.2.0";

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
    pub style_info: Option<StyleMetadata>,
    pub token_count: usize,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

impl DocumentNode {
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenDistribution {
    pub by_node_type: HashMap<String, TokenHistogram>,
    pub overall: TokenHistogram,
}

impl Default for TokenDistribution {
    fn default() -> Self {
        Self {
            by_node_type: HashMap::new(),
            overall: TokenHistogram::default(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeTypeDistribution {
    pub counts: HashMap<String, usize>,
    pub percentages: HashMap<String, f32>,
}

impl Default for NodeTypeDistribution {
    fn default() -> Self {
        Self {
            counts: HashMap::new(),
            percentages: HashMap::new(),
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TikaOutput {
    pub xhtml_content: String,
    pub metadata: DocumentMetadata,
    pub text_elements: Vec<PdfTextElement>,
    /// XHTML content hash for Level 2 cache key generation
    pub xhtml_hash: String,
    // New enhanced structures
    pub style_data: StyleData, // CSS font classes (always present)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmark_data: Option<BookmarkData>, // PDF bookmarks/outline
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfTextElement {
    pub text: String,
    pub style_info: FontClass, // Self-contained style information (no Option)
    pub bounding_box: BoundingBox, // Required positioning (no Option)
    pub page_number: u32,
    pub paragraph_number: u32, // Which paragraph this belongs to
    pub line_number: u32,      // data-line from XHTML
    pub segment_number: u32,   // data-segment from XHTML
    pub reading_order: u32,    // computed from line + segment
    pub bookmark_match: Option<BookmarkSection>, // Full bookmark section if this span matches
    pub token_count: usize,    // Pre-calculated token count for performance
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
        if extracted.title.is_some() { self.title = extracted.title; }
        if extracted.author.is_some() { self.author = extracted.author; }
        if extracted.language.is_some() { self.language = extracted.language; }
        if extracted.page_count > 0 { self.page_count = extracted.page_count; }
        if extracted.publisher.is_some() { self.publisher = extracted.publisher; }
        if extracted.creator_tool.is_some() { self.creator_tool = extracted.creator_tool; }
        if extracted.producer.is_some() { self.producer = extracted.producer; }
        if extracted.pdf_version.is_some() { self.pdf_version = extracted.pdf_version; }
        if extracted.created.is_some() { self.created = extracted.created; }
        if extracted.modified.is_some() { self.modified = extracted.modified; }
        if extracted.description.is_some() { self.description = extracted.description; }
        if extracted.encrypted.is_some() { self.encrypted = extracted.encrypted; }
        if extracted.has_marked_content.is_some() { self.has_marked_content = extracted.has_marked_content; }
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
    elements.iter()
        .find(|e| e.element_type == ParsedElementType::Section)
        .map(|e| e.text.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Document analysis meta-attributes calculated from text elements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentAnalysis {
    /// Count of each exact font size found in the document
    pub font_size_counts: HashMap<String, usize>, // Use String for JSON compatibility
    /// Count of each font family found in the document
    pub font_family_counts: HashMap<String, usize>,
    /// Count of bold vs non-bold text elements (bold_count, non_bold_count)
    pub bold_counts: (usize, usize),
    /// Count of italic vs non-italic text elements (italic_count, non_italic_count)
    pub italic_counts: (usize, usize),

    /// Most frequently occurring font size in the document
    pub most_common_font_size: f32,
    /// Most frequently occurring font family in the document
    pub most_common_font_family: String,
    /// All font sizes found, sorted for analysis
    pub all_font_sizes: Vec<f32>,
}

impl DocumentAnalysis {
    /// Create document analysis from text elements
    pub fn analyze_text_elements(text_elements: &[PdfTextElement]) -> Self {
        let mut font_size_counts: HashMap<String, usize> = HashMap::new();
        let mut font_family_counts: HashMap<String, usize> = HashMap::new();
        let mut bold_count = 0;
        let mut non_bold_count = 0;
        let mut italic_count = 0;
        let mut non_italic_count = 0;
        let mut font_sizes = Vec::new();

        for element in text_elements {
            let style = &element.style_info;

            // Count font sizes
            let size_key = format!("{:.1}", style.font_size);
            *font_size_counts.entry(size_key).or_insert(0) += 1;
            font_sizes.push(style.font_size);

            // Count font families
            *font_family_counts
                .entry(style.font_family.clone())
                .or_insert(0) += 1;

            // Count bold/non-bold
            let is_bold = style.font_weight.to_lowercase().contains("bold");
            if is_bold {
                bold_count += 1;
            } else {
                non_bold_count += 1;
            }

            // Count italic/non-italic
            let is_italic = style.font_style.to_lowercase().contains("italic");
            if is_italic {
                italic_count += 1;
            } else {
                non_italic_count += 1;
            }
        }

        // Find most common font size
        let most_common_font_size = font_size_counts
            .iter()
            .max_by_key(|(_, &count)| count)
            .and_then(|(size_str, _)| size_str.parse::<f32>().ok())
            .unwrap_or(12.0);

        // Find most common font family
        let most_common_font_family = font_family_counts
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(family, _)| family.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Sort font sizes for analysis
        font_sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        Self {
            font_size_counts,
            font_family_counts,
            bold_counts: (bold_count, non_bold_count),
            italic_counts: (italic_count, non_italic_count),
            most_common_font_size,
            most_common_font_family,
            all_font_sizes: font_sizes,
        }
    }
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
    pub style_info: FontClass,     // Rich font data (no Option)
    pub bounding_box: BoundingBox, // Always present positioning
    pub page_number: u32,
    pub paragraph_number: u32,                   // New: paragraph context
    pub reading_order: u32,                      // New: spatial reading order
    pub bookmark_match: Option<BookmarkSection>, // New: bookmark section data
    pub token_count: usize,                      // Pre-calculated token count for performance
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ParsedElementType {
    Section,
    Paragraph,
    List,
    ListItem,
}
