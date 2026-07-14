use crate::cache::GraphCacheKey;
use crate::types::PreprocessorOutput;
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

// =============================================================================
// Cache Point System (CR-11)
// =============================================================================
// The pipeline has four discrete cache points, numbered by position.
// "CachePoint" (not "CacheLayer") to avoid collision with L0/L1/L2 semantic layers.

/// A discrete cache point in the processing pipeline.
/// Ordered by pipeline position: C0 < C1 < C2 < C3.
///
/// The tier split is intermediate-vs-output: C0–C2 are config-independent
/// **intermediates** (keyed by `pdf_hash` alone); C3 is the config-dependent
/// **output** — the finished `DocumentGraph`, keyed by `pdf_hash + config_hash`.
/// `bgraph.md`/`bgraph.json` are serializations *of* C3, emitted on demand;
/// the graph is the canonical, format-neutral output (`graph_sha256` is over
/// it). This is why C3, and only C3, folds config into its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CachePoint {
    /// C0: Original PDF bytes (source)
    C0,
    /// C1: Blazegraph XHTML from Tika/JNI extraction (intermediate)
    C1,
    /// C2: PreprocessorOutput — parsed elements + metadata (intermediate)
    C2,
    /// C3: the cached **output** — the config-keyed `DocumentGraph`
    /// (serialized to bgraph.md/json on demand)
    C3,
}

impl CachePoint {
    /// All cache points in pipeline order.
    pub fn all() -> &'static [CachePoint] {
        &[
            CachePoint::C0,
            CachePoint::C1,
            CachePoint::C2,
            CachePoint::C3,
        ]
    }

    /// Cache points at and downstream of this point (for cascade operations).
    pub fn cascade(&self) -> Vec<CachePoint> {
        CachePoint::all()
            .iter()
            .copied()
            .filter(|p| p >= self)
            .collect()
    }

    /// Directory name for this cache point.
    pub fn dir_name(&self) -> &'static str {
        match self {
            CachePoint::C0 => "c0-pdf",
            CachePoint::C1 => "c1-xhtml",
            CachePoint::C2 => "c2-preprocessor",
            CachePoint::C3 => "c3-graph",
        }
    }

    /// Parse from CLI string (e.g., "c0", "c1", "c2", "c3", "all").
    pub fn from_str_with_all(s: &str) -> Result<Option<Self>> {
        match s.to_lowercase().as_str() {
            "c0" => Ok(Some(CachePoint::C0)),
            "c1" => Ok(Some(CachePoint::C1)),
            "c2" => Ok(Some(CachePoint::C2)),
            "c3" => Ok(Some(CachePoint::C3)),
            "all" => Ok(None), // None = all points
            _ => Err(anyhow!(
                "Invalid cache point: '{}'. Use c0, c1, c2, c3, or all",
                s
            )),
        }
    }
}

impl std::fmt::Display for CachePoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                CachePoint::C0 => "C0 (PDF)",
                CachePoint::C1 => "C1 (XHTML)",
                CachePoint::C2 => "C2 (Preprocessor)",
                CachePoint::C3 => "C3 (Graph)",
            }
        )
    }
}

/// Controls which cache points to bypass during processing.
/// Cascade: fresh-from C1 means skip C1, C2, C3 caches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshFrom {
    /// Use all caches normally
    None,
    /// Skip all caches, reprocess everything from PDF
    C0,
    /// Re-extract XHTML from Tika, reparse, rebuild graph
    C1,
    /// Reparse elements from cached XHTML, rebuild graph
    C2,
    /// Rebuild graph from cached preprocessor output
    C3,
}

impl FreshFrom {
    /// Should the cache be consulted for this point?
    pub fn should_use_cache(&self, point: CachePoint) -> bool {
        match self {
            FreshFrom::None => true,
            FreshFrom::C0 => false,
            FreshFrom::C1 => point < CachePoint::C1,
            FreshFrom::C2 => point < CachePoint::C2,
            FreshFrom::C3 => point < CachePoint::C3,
        }
    }

    /// Parse from CLI string.
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "c0" => Ok(FreshFrom::C0),
            "c1" => Ok(FreshFrom::C1),
            "c2" => Ok(FreshFrom::C2),
            "c3" => Ok(FreshFrom::C3),
            _ => Err(anyhow!(
                "Invalid fresh-from value: '{}'. Use c0, c1, c2, or c3",
                s
            )),
        }
    }
}

