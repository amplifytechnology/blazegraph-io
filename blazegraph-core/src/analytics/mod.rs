// Document analytics pre-pass.
//
// Produces shared descriptive statistics about a parsed document — font,
// geometry, per-page distributions — that downstream rules consume as
// priors instead of recomputing locally.
//
// See `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`
// for the full design. Inspired by Postgres `pg_statistic`: descriptive
// primitives in core, threshold decisions in consumers.
//
// This module currently defines the type and trait shape only. The
// `observe()` and `finalize()` bodies are stubs; subsequent blocks
// (02 FontStats, 03 GeometryStats, 04 PageStats) fill them in.

pub mod builder;
pub mod font;
pub mod geometry;
pub mod page_roles;
pub mod page_stats;
pub mod reading_order;
pub mod region;
pub mod repetition;
pub mod statistic;

pub use builder::{AnalysisBuilder, DocumentAnalysis};
pub use font::{FontStats, FontStatsBuilder, FontStatsConfig};
pub use geometry::{
    ColumnLayout, DensityGrid, GeometryDiagnostic, GeometryStats, GeometryStatsBuilder,
    GeometryStatsConfig, PageDimensions,
};
pub use page_roles::{classify_page_roles, PageRoleKind, PageRolesConfig};
pub use page_stats::{
    PageSignature, PageStats, PageStatsBuilder, PageStatsConfig, RegionSignature,
};
pub use reading_order::tag_and_resort;
pub use region::{
    CutAxis, PageRegionDiagnostic, PageRegions, Region, RegionBox, RegionStats, RegionStatsBuilder,
    RegionStatsConfig,
};
pub use repetition::{RepetitionMap, RepetitionRecord, YBucket};
pub use statistic::{FinalizationContext, Statistic};

#[cfg(test)]
mod tests;
