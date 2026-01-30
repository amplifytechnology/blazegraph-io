use crate::types::DocumentType;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;

// Default value functions for serde
fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsingConfig {
    pub document_type: DocumentType,
    #[serde(default)]
    pub section_and_hierarchy: SectionAndHierarchyConfig,
    pub spatial_clustering: SpatialClusteringConfig,
    pub section_patterns: Vec<String>,
    /// Include raw Tika XML/HTML output in graph metadata for debugging
    #[serde(default)]
    pub include_raw_tika: bool,
    /// Pipeline configuration - defines which rules to run and in what order
    #[serde(default)]
    pub pipeline: PipelineConfig,
    /// List detection configuration
    #[serde(default)]
    pub list_detection: ListDetectionConfig,
    /// Size enforcement configuration
    #[serde(default)]
    pub size_enforcer: SizeEnforcerConfig,
    /// Minimal parse mode - bypasses all rule processing and returns only base conversion
    #[serde(default)]
    pub minimal_parse: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    /// List of rules to run in order
    pub rules: Vec<RuleConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleConfig {
    /// Name of the rule
    pub name: String,
    /// Whether this rule is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            rules: vec![
                RuleConfig {
                    name: "SpatialClustering+StyleAnalysis".to_string(),
                    enabled: true,
                },
                RuleConfig {
                    name: "Validation".to_string(),
                    enabled: true,
                },
            ],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SectionAndHierarchyConfig {
    /// Font size analysis parameters
    /// Percentage above median for large headers (0.0-1.0)
    pub large_header_threshold: f32,
    /// Percentage above median for medium headers (0.0-1.0)
    pub medium_header_threshold: f32,
    /// Percentage above median for small headers (0.0-1.0)
    pub small_header_threshold: f32,
    /// Minimum absolute font size to consider for headers
    pub min_header_size: f32,
    /// Use bold text as additional header indicator
    pub use_bold_indicator: bool,
    /// Require bold text to be larger than typical content to be considered a section
    /// true = strict (bold AND larger), false = permissive (bold OR larger)  
    pub bold_size_strict: bool,

    /// Contextual hierarchy parameters
    /// Maximum hierarchy depth to create
    pub max_depth: u32,
    /// Font size difference tolerance for considering sections at same level (points)
    pub font_size_tolerance: f32,
    /// Whether to enforce max depth limit (if false, allows unlimited depth)
    pub enforce_max_depth: bool,
    /// Starting level for first section (document root is level 0)
    pub starting_section_level: u32,

    /// Pattern-based section detection configuration
    pub pattern_detection: PatternDetectionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternDetectionConfig {
    /// Whether pattern-based detection is enabled
    pub enabled: bool,
    /// Regex patterns to match section headers
    pub patterns: Vec<String>,
    /// Whether to respect font size constraints even when pattern matches
    pub respect_font_constraints: bool,
}

impl Default for PatternDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            patterns: vec![
                // More restrictive patterns to avoid false positives
                r"^[A-Z][A-Z\s]{2,}$".to_string(), // ALL CAPS (min 3 chars total)
                r"^\d+\.\s+[A-Z][a-z]{3,}".to_string(), // "1. Title" (min 4 chars in title)
                r"^(Chapter|Section|Part|Article)\s+\d+".to_string(), // Explicit structural words
                r"^[A-Z][a-z]{2,}(?:\s+[A-Z][a-z]{2,})*:$".to_string(), // "Title Case:" (with colon, min 3 chars per word)
            ],
            respect_font_constraints: true,
        }
    }
}

