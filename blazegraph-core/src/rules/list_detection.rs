use crate::config::{ListDetectionConfig, ListValidationConfig, SequentialNumberingConfig, MathematicalContextConfig, HyphenContextConfig};
use crate::types::ListSequence;
use anyhow::Result;
use regex::Regex;

use super::engine::{ParseRule, ParsedElement, ParsedElementType};

// ============================================================================
// LIST VALIDATION FRAMEWORK - False Positive Elimination
// ============================================================================

/// Trait for implementing list validation rules
trait ListValidationRule {
    fn validate(&self, list_items: &[ParsedElement]) -> bool;
    fn name(&self) -> &str;
}

/// Minimum size validation rule - lists must have more than one item
struct MinimumSizeRule;

impl ListValidationRule for MinimumSizeRule {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        list_items.len() > 1
    }

    fn name(&self) -> &str {
        "MinimumSizeRule"
    }
}

/// First item validation rule - numbered lists must start with "1" or equivalent
struct FirstItemRule;

impl ListValidationRule for FirstItemRule {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        if let Some(first_item) = list_items.first() {
            self.starts_with_first_value(&first_item.text)
        } else {
            false
        }
    }

    fn name(&self) -> &str {
        "FirstItemRule"
    }
}

impl FirstItemRule {
    fn starts_with_first_value(&self, text: &str) -> bool {
        let text = text.trim();
        
        // Check for numbered patterns: 1., 1), (1)
        if let Ok(regex) = Regex::new(r"^(\d+)[\.\)]") {
            if let Some(captures) = regex.captures(text) {
                if let Some(number_match) = captures.get(1) {
                    return number_match.as_str() == "1";
                }
            }
        }
        
        // Check for parenthetical: (1)
        if let Ok(regex) = Regex::new(r"^\((\d+)\)") {
            if let Some(captures) = regex.captures(text) {
                if let Some(number_match) = captures.get(1) {
                    return number_match.as_str() == "1";
                }
            }
        }
        
        // Check for alphabetic patterns: a., a), A., A)
        if let Ok(regex) = Regex::new(r"^([a-zA-Z])[\.\)]") {
            if let Some(captures) = regex.captures(text) {
                if let Some(letter_match) = captures.get(1) {
                    let letter = letter_match.as_str();
                    return letter == "a" || letter == "A";
                }
            }
        }
        
        // Check for roman numerals: i., I.
        if let Ok(regex) = Regex::new(r"^([ivxIVX]+)[\.\)]") {
            if let Some(captures) = regex.captures(text) {
                if let Some(roman_match) = captures.get(1) {
                    let roman = roman_match.as_str();
                    return roman == "i" || roman == "I";
                }
            }
        }
        
        // If no numbered pattern found, consider it valid (might be bullet list)
        true
    }
}

/// Parenthetical context validation rule - if using (n) format, must start with (1)
struct ParentheticalContextRule;

impl ListValidationRule for ParentheticalContextRule {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        // Check if any item uses parenthetical numbering format
        let has_parenthetical = list_items.iter()
            .any(|item| self.is_parenthetical_number(&item.text));
            
        if has_parenthetical {
            // If using parenthetical format, first item must be (1)
            self.first_item_is_parenthetical_one(list_items)
        } else {
            // Non-parenthetical lists use other validation rules
            true
        }
    }

    fn name(&self) -> &str {
        "ParentheticalContextRule"
    }
}

impl ParentheticalContextRule {
    fn is_parenthetical_number(&self, text: &str) -> bool {
        let text = text.trim();
        if let Ok(regex) = Regex::new(r"^\(\d+\)") {
            regex.is_match(text)
        } else {
            false
        }
    }

    fn first_item_is_parenthetical_one(&self, list_items: &[ParsedElement]) -> bool {
        if let Some(first_item) = list_items.first() {
            let text = first_item.text.trim();
            if let Ok(regex) = Regex::new(r"^\((\d+)\)") {
                if let Some(captures) = regex.captures(text) {
                    if let Some(number_match) = captures.get(1) {
                        return number_match.as_str() == "1";
                    }
                }
            }
        }
        false
    }
}

/// Sequential numbering validation rule - validate that lists have sequential numbering without gaps
struct SequentialNumberingRule<'a> {
    config: &'a SequentialNumberingConfig,
}

impl<'a> ListValidationRule for SequentialNumberingRule<'a> {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        // Extract numbers from list items
        let numbers = self.extract_numbers(list_items);
        
