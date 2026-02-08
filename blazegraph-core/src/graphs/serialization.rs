use crate::types::*;
use anyhow::Result;

impl DocumentGraph {
    pub fn to_sequential_format(&self) -> SequentialDocument {
        // Collect all nodes and sort by text_order, with root node first
        let mut nodes: Vec<&DocumentNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| {
            // Document root (with text_order = None) should come first
            match (a.text_order, b.text_order) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a_order), Some(b_order)) => a_order.cmp(&b_order),
            }
        });

        let segments: Vec<SequentialSegment> = nodes
            .into_iter()
            .enumerate()
            .map(|(index, node)| SequentialSegment {
                id: index,
                node_type: node.node_type.clone(),
                text: node.content.text.clone(),
                location: node.location.clone(),
                style: node.style_info.clone(),
                tokens: node.token_count,
            })
            .collect();

        SequentialDocument {
            format: "sequential".to_string(),
            segments,
            structural_profile: self.structural_profile.clone(),
        }
    }

    pub fn to_flat_format(&self) -> FlatDocument {
        // Collect all nodes and sort by text_order, with root node first
        let mut nodes: Vec<&DocumentNode> = self.nodes.values().collect();
        nodes.sort_by(|a, b| {
            // Document root (with text_order = None) should come first
            match (a.text_order, b.text_order) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a_order), Some(b_order)) => a_order.cmp(&b_order),
            }
        });

        let chunks: Vec<String> = nodes
            .into_iter()
            .map(|node| node.content.text.clone())
            .collect();

        FlatDocument {
            format: "flat".to_string(),
            chunks,
        }
    }

    pub fn save_with_format(&self, path: &str, format: &str) -> Result<()> {
        match format {
            "sequential" => {
                let sequential = self.to_sequential_format();
                let json = serde_json::to_string_pretty(&sequential)?;
                std::fs::write(path, json)?;
            }
            "flat" => {
                let flat = self.to_flat_format();
                let json = serde_json::to_string_pretty(&flat)?;
                std::fs::write(path, json)?;
            }
            "graph" | _ => {
                self.save_to_json(path)?;
            }
        }
        Ok(())
    }
}