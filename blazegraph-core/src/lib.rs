// Blazegraph Core Library
//
// Provides document processing with pluggable preprocessor architecture.
// Main interface for converting documents to semantic graphs.

/// The **code axis** of the version model (arch-15 § Version model):
/// the blazegraph-core crate version (e.g. `"0.3.0"`), from
/// `CARGO_PKG_VERSION` at build time. This is the canonical "parser
/// version" for `ParseProvenance.blazegraph_version`: consumers that
/// produce graphs through this crate (CLI, API) should stamp this rather
/// than their own crate version. Signals code changed — *correlates* to
/// output diffs (text-formatting / hash-scheme), doesn't *define*
/// structure. Content-addressed consumers (URD) pin this alongside
/// [`BGRAPH_FORMAT_VERSION`] for exact-output reproducibility. Standing
/// discipline (CR-87): bump this whenever output changes (schema or
/// formatting) so the graph cache invalidates correctly — the cache key
/// tracks it via [`cache::versions::BLAZEGRAPH_VERSION`].
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The **schema/format axis** of the version model (arch-15): the single,
/// serialization-neutral version of the emitted artifact, stamped
/// identically in the bgraph.md doc-level `schema` field and the json
/// `SortedDocumentGraph.schema_version` (CR-87). "Can my code read this
/// shape?" — the structural contract, orthogonal to `graph_sha256`
/// (content identity) and to [`VERSION`] (code identity). Defined in the
/// md preprocessor (where the format lineage history lives); re-exported
/// here so json-side and downstream consumers reach a neutral path.
pub use preprocessors::md::BGRAPH_FORMAT_VERSION;

pub mod analytics;
pub mod cache;
pub mod classifier;
pub mod config;
pub mod graphs;
pub mod preprocessors;
pub mod processor;
pub mod rules;
pub mod storage;
pub mod tokens;
pub mod types;

// Re-export main types and functions for easy use
pub use analytics::DocumentAnalysis;
pub use config::ParsingConfig;
pub use graphs::NodeIdGenerator;
pub use preprocessors::{PdfPreprocessor, Preprocessor, TikaPreprocessor};
pub use processor::{DocumentProcessor, PipelineStages};
pub use storage::{CacheDefaults, CachePoint, FreshFrom};
pub use types::*;

// Re-export backends for direct use
#[cfg(feature = "jni-backend")]
pub use preprocessors::TikaJniBackend;