        if numbers.is_empty() {
            return true; // No numbers found, let other rules validate
        }
        
        // Check if numbers form a valid sequence
        self.is_sequential_sequence(&numbers)
    }
    
    fn name(&self) -> &str {
        "SequentialNumberingRule"
    }
}

impl<'a> SequentialNumberingRule<'a> {
    fn new(config: &'a SequentialNumberingConfig) -> Self {
        Self { config }
    }
    
    fn extract_numbers(&self, list_items: &[ParsedElement]) -> Vec<u32> {
        let mut numbers = Vec::new();
        
        for item in list_items {
            let text = item.text.trim();
            
            // Try to extract number from various formats
            if let Some(number) = self.extract_number_from_text(text) {
                numbers.push(number);
            }
        }
        
        numbers
    }
    
    fn extract_number_from_text(&self, text: &str) -> Option<u32> {
        // Try numbered patterns: 1., 1), (1)
        if let Ok(regex) = Regex::new(r"^(\d+)[\.\)]") {
            if let Some(captures) = regex.captures(text) {
                if let Some(number_match) = captures.get(1) {
                    return number_match.as_str().parse().ok();
                }
            }
        }
        
        // Try parenthetical: (1)
        if let Ok(regex) = Regex::new(r"^\((\d+)\)") {
            if let Some(captures) = regex.captures(text) {
                if let Some(number_match) = captures.get(1) {
                    return number_match.as_str().parse().ok();
                }
            }
        }
        
        // Try alphabetic patterns if enabled: a., a), A., A)
        if self.config.allow_letter_sequences {
            if let Ok(regex) = Regex::new(r"^([a-zA-Z])[\.\)]") {
                if let Some(captures) = regex.captures(text) {
                    if let Some(letter_match) = captures.get(1) {
                        let letter = letter_match.as_str().chars().next()?;
                        // Convert letter to number: a/A=1, b/B=2, etc.
                        let number = match letter {
                            'a'..='z' => (letter as u8 - b'a' + 1) as u32,
                            'A'..='Z' => (letter as u8 - b'A' + 1) as u32,
                            _ => return None,
                        };
                        return Some(number);
                    }
                }
            }
        }
        
        None
    }
    
    fn is_sequential_sequence(&self, numbers: &[u32]) -> bool {
        if numbers.len() <= 1 {
            return true; // Single items or empty lists are handled by other rules
        }
        
        // Must start with 1
        if numbers[0] != 1 {
            return false;
        }
        
        // Check for sequential increment with gap tolerance
        for i in 1..numbers.len() {
            let expected = numbers[i-1] + 1;
            let actual = numbers[i];
            let gap = actual.saturating_sub(expected);
            
            if gap > self.config.max_gap_tolerance {
                return false; // Gap too large
            }
        }
        
        true
    }
}

/// Mathematical context validation rule - reject mathematical symbols in mathematical contexts
struct MathematicalContextRule<'a> {
    config: &'a MathematicalContextConfig,
}

impl<'a> ListValidationRule for MathematicalContextRule<'a> {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        // Check if any list items use mathematical symbols
        let uses_math_symbols = list_items.iter()
            .any(|item| self.contains_mathematical_symbols(&item.text));
            
        if uses_math_symbols {
            // If using math symbols, must NOT be in mathematical context
            !self.is_mathematical_context(list_items)
        } else {
            true // Non-math symbols always valid
        }
    }
    
    fn name(&self) -> &str {
        "MathematicalContextRule"
    }
}

impl<'a> MathematicalContextRule<'a> {
    fn new(config: &'a MathematicalContextConfig) -> Self {
        Self { config }
    }
    
    fn contains_mathematical_symbols(&self, text: &str) -> bool {
        self.config.symbols.iter()
            .any(|symbol| text.contains(symbol))
    }
    
    fn is_mathematical_context(&self, list_items: &[ParsedElement]) -> bool {
        // Look for mathematical context indicators in the text
        list_items.iter().any(|item| {
            let text = item.text.to_lowercase();
            
            // Check for mathematical terms
            self.config.terms.iter().any(|term| text.contains(term)) ||
            // Check for mathematical notation patterns
            self.contains_mathematical_notation(&text)
        })
    }
    
