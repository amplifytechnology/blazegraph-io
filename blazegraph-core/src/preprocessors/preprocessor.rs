// Preprocessor abstraction for document processing
//
// This module defines the boundary between document preprocessing (PDF -> TextElements)
// and semantic processing (TextElements -> Graph). The preprocessor abstraction allows
// for different document parsing backends while maintaining a consistent interface.

use crate::types::*;
use anyhow::Result;
use std::path::Path;



/// Preprocessor trait - converts documents to TextElements
/// 
/// This is the key abstraction boundary in blazegraph. Preprocessors handle:
/// - Document format parsing (PDF, Word, etc.)
/// - Text extraction and positioning
/// - Basic structure detection (pages, paragraphs, etc.)
/// 
/// Everything after this point works with TextElements and is format-agnostic.
/// 
/// The preprocessing happens in two clear steps:
/// 1. Document -> Markup Language (e.g., PDF -> XHTML)
/// 2. Markup Language -> PreprocessorOutput (structured data)
pub trait Preprocessor {
    /// Step 1: Convert document to markup language
    /// 
    /// For Tika: PDF bytes -> XHTML
    /// For other preprocessors: DOC bytes -> HTML, etc.
    /// This step handles the raw document format conversion.
    fn parse_pdf_to_markup_language(&self, pdf_bytes: &[u8]) -> Result<String>;
    
    /// Step 2: Convert markup language to structured output
    /// 
    /// Parses markup (XHTML, HTML, etc.) into our structured format
    /// with text elements, metadata, styling, and bookmarks.
    /// This step is format-agnostic after step 1.
    fn parse_markup_to_preprocessor_output(&self, markup: &str) -> Result<PreprocessorOutput>;
    
    /// Convenience method: Full document processing (combines both steps)
    /// 
    /// This is the main entry point for document processing.
    /// Default implementation calls the two steps in sequence.
    fn process(&self, pdf_bytes: &[u8]) -> Result<PreprocessorOutput> {
        let markup = self.parse_pdf_to_markup_language(pdf_bytes)?;
        self.parse_markup_to_preprocessor_output(&markup)
    }
    
    /// Convenience method: Process from file path
    /// 
    /// Reads file and processes the bytes. Useful for CLI and backwards compatibility.
    fn process_file(&self, input: &Path) -> Result<PreprocessorOutput> {
        let pdf_bytes = std::fs::read(input)?;
        self.process(&pdf_bytes)
    }
    
    /// Get preprocessor name for debugging/logging
    fn name(&self) -> &str;
    
    /// Check if preprocessor supports the given file type
    fn supports_file_type(&self, path: &Path) -> bool;
}

