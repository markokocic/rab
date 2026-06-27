//! MCP config loading — loads and merges global + project configs.

use crate::extensions::mcp::types::McpConfig;
use std::path::{Path, PathBuf};

/// Load MCP config, merging global `~/.rab/agent/mcp.json` with project `.rab/mcp.json`.
/// Project values take precedence over global ones (same merge pattern as Settings).
pub fn load_mcp_config(cwd: &Path) -> McpConfig {
    let global_path = global_config_path();
    let project_path = cwd.join(".rab").join("mcp.json");

    let mut config = load_file(&global_path).unwrap_or_default();
    let project = load_file(&project_path).unwrap_or_default();

    // Merge project over global
    for (name, entry) in project.mcp_servers {
        config.mcp_servers.insert(name, entry);
    }
    if let Some(settings) = project.settings {
        config.settings = Some(settings);
    }

    config
}

/// Load a single mcp.json file.
fn load_file(path: &Path) -> Option<McpConfig> {
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Path to the global MCP config file.
fn global_config_path() -> PathBuf {
    directories::BaseDirs::new()
        .map(|d| d.home_dir().join(".rab").join("agent").join("mcp.json"))
        .unwrap_or_else(|| PathBuf::from("/tmp/.rab/agent/mcp.json"))
}

/// Compute a hash of a server definition for cache invalidation.
pub fn compute_server_config_hash(entry: &crate::extensions::mcp::types::ServerEntry) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    if let Some(ref cmd) = entry.command {
        cmd.hash(&mut hasher);
    }
    entry.args.hash(&mut hasher);
    // Include env keys (not values) so config change detection works
    if let Some(ref env) = entry.env {
        let mut keys: Vec<&String> = env.keys().collect();
        keys.sort();
        for k in keys {
            k.hash(&mut hasher);
        }
    }
    entry.cwd.hash(&mut hasher);
    entry.url.hash(&mut hasher);
    hasher.finish()
}