/// Which cache points are enabled for writing.
#[derive(Debug, Clone)]
pub struct CacheDefaults {
    pub c0_pdf: bool,
    pub c1_xhtml: bool,
    pub c2_preprocessor: bool,
    pub c3_graph: bool,
    /// Analytics sidecar (`stat/**`) write policy. Not a cache tier — a
    /// run-mode gate on the `dump_stats` write into the cache dir. A read-only
    /// replay (golden freeze) sets this `false` so it never dirties the
    /// committed fixture, independent of the config's `dump_analytics`.
    pub stat: bool,
}

impl Default for CacheDefaults {
    fn default() -> Self {
        Self {
            c0_pdf: false,
            c1_xhtml: true,
            c2_preprocessor: true,
            c3_graph: false,
            stat: true,
        }
    }
}

impl CacheDefaults {
    /// Should we write to this cache point?
    pub fn should_write(&self, point: CachePoint) -> bool {
        match point {
            CachePoint::C0 => self.c0_pdf,
            CachePoint::C1 => self.c1_xhtml,
            CachePoint::C2 => self.c2_preprocessor,
            CachePoint::C3 => self.c3_graph,
        }
    }
}

/// Result of a cache clear operation.
pub struct CacheClearResult {
    pub deleted: Vec<(CachePoint, usize)>,
}

// =============================================================================
// Storage Trait
// =============================================================================

/// Storage abstraction for caching pipeline results at each cache point.
pub trait DocumentStorage {
    // C0: PDF storage
    fn get_pdf(&self, hash: &str) -> Result<Option<Vec<u8>>>;
    fn store_pdf(&self, hash: &str, data: &[u8]) -> Result<()>;

    // C1: Blazegraph XHTML (raw string, not JSON)
    fn get_xhtml(&self, pdf_hash: &str) -> Result<Option<String>>;
    fn store_xhtml(&self, pdf_hash: &str, xhtml: &str) -> Result<()>;

    // C2: PreprocessorOutput (parsed elements + metadata)
    fn get_preprocessor_output(&self, pdf_hash: &str) -> Result<Option<PreprocessorOutput>>;
    fn store_preprocessor_output(&self, pdf_hash: &str, output: &PreprocessorOutput) -> Result<()>;

    // C3: Graph output (config-dependent)
    fn get_graph_output(
        &self,
        cache_key: &GraphCacheKey,
    ) -> Result<Option<crate::cache::GraphCacheValue>>;
    fn store_graph_output(
        &self,
        cache_key: &GraphCacheKey,
        cache_value: &crate::cache::GraphCacheValue,
    ) -> Result<()>;

    /// Sidecar dump for one analytics-stat-kind output. Path:
    /// `{cache_dir}/stat/<stat_name>/<pdf_hash>.json`. Per-stat scoping (one
    /// folder per `Statistic::NAME`) lets future stat kinds (RegionStats,
    /// PageOutlier, etc.) drop in without colliding. This is *not* a pipeline
    /// cache: nothing reads it back during processing — it exists for offline
    /// data-science tooling (Python prototype diff, calibration sweeps).
    fn store_stat(&self, pdf_hash: &str, stat_name: &str, json: &str) -> Result<()>;

    // Cache management
    fn clear_cache(&self, from_point: Option<CachePoint>) -> Result<CacheClearResult>;
}

// =============================================================================
// File-based Storage
// =============================================================================

/// File-based storage implementation using local cache directory.
pub struct FileStorage {
    cache_dir: String,
}

impl FileStorage {
    pub fn new(cache_dir: &str) -> Result<Self> {
        fs::create_dir_all(cache_dir)?;
        for point in CachePoint::all() {
            fs::create_dir_all(format!("{}/{}", cache_dir, point.dir_name()))?;
        }
        fs::create_dir_all(format!("{cache_dir}/debug"))?;

        Ok(Self {
            cache_dir: cache_dir.to_string(),
        })
    }

    pub fn cache_dir(&self) -> &str {
        &self.cache_dir
    }

    fn pdf_path(&self, hash: &str) -> String {
        format!("{}/c0-pdf/{}.pdf", self.cache_dir, hash)
    }

    fn xhtml_path(&self, hash: &str) -> String {
        format!("{}/c1-xhtml/{}.xhtml", self.cache_dir, hash)
    }

    fn preprocessor_path(&self, hash: &str) -> String {
        format!("{}/c2-preprocessor/{}.json", self.cache_dir, hash)
    }

