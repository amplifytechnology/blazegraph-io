// Composite builder that drives the single-pass walk and the dependency-ordered
// finalization. See
// `docs/P2/core/design-flows/2026-04-28-document-analytics-and-header-footer-classification.md`.

use serde::{Deserialize, Serialize};

use crate::analytics::font::{FontStats, FontStatsBuilder};
use crate::analytics::geometry::{GeometryStats, GeometryStatsBuilder};
use crate::analytics::page_roles::{classify_page_roles, PageRolesConfig};
use crate::analytics::page_stats::{PageStats, PageStatsBuilder};
use crate::analytics::region::{RegionStats, RegionStatsBuilder};
use crate::analytics::statistic::{FinalizationContext, Statistic};
use crate::types::PdfTextElement;

/// Composite builder that drives a single-pass walk over text elements,
/// dispatching each element to all enabled stat kinds.
///
/// Construct with [`AnalysisBuilder::new`], call [`AnalysisBuilder::observe`]
/// once per element, then [`AnalysisBuilder::finalize`] to obtain a
/// [`DocumentAnalysis`].
#[derive(Debug, Default)]
pub struct AnalysisBuilder {
    pub font: FontStatsBuilder,
    pub geometry: GeometryStatsBuilder,
    pub page_stats: PageStatsBuilder,
    pub region: RegionStatsBuilder,
}

impl AnalysisBuilder {
    /// Construct an empty builder ready to observe elements.
    pub fn new() -> Self {
        Self::default()
    }

    /// Dispatch an element to every enabled stat kind. Called once per element
    /// in document reading order.
    pub fn observe(&mut self, element: &PdfTextElement) {
        self.font.observe(element);
        self.geometry.observe(element);
        self.page_stats.observe(element);
        self.region.observe(element);
    }

    /// Finalize all stat kinds in dependency order and produce a
    /// [`DocumentAnalysis`]. Order: `font → geometry → region → page_stats`.
    /// Font has no cross-stat dependencies; geometry reads font; region
    /// reads geometry; page_stats reads font, geometry, and region (it
    /// attaches per-leaf `RegionSignature`s to the per-page Region trees).
    pub fn finalize(self) -> DocumentAnalysis {
        // Font has no dependencies.
        let empty_ctx = FinalizationContext::default();
        let font = self.font.finalize(&empty_ctx);

        // Geometry depends on the finalized FontStats — its per-page footer
        // walk reads the document-level body size instead of a fragile
        // per-page median. See `find_per_page_footer_line` in geometry.rs
        // for the rationale.
        let geometry_ctx = FinalizationContext {
            font: Some(&font),
            geometry: None,
            region: None,
        };
        let geometry = self.geometry.finalize(&geometry_ctx);

        // RegionStats depends on GeometryStats (body box + column dividers).
        let region_ctx = FinalizationContext {
            font: Some(&font),
            geometry: Some(&geometry),
            region: None,
        };
        let region = self.region.finalize(&region_ctx);

        // PageStats depends on font + geometry (heatmap) + region (the
        // per-page Region trees its per-leaf signatures attach to).
        let page_ctx = FinalizationContext {
            font: Some(&font),
            geometry: Some(&geometry),
            region: Some(&region),
        };
        let mut page_stats = self.page_stats.finalize(&page_ctx);

        // Page-roles classifier (Block 06) — analytics post-pass that
        // assigns each PageSignature.role and the derived body_pages
        // extent on PageStats. Runs unconditionally; downstream rules
        // pull `body_start_page` / `body_end_page` for filtering.
        classify_page_roles(&mut page_stats, &PageRolesConfig::default());

        DocumentAnalysis {
            font,
            geometry,
            page_stats,
            region,
        }
    }
}

/// Composite output of the analytics pre-pass. Carries one finalized output
/// per stat kind. Lives in pipeline memory; not serialized into the public
/// graph output (a separate sidecar dump may serialize it for development
/// purposes).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DocumentAnalysis {
    pub font: FontStats,
    pub geometry: GeometryStats,
    pub page_stats: PageStats,
    pub region: RegionStats,
}