impl Default for SectionAndHierarchyConfig {
    fn default() -> Self {
        Self {
            large_header_threshold: 0.7,
            medium_header_threshold: 0.3,
            small_header_threshold: 0.1,
            min_header_size: 8.5,
            use_bold_indicator: true,
            bold_size_strict: true,  // Default to strict mode (bold AND larger)
            max_depth: 5,
            font_size_tolerance: 0.1,
            enforce_max_depth: true,
            starting_section_level: 1,
            pattern_detection: PatternDetectionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialClusteringConfig {
    /// Enable spatial clustering (if false, falls back to old method)
    pub enabled: bool,
    /// Enable paragraph merging based on Tika's paragraph_number detection
    #[serde(default = "default_true")]
    pub enable_paragraph_merging: bool,
    /// Enable spatial adjacency clustering (groups spatially adjacent elements)
    #[serde(default)]
    pub enable_spatial_adjacency: bool,
    /// Minimum line height in points
    pub min_line_height: f32,
    /// Multiplier for line height to detect section breaks (e.g., 0.8 = 80% of line height)
    pub vertical_gap_threshold_multiplier: f32,
    /// X-coordinate tolerance for text alignment in points
    pub horizontal_alignment_tolerance: f32,
    /// Line tolerance as percentage of line height for grouping text lines
    pub line_grouping_tolerance: f32,
    /// Configuration for section clustering
    pub sections: ElementClusteringConfig,
    /// Configuration for paragraph clustering
    pub paragraphs: ElementClusteringConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementClusteringConfig {
    /// Minimum segment size in characters (segments smaller than this get merged)
    pub min_segment_size: usize,
    /// Maximum segment size in characters (segments larger than this get split if possible)
    pub max_segment_size: usize,
}

// Default value functions for list detection
fn default_y_tolerance() -> f32 {
    15.0
}


fn default_false() -> bool {
    false
}

fn default_bullet_patterns() -> Vec<String> {
    vec![
        "•".to_string(),
        "·".to_string(),
        "●".to_string(),
        "■".to_string(),
        "▪".to_string(),
        "▫".to_string(),
        "◦".to_string(),
        "‣".to_string(),
        "⁃".to_string(),
        "-".to_string(),
        "*".to_string(),
        "→".to_string(),
        "➤".to_string(),
        "✓".to_string(),
        "&bull;".to_string(),
        "&middot;".to_string(),
    ]
}

fn default_numbered_patterns() -> Vec<String> {
    vec![
        r"^\d+\.".to_string(),    // 1., 2., 3.
        r"^\d+\)".to_string(),    // 1), 2), 3)
        r"^\(\d+\)".to_string(),  // (1), (2), (3)
        r"^[a-z]\.".to_string(),  // a., b., c.
        r"^[a-z]\)".to_string(),  // a), b), c)
        r"^[A-Z]\.".to_string(),  // A., B., C.
        r"^[A-Z]\)".to_string(),  // A), B), C)
        r"^[ivx]+\.".to_string(), // i., ii., iii.
        r"^[IVX]+\.".to_string(), // I., II., III.
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListDetectionConfig {
    /// Whether list detection is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Phase 1: Sequence Detection (NEW)
    /// How far to look for next marker (in elements)
    #[serde(default = "default_sequence_lookahead_elements")]
    pub sequence_lookahead_elements: usize,

    /// Elements past last marker to include in sequence boundary
    #[serde(default = "default_sequence_boundary_extension")]
    pub sequence_boundary_extension: usize,

    /// Phase 2: Content Classification
    /// Y-coordinate tolerance for considering elements on the same line (in points)
    #[serde(default = "default_y_tolerance")]
    pub y_tolerance: f32,


    /// List item patterns
    /// Bullet point patterns to detect
    #[serde(default = "default_bullet_patterns")]
    pub bullet_patterns: Vec<String>,

    /// Numbered list patterns (regex)
    #[serde(default = "default_numbered_patterns")]
    pub numbered_patterns: Vec<String>,

    /// List grouping behavior
    /// Whether to create List container nodes for consecutive list items
    #[serde(default = "default_true")]
    pub create_list_containers: bool,

    /// Whether to preserve individual ListItem nodes within List containers
    #[serde(default = "default_false")]
    pub preserve_list_items: bool,

    /// Maximum number of elements to look ahead for list item continuation
    #[serde(default = "default_max_lookahead_elements")]
    pub max_lookahead_elements: usize,

    /// Last list item boundary detection
    /// Y-gap threshold (in points) for detecting spatial disconnects in last list items
    /// TODO: OPTIMIZATION_DESIGN phase - fine-tune this value based on document types
    #[serde(default = "default_last_item_boundary_gap")]
    pub last_item_boundary_gap: f32,

    /// Phase 2.5: List Validation (NEW)
    /// Configuration for validating detected lists to eliminate false positives
    #[serde(default)]
    pub validation: ListValidationConfig,
}

fn default_sequence_lookahead_elements() -> usize {
    10 // Elements to look ahead for next marker in sequence
}

fn default_sequence_boundary_extension() -> usize {
    3 // Elements past last marker to include for boundary detection
}

fn default_max_lookahead_elements() -> usize {
    25 // Increased from 5 to handle more complex list structures
}

fn default_last_item_boundary_gap() -> f32 {
    80.0 // Y-gap threshold for sequence end detection (increased from 20.0)
}

// List validation default functions
fn default_validation_enabled() -> bool {
    true
}

// Advanced validation rule configurations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequentialNumberingConfig {
    /// Allow letter sequences (a, b, c) in addition to numbers
    #[serde(default = "default_true")]
    pub allow_letter_sequences: bool,
    
    /// Maximum gap tolerance between numbers (0 = no gaps allowed)
    #[serde(default = "default_zero")]
    pub max_gap_tolerance: u32,
}

impl Default for SequentialNumberingConfig {
    fn default() -> Self {
        Self {
            allow_letter_sequences: true,
            max_gap_tolerance: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MathematicalContextConfig {
    /// Mathematical symbols to detect
    #[serde(default = "default_mathematical_symbols")]
    pub symbols: Vec<String>,
    
    /// Mathematical terms that indicate context
    #[serde(default = "default_mathematical_terms")]
    pub terms: Vec<String>,
}

impl Default for MathematicalContextConfig {
    fn default() -> Self {
        Self {
            symbols: default_mathematical_symbols(),
            terms: default_mathematical_terms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HyphenContextConfig {
    /// Strategy for handling hyphens: "reject", "strict", "context_aware"
    #[serde(default = "default_hyphen_strategy")]
    pub strategy: String,
    
    /// Require space after hyphen for valid lists
    #[serde(default = "default_true")]
    pub require_space_after: bool,
}

impl Default for HyphenContextConfig {
    fn default() -> Self {
        Self {
            strategy: default_hyphen_strategy(),
            require_space_after: true,
        }
    }
}

// Default value functions for advanced validation
fn default_zero() -> u32 {
    0
}

fn default_mathematical_symbols() -> Vec<String> {
    vec![
        "→".to_string(),
        "←".to_string(), 
        "⇒".to_string(),
        "⇐".to_string(),
        "∀".to_string(),
        "∃".to_string(),
    ]
}

fn default_mathematical_terms() -> Vec<String> {
    vec![
        "equation".to_string(),
        "formula".to_string(),
        "coordinates".to_string(),
        "system".to_string(),
        "transform".to_string(),
    ]
}

fn default_hyphen_strategy() -> String {
    "strict".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListValidationConfig {
    /// Whether list validation is enabled
    #[serde(default = "default_validation_enabled")]
    pub enabled: bool,
    
    /// Minimum number of items required for a valid list
    #[serde(default = "default_true")]
    pub minimum_size_check: bool,
    
    /// Validate that numbered lists start with "1" (or equivalent first item)
    #[serde(default = "default_true")]
    pub first_item_validation: bool,
    
    /// If using parenthetical numbering (n), must start with (1)
    #[serde(default = "default_true")]
    pub parenthetical_context_check: bool,
    
    // Advanced validation rules (enabled by default)
    #[serde(default = "default_true")]
    pub sequential_numbering_check: bool,
    
    #[serde(default = "default_true")]
    pub mathematical_context_check: bool,
    
    #[serde(default = "default_true")]
    pub hyphen_context_check: bool,
    
    // Rule-specific configurations
    #[serde(default)]
    pub sequential_numbering: SequentialNumberingConfig,
    
    #[serde(default)]
    pub mathematical_context: MathematicalContextConfig,
    
    #[serde(default)]
    pub hyphen_context: HyphenContextConfig,
    
    // Future validation rules (disabled by default)
    #[serde(default = "default_false")]
    pub sequence_pattern_check: bool,
    
    #[serde(default = "default_false")]
    pub content_quality_check: bool,
    
    #[serde(default = "default_false")]
    pub spatial_coherence_check: bool,
}

impl Default for ListValidationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            minimum_size_check: true,
            first_item_validation: true,
            parenthetical_context_check: true,
            sequential_numbering_check: true,
            mathematical_context_check: true,
            hyphen_context_check: true,
            sequential_numbering: SequentialNumberingConfig::default(),
            mathematical_context: MathematicalContextConfig::default(),
            hyphen_context: HyphenContextConfig::default(),
            sequence_pattern_check: false,
            content_quality_check: false,
            spatial_coherence_check: false,
        }
    }
}

// SizeEnforcerRule default functions
fn default_max_size() -> usize {
    800 // characters by default
}

fn default_size_unit() -> String {
    "characters".to_string()
}

fn default_min_split_size_ratio() -> f32 {
    0.25 // 25% of max_size
}

fn default_max_iterations() -> usize {
    10 // safety limit for recursive splitting
}

fn default_split_direction() -> String {
    "vertical".to_string() // split chunks stack vertically like separate paragraphs
}

impl Default for ListDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sequence_lookahead_elements: default_sequence_lookahead_elements(),
            sequence_boundary_extension: default_sequence_boundary_extension(),
            y_tolerance: default_y_tolerance(),
            bullet_patterns: default_bullet_patterns(),
            numbered_patterns: default_numbered_patterns(),
            create_list_containers: true,
            preserve_list_items: false,
            max_lookahead_elements: default_max_lookahead_elements(),
            last_item_boundary_gap: default_last_item_boundary_gap(),
            validation: ListValidationConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SizeEnforcerConfig {
    /// Whether size enforcement is enabled
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum allowed size for any single node
    #[serde(default = "default_max_size")]
    pub max_size: usize,

    /// What to measure: "characters", "words", or "bytes"
    #[serde(default = "default_size_unit")]
    pub size_unit: String,

    /// Ensure sentence boundaries are respected when splitting
    #[serde(default = "default_true")]
    pub preserve_sentences: bool,

    /// Minimum size of resulting chunks (as ratio of max_size)
    #[serde(default = "default_min_split_size_ratio")]
    pub min_split_size_ratio: f32,

    /// Enable recursive splitting until all nodes are compliant
    #[serde(default = "default_true")]
    pub recursive: bool,

    /// Safety limit for recursive splitting
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,

    /// How to split bounding boxes: "horizontal" (side-by-side) or "vertical" (stacked)
    #[serde(default = "default_split_direction")]
    pub split_direction: String,
}

impl Default for SizeEnforcerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 800,
            size_unit: "characters".to_string(),
            preserve_sentences: true,
            min_split_size_ratio: 0.25,
            recursive: true,
            max_iterations: 10,
            split_direction: "vertical".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigManager {
    configs: HashMap<DocumentType, ParsingConfig>,
    default_config: ParsingConfig,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        let mut manager = Self {
            configs: HashMap::new(),
            default_config: Self::create_default_generic_config(),
        };

        // Load built-in configs
        manager.load_builtin_configs()?;

        Ok(manager)
    }

    pub fn get_config(&self, doc_type: &DocumentType) -> &ParsingConfig {
        self.configs.get(doc_type).unwrap_or(&self.default_config)
    }

    pub fn load_config_from_file(&mut self, path: &str) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let config: ParsingConfig = serde_yaml::from_str(&content)?;
        self.configs.insert(config.document_type.clone(), config);
        Ok(())
    }

    fn load_builtin_configs(&mut self) -> Result<()> {
        // Generic document config (for our sample PDFs)
        let generic_config = Self::create_default_generic_config();
        self.configs.insert(DocumentType::Generic, generic_config);

        // Academic paper config (more conservative thresholds)
        let academic_config = ParsingConfig {
            document_type: DocumentType::AcademicPaper,
            section_and_hierarchy: SectionAndHierarchyConfig {
                large_header_threshold: 0.8, // Higher threshold for academic papers
                medium_header_threshold: 0.4,
                small_header_threshold: 0.15,
                min_header_size: 10.0,
                use_bold_indicator: true,
                bold_size_strict: true,
                max_depth: 4,
                font_size_tolerance: 0.1,
                enforce_max_depth: true,
                starting_section_level: 1,
                pattern_detection: PatternDetectionConfig::default(),
            },
            spatial_clustering: SpatialClusteringConfig {
                enabled: true,
                enable_paragraph_merging: true,
                enable_spatial_adjacency: false,
                min_line_height: 9.0, // Slightly larger for academic papers
                vertical_gap_threshold_multiplier: 1.2, // More conservative - bigger gaps needed
                horizontal_alignment_tolerance: 8.0, // Tighter alignment for academic formatting
                line_grouping_tolerance: 0.25, // Tighter line grouping
                sections: ElementClusteringConfig {
                    min_segment_size: 50,  // Sections can be short titles
                    max_segment_size: 500, // Keep section headers concise
                },
                paragraphs: ElementClusteringConfig {
                    min_segment_size: 200,   // Larger minimum for academic content
                    max_segment_size: 12000, // Allow larger segments for detailed methods/results
                },
            },
            section_patterns: vec![
                "abstract".to_string(),
                "introduction".to_string(),
                "methodology".to_string(),
                "results".to_string(),
                "discussion".to_string(),
                "conclusion".to_string(),
                "references".to_string(),
            ],
            include_raw_tika: false, // Default to false for backward compatibility
            pipeline: PipelineConfig::default(),
            list_detection: ListDetectionConfig::default(),
            size_enforcer: SizeEnforcerConfig::default(), // TODO: OPTIMIZATION_DESIGN phase - document type specific tuning
            minimal_parse: false,
        };
        self.configs
            .insert(DocumentType::AcademicPaper, academic_config);

        // Legal contract config (strict hierarchy)
        let legal_config = ParsingConfig {
            document_type: DocumentType::LegalContract,
            section_and_hierarchy: SectionAndHierarchyConfig {
                large_header_threshold: 0.6,
                medium_header_threshold: 0.3,
                small_header_threshold: 0.1,
                min_header_size: 9.0,
                use_bold_indicator: true,
                bold_size_strict: true,
                max_depth: 5,
                font_size_tolerance: 0.1,
                enforce_max_depth: true,
                starting_section_level: 1,
                pattern_detection: PatternDetectionConfig::default(),
            },
            spatial_clustering: SpatialClusteringConfig {
                enabled: true,
                enable_paragraph_merging: true,
                enable_spatial_adjacency: false,
                min_line_height: 8.5,
                vertical_gap_threshold_multiplier: 0.6, // Sensitive to small gaps in legal docs
                horizontal_alignment_tolerance: 12.0,   // Allow for indented legal clauses
                line_grouping_tolerance: 0.2, // Very tight - legal docs have precise formatting
                sections: ElementClusteringConfig {
                    min_segment_size: 30,  // Very short legal section titles
                    max_segment_size: 200, // Keep section headers concise
                },
                paragraphs: ElementClusteringConfig {
                    min_segment_size: 50,   // Smaller minimum - legal clauses can be short
                    max_segment_size: 5000, // Moderate maximum - keep clauses digestible
                },
            },
            section_patterns: vec![
                "article".to_string(),
                "section".to_string(),
                "clause".to_string(),
                "whereas".to_string(),
                "terms".to_string(),
                "conditions".to_string(),
            ],
            include_raw_tika: false, // Default to false for backward compatibility
            pipeline: PipelineConfig::default(),
            list_detection: ListDetectionConfig::default(),
            size_enforcer: SizeEnforcerConfig::default(), // TODO: OPTIMIZATION_DESIGN phase
            minimal_parse: false,
        };
        self.configs
            .insert(DocumentType::LegalContract, legal_config);

        Ok(())
    }

    fn create_default_generic_config() -> ParsingConfig {
        ParsingConfig {
            document_type: DocumentType::Generic,
            section_and_hierarchy: SectionAndHierarchyConfig::default(),
            spatial_clustering: SpatialClusteringConfig {
                enabled: true,                          // Enable spatial clustering by default
                enable_paragraph_merging: true,         // Enable paragraph merging by default
                enable_spatial_adjacency: false,        // Disable spatial adjacency by default
                min_line_height: 8.0,                   // Minimum line height in points
                vertical_gap_threshold_multiplier: 0.8, // 80% of line height = section break
                horizontal_alignment_tolerance: 10.0,   // 10 points for alignment
                line_grouping_tolerance: 0.3,           // 30% of line height for same line
                sections: ElementClusteringConfig {
                    min_segment_size: 20,  // Short section titles allowed
                    max_segment_size: 300, // Keep section headers concise
                },
                paragraphs: ElementClusteringConfig {
                    min_segment_size: 100,  // Minimum 100 chars per segment
                    max_segment_size: 8000, // Maximum 8000 chars per segment
                },
            },
            section_patterns: vec![
                // Generic patterns that might indicate sections
                "chapter".to_string(),
                "section".to_string(),
                "part".to_string(),
                "overview".to_string(),
                "summary".to_string(),
                "background".to_string(),
                "principles".to_string(),
                "approach".to_string(),
            ],
            include_raw_tika: false, // Default to false for backward compatibility
            pipeline: PipelineConfig::default(),
            list_detection: ListDetectionConfig::default(),
            size_enforcer: SizeEnforcerConfig::default(), // TODO: OPTIMIZATION_DESIGN phase
            minimal_parse: false,
        }
    }
}

impl Default for ConfigManager {
    fn default() -> Self {
        Self::new().expect("Failed to create default ConfigManager")
    }
}

impl ParsingConfig {
    /// Load config from file path (functional approach)
    pub fn load_from_file(path: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: ParsingConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }
    
    /// Load config with fallback to default
    pub fn load_with_fallback(path: Option<&str>) -> Self {
        match path {
            Some(p) => Self::load_from_file(p).unwrap_or_else(|_| {
                eprintln!("⚠️  Failed to load config from {}, using defaults", p);
                Self::default()
            }),
            None => Self::default(),
        }
    }
}

impl Default for ParsingConfig {
    fn default() -> Self {
        // Use the generic config as default
        Self {
            document_type: DocumentType::Generic,
            section_and_hierarchy: SectionAndHierarchyConfig::default(),
            spatial_clustering: SpatialClusteringConfig {
                enabled: true,
                enable_paragraph_merging: true,
                enable_spatial_adjacency: false,
                min_line_height: 8.0,
                vertical_gap_threshold_multiplier: 0.8,
                horizontal_alignment_tolerance: 10.0,
                line_grouping_tolerance: 0.3,
                sections: ElementClusteringConfig {
                    min_segment_size: 20,
                    max_segment_size: 300,
                },
                paragraphs: ElementClusteringConfig {
                    min_segment_size: 100,
                    max_segment_size: 8000,
                },
            },
            section_patterns: vec![],
            include_raw_tika: false,
            pipeline: PipelineConfig::default(),
            list_detection: ListDetectionConfig::default(),
            size_enforcer: SizeEnforcerConfig::default(),
            minimal_parse: false,
        }
    }
}
