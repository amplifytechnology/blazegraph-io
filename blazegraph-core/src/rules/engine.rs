use crate::config::{ConfigManager, ParsingConfig};
use crate::types::*;
use anyhow::Result;
use regex::Regex;

// Import rule types (only active rules)
use super::section_detection::SectionAndHierarchyDetectionRule;
use super::spatial_clustering::SpatialClusteringRule;
use super::validation::ValidationRule;

// Disabled rules (will be rewritten):
// use super::list_detection::ListDetectionRule;
// use super::pattern_detection::PatternBasedSectionDetectionRule;
// use super::size_enforcer::SizeEnforcerRule;

// Debug configuration for pipeline tracing
#[derive(Debug, Clone)]
pub struct DebugConfig {
    pub enabled: bool,
    pub filter_patterns: Vec<String>,
}

impl DebugConfig {
    pub fn new(enabled: bool, filter_patterns: Vec<String>) -> Self {
        Self {
            enabled,
            filter_patterns,
        }
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            filter_patterns: Vec::new(),
        }
    }
}

/// Debug utility function to trace elements through the pipeline
pub fn debug_pipeline_elements(
    rule_name: &str,
    elements: &[ParsedPdfElement],
    debug_config: &DebugConfig,
) {
    if !debug_config.enabled || debug_config.filter_patterns.is_empty() {
        return;
    }

    let matching_elements: Vec<_> = elements
        .iter()
        .enumerate()
        .filter(|(_, element)| {
            debug_config.filter_patterns.iter().any(|pattern| {
                // Try regex first, fall back to simple string contains
                if let Ok(regex) = Regex::new(pattern) {
                    regex.is_match(&element.text)
                } else {
                    element.text.contains(pattern)
                }
            })
        })
        .collect();

    if !matching_elements.is_empty() {
        println!(
            "🔍 [{}] {} matching elements:",
            rule_name,
            matching_elements.len()
        );
        for (index, element) in matching_elements {
            let text_preview = if element.text.len() > 50 {
                format!("{}...", &element.text[..47])
            } else {
                element.text.clone()
            };
            println!(
                "  Element {}: \"{}\" ({:?}, depth: {}, text_order: {})",
                index,
                text_preview,
                element.element_type,
                element.hierarchy_level,
                element.position
            );
        }
        println!();
    }
}

pub struct RuleEngine {
    config_manager: ConfigManager,
    debug_config: DebugConfig,
    minimal_parse_override: Option<bool>,
    pub rule_timings: std::cell::RefCell<Vec<(String, std::time::Duration)>>,
}

impl RuleEngine {
    pub fn new() -> Result<Self> {
        let config_manager = ConfigManager::new()?;

        Ok(Self {
            config_manager,
            debug_config: DebugConfig::disabled(),
            minimal_parse_override: None,
            rule_timings: std::cell::RefCell::new(Vec::new()),
        })
    }

    pub fn set_debug_config(&mut self, debug_config: DebugConfig) {
        self.debug_config = debug_config;
    }

    pub fn set_minimal_parse_override(&mut self, minimal_parse: bool) {
        self.minimal_parse_override = Some(minimal_parse);
    }

    pub fn load_custom_config(&mut self, config_path: &str) -> Result<()> {
        println!("📁 Loading custom config from: {config_path}");
        self.config_manager.load_config_from_file(config_path)?;
        println!("✅ Custom config loaded successfully");
        Ok(())
    }

    /// Get the current configuration for cache key generation (requires document type)
    pub fn get_config_for_cache(
        &self,
        doc_type: &crate::types::DocumentType,
    ) -> &crate::config::ParsingConfig {
        self.config_manager.get_config(doc_type)
    }