    fn contains_mathematical_notation(&self, text: &str) -> bool {
        // Look for mathematical notation patterns
        // Subscripts and superscripts, Greek letters, etc.
        let patterns = [
            r"\w+\^\w+",     // Superscripts: x^2
            r"\w+_\w+",      // Subscripts: x_1
            r"[α-ω]",        // Greek letters
            r"\b[xy]\s*=",   // Variable assignments
            r"\d+\s*=",      // Equation patterns
        ];
        
        patterns.iter().any(|pattern| {
            if let Ok(regex) = Regex::new(pattern) {
                regex.is_match(text)
            } else {
                false
            }
        })
    }
}

/// Hyphen context validation rule - be strict about when hyphens count as list markers
struct HyphenContextRule<'a> {
    config: &'a HyphenContextConfig,
}

impl<'a> ListValidationRule for HyphenContextRule<'a> {
    fn validate(&self, list_items: &[ParsedElement]) -> bool {
        // Check if any items start with hyphen
        let uses_hyphens = list_items.iter()
            .any(|item| self.starts_with_hyphen(&item.text));
            
        if uses_hyphens {
            // If using hyphens, apply strict validation based on strategy
            self.validate_hyphen_context(list_items)
        } else {
            true // Non-hyphen lists always valid
        }
    }
    
    fn name(&self) -> &str {
        "HyphenContextRule"
    }
}

impl<'a> HyphenContextRule<'a> {
    fn new(config: &'a HyphenContextConfig) -> Self {
        Self { config }
    }
    
    fn starts_with_hyphen(&self, text: &str) -> bool {
        text.trim().starts_with('-')
    }
    
    fn validate_hyphen_context(&self, list_items: &[ParsedElement]) -> bool {
        match self.config.strategy.as_str() {
            "reject" => false, // Never allow hyphen lists
            
            "strict" => {
                // Must start at line beginning with space after hyphen
                list_items.iter().all(|item| {
                    let text = item.text.trim();
                    // Require "- " pattern (hyphen followed by space)
                    let has_valid_hyphen = if self.config.require_space_after {
                        text.starts_with("- ")
                    } else {
                        text.starts_with('-')
                    };
                    
                    has_valid_hyphen &&
                    !self.looks_like_word_continuation(&item.text) &&
                    !self.looks_like_mathematical_minus(&item.text)
                })
            },
            
            "context_aware" => {
                // Advanced context analysis - for future implementation
                !self.is_word_continuation_context(list_items) &&
                !self.is_mathematical_minus_context(list_items)
            },
            
            _ => true, // Unknown strategy, default to permissive
        }
    }
    
    fn looks_like_word_continuation(&self, text: &str) -> bool {
        // Pattern: "word-word" or hyphenated compound words
        // Look for letters before and after hyphen
        let hyphen_pos = text.find('-');
        if let Some(pos) = hyphen_pos {
            let before = &text[..pos];
            let after = &text[pos+1..];
            
            // Check if there are letters before and after the hyphen
            before.chars().any(|c| c.is_alphabetic()) &&
            after.chars().any(|c| c.is_alphabetic())
        } else {
            false
        }
    }
    
    fn looks_like_mathematical_minus(&self, text: &str) -> bool {
        // Look for mathematical minus signs: "- 5", "x - y", etc.
        if let Ok(regex) = Regex::new(r"-\s*\d") {
            regex.is_match(text)
        } else {
            false
        }
    }
    
    fn is_word_continuation_context(&self, list_items: &[ParsedElement]) -> bool {
        // Check if this appears to be word continuation context
        list_items.iter().any(|item| {
            self.looks_like_word_continuation(&item.text)
        })
    }
    
    fn is_mathematical_minus_context(&self, list_items: &[ParsedElement]) -> bool {
        // Check for mathematical context with minus signs
        list_items.iter().any(|item| {
            self.looks_like_mathematical_minus(&item.text)
        })
    }
}

/// List validator that orchestrates multiple validation rules
struct ListValidator<'a> {
    config: &'a ListValidationConfig,
}

impl<'a> ListValidator<'a> {
    fn new(config: &'a ListValidationConfig) -> Self {
        Self { config }
    }

