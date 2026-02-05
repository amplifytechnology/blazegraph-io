use super::engine::ParseRule;
use crate::config::{ElementClusteringConfig, ParsingConfig};
use crate::types::BoundingBox;
use crate::types::*;
use anyhow::Result;

pub struct SpatialClusteringRule<'a> {
    config: &'a ParsingConfig,
}

impl<'a> SpatialClusteringRule<'a> {
    pub fn new(config: &'a ParsingConfig) -> Self {
        Self { config }
    }
}

impl<'a> ParseRule for SpatialClusteringRule<'a> {
    fn apply(&self, elements: Vec<ParsedPdfElement>) -> Result<Vec<ParsedPdfElement>> {
        println!(
            "🧩 SpatialClustering rule applied - clustering {} elements by adjacency",
            elements.len()
        );

        if elements.is_empty() {
            return Ok(elements);
        }

        let original_count = elements.len();
        let mut clustered_elements = elements;

        // Step 1: Paragraph merging (if enabled)
        if self.config.spatial_clustering.enable_paragraph_merging {
            println!("   📝 Step 1: Paragraph merging enabled");
            clustered_elements = self.cluster_paragraphs_elements(clustered_elements)?;
        } else {
            println!("   ⏭️  Step 1: Paragraph merging disabled");
        }

        // Step 2: Spatial adjacency clustering (if enabled)
        if self.config.spatial_clustering.enable_spatial_adjacency {
            println!("   🧩 Step 2: Spatial adjacency clustering enabled");
            clustered_elements = self.cluster_adjacent_elements(clustered_elements)?;
        } else {
            println!("   ⏭️  Step 2: Spatial adjacency clustering disabled");
        }

        println!(
            "   ✅ Clustered into {} elements (reduced from {})",
            clustered_elements.len(),
            original_count
        );

        Ok(clustered_elements)
    }

    fn name(&self) -> &str {
        "SpatialClustering"
    }
}

impl<'a> SpatialClusteringRule<'a> {
    fn cluster_paragraphs_elements(
        &self,
        elements: Vec<ParsedPdfElement>,
    ) -> Result<Vec<ParsedPdfElement>> {
        println!("🔗 Clustering paragraph segments by paragraph_number and page...");

        if elements.is_empty() {
            return Ok(elements);
        }

        // Group elements by (page_number, paragraph_number)
        let mut paragraph_groups: std::collections::HashMap<(u32, u32), Vec<ParsedPdfElement>> =
            std::collections::HashMap::new();

        for element in elements {
            let key = (element.page_number, element.paragraph_number);
            paragraph_groups
                .entry(key)
                .or_insert_with(Vec::new)
                .push(element);
        }

        let original_count = paragraph_groups.values().map(|v| v.len()).sum::<usize>();
        let mut clustered_elements = Vec::new();

        // Process each paragraph group
        for ((_page_num, _para_num), mut group) in paragraph_groups {
            if group.len() == 1 {
                // Single element - just add it as-is
                clustered_elements.push(group.into_iter().next().unwrap());
            } else {
                // Multiple segments in this paragraph - merge them
                // Sort by reading_order to maintain proper text flow
                group.sort_by_key(|e| e.reading_order);

                let _group_len = group.len();
                let mut group_iter = group.into_iter();

                // Start with the first element as the base
                let mut merged_element = group_iter.next().unwrap();

                // Merge all subsequent elements into the first one
                for element in group_iter {
                    // Merge text with space separator
                    merged_element.text = format!("{} {}", merged_element.text, element.text);

                    // Expand bounding box to encompass all segments
                    merged_element.bounding_box = self
                        .merge_bounding_boxes(&merged_element.bounding_box, &element.bounding_box);

                    // Sum token counts for efficient aggregation
                    merged_element.token_count += element.token_count;

                    // Keep the earliest reading_order (from the sorted first element)
                    // Other fields like style_info, page_number, paragraph_number stay from first element
                }

                // println!("   📄 Page {}, Paragraph {}: Merged {} segments",
                //     page_num, para_num, group_len);

                clustered_elements.push(merged_element);
            }
        }

        // Sort the final result by page and reading order for consistent output
        clustered_elements.sort_by(|a, b| {
            a.page_number
                .cmp(&b.page_number)
                .then(a.reading_order.cmp(&b.reading_order))
        });

        println!(
            "   ✅ Clustered {} segments into {} paragraphs",
            original_count,
            clustered_elements.len()
        );

        Ok(clustered_elements)
    }
    /// Cluster adjacent elements of the same type and hierarchy level on the same page
    fn cluster_adjacent_elements(
        &self,
        elements: Vec<ParsedPdfElement>,
    ) -> Result<Vec<ParsedPdfElement>> {
        let mut clustered = Vec::new();
        let mut current_cluster: Option<ParsedPdfElement> = None;

        for element in elements {
            match &mut current_cluster {
                None => {
                    // Start first cluster
                    current_cluster = Some(element);
                }
                Some(cluster) => {
                    // Check if this element can be merged with current cluster
                    if self.can_merge_elements(cluster, &element) {
                        // Merge element into current cluster
                        self.merge_elements(cluster, element);
                    } else {
                        // Can't merge - finish current cluster and start new one
                        clustered.push(current_cluster.take().unwrap());
                        current_cluster = Some(element);
                    }
                }
            }
        }

        // Don't forget the last cluster
        if let Some(cluster) = current_cluster {
            clustered.push(cluster);
        }

        Ok(clustered)
    }