    pub fn apply_rules(
        &self,
        text_elements: &[PdfTextElement],
        classification: &ClassificationResult,
        document_analysis: &DocumentAnalysis,
        font_size_analysis: &FontSizeAnalysis,
        style_data: &StyleData,
    ) -> Result<Vec<ParsedPdfElement>> {
        // Create a minimal StyleData from the text elements for backward compatibility
        println!(
            "⚙️  Applying enhanced parsing rules with SEQUENTIAL PIPELINE for: {:?}",
            classification.document_type
        );
        println!("📊 Available text elements: {}", text_elements.len());

        // Get the appropriate config for this document type
        let config = self
            .config_manager
            .get_config(&classification.document_type);
        println!(
            "📝 Using config thresholds: large={:.1}%, medium={:.1}%, small={:.1}%",
            config.section_and_hierarchy.large_header_threshold * 100.0,
            config.section_and_hierarchy.medium_header_threshold * 100.0,
            config.section_and_hierarchy.small_header_threshold * 100.0
        );

        // STEP 1: Always do base conversion first (TextElement → ParsedElement)
        println!("🔧 Applying BaseConversion...");
        // Use enhanced conversion pipeline for rich semantic data
        let mut elements = self.convert_text_elements_to_parsed(text_elements);
        debug_pipeline_elements("BaseConversion", &elements, &self.debug_config);
        println!("   ✅ {} elements after BaseConversion", elements.len());

        // STEP 2: Check for minimal parse bypass (CLI override takes precedence)
        let minimal_parse = self.minimal_parse_override.unwrap_or(config.minimal_parse);
        if minimal_parse {
            println!("⚡ Minimal parse mode enabled - bypassing all rule processing");
            return Ok(elements);
        }

        // STEP 3: Apply rules in sequence based on config
        println!("🔗 Executing config-driven rule pipeline...");

        // Clear previous timings
        self.rule_timings.borrow_mut().clear();

        for rule_config in &config.pipeline.rules {
            if !rule_config.enabled {
                println!("   ⏭️  Skipping disabled rule: {}", rule_config.name);
                continue;
            }

            println!("🔧 Applying rule: {}", rule_config.name);
            elements = self.apply_rule_by_name(
                &rule_config.name,
                elements,
                text_elements,
                config,
                document_analysis,
                font_size_analysis,
                style_data,
            )?;
            println!(
                "   ✅ {} elements after {}",
                elements.len(),
                rule_config.name
            );
        }

        Ok(elements)
    }

    /// Apply rules with explicit config (new config flow pattern)
    pub fn apply_rules_with_config(
        &self,
        text_elements: &[PdfTextElement],
        classification: &ClassificationResult,
        document_analysis: &DocumentAnalysis,
        font_size_analysis: &FontSizeAnalysis,
        style_data: &StyleData,
        config: &ParsingConfig,
    ) -> Result<Vec<ParsedPdfElement>> {
        println!(
            "⚙️  Applying rules with config flow for: {:?}",
            classification.document_type
        );
        println!("📊 Available text elements: {}", text_elements.len());

        // Convert text elements to parsed elements as starting point
        let mut elements = self.convert_text_elements_to_parsed(text_elements);

        // Apply each enabled rule from the config
        for rule_config in &config.pipeline.rules {
            if !rule_config.enabled {
                println!("   ⏭️ Skipping disabled rule: {}", rule_config.name);
                continue;
            }

            println!("   🔄 Applying rule: {}", rule_config.name);
            elements = self.apply_rule_by_name(
                &rule_config.name,
                elements,
                text_elements,
                config,
                document_analysis,
                font_size_analysis,
                style_data,
            )?;
            println!(
                "   ✅ {} elements after {}",
                elements.len(),
                rule_config.name
            );
        }

        Ok(elements)
    }

    fn apply_rule_by_name(
        &self,
        rule_name: &str,
        elements: Vec<ParsedPdfElement>,
        text_elements: &[PdfTextElement],
        config: &ParsingConfig,
        document_analysis: &DocumentAnalysis,
        font_size_analysis: &FontSizeAnalysis,
        style_data: &StyleData,
    ) -> Result<Vec<ParsedPdfElement>> {
        let rule_start = std::time::Instant::now();
        let result = match rule_name {
            "SpatialClustering" => {
                println!("🧩 APPLYING SPATIAL CLUSTERING...");
                let spatial_rule = SpatialClusteringRule::new(config);
                let result = spatial_rule.apply(elements)?;
                debug_pipeline_elements("SpatialClustering", &result, &self.debug_config);
                Ok(result)
            }
            "Validation" => {
                println!("🔍 APPLYING VALIDATION...");
                let validation_rule = ValidationRule::new(config);
                let result = validation_rule.apply(elements)?;
                debug_pipeline_elements("Validation", &result, &self.debug_config);
                Ok(result)
            }
            "SectionDetection" => {
                println!("📝 DETECTING SECTIONS AND ASSIGNING HIERARCHY...");
                let section_rule = SectionAndHierarchyDetectionRule::new(
                    self,
                    text_elements,
                    config,
                    document_analysis,
                    font_size_analysis,
                    style_data,
                );
                let result = section_rule.apply(elements)?;
                debug_pipeline_elements("SectionDetection", &result, &self.debug_config);
                Ok(result)
            }
            "PatternBasedSectionDetection" => {
                println!("🔍 PATTERN-BASED SECTION DETECTION (DISABLED - WILL BE REWRITTEN)");
                println!(
                    "   ⏭️  Passing through {} elements unchanged",
                    elements.len()
                );
                Ok(elements)
            }
            "ListDetection" => {
                println!("📝 LIST DETECTION (DISABLED - WILL BE REWRITTEN)");
                println!(
                    "   ⏭️  Passing through {} elements unchanged",
                    elements.len()
                );
                Ok(elements)
            }
            "SizeEnforcer" => {
                println!("🔪 SIZE ENFORCEMENT (DISABLED - WILL BE REWRITTEN)");
                println!(
                    "   ⏭️  Passing through {} elements unchanged",
                    elements.len()
                );
                Ok(elements)
            }
            _ => {
                println!("⚠️  Unknown rule: {rule_name}. Skipping...");
                Ok(elements)
            }
        };

        let rule_duration = rule_start.elapsed();
        self.rule_timings
            .borrow_mut()
            .push((rule_name.to_string(), rule_duration));
        result
    }

