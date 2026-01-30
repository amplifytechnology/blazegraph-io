use crate::config::{ParsingConfig, SizeEnforcerConfig};
use crate::rules::engine::{ParseRule, ParsedElement, ParsedElementType};
use crate::types::BoundingBox;
use anyhow::Result;
use regex::Regex;

pub struct SizeEnforcerRule {
    config: SizeEnforcerConfig, // Optimized: stores by value for lifetime simplicity
}

impl SizeEnforcerRule {
    pub fn new(config: &ParsingConfig) -> Self {
        Self {
            config: config.size_enforcer.clone(), // Optimized: one-time clone at construction, avoids lifetime complexity
        }
    }

    fn calculate_size(&self, text: &str) -> usize {
        match self.config.size_unit.as_str() {
            "characters" => text.chars().count(),
            "words" => text.split_whitespace().count(),
            "bytes" => text.len(),
            _ => text.chars().count(), // fallback to characters
        }
    }

    fn min_split_size(&self) -> usize {
        ((self.config.max_size as f32) * self.config.min_split_size_ratio) as usize
    }

    fn needs_splitting(&self, element: &ParsedElement) -> bool {
        self.config.enabled && self.calculate_size(&element.text) > self.config.max_size
    }

    fn calculate_split_bounding_box(
        &self,
        original_bbox: &BoundingBox,
        chunk_start_ratio: f32,
        chunk_end_ratio: f32,
    ) -> BoundingBox {
        match self.config.split_direction.as_str() {
            "horizontal" => {
                // Split horizontally - each chunk gets a left-to-right slice
                // This represents sequential reading flow within a single line
                let chunk_width = original_bbox.width * (chunk_end_ratio - chunk_start_ratio);
                let x_offset = original_bbox.width * chunk_start_ratio;
                BoundingBox {
                    x: original_bbox.x + x_offset,
                    y: original_bbox.y,
                    width: chunk_width,
                    height: original_bbox.height,
                }
            }
            "vertical" | _ => {
                // Split vertically - each chunk gets a top-to-bottom slice
                // This represents separate text blocks stacking like paragraphs
                let chunk_height = original_bbox.height * (chunk_end_ratio - chunk_start_ratio);
                let y_offset = original_bbox.height * chunk_start_ratio;
                BoundingBox {
                    x: original_bbox.x,
                    y: original_bbox.y + y_offset,
                    width: original_bbox.width,
                    height: chunk_height,
                }
            }
        }
    }

    fn split_element(&self, element: ParsedElement) -> Result<Vec<ParsedElement>> {
        if !self.needs_splitting(&element) {
            return Ok(vec![element]);
        }

        // OWNERSHIP_DESIGN phase: Pass element by value to avoid cloning text
        let target_size = self.config.max_size;

        match element.element_type {
            ParsedElementType::List => self.split_list(element, target_size),
            ParsedElementType::Paragraph => self.split_paragraph(element, target_size),
            ParsedElementType::Section => self.split_section(element, target_size),
            ParsedElementType::ListItem => self.split_list_item(element, target_size),
        }
    }

    fn split_list(&self, element: ParsedElement, target_size: usize) -> Result<Vec<ParsedElement>> {
        // For lists, we try to split by lines (list items)
        let lines: Vec<&str> = element.text.lines().collect();
        if lines.len() <= 1 {
            // Single line list - treat as paragraph
            return self.split_paragraph(element, target_size);
        }

        let total_lines = lines.len();
        let mut result = Vec::new();
        let mut current_chunk = Vec::new();
        let mut current_size = 0;
        let mut lines_processed = 0;

        for line in lines {
            let line_size = self.calculate_size(line);

            // If adding this line would exceed target, flush current chunk
            if current_size + line_size > target_size && !current_chunk.is_empty() {
                let chunk_text = current_chunk.join("\n");
                let lines_in_chunk = current_chunk.len();
                let start_ratio = (lines_processed - lines_in_chunk) as f32 / total_lines as f32;
                let end_ratio = lines_processed as f32 / total_lines as f32;

                result.push(ParsedElement {
                    element_type: element.element_type.clone(),
                    text: chunk_text,
                    hierarchy_level: element.hierarchy_level,
                    position: element.position + result.len(),
                    style_info: element.style_info.clone(),
                    bounding_box: element.bounding_box.as_ref().map(|bbox| {
                        self.calculate_split_bounding_box(bbox, start_ratio, end_ratio)
                    }),
                    page_number: element.page_number,
                });
                current_chunk.clear();
                current_size = 0;
            }

            current_chunk.push(line);
            current_size += line_size;
            lines_processed += 1;
        }

        // Add remaining chunk - consume element to avoid partial moves
        if !current_chunk.is_empty() {
            let chunk_text = current_chunk.join("\n");
            let lines_in_chunk = current_chunk.len();
            let start_ratio = (lines_processed - lines_in_chunk) as f32 / total_lines as f32;
            let end_ratio = 1.0; // Final chunk goes to the end

            result.push(ParsedElement {
                element_type: element.element_type,
                text: chunk_text,
                hierarchy_level: element.hierarchy_level,
                position: element.position + result.len(),
                style_info: element.style_info,
                bounding_box: element
                    .bounding_box
                    .map(|bbox| self.calculate_split_bounding_box(&bbox, start_ratio, end_ratio)),
                page_number: element.page_number,
            });
        }

        Ok(result)
    }