    fn graph_path(&self, cache_key: &GraphCacheKey) -> String {
        format!(
            "{}/c3-graph/{}.json",
            self.cache_dir,
            cache_key.to_cache_hash()
        )
    }

    /// Delete all files in a cache point directory, return count deleted.
    fn clear_dir(&self, point: CachePoint) -> Result<usize> {
        let dir = format!("{}/{}", self.cache_dir, point.dir_name());
        let path = Path::new(&dir);
        if !path.exists() {
            return Ok(0);
        }
        let mut count = 0;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                fs::remove_file(entry.path())?;
                count += 1;
            }
        }
        Ok(count)
    }
}

impl DocumentStorage for FileStorage {
    // C0: PDF storage
    fn get_pdf(&self, hash: &str) -> Result<Option<Vec<u8>>> {
        let path = self.pdf_path(hash);
        if Path::new(&path).exists() {
            Ok(Some(fs::read(path)?))
        } else {
            Ok(None)
        }
    }

    fn store_pdf(&self, hash: &str, data: &[u8]) -> Result<()> {
        let path = self.pdf_path(hash);
        fs::write(path, data)?;
        Ok(())
    }

    // C1: Blazegraph XHTML (raw string file, .xhtml extension)
    fn get_xhtml(&self, pdf_hash: &str) -> Result<Option<String>> {
        let path = self.xhtml_path(pdf_hash);
        if Path::new(&path).exists() {
            Ok(Some(fs::read_to_string(path)?))
        } else {
            Ok(None)
        }
    }

    fn store_xhtml(&self, pdf_hash: &str, xhtml: &str) -> Result<()> {
        let path = self.xhtml_path(pdf_hash);
        fs::write(path, xhtml)?;
        Ok(())
    }

    // C2: PreprocessorOutput
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

    // C3: Graph output
    fn get_graph_output(
        &self,
        cache_key: &GraphCacheKey,
    ) -> Result<Option<crate::cache::GraphCacheValue>> {
        let path = self.graph_path(cache_key);
        if Path::new(&path).exists() {
            let json_str = fs::read_to_string(path)?;
            let cache_value: crate::cache::GraphCacheValue = serde_json::from_str(&json_str)
                .map_err(|e| anyhow!("Failed to deserialize cached GraphCacheValue: {}", e))?;
            Ok(Some(cache_value))
        } else {
            Ok(None)
        }
    }

    fn store_graph_output(
        &self,
        cache_key: &GraphCacheKey,
        cache_value: &crate::cache::GraphCacheValue,
    ) -> Result<()> {
        let path = self.graph_path(cache_key);
        let json_str = serde_json::to_string_pretty(cache_value)
            .map_err(|e| anyhow!("Failed to serialize GraphCacheValue: {}", e))?;
        fs::write(path, json_str)?;
        Ok(())
    }

    // Sidecar: per-stat analytics dump
    fn store_stat(&self, pdf_hash: &str, stat_name: &str, json: &str) -> Result<()> {
        let dir = format!("{}/stat/{}", self.cache_dir, stat_name);
        fs::create_dir_all(&dir)?;
        let path = format!("{dir}/{pdf_hash}.json");
        fs::write(path, json)?;
        Ok(())
    }

    // Cache management: cascading clear
    fn clear_cache(&self, from_point: Option<CachePoint>) -> Result<CacheClearResult> {
        let points_to_clear: Vec<CachePoint> = match from_point {
            Some(point) => point.cascade(),
            None => CachePoint::all().to_vec(), // "all"
        };

        let mut deleted = Vec::new();
        for point in points_to_clear {
            let count = self.clear_dir(point)?;
            if count > 0 {
                deleted.push((point, count));
            }
        }

        // Also clear debug/ when clearing all
        if from_point.is_none() {
            let debug_dir = format!("{}/debug", self.cache_dir);
            if Path::new(&debug_dir).exists() {
                let mut count = 0;
                for entry in fs::read_dir(&debug_dir)? {
                    let entry = entry?;
                    if entry.file_type()?.is_file() {
                        fs::remove_file(entry.path())?;
                        count += 1;
                    }
                }
                if count > 0 {
                    // Report as part of the output but not tied to a CachePoint
                    println!("   Deleted: {} files from debug/", count);
                }
            }
        }

        Ok(CacheClearResult { deleted })
    }
}

// =============================================================================
// Hash Functions
// =============================================================================