    /// Semantic font size analysis using StyleData for intelligent header detection
    /// This provides rich insights about font usage patterns to make smart decisions
    pub fn analyze_font_sizes(
        &self,
        text_elements: &[PdfTextElement],
        style_data: &StyleData,
    ) -> FontSizeAnalysis {
        // STEP 1: Count frequency of each font class used in text elements (single pass)
        let mut class_usage_counts = std::collections::HashMap::new();
        for element in text_elements {
            *class_usage_counts
                .entry(element.style_info.class_name.clone())
                .or_insert(0) += 1;
        }

        // STEP 2: Build size frequency map from StyleData + usage counts
        let mut size_frequency_map = std::collections::HashMap::new();
        let mut font_sizes = Vec::new();
        let mut size_to_count_vec: Vec<(f32, usize)> = Vec::new(); // (size, count) pairs

        for (class_name, usage_count) in &class_usage_counts {
            if let Some(font_class) = style_data.font_classes.get(class_name) {
                let size_key = format!("{:.1}", font_class.font_size); // Convert to string key
                *size_frequency_map.entry(size_key).or_insert(0) += usage_count;

                // Update size_to_count_vec
                if let Some(existing) = size_to_count_vec
                    .iter_mut()
                    .find(|(size, _)| (size - font_class.font_size).abs() < 0.01)
                {
                    existing.1 += usage_count;
                } else {
                    size_to_count_vec.push((font_class.font_size, *usage_count));
                }

                // Add font size multiple times based on its frequency in the document
                for _ in 0..*usage_count {
                    font_sizes.push(font_class.font_size);
                }
            }
        }

        if font_sizes.is_empty() {
            return FontSizeAnalysis::default();
        }

        // STEP 3: Calculate basic statistics
        font_sizes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let min_size = font_sizes[0];
        let max_size = font_sizes[font_sizes.len() - 1];
        let median_size = font_sizes[font_sizes.len() / 2];
        let total_elements = font_sizes.len();

        // STEP 4: Find most common size and class (likely body text)
        let (most_common_size, max_frequency) = size_to_count_vec
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(size, count)| (*size, *count))
            .unwrap_or((median_size, 1));

        let most_common_class = class_usage_counts
            .iter()
            .max_by_key(|(_, &count)| count)
            .map(|(class, _)| class.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // STEP 5: Identify rare large sizes (potential headers)
        let frequency_threshold = (total_elements as f32 * 0.1).max(1.0) as usize; // Less than 10% usage
        let rare_large_sizes: Vec<f32> = size_to_count_vec
            .iter()
            .filter(|(size, count)| *size > median_size && *count <= frequency_threshold)
            .map(|(size, _)| *size)
            .collect();

        // STEP 6: Determine potential header sizes (semantic analysis)
        let mut potential_header_sizes: Vec<f32> = size_to_count_vec
            .iter()
            .filter(|(size, count)| {
                // Headers are typically: larger than body text AND used less frequently
                *size > most_common_size && *count < max_frequency / 2
            })
            .map(|(size, _)| *size)
            .collect();
        potential_header_sizes.sort_by(|a, b| b.partial_cmp(a).unwrap()); // Largest first

        // STEP 7: Build hierarchy levels (sizes sorted by semantic importance)
        let mut hierarchy_levels: Vec<(f32, usize)> = size_to_count_vec.clone();
        // Sort by size (descending) but weight by frequency - headers are large but rare
        hierarchy_levels.sort_by(|(size_a, count_a), (size_b, count_b)| {
            // Primary: size (larger first)
            // Secondary: rarity for same size (rarer first, indicating more important headers)
            match size_b.partial_cmp(size_a).unwrap() {
                std::cmp::Ordering::Equal => count_a.cmp(count_b), // Rarer first
                other => other,
            }
        });
        let hierarchy_levels: Vec<f32> =
            hierarchy_levels.into_iter().map(|(size, _)| size).collect();

        // STEP 8: Calculate usage ratio (uniformity measure)
        let size_usage_ratio = max_frequency as f32 / total_elements as f32;

        // STEP 9: Determine body text size (most semantic)
        let body_text_size = most_common_size; // The most frequently used size is body text

        println!("🎯 Semantic Font Analysis Results:");
        println!(
            "   📊 {} unique classes, {} total elements",
            class_usage_counts.len(),
            total_elements
        );
        println!(
            "   📏 Size range: {:.1}pt - {:.1}pt (median: {:.1}pt)",
            min_size, max_size, median_size
        );
        println!(
            "   📝 Body text: {:.1}pt ({} elements, {:.1}% usage)",
            body_text_size,
            max_frequency,
            size_usage_ratio * 100.0
        );
        println!("   🎯 Potential headers: {:?}", potential_header_sizes);
        println!("   📚 Hierarchy levels: {:?}", hierarchy_levels);
        if !rare_large_sizes.is_empty() {
            println!("   ⭐ Rare large sizes: {:?}", rare_large_sizes);
        }

        FontSizeAnalysis {
            median_size,
            min_size,
            max_size,
            most_common_size,
            most_common_class,
            rare_large_sizes,
            size_frequency_map,
            class_usage_counts,
            potential_header_sizes,
            body_text_size,
            hierarchy_levels,
            size_usage_ratio,
        }
    }

