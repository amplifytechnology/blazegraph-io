// Main rules module - delegates to semantic sub-modules
// This file coordinates the rule system but actual implementations are in:
// - engine.rs: RuleEngine and shared utilities
// - section_detection.rs: Font-based section detection
// - pattern_detection.rs: Pattern-based section promotion
// - spatial_clustering.rs: Spatial clustering and style analysis
// - validation.rs: Final validation and cleanup

// Import sub-modules directly - they are in the rules/ directory
pub mod engine;
pub mod section_detection;
pub mod spatial_clustering;
pub mod validation;

// Disabled modules (will be rewritten):
// pub mod list_detection;
// pub mod pattern_detection;
// pub mod size_enforcer;

// Re-export everything for backwards compatibility
pub use engine::*;