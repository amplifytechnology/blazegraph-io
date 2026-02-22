use super::engine::{FontSizeAnalysis, ParseRule, RuleEngine};
use crate::config::{ParsingConfig, SectionAndHierarchyConfig};
use crate::types::*;
use crate::types::{DocumentAnalysis, PdfTextElement, StyleData};
use anyhow::Result;

// SectionAndHierarchyDetectionRule - detects sections and assigns contextual hierarchy levels to all elements
pub struct SectionAndHierarchyDetectionRule<'a> {
    _engine: &'a RuleEngine,
    text_elements: &'a [PdfTextElement],
    config: &'a ParsingConfig,
    document_analysis: &'a DocumentAnalysis,
    font_size_analysis: &'a FontSizeAnalysis,
    _style_data: &'a StyleData,
}

impl<'a> SectionAndHierarchyDetectionRule<'a> {
    pub fn new(
        engine: &'a RuleEngine,
        text_elements: &'a [PdfTextElement],
        config: &'a ParsingConfig,
        document_analysis: &'a DocumentAnalysis,
        font_size_analysis: &'a FontSizeAnalysis,
        style_data: &'a StyleData,
    ) -> Self {
        Self {
            _engine: engine,
            text_elements,
            config,
            document_analysis,
            font_size_analysis,
            _style_data: style_data,
        }
    }
}

impl<'a> ParseRule for SectionAndHierarchyDetectionRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        println!("📝 Applying section detection and contextual hierarchy assignment to {} existing elements...", elements.len());

        // If no elements provided, create initial elements from text_elements
        let input_elements = if elements.is_empty() {
            println!("   📋 No input elements, creating initial elements from TextElements");
            self.text_elements
                .iter()
                .enumerate()
                .map(|(i, text_element)| {
                    ParsedPdfElement {
                        element_type: ParsedElementType::Paragraph, // Default all to paragraph initially
                        text: text_element.text.clone(),
                        hierarchy_level: 3, // Default hierarchy level (will be updated)
                        position: i,
                        style_info: text_element.style_info.clone(),
                        bounding_box: text_element.bounding_box.clone(),
                        page_number: text_element.page_number,
                        paragraph_number: text_element.paragraph_number,
                        reading_order: text_element.reading_order,
                        bookmark_match: text_element.bookmark_match.clone(),
                        token_count: text_element.token_count, // Use pre-calculated token count
                    }
                })
                .collect()
        } else {
            elements
        };

        // Initialize hierarchy context for contextual level tracking
        let mut hierarchy_context = HierarchyContext::new();
        let mut processed_elements = Vec::new();

        for element in input_elements {
            // Find corresponding TextElement for style analysis
            let text_element = self.text_elements.get(element.position);

            if let Some(text_elem) = text_element {
                let (new_element_type, new_hierarchy_level) = self
                    .classify_individual_element_contextual(
                        text_elem,
                        self.font_size_analysis,
                        &element,
                        &mut hierarchy_context,
                    );

                // Update element with new classification (which may be unchanged if not a section)
                processed_elements.push(ParsedPdfElement {
                    element_type: new_element_type,
                    hierarchy_level: new_hierarchy_level,
                    ..element // Keep all other fields unchanged
                });
            } else {
                // No corresponding TextElement found, can't do font analysis
                // But still assign contextual hierarchy level for content
                let content_level = hierarchy_context.get_content_level();
                processed_elements.push(ParsedPdfElement {
                    hierarchy_level: content_level,
                    ..element // Keep all other fields unchanged
                });
            }
        }

        let sections_detected = processed_elements
            .iter()
            .filter(|e| e.element_type == ParsedElementType::Section)
            .count();
        println!("   ✅ Detected {} sections and assigned contextual hierarchy levels to all {} elements",
                sections_detected, processed_elements.len());
        Ok(processed_elements)
    }

    fn name(&self) -> &str {
        "SectionAndHierarchyDetection"
    }
}