    /// Base conversion method: Convert TextElements to ParsedElements
    /// Uses rich semantic data from the enhanced TextElement structure
    pub fn convert_text_elements_to_parsed(
        &self,
        text_elements: &[PdfTextElement],
    ) -> Vec<ParsedPdfElement> {
        let mut elements = Vec::new();

        // Convert each enhanced TextElement to ParsedElementNew
        for (position, text_element) in text_elements.iter().enumerate() {
            // Skip empty text elements
            if text_element.text.trim().is_empty() {
                continue;
            }

            let paragraph_element = ParsedPdfElement {
                element_type: ParsedElementType::Paragraph,
                text: text_element.text.trim().to_string(),
                hierarchy_level: 1, // All elements start at level 1 for base conversion
                position,
                style_info: text_element.style_info.clone(), // Rich FontClass data
                bounding_box: text_element.bounding_box.clone(), // Always present
                page_number: text_element.page_number,
                paragraph_number: text_element.paragraph_number, // New semantic data
                reading_order: text_element.reading_order,       // Spatial ordering
                bookmark_match: text_element.bookmark_match.clone(), // Section context
                token_count: text_element.token_count,           // Use pre-calculated token count
            };

            elements.push(paragraph_element);
        }

        elements
    }
}

// Shared types and utilities
// TODO: Move to types?
#[derive(Debug, Clone)]
pub struct FontSizeAnalysis {
    // Basic statistics
    pub median_size: f32,
    pub min_size: f32,
    pub max_size: f32,

    // Usage-based insights
    pub most_common_size: f32,      // Likely body text
    pub most_common_class: String,  // The dominant font class
    pub rare_large_sizes: Vec<f32>, // Sizes used infrequently + larger than median (likely headers)

    // Style distribution
    pub size_frequency_map: std::collections::HashMap<String, usize>, // font_size_string -> count
    pub class_usage_counts: std::collections::HashMap<String, usize>, // class_name -> count

    // Semantic insights
    pub potential_header_sizes: Vec<f32>, // Sizes that are likely headers based on frequency + size
    pub body_text_size: f32,              // Most likely body text size

    // Hierarchy insights
    pub hierarchy_levels: Vec<f32>, // Distinct sizes sorted by frequency and size (largest first)
    pub size_usage_ratio: f32, // Ratio of most common to total elements (higher = more uniform)
}

impl Default for FontSizeAnalysis {
    fn default() -> Self {
        Self {
            median_size: 12.0,
            min_size: 12.0,
            max_size: 12.0,
            most_common_size: 12.0,
            most_common_class: "default".to_string(),
            rare_large_sizes: Vec::new(),
            size_frequency_map: std::collections::HashMap::new(),
            class_usage_counts: std::collections::HashMap::new(),
            potential_header_sizes: Vec::new(),
            body_text_size: 12.0,
            hierarchy_levels: Vec::new(),
            size_usage_ratio: 1.0,
        }
    }
}

// Sequential rule pipeline infrastructure
pub trait ParseRule {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>>;
    fn name(&self) -> &str;
}