    /// Validate a list using all enabled validation rules
    fn validate_list(&self, list_items: &[ParsedElement]) -> bool {
        if !self.config.enabled {
            return true; // Validation disabled - accept all lists
        }

        // Apply minimum size rule
        if self.config.minimum_size_check {
            let rule = MinimumSizeRule;
            if !rule.validate(list_items) {
                // println!("   ❌ List rejected by {}: {} items", rule.name(), list_items.len());
                return false;
            }
        }

        // Apply first item validation rule
        if self.config.first_item_validation {
            let rule = FirstItemRule;
            if !rule.validate(list_items) {
                if let Some(_first_item) = list_items.first() {
                    // println!("   ❌ List rejected by {}: starts with '{}'", rule.name(), first_item.text.trim());
                }
                return false;
            }
        }

        // Apply parenthetical context rule
        if self.config.parenthetical_context_check {
            let rule = ParentheticalContextRule;
            if !rule.validate(list_items) {
                // println!("   ❌ List rejected by {}: invalid parenthetical context", rule.name());
                return false;
            }
        }

        // Apply sequential numbering rule
        if self.config.sequential_numbering_check {
            let rule = SequentialNumberingRule::new(&self.config.sequential_numbering);
            if !rule.validate(list_items) {
                // println!("   ❌ List rejected by {}: sequence gap detected", rule.name());
                return false;
            }
        }

        // Apply mathematical context rule
        if self.config.mathematical_context_check {
            let rule = MathematicalContextRule::new(&self.config.mathematical_context);
            if !rule.validate(list_items) {
                // println!("   ❌ List rejected by {}: mathematical context detected", rule.name());
                return false;
            }
        }

        // Apply hyphen context rule
        if self.config.hyphen_context_check {
            let rule = HyphenContextRule::new(&self.config.hyphen_context);
            if !rule.validate(list_items) {
                // println!("   ❌ List rejected by {}: invalid hyphen context", rule.name());
                return false;
            }
        }

        // All enabled validation rules passed
        true
    }
}

// Enhanced List Detection Rule - config-driven with improved spatial detection
pub struct ListDetectionRule<'a> {
    config: &'a ListDetectionConfig,
}

impl<'a> ListDetectionRule<'a> {
    pub fn new(config: &'a ListDetectionConfig) -> Self {
        Self { config }
    }

    /// Detect if text starts with a bullet point pattern based on config
    fn is_bullet_item(&self, text: &str) -> bool {
        let text = text.trim();

        for pattern in &self.config.bullet_patterns {
            if text.starts_with(pattern) {
                return true;
            }
        }

        false
    }

    /// Detect if text starts with a numbered list pattern based on config
    fn is_numbered_item(&self, text: &str) -> bool {
        let text = text.trim();

        for pattern_str in &self.config.numbered_patterns {
            if let Ok(regex) = Regex::new(pattern_str) {
                if regex.is_match(text) {
                    return true;
                }
            }
        }

        false
    }

    /// Check if text might be a list item based on config patterns
    fn is_potential_list_item(&self, text: &str) -> bool {
        self.is_bullet_item(text) || self.is_numbered_item(text)
    }


    // NOTE: Old sequential list detection methods removed in favor of two-phase approach
    // The two-phase implementation provides better handling of complex list structures

    /// Create a complete ListItem from marker and content parts
    fn create_list_item(
        &self,
        elements: &[ParsedElement],
        marker_index: usize,
        content_indices: &[usize],
    ) -> ParsedElement {
        let marker_element = &elements[marker_index];

        // Combine marker and content text efficiently
        let marker_text = marker_element.text.trim();
        let estimated_capacity = marker_text.len() + content_indices.len() * 100; // Rough estimate
        let mut combined_text = String::with_capacity(estimated_capacity);

        combined_text.push_str(marker_text);

        for &content_idx in content_indices {
            let content = &elements[content_idx];
            let content_text = content.text.trim();
            if !content_text.is_empty() {
                if !combined_text.is_empty() {
                    combined_text.push(' ');
                }
                combined_text.push_str(content_text);
            }
        }

        // OWNERSHIP: Calculate bounding box for entire list item (marker + content)
        let mut item_elements = vec![marker_element];
        for &idx in content_indices {
            item_elements.push(&elements[idx]);
        }
        let item_bbox = self.calculate_aggregate_bounding_box_from_refs(&item_elements);

        ParsedElement {
            element_type: ParsedElementType::ListItem,
            text: combined_text,
            hierarchy_level: marker_element.hierarchy_level,
            position: marker_element.position,
            style_info: marker_element.style_info.clone(), // Strategic clone - style is small
            bounding_box: item_bbox,
            page_number: marker_element.page_number,
        }
    }

