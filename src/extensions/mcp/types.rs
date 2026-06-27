//! MCP config types — matching pi-mcp-adapter's config schema.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Root MCP config matching pi's mcp.json format.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct McpConfig {
    #[serde(default)]
    pub mcp_servers: HashMap<String, ServerEntry>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settings: Option<McpSettings>,
}

/// A single MCP server definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerEntry {
    /// Command to spawn (e.g. "npx", "node").
    /// Optional for URL-based servers.
    #[serde(default)]
    pub command: Option<String>,

    #[serde(default)]
    pub args: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,

    /// Working directory for the server process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,

    /// Lifecycle mode: "keep-alive" | "lazy" | "eager".
    /// Default: "lazy"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<String>,

    /// Idle timeout in minutes (overrides global setting for this server).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout: Option<u64>,

    /// Enable direct tools for this server (register as individual LLM tools).
    /// true = all tools, string[] = only named tools, false = proxy-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direct_tools: Option<serde_json::Value>,

    /// Exclude specific MCP tools by original or prefixed name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_tools: Vec<String>,

    /// For HTTP-based servers: the URL endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,

    /// HTTP headers for URL-based servers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,
}

/// Global MCP settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSettings {
    /// Tool naming prefix mode: "server" (default), "none", or "short".
    #[serde(default = "default_tool_prefix")]
    pub tool_prefix: String,

    /// Idle timeout in minutes (default: 10, 0 = disable).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,

    /// Enable direct tools globally for all servers.
    #[serde(default)]
    pub direct_tools: bool,
}

fn default_tool_prefix() -> String {
    "server".to_string()
}

fn default_idle_timeout() -> u64 {
    10
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            tool_prefix: default_tool_prefix(),
            idle_timeout: default_idle_timeout(),
            direct_tools: false,
        }
    }
}

/// Format a tool name with server prefix.
pub fn format_tool_name(tool_name: &str, server_name: &str, prefix_mode: &str) -> String {
    match prefix_mode {
        "none" => tool_name.to_string(),
        "short" => {
            let short = server_name
                .trim_end_matches("mcp")
                .trim_end_matches("-mcp")
                .trim_end_matches('_');
            let p = short.replace('-', "_");
            if p.is_empty() {
                tool_name.to_string()
            } else {
                format!("{}_{}", p, tool_name)
            }
        }
        _ => {
            // "server" mode
            let p = server_name.replace('-', "_");
            format!("{}_{}", p, tool_name)
        }
    }
}

/// Cached tool metadata (persisted to disk for fast startup).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedTool {
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

/// Per-server cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCacheEntry {
    pub config_hash: u64,
    pub tools: Vec<CachedTool>,
    pub cached_at: u64,
}

/// Root metadata cache structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataCache {
    pub version: u32,
    pub servers: HashMap<String, ServerCacheEntry>,
}
