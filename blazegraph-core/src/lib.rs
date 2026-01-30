// Blazegraph Core Library
//
// Provides document processing with pluggable preprocessor architecture.
// Main interface for converting documents to semantic graphs.

pub mod types;
pub mod preprocessors;
pub mod processor;
pub mod graphs;
pub mod cache;
pub mod config;
pub mod rules;
pub mod classifier;
pub mod storage;

// Re-export main types and functions for easy use
pub use types::*;
pub use preprocessors::{Preprocessor, PdfPreprocessor, TikaPreprocessor};
pub use processor::DocumentProcessor;
pub use config::ParsingConfig;

// Re-export backends for direct use
#[cfg(feature = "jni-backend")]
pub use preprocessors::TikaJniBackend;