    /// Create a List container from multiple ListItem elements (if configured)
    fn create_list_container(&self, list_items: Vec<ParsedElement>) -> ParsedElement {
        if list_items.is_empty() {
            panic!("Cannot create list container from empty items");
        }

        // Combine all list item texts efficiently
        let combined_text = list_items
            .iter()
            .map(|item| item.text.as_str()) // OWNERSHIP: Borrow strings instead of cloning
            .collect::<Vec<&str>>()
            .join("\n");

        // Use properties from the first item for the container
        let first_item = &list_items[0];

        // OWNERSHIP: Calculate aggregate bounding box for the entire list
        let aggregate_bbox = self.calculate_aggregate_bounding_box(&list_items);

        ParsedElement {
            element_type: ParsedElementType::List,
            text: combined_text,
            hierarchy_level: first_item.hierarchy_level,
            position: first_item.position,
            style_info: first_item.style_info.clone(), // Strategic clone - style is small
            bounding_box: aggregate_bbox,
            page_number: first_item.page_number,
        }
    }

    /// OWNERSHIP: Efficiently calculate aggregate bounding box from list items
    fn calculate_aggregate_bounding_box(
        &self,
        list_items: &[ParsedElement],
    ) -> Option<crate::types::BoundingBox> {
        // Data flow: [ParsedElement] → filter(has_bbox) → aggregate(min/max coords) → BoundingBox
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut found_any = false;

        // Single-pass aggregation - efficient for large lists
        for item in list_items {
            if let Some(ref bbox) = item.bounding_box {
                found_any = true;
                min_x = min_x.min(bbox.x);
                min_y = min_y.min(bbox.y);
                max_x = max_x.max(bbox.x + bbox.width);
                max_y = max_y.max(bbox.y + bbox.height);
            }
        }

        if found_any {
            Some(crate::types::BoundingBox {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            })
        } else {
            None
        }
    }

    /// OWNERSHIP: Helper for calculating bounding box from element references
    fn calculate_aggregate_bounding_box_from_refs(
        &self,
        elements: &[&ParsedElement],
    ) -> Option<crate::types::BoundingBox> {
        let mut min_x = f32::INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        let mut found_any = false;

        // Single-pass aggregation - efficient for mixed element types
        for element in elements {
            if let Some(ref bbox) = element.bounding_box {
                found_any = true;
                min_x = min_x.min(bbox.x);
                min_y = min_y.min(bbox.y);
                max_x = max_x.max(bbox.x + bbox.width);
                max_y = max_y.max(bbox.y + bbox.height);
            }
        }

        if found_any {
            Some(crate::types::BoundingBox {
                x: min_x,
                y: min_y,
                width: max_x - min_x,
                height: max_y - min_y,
            })
        } else {
            None
        }
    }

    /// SANITY CHECK: Detect if a list item contains only a marker (bullet, number, etc.)
    fn is_marker_only_list_item(&self, list_item: &ParsedElement) -> bool {
        let text = list_item.text.trim();
        
        // Check if it's just a bullet marker
        for pattern in &self.config.bullet_patterns {
            if text == pattern {
                return true;
            }
        }
        
        // Check if it's just a numbered marker (e.g., "1.", "a)", etc.)
        for pattern_str in &self.config.numbered_patterns {
            if let Ok(regex) = regex::Regex::new(pattern_str) {
                if regex.is_match(text) && text.len() <= 4 { // Short markers only
                    return true;
                }
            }
        }
        
        false
    }
    
    /// SANITY CHECK: Try to merge marker-only list item with adjacent content
    fn try_merge_with_adjacent_content(
        &self,
        elements: &[ParsedElement],
        marker_index: usize,
        consumed_indices: &mut std::collections::HashSet<usize>,
        marker_list_item: &ParsedElement,
    ) -> Option<ParsedElement> {
        // Look for the next non-consumed element that could be content
        for next_idx in (marker_index + 1)..elements.len() {
            if consumed_indices.contains(&next_idx) {
                continue;
            }
            
            let next_element = &elements[next_idx];
            
            // Skip other list markers
            if self.is_potential_list_item(&next_element.text) {
                break;
            }
            
            // Check if it's on roughly the same horizontal line
            if self.are_on_same_horizontal_line(&marker_list_item, next_element) {
                // Merge the content
                let combined_text = format!("{} {}", 
                    marker_list_item.text.trim(), 
                    next_element.text.trim()
                );
                
                // Mark the next element as consumed
                consumed_indices.insert(next_idx);
                
                // Create enhanced list item with combined content
                return Some(ParsedElement {
                    element_type: ParsedElementType::ListItem,
                    text: combined_text,
                    hierarchy_level: marker_list_item.hierarchy_level,
                    position: marker_list_item.position,
                    style_info: marker_list_item.style_info.clone(),
                    bounding_box: self.merge_bounding_boxes(&marker_list_item.bounding_box, &next_element.bounding_box),
                    page_number: marker_list_item.page_number,
                });
            }
            
            // If we found an element but it's not on the same line, stop looking
            break;
        }
        
        None
    }
    