impl<'a> SectionAndHierarchyDetectionRule<'a> {
    /// Classify a single text element and assign contextual hierarchy level based on spatial branching
    fn classify_individual_element_contextual(
        &self,
        element: &PdfTextElement,
        font_size_analysis: &FontSizeAnalysis,
        current_element: &ParsedPdfElement,
        hierarchy_context: &mut HierarchyContext,
    ) -> (ParsedElementType, u32) {
        // Check if this element is a header based on font size and style
        let is_header = {
            let font_size = element.style_info.font_size;
            // CRITICAL: Enforce minimum header size from config
            if font_size < self.config.section_and_hierarchy.min_header_size {
                false // Too small to be a header regardless of other factors
            } else {
                // Check font size thresholds AND minimum size requirement
                let is_bold = element
                    .style_info
                    .font_weight
                    .to_lowercase()
                    .contains("bold");
                let bold_logic = if self.config.section_and_hierarchy.bold_size_strict {
                    // Strict mode: bold AND larger than typical content
                    self.config.section_and_hierarchy.use_bold_indicator
                        && is_bold
                        && font_size > self.document_analysis.most_common_font_size
                } else {
                    // Permissive mode: bold OR larger (original behavior)
                    self.config.section_and_hierarchy.use_bold_indicator && is_bold
                };

                // Use semantic analysis: headers are larger than body text or in potential header sizes
                font_size > font_size_analysis.body_text_size
                    || font_size_analysis
                        .potential_header_sizes
                        .contains(&font_size)
                    || bold_logic
            }
        };

        // Check against section patterns
        let matches_section_pattern = self
            .config
            .section_patterns
            .iter()
            .any(|pattern| element.text.to_lowercase().contains(pattern));

        // Additional validation: prevent very short fragments from being headers
        let text_length = element.text.trim().len();
        let is_meaningful_header = if is_header || matches_section_pattern {
            // Allow meaningful section headers: minimum 3 characters, not just single words like "To", "Our"
            text_length >= 3 &&
            // Additional check: if it's very short, it should be bold or a potential header size
            (text_length >= 8 ||
             element.style_info.font_weight.to_lowercase().contains("bold") ||
             font_size_analysis.potential_header_sizes.contains(&element.style_info.font_size))
        } else {
            false
        };

        // SectionDetectionRule ONLY detects sections - use contextual hierarchy for levels
        if is_meaningful_header {
            // Get font size for contextual hierarchy calculation
            let font_size = element.style_info.font_size;

            let contextual_level =
                hierarchy_context.update_for_section(font_size, &self.config.section_and_hierarchy);
            (ParsedElementType::Section, contextual_level)
        } else {
            // Not a section - content gets current context level + 1
            let content_level = hierarchy_context.get_content_level();
            (current_element.element_type.clone(), content_level)
        }
    }
}

// HierarchyContext for tracking contextual hierarchy levels during section detection
#[derive(Debug, Clone)]
pub struct HierarchyContext {
    /// Current hierarchy level we're at
    current_level: u32,
    /// Font size of the most recent section header
    previous_section_font_size: Option<f32>,
    /// Track font sizes at each level for stepping back up
    level_font_sizes: Vec<f32>,
}

impl Default for HierarchyContext {
    fn default() -> Self {
        Self::new()
    }
}

impl HierarchyContext {
    pub fn new() -> Self {
        Self {
            current_level: 1, // Start at level 1 (document is level 0)
            previous_section_font_size: None,
            level_font_sizes: Vec::new(),
        }
    }

    /// Update context when we encounter a new section
    pub fn update_for_section(
        &mut self,
        font_size: f32,
        config: &SectionAndHierarchyConfig,
    ) -> u32 {
        let new_level = match self.previous_section_font_size {
            None => {
                // First section - use config starting level
                self.current_level = config.starting_section_level;
                self.level_font_sizes = vec![font_size];
                config.starting_section_level
            }
            Some(prev_font) => {
                if font_size < prev_font {
                    // Smaller font = subsection (go deeper)
                    let proposed_level = self.current_level + 1;

                    // Enforce max_depth constraint if enabled
                    if config.enforce_max_depth && proposed_level > config.max_depth {
                        // Don't go deeper than max_depth, stay at current level
                        self.level_font_sizes[self.current_level as usize - 1] = font_size;
                        self.current_level
                    } else {
                        // OK to go deeper
                        self.current_level = proposed_level;

                        // Ensure we have enough space in level_font_sizes
                        while self.level_font_sizes.len() < self.current_level as usize {
                            self.level_font_sizes.push(0.0);
                        }
                        self.level_font_sizes[self.current_level as usize - 1] = font_size;

                        self.current_level
                    }
                } else if (font_size - prev_font).abs() < config.font_size_tolerance {
                    // Same font size (with tolerance) = parallel branch (same level)
                    self.level_font_sizes[self.current_level as usize - 1] = font_size;
                    self.current_level
                } else {
                    // Larger font = step back up to appropriate level
                    self.current_level =
                        self.find_appropriate_level_for_font_size(font_size, config);

                    // Update font size for this level
                    while self.level_font_sizes.len() < self.current_level as usize {
                        self.level_font_sizes.push(0.0);
                    }
                    self.level_font_sizes[self.current_level as usize - 1] = font_size;

                    self.current_level
                }
            }
        };

        self.previous_section_font_size = Some(font_size);
        new_level
    }

    /// Find the appropriate level for a font size when stepping back up
    fn find_appropriate_level_for_font_size(
        &self,
        font_size: f32,
        config: &SectionAndHierarchyConfig,
    ) -> u32 {
        // Look through existing levels to find one with similar font size
        for (level_idx, &level_font_size) in self.level_font_sizes.iter().enumerate() {
            if (font_size - level_font_size).abs() < config.font_size_tolerance {
                return (level_idx + 1) as u32;
            }
        }

        // If no existing level matches, find the appropriate level based on font size comparison
        for (level_idx, &level_font_size) in self.level_font_sizes.iter().enumerate() {
            if font_size > level_font_size {
                return (level_idx + 1) as u32;
            }
        }

        // Default to level 1 if we can't determine
        1
    }

    /// Get level for content (paragraphs, lists) - always one level deeper than current section
    pub fn get_content_level(&self) -> u32 {
        self.current_level + 1
    }
}
