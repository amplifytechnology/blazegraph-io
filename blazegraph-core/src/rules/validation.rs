use super::engine::ParseRule;
use crate::config::ParsingConfig;
use crate::types::*;
use anyhow::Result;

// ValidationRule - structural validation and consistency checks
pub struct ValidationRule<'a> {
    config: &'a ParsingConfig,
}

#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub issues: Vec<ValidationIssue>,
    pub quality_score: f32,
    pub total_elements: usize,
}

#[derive(Debug, Clone)]
pub enum ValidationIssue {
    HierarchyJump {
        from_level: u32,
        to_level: u32,
        from_pos: usize,
        to_pos: usize,
    },
    OrphanedElement {
        level: u32,
        position: usize,
        text_preview: String,
    },
    SuspiciousSection {
        position: usize,
        text: String,
        reason: String,
    },
    ReadingOrderInconsistency {
        position: usize,
        expected_order: u32,
        actual_order: u32,
    },
    PageInconsistency {
        position: usize,
        page: u32,
        issue: String,
    },
    InvalidPosition {
        position: usize,
        coordinates: String,
    },
}

impl<'a> ValidationRule<'a> {
    pub fn new(config: &'a ParsingConfig) -> Self {
        Self { config }
    }
}

impl<'a> ParseRule for ValidationRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        println!("🔍 APPLYING STRUCTURAL VALIDATION...");
        println!(
            "   🔍 Validating {} elements for structural consistency",
            elements.len()
        );

        // Perform validation checks and generate report
        let validation_report = self.validate_structure(&elements);

        // Print validation results
        self.print_validation_report(&validation_report);

        // For now, return elements unchanged (pure validation)
        // In the future, we could optionally fix some issues if needed
        Ok(elements)
    }

    fn name(&self) -> &str {
        "StructuralValidation"
    }
}

impl<'a> ValidationRule<'a> {
    /// Perform comprehensive structural validation
    fn validate_structure(&self, elements: &[ParsedPdfElement]) -> ValidationReport {
        let mut issues = Vec::new();
        let total_elements = elements.len();

        // 1. Validate hierarchy consistency
        self.validate_hierarchy_structure(elements, &mut issues);

        // 2. Validate reading order consistency
        self.validate_reading_order_consistency(elements, &mut issues);

        // 3. Validate position and coordinate consistency
        self.validate_position_consistency(elements, &mut issues);

        // 4. Validate page consistency
        self.validate_page_consistency(elements, &mut issues);

        // 5. Check for suspicious sections
        self.validate_section_quality(elements, &mut issues);

        // Calculate quality score (1.0 = perfect, 0.0 = many issues)
        let quality_score = if total_elements == 0 {
            1.0
        } else {
            (1.0 - (issues.len() as f32 / total_elements as f32)).max(0.0)
        };

        ValidationReport {
            issues,
            quality_score,
            total_elements,
        }
    }

    /// Check for hierarchy jumps and orphaned elements
    fn validate_hierarchy_structure(
        &self,
        elements: &[ParsedPdfElement],
        issues: &mut Vec<ValidationIssue>,
    ) {
        let max_depth = self.config.section_and_hierarchy.max_depth;

        for (i, element) in elements.iter().enumerate() {
            // Check for hierarchy exceeding max depth
            if element.hierarchy_level > max_depth {
                issues.push(ValidationIssue::OrphanedElement {
                    level: element.hierarchy_level,
                    position: i,
                    text_preview: element.text.chars().take(50).collect(),
                });
            }

            // Check for hierarchy jumps (skipping levels)
            if i > 0 {
                let prev_level = elements[i - 1].hierarchy_level;
                let curr_level = element.hierarchy_level;

                // Flag jumps of more than 1 level
                if curr_level > prev_level + 1 {
                    issues.push(ValidationIssue::HierarchyJump {
                        from_level: prev_level,
                        to_level: curr_level,
                        from_pos: i - 1,
                        to_pos: i,
                    });
                }
            }
        }
    }

    /// Validate reading order consistency
    fn validate_reading_order_consistency(
        &self,
        elements: &[ParsedPdfElement],
        issues: &mut Vec<ValidationIssue>,
    ) {
        let mut expected_order = 0u32;

        for (i, element) in elements.iter().enumerate() {
            // Reading order should generally be sequential (with some tolerance)
            if element.reading_order < expected_order.saturating_sub(5)
                || element.reading_order > expected_order + 10
            {
                issues.push(ValidationIssue::ReadingOrderInconsistency {
                    position: i,
                    expected_order,
                    actual_order: element.reading_order,
                });
            }
            expected_order = element.reading_order + 1;
        }
    }

