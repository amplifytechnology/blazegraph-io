use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Version constants for cache invalidation.
pub mod versions {
    /// Graph-cache key component tracking the **code axis** of the
    /// version model (arch-15). CR-87 Option A: this is now
    /// [`crate::VERSION`] (the crate's `CARGO_PKG_VERSION`) rather than a
    /// hand-maintained literal that drifted (it was stuck at `0.1.1`
    /// while the crate moved to `0.2.2`, so output changes never
    /// invalidated cached graphs). Standing discipline: bump
    /// `crate::VERSION` whenever output changes (schema or formatting) —
    /// the cache then invalidates correctly, for free. The drift-guard
    /// test asserts `BLAZEGRAPH_VERSION == crate::VERSION`.
    pub const BLAZEGRAPH_VERSION: &str = crate::VERSION;
    pub const PROCESSING_VERSION: &str = "1.0.0";

    /// **Preprocessor-interface version** — the *preprocessor axis* of the
    /// version model (arch-15), a **debug** signal for *us* (not a
    /// consumer contract): it disambiguates whether output drift came
    /// from the blazegraph side or the preprocessor side of the
    /// preprocessor→graph interface. Tika is the current preprocessor;
    /// the const is named generically because the model admits others
    /// (bump it when a preprocessor's output/interface contract changes).
    /// Renamed from `TIKA_INTERFACE_VERSION` (CR-87) — the value is a
    /// Tika-era baseline, but the axis is preprocessor-general.
    pub const PREPROCESSOR_INTERFACE_VERSION: &str = "1.0.0";
}

/// Level 2 Cache Key (Config + XHTML → Graph)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct GraphCacheKey {
    pub xhtml_hash: String,
    pub config_hash: String,
    pub blazegraph_version: String,
    pub processing_version: String,
}

impl GraphCacheKey {
    pub fn new(xhtml_hash: String, config_hash: String) -> Self {
        Self {
            xhtml_hash,
            config_hash,
            blazegraph_version: versions::BLAZEGRAPH_VERSION.to_string(),
            processing_version: versions::PROCESSING_VERSION.to_string(),
        }
    }

    /// Compute cache key hash for storage
    pub fn to_cache_hash(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(&self.xhtml_hash);
        hasher.update(&self.config_hash);
        hasher.update(&self.blazegraph_version);
        hasher.update(&self.processing_version);
        format!("{:x}", hasher.finalize())
    }
}

/// Level 2 Cache Value (Graph with metadata)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCacheValue {
    pub graph: DocumentGraph,
    pub created_at: DateTime<Utc>,
    pub processing_time_ms: u64,
    pub cache_version: String,
}

impl GraphCacheValue {
    pub fn new(graph: DocumentGraph, processing_time_ms: u64) -> Self {
        Self {
            graph,
            created_at: Utc::now(),
            processing_time_ms,
            cache_version: versions::BLAZEGRAPH_VERSION.to_string(),
        }
    }
}