    fn split_paragraph(
        &self,
        element: ParsedElement,
        target_size: usize,
    ) -> Result<Vec<ParsedElement>> {
        if self.config.preserve_sentences {
            self.split_by_sentences(element, target_size)
        } else {
            self.split_by_position(element, target_size)
        }
    }

    fn split_section(
        &self,
        element: ParsedElement,
        target_size: usize,
    ) -> Result<Vec<ParsedElement>> {
        // Sections are treated like paragraphs for splitting purposes
        self.split_paragraph(element, target_size)
    }

    fn split_list_item(
        &self,
        element: ParsedElement,
        target_size: usize,
    ) -> Result<Vec<ParsedElement>> {
        // List items are treated like paragraphs for splitting purposes
        self.split_paragraph(element, target_size)
    }

    fn split_by_sentences(
        &self,
        mut element: ParsedElement,
        target_size: usize,
    ) -> Result<Vec<ParsedElement>> {
        // Simple sentence boundary detection - EXPLORE phase: basic implementation
        let sentence_regex = Regex::new(r"[.!?]+\s+").unwrap();
        let mut sentences = Vec::new();
        let mut sentence_positions = Vec::new();
        let mut start = 0;

        for mat in sentence_regex.find_iter(&element.text) {
            let end = mat.end();
            sentences.push(&element.text[start..end]);
            sentence_positions.push((start, end));
            start = end;
        }

        // Add remaining text if any
        if start < element.text.len() {
            sentences.push(&element.text[start..]);
            sentence_positions.push((start, element.text.len()));
        }

        if sentences.is_empty() || sentences.len() == 1 {
            // No sentence boundaries or single sentence - split by position
            return self.split_by_position(element, target_size);
        }

        let total_text_len = element.text.len();
        let mut result = Vec::new();
        let mut current_chunk = Vec::new();
        let mut current_size = 0;
        let mut chunk_start_pos = 0;
        let mut sentence_idx = 0;

        for sentence in sentences {
            let sentence_size = self.calculate_size(sentence);

            // If adding this sentence would exceed target, flush current chunk
            if current_size + sentence_size > target_size && !current_chunk.is_empty() {
                let chunk_text = current_chunk.join("").trim().to_string();
                if self.calculate_size(&chunk_text) >= self.min_split_size() {
                    let chunk_end_pos = sentence_positions[sentence_idx - 1].1;
                    let start_ratio = chunk_start_pos as f32 / total_text_len as f32;
                    let end_ratio = chunk_end_pos as f32 / total_text_len as f32;

                    result.push(ParsedElement {
                        element_type: element.element_type.clone(),
                        text: chunk_text,
                        hierarchy_level: element.hierarchy_level,
                        position: element.position + result.len(),
                        style_info: element.style_info.clone(),
                        bounding_box: element.bounding_box.as_ref().map(|bbox| {
                            self.calculate_split_bounding_box(bbox, start_ratio, end_ratio)
                        }),
                        page_number: element.page_number,
                    });
                }
                current_chunk.clear();
                current_size = 0;
                chunk_start_pos = sentence_positions[sentence_idx].0;
            }

            current_chunk.push(sentence);
            current_size += sentence_size;
            sentence_idx += 1;
        }

        // Add remaining chunk (consume element here to avoid partial move)
        if !current_chunk.is_empty() {
            let chunk_text = current_chunk.join("").trim().to_string();
            if self.calculate_size(&chunk_text) >= self.min_split_size() {
                let start_ratio = chunk_start_pos as f32 / total_text_len as f32;
                let end_ratio = 1.0; // Final chunk goes to the end

                element.text = chunk_text;
                element.position += result.len();
                element.bounding_box = element
                    .bounding_box
                    .map(|bbox| self.calculate_split_bounding_box(&bbox, start_ratio, end_ratio));
                result.push(element);
                return Ok(result);
            }
        }

        // Fallback to position-based splitting if sentence splitting didn't work well
        if result.is_empty() {
            return self.split_by_position(element, target_size);
        }

        Ok(result)
    }

