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

pub mod traits;
pub mod pdf;

// Re-export main types
pub use traits::Preprocessor;
pub use pdf::{PdfPreprocessor, PdfBackend, PdfBackendImpl};

// Re-export backends
#[cfg(feature = "jni-backend")]
pub use pdf::TikaJniBackend;

// Legacy alias for backwards compatibility
pub use pdf::TikaPreprocessor;