    /// Check if two elements can be merged (same type, hierarchy level, page, and spatially adjacent)
    fn can_merge_elements(&self, cluster: &ParsedPdfElement, element: &ParsedPdfElement) -> bool {
        // Must be same type
        if cluster.element_type != element.element_type {
            return false;
        }

        // Must be same hierarchy level
        if cluster.hierarchy_level != element.hierarchy_level {
            return false;
        }

        // Must be on same page
        if cluster.page_number != element.page_number {
            return false;
        }

        // Check size limits based on element type
        let config = self.get_clustering_config_for_type(&cluster.element_type);
        let combined_length = cluster.text.len() + element.text.len() + 1; // +1 for space

        if combined_length > config.max_segment_size {
            return false;
        }

        // CRITICAL FIX: Check spatial proximity - elements must be spatially adjacent to merge
        if !self.are_spatially_adjacent(cluster, element) {
            return false;
        }

        true
    }

    /// Merge element into cluster, updating text and bounding box
    fn merge_elements(&self, cluster: &mut ParsedPdfElement, element: ParsedPdfElement) {
        // Merge text with space separator
        cluster.text = format!("{} {}", cluster.text, element.text);

        // Merge bounding boxes (both elements always have bounding boxes now)
        cluster.bounding_box =
            self.merge_bounding_boxes(&cluster.bounding_box, &element.bounding_box);

        // Sum token counts for efficient aggregation
        cluster.token_count += element.token_count;

        // Keep cluster's style_info (first element's style is representative)
    }

    /// Get appropriate clustering config based on element type
    fn get_clustering_config_for_type(
        &self,
        element_type: &ParsedElementType,
    ) -> &ElementClusteringConfig {
        match element_type {
            ParsedElementType::Section => &self.config.spatial_clustering.sections,
            ParsedElementType::Paragraph
            | ParsedElementType::List
            | ParsedElementType::ListItem => &self.config.spatial_clustering.paragraphs,
        }
    }

    /// Merge two bounding boxes into one that encompasses both
    fn merge_bounding_boxes(&self, bbox1: &BoundingBox, bbox2: &BoundingBox) -> BoundingBox {
        let min_x = bbox1.x.min(bbox2.x); // Leftmost x
        let min_y = bbox1.y.min(bbox2.y); // Topmost y
        let max_x = (bbox1.x + bbox1.width).max(bbox2.x + bbox2.width); // Rightmost x
        let max_y = (bbox1.y + bbox1.height).max(bbox2.y + bbox2.height); // Bottommost y

        BoundingBox {
            x: min_x,
            y: min_y,
            width: max_x - min_x,  // Span full width
            height: max_y - min_y, // Span full height
        }
    }

    /// Check if two elements are spatially adjacent (close enough to merge)
    fn are_spatially_adjacent(
        &self,
        cluster: &ParsedPdfElement,
        element: &ParsedPdfElement,
    ) -> bool {
        // Both elements always have bounding boxes now
        let cluster_bbox = &cluster.bounding_box;
        let element_bbox = &element.bounding_box;

        // Calculate vertical distance between elements
        let cluster_bottom = cluster_bbox.y + cluster_bbox.height;
        let element_top = element_bbox.y;
        let element_bottom = element_bbox.y + element_bbox.height;
        let cluster_top = cluster_bbox.y;

        // Calculate vertical gap (positive if there's space between elements)
        let vertical_gap = if cluster_bottom <= element_top {
            // Cluster is above element
            element_top - cluster_bottom
        } else if element_bottom <= cluster_top {
            // Element is above cluster
            cluster_top - element_bottom
        } else {
            // Elements overlap vertically - they're definitely adjacent
            0.0
        };

        // Calculate maximum allowed vertical gap using config
        let min_line_height = self.config.spatial_clustering.min_line_height;
        let gap_multiplier = self
            .config
            .spatial_clustering
            .vertical_gap_threshold_multiplier;
        let max_vertical_gap = min_line_height * gap_multiplier;

        // Check if vertical gap is within acceptable range
        if vertical_gap > max_vertical_gap {
            return false;
        }

        // Check horizontal alignment - elements should have some horizontal overlap or be close
        let cluster_left = cluster_bbox.x;
        let cluster_right = cluster_bbox.x + cluster_bbox.width;
        let element_left = element_bbox.x;
        let element_right = element_bbox.x + element_bbox.width;

        let horizontal_tolerance = self
            .config
            .spatial_clustering
            .horizontal_alignment_tolerance;

        // Check if elements have horizontal overlap or are within tolerance
        let horizontal_overlap = cluster_right.max(element_right) - cluster_left.min(element_left)
            < (cluster_bbox.width + element_bbox.width + horizontal_tolerance);

        if !horizontal_overlap {
            return false;
        }

        true
    }
}
