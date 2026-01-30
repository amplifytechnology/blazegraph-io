use crate::config::ParsingConfig;
use anyhow::Result;
use regex::Regex;

use super::engine::{ParseRule, ParsedElement, ParsedElementType};

// PatternBasedSectionDetectionRule - promotes elements to sections based on regex patterns
pub struct PatternBasedSectionDetectionRule<'a> {
    patterns: Vec<regex::Regex>,
    config: &'a ParsingConfig,
}

impl<'a> PatternBasedSectionDetectionRule<'a> {
    pub fn new(config: &'a ParsingConfig) -> Result<Self> {
        // Compile patterns from config
        let mut patterns = Vec::new();
        for pattern_str in &config.section_and_hierarchy.pattern_detection.patterns {
            patterns.push(Regex::new(pattern_str)?);
        }

        Ok(Self { patterns, config })
    }
}

impl<'a> ParseRule for PatternBasedSectionDetectionRule<'a> {
    fn apply(&self, elements: Vec<ParsedElement>) -> Result<Vec<ParsedElement>> {
        if !self.config.section_and_hierarchy.pattern_detection.enabled {
            println!("   ⏭️  Pattern detection disabled, skipping");
            return Ok(elements);
        }

        println!("🔍 APPLYING PATTERN-BASED SECTION DETECTION...");
        println!(
            "   📝 Checking {} patterns against {} elements",
            self.patterns.len(),
            elements.len()
        );

        let mut promoted_count = 0;
        let mut result_elements = Vec::new();

        for mut element in elements {
            if element.element_type == ParsedElementType::Paragraph
                && self.should_be_section(&element)
            {
                println!(
                    "   🔼 Pattern matched: '{}' -> Section",
                    element.text.chars().take(50).collect::<String>()
                );
                element.element_type = ParsedElementType::Section;
                promoted_count += 1;
            }
            result_elements.push(element);
        }

        println!("   ✅ Promoted {promoted_count} elements to sections based on patterns");
        Ok(result_elements)
    }

    fn name(&self) -> &str {
        "PatternBasedSectionDetection"
    }
}

impl<'a> PatternBasedSectionDetectionRule<'a> {
    fn matches_pattern(&self, text: &str) -> bool {
        self.patterns.iter().any(|pattern| pattern.is_match(text))
    }

    fn should_be_section(&self, element: &ParsedElement) -> bool {
        // Only upgrade to section if pattern matches AND font constraints are met
        if !self.matches_pattern(&element.text) {
            return false;
        }

        // If respect_font_constraints is false, pattern match is sufficient
        if !self
            .config
            .section_and_hierarchy
            .pattern_detection
            .respect_font_constraints
        {
            return true;
        }

        // Check font size constraints
        if let Some(style) = &element.style_info {
            if let Some(font_size) = style.font_size {
                // Must meet minimum size OR be bold (if bold indicator enabled)
                font_size >= self.config.section_and_hierarchy.min_header_size
                    || (self.config.section_and_hierarchy.use_bold_indicator && style.is_bold)
            } else {
                // No font size info - only allow if bold and bold indicator enabled
                self.config.section_and_hierarchy.use_bold_indicator && style.is_bold
            }
        } else {
            // No style info - pattern alone isn't enough when respecting constraints
            false
        }
    }
}
