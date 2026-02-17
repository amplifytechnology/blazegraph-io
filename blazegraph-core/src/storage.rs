use crate::types::{TikaOutput, PreprocessorOutput};
use crate::cache::{GraphCacheKey, GraphCacheValue};
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

/// Storage abstraction for caching PDF processing results
pub trait DocumentStorage {
    // Level 0: PDF storage (unused currently)
    fn _get_pdf(&self, hash: &str) -> Result<Option<Vec<u8>>>;
    fn _store_pdf(&self, hash: &str, data: &[u8]) -> Result<()>;

    // Level 1: Tika processing cache (PDF → XHTML) - Legacy
    fn get_tika_output(&self, pdf_hash: &str) -> Result<Option<TikaOutput>>;
    fn store_tika_output(&self, pdf_hash: &str, output: &TikaOutput) -> Result<()>;

    // Level 1: Preprocessor cache (PDF → PreprocessorOutput) - New generalized interface
    fn get_preprocessor_output(&self, pdf_hash: &str) -> Result<Option<PreprocessorOutput>>;
    fn store_preprocessor_output(&self, pdf_hash: &str, output: &PreprocessorOutput) -> Result<()>;

    // Level 2: Graph processing cache (XHTML + Config → Graph) - NEW
    fn get_graph_output(&self, cache_key: &GraphCacheKey) -> Result<Option<GraphCacheValue>>;
    fn store_graph_output(&self, cache_key: &GraphCacheKey, cache_value: &GraphCacheValue) -> Result<()>;
}

/// File-based storage implementation using local cache directory
pub struct FileStorage {
    cache_dir: String,
}

impl FileStorage {
    pub fn new(cache_dir: &str) -> Result<Self> {
        // Ensure cache directory exists
        fs::create_dir_all(cache_dir)?;
        fs::create_dir_all(format!("{cache_dir}/pdfs"))?;
        fs::create_dir_all(format!("{cache_dir}/tika"))?;
        fs::create_dir_all(format!("{cache_dir}/preprocessor"))?; // NEW: Generalized preprocessor cache
        fs::create_dir_all(format!("{cache_dir}/graph"))?; // NEW: Level 2 cache directory

        Ok(Self {
            cache_dir: cache_dir.to_string(),
        })
    }

    fn pdf_path(&self, hash: &str) -> String {
        format!("{}/pdfs/{}.pdf", self.cache_dir, hash)
    }

    fn tika_path(&self, hash: &str) -> String {
        format!("{}/tika/{}.json", self.cache_dir, hash)
    }

    fn preprocessor_path(&self, hash: &str) -> String {
        format!("{}/preprocessor/{}.json", self.cache_dir, hash)
    }

    fn graph_path(&self, cache_key: &GraphCacheKey) -> String {
        format!("{}/graph/{}.json", self.cache_dir, cache_key.to_cache_hash())
    }
}

impl DocumentStorage for FileStorage {
    fn _get_pdf(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let path = self.pdf_path(hash);
        if Path::new(&path).exists() {
            Ok(Some(fs::read(path)?))
        } else {
            Ok(None)
        }
    }

    fn _store_pdf(&self, hash: &str, data: &[u8]) -> Result<()> {
        let path = self.pdf_path(hash);
        fs::write(path, data)?;
        Ok(())
    }