    /// Validate position and coordinate consistency
    fn validate_position_consistency(
        &self,
        elements: &[ParsedPdfElement],
        issues: &mut Vec<ValidationIssue>,
    ) {
        for (i, element) in elements.iter().enumerate() {
            let bbox = &element.bounding_box;

            // Check for impossible coordinates
            if bbox.x < 0.0 || bbox.y < 0.0 || bbox.width <= 0.0 || bbox.height <= 0.0 {
                issues.push(ValidationIssue::InvalidPosition {
                    position: i,
                    coordinates: format!(
                        "x:{:.1}, y:{:.1}, w:{:.1}, h:{:.1}",
                        bbox.x, bbox.y, bbox.width, bbox.height
                    ),
                });
            }
        }
    }

    /// Validate page consistency
    fn validate_page_consistency(
        &self,
        elements: &[ParsedPdfElement],
        issues: &mut Vec<ValidationIssue>,
    ) {
        for (i, element) in elements.iter().enumerate() {
            // Check for reasonable page numbers
            if element.page_number == 0 {
                issues.push(ValidationIssue::PageInconsistency {
                    position: i,
                    page: element.page_number,
                    issue: "Page number is 0 (should start from 1)".to_string(),
                });
            }

            // Check for huge page number jumps (might indicate parsing issues)
            if i > 0 {
                let prev_page = elements[i - 1].page_number;
                let curr_page = element.page_number;
                if curr_page > prev_page + 5 {
                    // Allow some tolerance
                    issues.push(ValidationIssue::PageInconsistency {
                        position: i,
                        page: curr_page,
                        issue: format!("Large page jump from {} to {}", prev_page, curr_page),
                    });
                }
            }
        }
    }

    /// Check for suspicious sections
    fn validate_section_quality(
        &self,
        elements: &[ParsedPdfElement],
        issues: &mut Vec<ValidationIssue>,
    ) {
        for (i, element) in elements.iter().enumerate() {
            if element.element_type == ParsedElementType::Section {
                let text = element.text.trim();

                // Flag very short sections
                if text.len() < 3 {
                    issues.push(ValidationIssue::SuspiciousSection {
                        position: i,
                        text: text.to_string(),
                        reason: "Section text too short (< 3 characters)".to_string(),
                    });
                }

                // Flag sections that are too long (might be misclassified paragraphs)
                if text.len() > 200 {
                    issues.push(ValidationIssue::SuspiciousSection {
                        position: i,
                        text: text.chars().take(50).collect::<String>() + "...",
                        reason: "Section text unusually long (> 200 characters)".to_string(),
                    });
                }
            }
        }
    }

    /// Print validation report to console
    fn print_validation_report(&self, report: &ValidationReport) {
        println!("   📊 Validation Report:");
        println!("      📈 Quality Score: {:.2}/1.00", report.quality_score);
        println!("      🔍 Issues Found: {}", report.issues.len());

        if report.issues.is_empty() {
            println!("      ✅ No structural issues detected!");
        } else {
            println!("      ⚠️  Issues detected:");
            for issue in &report.issues {
                match issue {
                    ValidationIssue::HierarchyJump {
                        from_level,
                        to_level,
                        from_pos,
                        to_pos,
                    } => {
                        println!(
                            "         📊 Hierarchy jump: Level {} → {} (positions {}-{})",
                            from_level, to_level, from_pos, to_pos
                        );
                    }
                    ValidationIssue::OrphanedElement {
                        level,
                        position,
                        text_preview,
                    } => {
                        println!(
                            "         🏝️  Orphaned element: Level {} at position {} (\"{}\")",
                            level, position, text_preview
                        );
                    }
                    ValidationIssue::SuspiciousSection {
                        position,
                        text,
                        reason,
                    } => {
                        println!(
                            "         🤔 Suspicious section at {}: \"{}\" ({})",
                            position, text, reason
                        );
                    }
                    ValidationIssue::ReadingOrderInconsistency {
                        position,
                        expected_order,
                        actual_order,
                    } => {
                        println!(
                            "         📖 Reading order issue at {}: expected ~{}, got {}",
                            position, expected_order, actual_order
                        );
                    }
                    ValidationIssue::PageInconsistency {
                        position,
                        page,
                        issue,
                    } => {
                        println!(
                            "         📄 Page issue at {} (page {}): {}",
                            position, page, issue
                        );
                    }
                    ValidationIssue::InvalidPosition {
                        position,
                        coordinates,
                    } => {
                        println!(
                            "         📍 Invalid coordinates at {}: {}",
                            position, coordinates
                        );
                    }
                }
            }
        }
    }
}
