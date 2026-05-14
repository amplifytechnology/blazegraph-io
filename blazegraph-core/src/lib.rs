// Blazegraph Core Library
//
// Provides document processing with pluggable preprocessor architecture.
// Main interface for converting documents to semantic graphs.

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
