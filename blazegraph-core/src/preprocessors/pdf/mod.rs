//! PDF Preprocessor
//!
//! Main preprocessor for PDF documents. Uses pluggable backends to extract
//! PDF content to Blazegraph XHTML, then parses into PreprocessorOutput.

pub mod backends;
pub mod xhtml_parser;

use crate::preprocessors::traits::Preprocessor;
use crate::types::*;
use anyhow::Result;
use std::path::Path;

pub use backends::PdfBackend;

#[cfg(feature = "jni-backend")]
pub use backends::TikaJniBackend;

/// Backend enum for runtime backend selection
pub enum PdfBackendImpl {
    #[cfg(feature = "jni-backend")]
    Jni(TikaJniBackend),
}

impl PdfBackend for PdfBackendImpl {
    fn extract_to_xhtml(&self, pdf_bytes: &[u8]) -> Result<String> {
        match self {
            #[cfg(feature = "jni-backend")]
            PdfBackendImpl::Jni(backend) => backend.extract_to_xhtml(pdf_bytes),
        }
    }

    fn name(&self) -> &str {
        match self {
            #[cfg(feature = "jni-backend")]
            PdfBackendImpl::Jni(backend) => backend.name(),
        }
    }

    fn is_healthy(&self) -> bool {
        match self {
            #[cfg(feature = "jni-backend")]
            PdfBackendImpl::Jni(backend) => backend.is_healthy(),
        }
    }
}

/// PDF Preprocessor with pluggable backend
///
/// Processes PDF documents through two stages:
/// 1. Backend extraction: PDF bytes → Blazegraph XHTML
/// 2. XHTML parsing: Blazegraph XHTML → PreprocessorOutput
pub struct PdfPreprocessor {
    backend: PdfBackendImpl,
}

impl PdfPreprocessor {
    /// Create PdfPreprocessor with JNI backend (default JVM settings)
    ///
    /// # Arguments
    /// * `jre_path` - Path to JRE directory
    /// * `jar_path` - Path to blazing-tika.jar
    #[cfg(feature = "jni-backend")]
    pub fn new_with_jni(jre_path: &Path, jar_path: &Path) -> Result<Self> {
        Ok(Self {
            backend: PdfBackendImpl::Jni(TikaJniBackend::new(jre_path, jar_path)?),
        })
    }

    /// Create PdfPreprocessor with JNI backend and custom JVM arguments
    ///
    /// # Arguments
    /// * `jre_path` - Path to JRE directory
    /// * `jar_path` - Path to blazing-tika.jar
    /// * `jvm_args` - Additional JVM arguments (e.g., "-Xmx4g", "-XX:+UseG1GC")
    ///
    /// # Example
    /// ```ignore
    /// let jvm_args = vec![
    ///     "-Xms1g".to_string(),
    ///     "-Xmx4g".to_string(),
    ///     "-XX:+UseG1GC".to_string(),
    ///     "-XX:MaxGCPauseMillis=100".to_string(),
    /// ];
    /// let preprocessor = PdfPreprocessor::new_with_jni_args(&jre_path, &jar_path, &jvm_args)?;
    /// ```
    #[cfg(feature = "jni-backend")]
    pub fn new_with_jni_args(jre_path: &Path, jar_path: &Path, jvm_args: &[String]) -> Result<Self> {
        Ok(Self {
            backend: PdfBackendImpl::Jni(TikaJniBackend::new_with_args(
                jre_path, jar_path, jvm_args,
            )?),
        })
    }

    /// Get the backend name for logging
    pub fn backend_name(&self) -> &str {
        self.backend.name()
    }

    /// Check if the backend is healthy
    pub fn is_healthy(&self) -> bool {
        self.backend.is_healthy()
    }
}

impl Preprocessor for PdfPreprocessor {
    /// Step 1: Extract PDF to XHTML via backend
    fn parse_pdf_to_markup_language(&self, pdf_bytes: &[u8]) -> Result<String> {
        self.backend.extract_to_xhtml(pdf_bytes)
    }

    /// Step 2: Parse XHTML to PreprocessorOutput
    fn parse_markup_to_preprocessor_output(&self, markup: &str) -> Result<PreprocessorOutput> {
        xhtml_parser::parse_xhtml(markup)
    }

    fn name(&self) -> &str {
        "PdfPreprocessor"
    }

    fn supports_file_type(&self, path: &Path) -> bool {
        if let Some(extension) = path.extension() {
            matches!(
                extension.to_str().unwrap_or("").to_lowercase().as_str(),
                "pdf"
            )
        } else {
            false
        }
    }
}

// Legacy type alias for backwards compatibility
pub type TikaPreprocessor = PdfPreprocessor;