    fn split_by_position(
        &self,
        element: ParsedElement,
        target_size: usize,
    ) -> Result<Vec<ParsedElement>> {
        let mut result = Vec::new();
        let chars: Vec<char> = element.text.chars().collect();
        let mut start = 0;

        while start < chars.len() {
            let mut end = start + target_size;
            if end >= chars.len() {
                end = chars.len();
            } else {
                // Try to find a good break point (space, punctuation)
                for i in (start + (target_size / 2)..end).rev() {
                    if chars[i].is_whitespace() || chars[i].is_ascii_punctuation() {
                        end = i + 1;
                        break;
                    }
                }
            }

            let chunk_text: String = chars[start..end]
                .iter()
                .collect::<String>()
                .trim()
                .to_string();
            if !chunk_text.is_empty() && self.calculate_size(&chunk_text) >= self.min_split_size() {
                let total_chars = chars.len();
                let start_ratio = start as f32 / total_chars as f32;
                let end_ratio = end as f32 / total_chars as f32;

                result.push(ParsedElement {
                    element_type: element.element_type.clone(),
                    text: chunk_text,
                    hierarchy_level: element.hierarchy_level,
                    position: element.position + result.len(),
                    style_info: element.style_info.clone(),
                    bounding_box: element.bounding_box.as_ref().map(|bbox| {
                        self.calculate_split_bounding_box(bbox, start_ratio, end_ratio)
                    }),
                    page_number: element.page_number,
                });
            }

            start = end;
        }

        // Fallback: keep original element even if oversized
        if result.is_empty() {
            result.push(element);
        }

        Ok(result)
    }

    fn apply_recursive_splitting(
        &self,
        elements: Vec<ParsedElement>,
    ) -> Result<Vec<ParsedElement>> {
        let mut result = elements;
        let mut iteration = 0;

        while iteration < self.config.max_iterations {
            let mut has_oversized = false;
            let mut new_result = Vec::new();

            for element in result {
                let split_elements = self.split_element(element)?;

                // Check if any resulting elements are still oversized
                for split_element in &split_elements {
                    if self.needs_splitting(split_element) {
                        has_oversized = true;
                    }
                }

                new_result.extend(split_elements);
            }

            result = new_result;
            iteration += 1;

            if !has_oversized {
                break;
            }
        }

        Ok(result)
    }
}

impl ParseRule for SizeEnforcerRule {
    fn apply(&self, elements: Vec<ParsedElement>) -> Result<Vec<ParsedElement>> {
        if !self.config.enabled {
            return Ok(elements);
        }

        println!("🔪 APPLYING SIZE ENFORCEMENT...");
        println!(
            "   ⚙️ Config: max_size={}, unit={}, preserve_sentences={}, recursive={}",
            self.config.max_size,
            self.config.size_unit,
            self.config.preserve_sentences,
            self.config.recursive
        );

        let input_count = elements.len();
        let oversized_count = elements.iter().filter(|e| self.needs_splitting(e)).count();

        let result = if self.config.recursive {
            self.apply_recursive_splitting(elements)?
        } else {
            let mut result = Vec::new();
            for element in elements {
                result.extend(self.split_element(element)?);
            }
            result
        };

        let output_count = result.len();
        println!("   ✅ Split {oversized_count} oversized elements into {output_count} total elements ({input_count}→{output_count})");

        Ok(result)
    }

    fn name(&self) -> &str {
        "SizeEnforcer"
    }
}