    fn get_tika_output(&self, pdf_hash: &str) -> Result<Option<TikaOutput>> {
        let path = self.tika_path(pdf_hash);
        if Path::new(&path).exists() {
            let json_str = fs::read_to_string(path)?;
            let output: TikaOutput = serde_json::from_str(&json_str)
                .map_err(|e| anyhow!("Failed to deserialize cached TikaOutput: {}", e))?;
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }

    fn store_tika_output(&self, pdf_hash: &str, output: &TikaOutput) -> Result<()> {
        let path = self.tika_path(pdf_hash);
        let json_str = serde_json::to_string_pretty(output)
            .map_err(|e| anyhow!("Failed to serialize TikaOutput: {}", e))?;
        fs::write(path, json_str)?;
        Ok(())
    }

    fn get_preprocessor_output(&self, pdf_hash: &str) -> Result<Option<PreprocessorOutput>> {
        let path = self.preprocessor_path(pdf_hash);
        if Path::new(&path).exists() {
            let json_str = fs::read_to_string(path)?;
            let output: PreprocessorOutput = serde_json::from_str(&json_str)
                .map_err(|e| anyhow!("Failed to deserialize cached PreprocessorOutput: {}", e))?;
            Ok(Some(output))
        } else {
            Ok(None)
        }
    }

    fn store_preprocessor_output(&self, pdf_hash: &str, output: &PreprocessorOutput) -> Result<()> {
        let path = self.preprocessor_path(pdf_hash);
        let json_str = serde_json::to_string_pretty(output)
            .map_err(|e| anyhow!("Failed to serialize PreprocessorOutput: {}", e))?;
        fs::write(path, json_str)?;
        Ok(())
    }

    // Level 2: Graph processing cache implementation
    fn get_graph_output(&self, cache_key: &GraphCacheKey) -> Result<Option<GraphCacheValue>> {
        let path = self.graph_path(cache_key);
        if Path::new(&path).exists() {
            let json_str = fs::read_to_string(path)?;
            let cache_value: GraphCacheValue = serde_json::from_str(&json_str)
                .map_err(|e| anyhow!("Failed to deserialize cached GraphCacheValue: {}", e))?;
            Ok(Some(cache_value))
        } else {
            Ok(None)
        }
    }

    fn store_graph_output(&self, cache_key: &GraphCacheKey, cache_value: &GraphCacheValue) -> Result<()> {
        let path = self.graph_path(cache_key);
        let json_str = serde_json::to_string_pretty(cache_value)
            .map_err(|e| anyhow!("Failed to serialize GraphCacheValue: {}", e))?;
        fs::write(path, json_str)?;
        Ok(())
    }
}

/// Calculate a fast hash for PDF content using start + end chunks
pub fn calculate_pdf_hash(pdf_bytes: &[u8]) -> String {
    let chunk_size = 1024; // 1KB from start and end
    let mut hasher = Sha256::new();

    // Hash file size first (for quick differentiation)
    hasher.update(pdf_bytes.len().to_le_bytes());

    // Hash first chunk
    let start_end = std::cmp::min(chunk_size, pdf_bytes.len());
    hasher.update(&pdf_bytes[0..start_end]);

    // Hash last chunk (if file is large enough)
    if pdf_bytes.len() > chunk_size {
        let end_start = pdf_bytes.len() - chunk_size;
        hasher.update(&pdf_bytes[end_start..]);
    }

    format!("{:x}", hasher.finalize())
}

/// Calculate hash for configuration data (for Level 2 cache key)
pub fn calculate_config_hash<T: serde::Serialize>(config: &T) -> Result<String> {
    let config_json = serde_json::to_string(config)
        .map_err(|e| anyhow!("Failed to serialize config for hashing: {}", e))?;
    
    let mut hasher = Sha256::new();
    hasher.update(config_json.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

/// Calculate hash for XHTML content (for Level 2 cache key)
pub fn calculate_xhtml_hash(xhtml: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(xhtml.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// No-op storage implementation that disables all caching
pub struct NoOpStorage;

impl Default for NoOpStorage {
    fn default() -> Self {
        Self::new()
    }
}

impl NoOpStorage {
    pub fn new() -> Self {
        Self
    }
}

impl DocumentStorage for NoOpStorage {
    fn _get_pdf(&self, _hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(None) // Always cache miss
    }

    fn _store_pdf(&self, _hash: &str, _data: &[u8]) -> Result<()> {
        Ok(()) // No-op
    }

    fn get_tika_output(&self, _pdf_hash: &str) -> Result<Option<TikaOutput>> {
        Ok(None) // Always cache miss
    }

    fn store_tika_output(&self, _pdf_hash: &str, _output: &TikaOutput) -> Result<()> {
        Ok(()) // No-op
    }

    fn get_preprocessor_output(&self, _pdf_hash: &str) -> Result<Option<PreprocessorOutput>> {
        Ok(None) // Always cache miss
    }

    fn store_preprocessor_output(&self, _pdf_hash: &str, _output: &PreprocessorOutput) -> Result<()> {
        Ok(()) // No-op
    }

    fn get_graph_output(&self, _cache_key: &GraphCacheKey) -> Result<Option<GraphCacheValue>> {
        Ok(None) // Always cache miss
    }

    fn store_graph_output(&self, _cache_key: &GraphCacheKey, _cache_value: &GraphCacheValue) -> Result<()> {
        Ok(()) // No-op
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdf_hash_consistency() {
        let pdf_data = b"test pdf content with some data";
        let hash1 = calculate_pdf_hash(pdf_data);
        let hash2 = calculate_pdf_hash(pdf_data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_pdf_hash_uniqueness() {
        let pdf1 = b"test pdf content 1";
        let pdf2 = b"test pdf content 2";
        let hash1 = calculate_pdf_hash(pdf1);
        let hash2 = calculate_pdf_hash(pdf2);
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_file_storage_roundtrip() {
        let temp_dir = std::env::temp_dir().join("blazegraph_test_cache");
        let storage = FileStorage::new(temp_dir.to_str().unwrap()).unwrap();

        let test_data = b"test pdf data";
        let hash = "test_hash";

        // Store and retrieve PDF
        storage._store_pdf(hash, test_data).unwrap();
        let retrieved = storage._get_pdf(hash).unwrap();
        assert_eq!(retrieved, Some(test_data.to_vec()));

        // Clean up
        std::fs::remove_dir_all(temp_dir).ok();
    }
}
