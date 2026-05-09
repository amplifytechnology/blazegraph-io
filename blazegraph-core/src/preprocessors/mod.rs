//! Document Preprocessors
//!
//! This module provides the preprocessing layer for converting various document
//! formats into a unified PreprocessorOutput that feeds into the graph builder.
//!
//! ## Architecture
//!
//! ```text
//! Document (PDF, DOCX, MD, etc.)
//!     ↓
//! [Format-specific Preprocessor]
//!     ↓
//! PreprocessorOutput (unified format)
//!     ↓
//! [Graph Builder]
//!     ↓
//! DocumentGraph
//! ```
//!
//! ## Available Preprocessors
//!
//! - `PdfPreprocessor` - PDF documents via JNI backend (Apache Tika)
//! - (Future) `MarkdownPreprocessor` - Markdown files
//! - (Future) `DocxPreprocessor` - Word documents

pub mod pdf;
pub mod traits;

// Re-export main types
pub use pdf::{PdfBackend, PdfBackendImpl, PdfPreprocessor};
pub use traits::Preprocessor;

// Re-export backends
#[cfg(feature = "jni-backend")]
pub use pdf::TikaJniBackend;

// Legacy alias for backwards compatibility
pub use pdf::TikaPreprocessor;
