use crate::types::*;
use std::collections::HashMap;

impl DocumentGraph {
    /// Compute enhanced metadata analytics for the entire graph
    pub fn compute_enhanced_metadata(&mut self) {
        let all_nodes: Vec<&DocumentNode> = self.nodes.values().collect();
        let analytics = GraphAnalytics::compute_analytics(&all_nodes);
        
        // Extract total_tokens before moving analytics fields
        let total_tokens = analytics.token_distribution.overall.total_tokens;
        
        // Update metadata with analytics results
        self.metadata.token_distribution = analytics.token_distribution;
        self.metadata.node_type_distribution = analytics.node_type_distribution;
        self.metadata.depth_distribution = analytics.depth_distribution;
        self.metadata.structural_health = analytics.structural_health;
        self.metadata.total_tokens = total_tokens;
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
            structural_health: Self::assess_structural_health(nodes),
        }
    }
    
    /// Compute histogram-based token distribution with adaptive binning
    fn compute_token_distribution(nodes: &[&DocumentNode]) -> TokenDistribution {
        let mut overall_tokens = Vec::new();
        let mut by_type: HashMap<String, Vec<usize>> = HashMap::new();
        
        // Collect token counts by type
        for node in nodes {
            overall_tokens.push(node.token_count);
            by_type.entry(node.node_type.clone())
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
        let mean = if total_count > 0 { total_tokens as f32 / total_count as f32 } else { 0.0 };
        let median = if sorted_tokens.is_empty() { 
            0.0 
        } else if sorted_tokens.len() % 2 == 0 {
            let mid = sorted_tokens.len() / 2;
            (sorted_tokens[mid - 1] + sorted_tokens[mid]) as f32 / 2.0
        } else {
            sorted_tokens[sorted_tokens.len() / 2] as f32
        };
        
        let mode = bins.iter()
            .max_by_key(|bin| bin.count)
            .map(|bin| bin.range_start);
            
        let variance = if total_count > 1 {
            let mean_val = mean;
            sorted_tokens.iter()
                .map(|&token| (token as f32 - mean_val).powi(2))
                .sum::<f32>() / (total_count - 1) as f32
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
            *depth_counts.entry(node.depth).or_insert(0) += 1;
            total_depth += node.depth;
            max_depth = max_depth.max(node.depth);
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
    
    /// Assess structural health metrics for GUI dashboard
    fn assess_structural_health(nodes: &[&DocumentNode]) -> StructuralHealth {
        let token_distribution = Self::compute_token_distribution(nodes);
        let node_type_distribution = Self::compute_node_type_distribution(nodes);
        let depth_distribution = Self::compute_depth_distribution(nodes);
        
        // Assess token variance level
        let token_variance_level = match token_distribution.overall.variance {
            v if v < 1000.0 => VarianceLevel::Low,
            v if v < 10000.0 => VarianceLevel::Medium,
            _ => VarianceLevel::High,
        };
        
        // Assess depth balance
        let depth_balance = match depth_distribution.avg_depth {
            d if d < 2.0 => BalanceLevel::Shallow,
            d if d > 5.0 => BalanceLevel::Deep,
            _ => BalanceLevel::Balanced,
        };
        
        // Assess node type richness
        let type_count = node_type_distribution.counts.len();
        let node_type_richness = match type_count {
            0..=2 => RichnessLevel::Sparse,
            3..=5 => RichnessLevel::Rich,
            _ => RichnessLevel::Unbalanced,
        };
        
        StructuralHealth {
            token_variance_level,
            depth_balance,
            node_type_richness,
        }
    }
}