/// SHA-256 of the source file bytes. Stable per-file across parser
/// versions; the obvious content-identity choice.
///
/// Renamed from `calculate_pdf_hash` (CR-47): the hash is used for
/// non-PDF sources too (markdown, future DOCX), and the previous
/// partial-coverage implementation (file size + first 1KB + last 1KB)
/// had a small but real collision risk that gained us nothing —
/// full SHA-256 on a typical document is microseconds.
pub fn calculate_source_hash(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Calculate hash for configuration data (for C3 cache key)
pub fn calculate_config_hash<T: serde::Serialize>(config: &T) -> Result<String> {
    let config_json = serde_json::to_string(config)
        .map_err(|e| anyhow!("Failed to serialize config for hashing: {}", e))?;

    let mut hasher = Sha256::new();
    hasher.update(config_json.as_bytes());
    Ok(format!("{:x}", hasher.finalize()))
}

/// Calculate hash for XHTML content
pub fn calculate_xhtml_hash(xhtml: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(xhtml.as_bytes());
    format!("{:x}", hasher.finalize())
}

// =============================================================================
// No-Op Storage (disables all caching)
// =============================================================================

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
    fn get_pdf(&self, _hash: &str) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn store_pdf(&self, _hash: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }
    fn get_xhtml(&self, _pdf_hash: &str) -> Result<Option<String>> {
        Ok(None)
    }
    fn store_xhtml(&self, _pdf_hash: &str, _xhtml: &str) -> Result<()> {
        Ok(())
    }
    fn get_preprocessor_output(&self, _pdf_hash: &str) -> Result<Option<PreprocessorOutput>> {
        Ok(None)
    }
    fn store_preprocessor_output(
        &self,
        _pdf_hash: &str,
        _output: &PreprocessorOutput,
    ) -> Result<()> {
        Ok(())
    }
    fn get_graph_output(
        &self,
        _cache_key: &GraphCacheKey,
    ) -> Result<Option<crate::cache::GraphCacheValue>> {
        Ok(None)
    }
    fn store_graph_output(
        &self,
        _cache_key: &GraphCacheKey,
        _cache_value: &crate::cache::GraphCacheValue,
    ) -> Result<()> {
        Ok(())
    }
    fn store_stat(&self, _pdf_hash: &str, _stat_name: &str, _json: &str) -> Result<()> {
        Ok(())
    }
    fn clear_cache(&self, _from_point: Option<CachePoint>) -> Result<CacheClearResult> {
        Ok(CacheClearResult { deleted: vec![] })
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_hash_consistency() {
        let data = b"test source content with some data";
        let hash1 = calculate_source_hash(data);
        let hash2 = calculate_source_hash(data);
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_source_hash_uniqueness() {
        let a = b"test source content 1";
        let b = b"test source content 2";
        let hash1 = calculate_source_hash(a);
        let hash2 = calculate_source_hash(b);
        assert_ne!(hash1, hash2);
    }

    /// CR-47: the previous partial-hash implementation hashed only the
    /// first 1KB and last 1KB. Two files differing only in their middle
    /// bytes collided. The new full SHA-256 distinguishes them.
    #[test]
    fn test_source_hash_distinguishes_middles() {
        // 4 KB total: 1 KB head, 2 KB middle, 1 KB tail. Vary the
        // middle only; head and tail are byte-identical.
        let head = vec![0x42u8; 1024];
        let tail = vec![0x99u8; 1024];
        let middle_a = vec![0x01u8; 2048];
        let middle_b = vec![0x02u8; 2048];

        let mut a = Vec::with_capacity(4096);
        a.extend_from_slice(&head);
        a.extend_from_slice(&middle_a);
        a.extend_from_slice(&tail);

        let mut b = Vec::with_capacity(4096);
        b.extend_from_slice(&head);
        b.extend_from_slice(&middle_b);
        b.extend_from_slice(&tail);

        assert_eq!(a.len(), b.len(), "same length");
        assert_eq!(a[..1024], b[..1024], "same first 1 KB");
        assert_eq!(a[a.len() - 1024..], b[b.len() - 1024..], "same last 1 KB");
        assert_ne!(
            calculate_source_hash(&a),
            calculate_source_hash(&b),
            "different middles must produce different hashes"
        );
    }

    /// Cross-check: the new function is exactly SHA-256 of the input.
    #[test]
    fn test_source_hash_matches_plain_sha256() {
        let data = b"hello";
        let mut hasher = Sha256::new();
        hasher.update(data);
        let expected = format!("{:x}", hasher.finalize());
        assert_eq!(calculate_source_hash(data), expected);
    }

    #[test]
    fn test_file_storage_roundtrip() {
        let temp_dir = std::env::temp_dir().join("blazegraph_test_cache_cr11");
        let _ = std::fs::remove_dir_all(&temp_dir); // clean slate
        let storage = FileStorage::new(temp_dir.to_str().unwrap()).unwrap();

        let test_data = b"test pdf data";
        let hash = "test_hash";

        // C0: Store and retrieve PDF
        storage.store_pdf(hash, test_data).unwrap();
        let retrieved = storage.get_pdf(hash).unwrap();
        assert_eq!(retrieved, Some(test_data.to_vec()));

        // C1: Store and retrieve XHTML
        let xhtml = "<html><body>test</body></html>";
        storage.store_xhtml(hash, xhtml).unwrap();
        let retrieved_xhtml = storage.get_xhtml(hash).unwrap();
        assert_eq!(retrieved_xhtml, Some(xhtml.to_string()));

        // Verify .xhtml file extension
        let xhtml_path = format!("{}/c1-xhtml/{}.xhtml", temp_dir.display(), hash);
        assert!(Path::new(&xhtml_path).exists());

        // Clean up
        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[test]
    fn test_cache_point_ordering() {
        assert!(CachePoint::C0 < CachePoint::C1);
        assert!(CachePoint::C1 < CachePoint::C2);
        assert!(CachePoint::C2 < CachePoint::C3);
    }

    #[test]
    fn test_cache_point_cascade() {
        assert_eq!(
            CachePoint::C0.cascade(),
            vec![
                CachePoint::C0,
                CachePoint::C1,
                CachePoint::C2,
                CachePoint::C3
            ]
        );
        assert_eq!(
            CachePoint::C1.cascade(),
            vec![CachePoint::C1, CachePoint::C2, CachePoint::C3]
        );
        assert_eq!(
            CachePoint::C2.cascade(),
            vec![CachePoint::C2, CachePoint::C3]
        );
        assert_eq!(CachePoint::C3.cascade(), vec![CachePoint::C3]);
    }

    #[test]
    fn test_fresh_from_cache_bypass() {
        // FreshFrom::None uses all caches
        assert!(FreshFrom::None.should_use_cache(CachePoint::C0));
        assert!(FreshFrom::None.should_use_cache(CachePoint::C3));

        // FreshFrom::C0 skips everything
        assert!(!FreshFrom::C0.should_use_cache(CachePoint::C0));
        assert!(!FreshFrom::C0.should_use_cache(CachePoint::C3));

        // FreshFrom::C2 uses C0 and C1, skips C2 and C3
        assert!(FreshFrom::C2.should_use_cache(CachePoint::C0));
        assert!(FreshFrom::C2.should_use_cache(CachePoint::C1));
        assert!(!FreshFrom::C2.should_use_cache(CachePoint::C2));
        assert!(!FreshFrom::C2.should_use_cache(CachePoint::C3));
    }

    #[test]
    fn test_clear_cache_cascade() {
        let temp_dir = std::env::temp_dir().join("blazegraph_test_clear_cr11");
        let _ = std::fs::remove_dir_all(&temp_dir);
        let storage = FileStorage::new(temp_dir.to_str().unwrap()).unwrap();

        // Populate C1 and C2
        storage.store_xhtml("hash1", "<html>test</html>").unwrap();
        storage.store_xhtml("hash2", "<html>test2</html>").unwrap();

        // Clear from C1 should cascade to C2 and C3
        let result = storage.clear_cache(Some(CachePoint::C1)).unwrap();
        assert!(result
            .deleted
            .iter()
            .any(|(p, c)| *p == CachePoint::C1 && *c == 2));

        // Verify files are gone
        assert!(storage.get_xhtml("hash1").unwrap().is_none());
        assert!(storage.get_xhtml("hash2").unwrap().is_none());

        std::fs::remove_dir_all(temp_dir).ok();
    }

    #[test]
    fn test_cache_defaults() {
        let defaults = CacheDefaults::default();
        assert!(!defaults.should_write(CachePoint::C0));
        assert!(defaults.should_write(CachePoint::C1));
        assert!(defaults.should_write(CachePoint::C2));
        assert!(!defaults.should_write(CachePoint::C3));
    }
}
