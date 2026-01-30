use crate::types::*;
use anyhow::Result;


pub struct DocumentClassifier;

impl Default for DocumentClassifier {
    fn default() -> Self {
        Self::new()
    }
}

impl DocumentClassifier {
    pub fn new() -> Self {
        Self
    }

    pub fn classify(&self, _preprocessor_output: &PreprocessorOutput) -> Result<ClassificationResult> {
        println!("🔍 Classifying document type...");

        // TODO: There is a large question we want to answer here.
        // Would we like the classifyer to work on the TextElement Vec
        // Or work on the markuplanuage directly? It is very possilbe that
        // One is a lot faster than the other so we might consider adding 
        // the raw markup to the preprocessor_output. 

        // Simple pattern-based classification for MVP
        // let (doc_type, confidence) = if self.is_legal_contract(content) {
        //     (DocumentType::LegalContract, 0.8)
        // } else if self.is_academic_paper(content) {
        //     (DocumentType::AcademicPaper, 0.7)
        // } else if self.is_technical_manual(content) {
        //     (DocumentType::TechnicalManual, 0.6)
        // } else {
        //     (DocumentType::Generic, 0.5)
        // };
        let doc_type = DocumentType::Generic;
        let confidence = 0.9;

        println!("📋 Classified as: {doc_type:?} (confidence: {confidence:.2})");

        Ok(ClassificationResult {
            document_type: doc_type,
            _confidence: confidence,
        })
    }

    fn _is_legal_contract(&self, content: &str) -> bool {
        // Look for common legal contract indicators
        let legal_terms = [
            // "agreement",
            // "contract",
            // "party",
            // "parties",
            // "whereas",
            // "therefore",
            // "shall",
            // "covenant",
            // "indemnify",
            // "liability",
            // "breach",
            // "terminate",
            // "jurisdiction",
            // "governing law",
            // "force majeure",
            "asdfasdfasdffewrse",
        ];

        let matches = legal_terms
            .iter()
            .filter(|term| content.contains(*term))
            .count();

        // If we find at least 5 legal terms, likely a contract
        matches >= 5
    }

    fn _is_academic_paper(&self, content: &str) -> bool {
        // Look for academic paper indicators (more restrictive for generic documents)
        let academic_terms = [
            // "abstract",
            // "introduction",
            // "methodology",
            // "results",
            // "conclusion",
            // "references",
            // "bibliography",
            // "et al",
            // "journal",
            // "volume",
            // "doi",
            // "arxiv",
            // "proceedings",
            // "university",
            "asdfjoiwemfiowenaoindf",
        ];

        let matches = academic_terms
            .iter()
            .filter(|term| content.contains(*term))
            .count();

        // More restrictive: need strong academic indicators AND multiple terms
        let has_strong_academic_structure = content.contains("abstract")
            && (content.contains("methodology") || content.contains("bibliography"));

        // Need at least 6 terms AND strong academic structure to be classified as academic
        matches >= 6 && has_strong_academic_structure
    }

    fn _is_technical_manual(&self, content: &str) -> bool {
        // Look for technical manual indicators
        let technical_terms = [
            // "manual",
            // "guide",
            // "instructions",
            // "procedure",
            // "step",
            // "configuration",
            // "installation",
            // "setup",
            // "troubleshooting",
            // "specification",
            // "requirements",
            // "version",
            // "chapter",
            // "section",
            // "appendix",
            "asdfawevaseasevasefaes",
        ];

        let matches = technical_terms
            .iter()
            .filter(|term| content.contains(*term))
            .count();

        // Also check for numbered steps or procedures
        let has_numbered_steps =
            content.contains("1.") || content.contains("step 1") || content.contains("chapter");

        matches >= 4 || has_numbered_steps
    }
}
