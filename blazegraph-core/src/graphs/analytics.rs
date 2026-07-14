use crate::types::*;
use std::collections::HashMap;

impl DocumentGraph {
    /// Compute the structural-profile aggregate for the entire graph.
    ///
    /// Block A / Amendment M (schema 0.8.0): `DocumentGraph` no longer
    /// *carries* a `structural_profile` — the profile is a pure function
    /// of the node set, json-only, recomputed at serialization time
    /// (`to_sorted_graph` / `to_sequential_format`) and never hashed.
    /// `document_type` is stamped `Generic`, mirroring the builder's
    /// historical placeholder (classification is not implemented).
    pub fn compute_structural_profile(&self) -> StructuralProfile {
        let all_nodes: Vec<&DocumentNode> = self.nodes.values().collect();
        let analytics = GraphAnalytics::compute_analytics(&all_nodes);

        // Extract total_tokens before moving analytics fields
        let total_tokens = analytics.token_distribution.overall.total_tokens;

        StructuralProfile {
            document_type: DocumentType::Generic,
            total_nodes: self.nodes.len(),
            total_tokens,
            token_distribution: analytics.token_distribution,
            node_type_distribution: analytics.node_type_distribution,
            depth_distribution: analytics.depth_distribution,
        }
    }
}

/// Analytics computer that can analyze any subset of nodes in the graph
pub struct GraphAnalytics;

impl GraphAnalytics {
    /// Compute analytics for any collection of nodes (enables subtree analysis)
    pub fn compute_analytics(nodes: &[&DocumentNode]) -> GraphAnalyticsResult {
        GraphAnalyticsResult {
            token_distribution: Self::compute_token_distribution(nodes),
            node_type_distribution: Self::compute_node_type_distribution(nodes),
            depth_distribution: Self::compute_depth_distribution(nodes),
        }
    }

    /// Compute histogram-based token distribution with adaptive binning
    fn compute_token_distribution(nodes: &[&DocumentNode]) -> TokenDistribution {
        let mut overall_tokens = Vec::new();
        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();

        // Collect token counts by type
        for node in nodes {
            overall_tokens.push(node.token_count);
            by_type
                .entry(node.node_type.clone())
                .or_default()
                .push(node.token_count);
        }

        let overall_histogram = Self::create_histogram(&overall_tokens);
        let mut type_histograms = HashMap::new();

        for (node_type, tokens) in by_type {
            type_histograms.insert(node_type, Self::create_histogram(&tokens));
        }

        TokenDistribution {
            overall: overall_histogram,
            by_node_type: type_histograms,
        }
    }

    /// Create histogram with adaptive binning based on data distribution
    fn create_histogram(token_counts: &[usize]) -> TokenHistogram {
        if token_counts.is_empty() {
            return TokenHistogram::default();
        }

        let mut sorted_tokens = token_counts.to_vec();
        sorted_tokens.sort_unstable();

        let min_tokens = sorted_tokens[0] as u32;
        let max_tokens = sorted_tokens[sorted_tokens.len() - 1] as u32;
        let total_tokens: usize = sorted_tokens.iter().sum();
        let total_count = sorted_tokens.len();

        // Generate adaptive bins (use equal-width for simplicity, can be enhanced)
        let bin_ranges = Self::generate_adaptive_bins(min_tokens, max_tokens, 10);
        let mut bins = Vec::new();

        for (range_start, range_end) in bin_ranges {
            let count = sorted_tokens
                .iter()
                .filter(|&&token| (token as u32) >= range_start && (token as u32) < range_end)
                .count();
            let token_sum: usize = sorted_tokens
                .iter()
                .filter(|&&token| (token as u32) >= range_start && (token as u32) < range_end)
                .sum();

            bins.push(HistogramBin {
                range_start,
                range_end,
                count,
                token_sum,
            });
        }

        // Calculate statistics
        let mean = if total_count > 0 {
            total_tokens as f32 / total_count as f32
        } else {
            0.0
        };
        let median = if sorted_tokens.is_empty() {
            0.0
        } else if sorted_tokens.len().is_multiple_of(2) {
            let mid = sorted_tokens.len() / 2;
            (sorted_tokens[mid - 1] + sorted_tokens[mid]) as f32 / 2.0
        } else {
            sorted_tokens[sorted_tokens.len() / 2] as f32
        };

        let mode = bins
            .iter()
            .max_by_key(|bin| bin.count)
            .map(|bin| bin.range_start);

        let variance = if total_count > 1 {
            let mean_val = mean;
            sorted_tokens
                .iter()
                .map(|&token| (token as f32 - mean_val).powi(2))
                .sum::<f32>()
                / (total_count - 1) as f32
        } else {
            0.0
        };

        TokenHistogram {
            bins,
            total_count,
            total_tokens,
            mean,
            median,
            mode,
            variance,
        }
    }

    /// Generate adaptive bin boundaries from data range
    fn generate_adaptive_bins(min_val: u32, max_val: u32, target_bins: usize) -> Vec<(u32, u32)> {
        if min_val >= max_val {
            return vec![(min_val, min_val + 1)];
        }

        let range = max_val - min_val;
        let bin_width = ((range as f32 / target_bins as f32).ceil() as u32).max(1);

        let mut bins = Vec::new();
        let mut current = min_val;

        while current < max_val {
            let end = (current + bin_width).min(max_val + 1);
            bins.push((current, end));
            current = end;
        }

        bins
    }

    /// Compute node type distribution with counts and percentages
    fn compute_node_type_distribution(nodes: &[&DocumentNode]) -> NodeTypeDistribution {
        let mut counts = HashMap::new();
        let total_nodes = nodes.len();

        for node in nodes {
            *counts.entry(node.node_type.clone()).or_insert(0) += 1;
        }

        let mut percentages = HashMap::new();
        for (node_type, count) in &counts {
            let percentage = if total_nodes > 0 {
                (*count as f32 / total_nodes as f32) * 100.0
            } else {
                0.0
            };
            percentages.insert(node_type.clone(), percentage);
        }

        NodeTypeDistribution {
            counts,
            percentages,
        }
    }

    /// Compute depth distribution and statistics
    fn compute_depth_distribution(nodes: &[&DocumentNode]) -> DepthDistribution {
        let mut depth_counts = HashMap::new();
        let mut total_depth = 0u32;
        let mut max_depth = 0u32;

        for node in nodes {
            let depth = node.location.semantic.depth;
            *depth_counts.entry(depth).or_insert(0) += 1;
            total_depth += depth;
            max_depth = max_depth.max(depth);
        }

        let avg_depth = if !nodes.is_empty() {
            total_depth as f32 / nodes.len() as f32
        } else {
            0.0
        };

        DepthDistribution {
            max_depth,
            depth_counts,
            avg_depth,
        }
    }
}