    /// Helper: Check if two elements are on roughly the same horizontal line
    fn are_on_same_horizontal_line(&self, elem1: &ParsedElement, elem2: &ParsedElement) -> bool {
        if let (Some(bbox1), Some(bbox2)) = (&elem1.bounding_box, &elem2.bounding_box) {
            let y_diff = (bbox1.y - bbox2.y).abs();
            y_diff <= self.config.y_tolerance
        } else {
            // Fallback: assume consecutive elements might be on same line
            elem1.page_number == elem2.page_number
        }
    }
    
    /// Helper: Merge two bounding boxes
    fn merge_bounding_boxes(
        &self,
        bbox1: &Option<crate::types::BoundingBox>,
        bbox2: &Option<crate::types::BoundingBox>,
    ) -> Option<crate::types::BoundingBox> {
        match (bbox1, bbox2) {
            (Some(b1), Some(b2)) => {
                let min_x = b1.x.min(b2.x);
                let min_y = b1.y.min(b2.y);
                let max_x = (b1.x + b1.width).max(b2.x + b2.width);
                let max_y = (b1.y + b1.height).max(b2.y + b2.height);
                
                Some(crate::types::BoundingBox {
                    x: min_x,
                    y: min_y,
                    width: max_x - min_x,
                    height: max_y - min_y,
                })
            }
            (Some(b), None) | (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        }
    }

    /// PHASE 1: Find possible list sequences using regex-based detection
    /// This identifies regions that likely contain lists without expensive spatial calculations
    fn find_possible_list_sequences(&self, elements: &[ParsedElement]) -> Vec<ListSequence> {
        let mut sequences = Vec::new();
        let mut current_sequence: Option<ListSequence> = None;

        for (i, element) in elements.iter().enumerate() {
            if self.is_potential_list_item(&element.text) {
                match &mut current_sequence {
                    Some(sequence) => {
                        // Check if this marker is within the lookahead distance of the last marker
                        if let Some(&last_marker_index) = sequence.marker_indices.last() {
                            let gap = i - last_marker_index;
                            if gap <= self.config.sequence_lookahead_elements {
                                // Still within the same sequence
                                sequence.marker_indices.push(i);
                                continue;
                            }
                        }
                        
                        // Too far from last marker - finalize current sequence and start new one
                        sequence.end_index = sequence.marker_indices.last().cloned()
                            .map(|idx| (idx + self.config.sequence_boundary_extension).min(elements.len() - 1))
                            .unwrap_or(sequence.start_index);
                        sequences.push(current_sequence.take().unwrap());
                    }
                    None => {
                        // No current sequence - this is a potential start
                    }
                }
                
                // Start new sequence
                current_sequence = Some(ListSequence {
                    start_index: i,
                    end_index: i, // Will be updated when sequence ends
                    marker_indices: vec![i],
                });
            }
        }

        // Finalize any remaining sequence
        if let Some(mut sequence) = current_sequence {
            sequence.end_index = sequence.marker_indices.last().cloned()
                .map(|idx| (idx + self.config.sequence_boundary_extension).min(elements.len() - 1))
                .unwrap_or(sequence.start_index);
            sequences.push(sequence);
        }

        sequences
    }

    /// PHASE 2: Process content within identified list sequences using spatial validation
    /// This focuses expensive spatial calculations only on regions likely to contain lists
    fn process_list_sequence(&self, elements: &[ParsedElement], sequence: &ListSequence) -> Vec<ParsedElement> {
        let mut result = Vec::new();
        let mut consumed_indices = std::collections::HashSet::new();

        // Process each marker in the sequence
        for (marker_idx, &global_marker_index) in sequence.marker_indices.iter().enumerate() {
            if consumed_indices.contains(&global_marker_index) {
                continue;
            }

            // Determine content end point for this marker
            let content_end_index = if marker_idx + 1 < sequence.marker_indices.len() {
                // Not the last marker - content goes until next marker
                sequence.marker_indices[marker_idx + 1]
            } else {
                // Last marker - use enhanced boundary detection with y_gap analysis
                self.find_last_item_boundary(elements, global_marker_index, sequence.end_index)
            };

            // Collect content for this list item
            let content_indices = self.collect_content_between_indices(
                elements, 
                global_marker_index, 
                content_end_index, 
                &consumed_indices
            );

            // Mark indices as consumed
            for &idx in &content_indices {
                consumed_indices.insert(idx);
            }
            consumed_indices.insert(global_marker_index);

            // Create complete list item
            let mut list_item = self.create_list_item(elements, global_marker_index, &content_indices);
            
            // SANITY CHECK: If list item contains only marker, try to merge with next paragraph
            // OWNERSHIP: Strategic cloning for error recovery - acceptable performance trade-off
            if self.is_marker_only_list_item(&list_item) {
                if let Some(enhanced_item) = self.try_merge_with_adjacent_content(
                    elements, 
                    global_marker_index, 
                    &mut consumed_indices, 
                    &list_item
                ) {
                    list_item = enhanced_item;
                }
            }
            
            result.push(list_item);
        }

        result
    }

    /// Helper: Find boundary for last list item using y_gap analysis
    fn find_last_item_boundary(&self, elements: &[ParsedElement], marker_index: usize, sequence_end: usize) -> usize {
        let marker_element = &elements[marker_index];
        let mut last_valid_index = marker_index;

        for i in (marker_index + 1)..=sequence_end {
            if i >= elements.len() {
                break;
            }
            
            let candidate = &elements[i];
            
            // Check y-gap for boundary detection
            if let (Some(marker_bbox), Some(candidate_bbox)) = (&marker_element.bounding_box, &candidate.bounding_box) {
                let y_gap = (candidate_bbox.y - (marker_bbox.y + marker_bbox.height)).abs();
                if y_gap > self.config.last_item_boundary_gap {
                    break; // Found boundary
                }
            }
            
            // Check if on same horizontal line (no horizontal tolerance needed)
            if self.are_on_same_horizontal_line(marker_element, candidate) {
                last_valid_index = i;
            } else {
                break;
            }
        }

        last_valid_index
    }

    /// Helper: Collect content indices between start and end, respecting consumed indices
    fn collect_content_between_indices(
        &self,
        elements: &[ParsedElement],
        start_index: usize,
        end_index: usize,
        consumed_indices: &std::collections::HashSet<usize>
    ) -> Vec<usize> {
        let mut content_indices = Vec::new();
        
        for i in (start_index + 1)..=end_index {
            if i >= elements.len() || consumed_indices.contains(&i) {
                continue;
            }
            
            // Skip other list markers
            if self.is_potential_list_item(&elements[i].text) {
                continue;
            }
            
            content_indices.push(i);
        }
        
        content_indices
    }

    /// Enhanced list detection with three-phase processing for proper element order preservation
    /// OWNERSHIP phase: Clear ownership patterns with strategic cloning only where needed
    fn detect_and_group_lists(&self, elements: Vec<ParsedElement>) -> Vec<ParsedElement> {
        // PHASE 1: Find possible list sequences using regex-based detection
        let sequences = self.find_possible_list_sequences(&elements);
        
        if sequences.is_empty() {
            // OWNERSHIP: No sequences found - return original elements (moved, no clone)
            return elements;
        }
        
        // PHASE 2: Process sequences to create new list elements
        let mut processed_results = Vec::new();
        let mut consumed_ranges = Vec::new();
        
        for sequence in sequences {
            // Process list sequence using spatial validation
            let list_items = self.process_list_sequence(&elements, &sequence);
            
            // PHASE 2.5: List Validation - eliminate false positives
            let validator = ListValidator::new(&self.config.validation);
            let is_valid_list = validator.validate_list(&list_items);
            
            // Only proceed if list passes validation
            if !list_items.is_empty() && is_valid_list {
                let mut list_group = list_items;
                let mut sequence_result = Vec::new();
                self.finalize_list_group(&mut sequence_result, &mut list_group);
                
                // Track the range consumed by this sequence
                consumed_ranges.push((sequence.start_index, sequence.end_index.min(elements.len() - 1)));
                
                // Add the processed results (could be one List container or multiple ListItems)
                processed_results.extend(sequence_result);
            }
        }
        
        // PHASE 3: Reconstruct element stream in proper document order
        self.preserve_element_order(&elements, processed_results, &consumed_ranges)
    }

    /// Helper function to finalize a group of list items without cloning
    fn finalize_list_group(
        &self,
        result: &mut Vec<ParsedElement>,
        current_list_items: &mut Vec<ParsedElement>,
    ) {
        if self.config.create_list_containers {
            // Take ownership to avoid cloning, then create container
            let items = std::mem::take(current_list_items);
            let list_container = self.create_list_container(items);
            result.push(list_container);

            // If we need to preserve individual items, we would need to clone here
            // But this is a rare configuration, so the optimization is still worthwhile
            if self.config.preserve_list_items {
                // In this case, we do need to clone since we consumed the items above
                // This could be optimized further by restructuring the container creation
                // For now, this is better than the previous version which always cloned
                result.extend(
                    self.create_individual_list_items_from_container(result.last().unwrap()),
                );
            }
        } else {
            // Just move individual list items without cloning
            result.append(current_list_items);
        }
    }

    /// Helper to extract individual items from a container (used only when preserve_list_items = true)
    fn create_individual_list_items_from_container(
        &self,
        container: &ParsedElement,
    ) -> Vec<ParsedElement> {
        // This is a fallback for the rare preserve_list_items case
        // In practice, most configs won't use this
        container
            .text
            .split('\n')
            .filter(|line| !line.trim().is_empty())
            .map(|line| ParsedElement {
                element_type: ParsedElementType::ListItem,
                text: line.trim().to_string(),
                hierarchy_level: container.hierarchy_level,
                position: container.position,
                style_info: container.style_info.clone(), // Still need clone here for rare case
                bounding_box: container.bounding_box.clone(), // Still need clone here for rare case
                page_number: container.page_number,
            })
            .collect()
    }

    /// PHASE 3: Preserve element order by reconstructing the stream in proper document order
    /// Following the established pattern from Element_Ordering_Design_Patterns.md
    fn preserve_element_order(
        &self,
        original_elements: &[ParsedElement],
        processed_results: Vec<ParsedElement>,
        consumed_ranges: &[(usize, usize)]
    ) -> Vec<ParsedElement> {
        let mut result = Vec::new();
        let mut original_idx = 0;
        let mut processed_idx = 0;
        
        for &(range_start, range_end) in consumed_ranges {
            // Add non-consumed elements before this range
            while original_idx < range_start {
                result.push(original_elements[original_idx].clone()); // Strategic clone - needed for order reconstruction
                original_idx += 1;
            }
            
            // Add processed element(s) for this range
            if processed_idx < processed_results.len() {
                result.push(processed_results[processed_idx].clone()); // Strategic clone - could be optimized with Vec ownership redesign
                processed_idx += 1;
            }
            
            // Skip consumed original elements
            original_idx = range_end + 1;
        }
        
        // Add remaining non-consumed elements
        while original_idx < original_elements.len() {
            result.push(original_elements[original_idx].clone()); // Strategic clone - needed for order reconstruction
            original_idx += 1;
        }
        
        result
    }
}

impl<'a> ParseRule for ListDetectionRule<'a> {
    fn apply(&self, elements: Vec<ParsedElement>) -> Result<Vec<ParsedElement>> {
        if !self.config.enabled {
            return Ok(elements);
        }

        println!("🔍 APPLYING ENHANCED LIST DETECTION...");
        println!("   📊 Input: {} elements", elements.len());
        println!(
            "   ⚙️ Config: y_tolerance={}, sequence_lookahead={}, boundary_extension={}",
            self.config.y_tolerance,
            self.config.sequence_lookahead_elements,
            self.config.sequence_boundary_extension
        );

        let processed_elements = self.detect_and_group_lists(elements);

        let list_count = processed_elements
            .iter()
            .filter(|e| e.element_type == ParsedElementType::List)
            .count();

        let list_item_count = processed_elements
            .iter()
            .filter(|e| e.element_type == ParsedElementType::ListItem)
            .count();

        println!(
            "   ✅ Detected {} lists and {} list items from {} elements",
            list_count,
            list_item_count,
            processed_elements.len()
        );

        Ok(processed_elements)
    }

    fn name(&self) -> &str {
        "EnhancedListDetection"
    }
}
