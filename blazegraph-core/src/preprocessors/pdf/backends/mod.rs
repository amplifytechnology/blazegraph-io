//! PDF Backend trait and backend enum
//!
//! Defines the interface that all PDF extraction backends must implement.
//! All backends produce the same Blazegraph XHTML intermediate format.

use anyhow::Result;

/// Backend trait for PDF extraction
///
/// All backends must produce the same Blazegraph XHTML format with:
/// - Page divs with data-page attributes
/// - Spans with data-bbox, data-line, data-segment attributes
/// - CSS font classes in <style> block
/// - Bookmark list in <ul> (if available)
///
/// This allows the XHTML parser to be shared across all backends.
pub trait PdfBackend: Send + Sync {
    /// Extract PDF bytes to Blazegraph XHTML format
    fn extract_to_xhtml(&self, pdf_bytes: &[u8]) -> Result<String>;

    /// Backend identifier for logging/debugging
    fn name(&self) -> &str;

    /// Check if backend is healthy/ready
    fn is_healthy(&self) -> bool;
}

// Re-export backends
#[cfg(feature = "jni-backend")]
pub mod jni;

#[cfg(feature = "jni-backend")]
pub use jni::TikaJniBackend;
