use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Version constants for cache invalidation
pub mod versions {
    pub const BLAZEGRAPH_VERSION: &str = "0.1.0";
    pub const PROCESSING_VERSION: &str = "1.0.0";
    pub const TIKA_INTERFACE_VERSION: &str = "1.0.0";
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