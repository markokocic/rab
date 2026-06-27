//! Persistent metadata cache — caches MCP tool definitions to disk for fast startup.
//! Mirrors pi-mcp-adapter's mcp-cache.json pattern.

use crate::extensions::mcp::types::{CachedTool, MetadataCache, ServerCacheEntry};
use std::collections::HashMap;
use std::path::PathBuf;

const CACHE_VERSION: u32 = 1;

/// Get the cache file path.
fn cache_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| {
            d.home_dir()
                .join(".rab")
                .join("agent")
                .join("mcp-cache.json")
        })
        .unwrap_or_else(|| PathBuf::from("/tmp/.rab/agent/mcp-cache.json"))
}

/// Load the metadata cache from disk.
pub fn load_cache() -> MetadataCache {
    let path = cache_path();
    if !path.exists() {
        return MetadataCache {
            version: CACHE_VERSION,
            servers: HashMap::new(),
        };
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            return MetadataCache {
                version: CACHE_VERSION,
                servers: HashMap::new(),
            };
        }
    };
    let cache: MetadataCache = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(_) => MetadataCache {
            version: CACHE_VERSION,
            servers: HashMap::new(),
        },
    };
    // Ignore old version caches
    if cache.version != CACHE_VERSION {
        return MetadataCache {
            version: CACHE_VERSION,
            servers: HashMap::new(),
        };
    }
    cache
}

/// Save the metadata cache to disk.
pub fn save_cache(cache: &MetadataCache) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string(cache) {
        let _ = std::fs::write(&path, &content);
    }
}

/// Update cache entry for a specific server after successful connection.
pub fn update_cache_entry(
    server_name: &str,
    config_hash: u64,
    yo_tools: &[yoagent::mcp::types::McpToolInfo],
) {
    let mut cache = load_cache();
    let tools: Vec<CachedTool> = yo_tools
        .iter()
        .map(|t| CachedTool {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: if t.input_schema.is_null() {
                serde_json::json!({"type": "object", "properties": {}})
            } else {
                t.input_schema.clone()
            },
        })
        .collect();

    cache.servers.insert(
        server_name.to_string(),
        ServerCacheEntry {
            config_hash,
            tools,
            cached_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        },
    );
    save_cache(&cache);
}

/// Check if a valid cache entry exists for a server.
pub fn has_valid_cache(server_name: &str, config_hash: u64) -> bool {
    let cache = load_cache();
    cache
        .servers
        .get(server_name)
        .is_some_and(|e| e.config_hash == config_hash)
}
