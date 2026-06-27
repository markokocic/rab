//! MCP (Model Context Protocol) extension.
//!
//! Provides:
//! - A unified `mcp` proxy tool (status, list, search, describe, connect, call, auth)
//! - Direct tool adapters for servers with `directTools` enabled
//! - Tool renderers for both proxy and direct MCP tools
//!
//! Mirrors pi-mcp-adapter's architecture but adapted to Rust/yoagent patterns.

mod cache;
mod config;
mod renderer;
pub mod server;
pub mod types;

use crate::agent::extension::{Extension, ToolWithMeta};
use cache::{has_valid_cache, load_cache, update_cache_entry};
use renderer::{McpProxyToolRenderer, McpToolRenderer};
use server::ServerManager;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;
use types::format_tool_name;
use yoagent::mcp::types::{McpContent, McpToolInfo};
use yoagent::types::{AgentTool, Content, ToolContext, ToolError, ToolResult};

// ── Re-exports for external use ────────────────────────────────────
pub use cache::{load_cache as load_metadata_cache, save_cache as save_metadata_cache};
pub use config::load_mcp_config;
pub use types::{CachedTool, McpConfig, McpSettings, MetadataCache, ServerCacheEntry, ServerEntry};

/// Maximum number of results returned by `mcp search`.
const MAX_SEARCH_RESULTS: usize = 30;

// ═══════════════════════════════════════════════════════════════════
// MCP Extension
// ═══════════════════════════════════════════════════════════════════

/// MCP Extension that provides MCP server management.
///
/// Provides:
/// - `mcp` proxy tool (gateway to all configured servers)
/// - Direct tools for servers with `directTools` enabled (optional)
/// - Tool renderers for MCP tool calls/results
pub struct McpExtension {
    config: McpConfig,
    manager: Arc<Mutex<ServerManager>>,
    /// Cached tool metadata by server name.
    tool_cache: Arc<Mutex<HashMap<String, Vec<McpToolInfo>>>>,
}

impl McpExtension {
    /// Create a new MCP extension from a loaded config.
    pub fn new(config: McpConfig) -> Self {
        let idle_timeout = config
            .settings
            .as_ref()
            .map(|s| s.idle_timeout)
            .unwrap_or(10);

        let mut manager = ServerManager::new(idle_timeout);

        // Register all servers from config
        for (name, entry) in &config.mcp_servers {
            let config_hash = config::compute_server_config_hash(entry);
            manager.register(name, entry.clone(), config_hash);
        }

        Self {
            config,
            manager: Arc::new(Mutex::new(manager)),
            tool_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create from config loaded from disk at the given working directory.
    pub fn from_cwd(cwd: &Path) -> Self {
        let config = load_mcp_config(cwd);
        Self::new(config)
    }

    /// Restore cached tool metadata from the on-disk cache.
    /// Should be called once at startup to prime the cache without connecting.
    pub async fn restore_cache(&self) {
        let cache = load_cache();
        let mut tool_cache = self.tool_cache.lock().await;
        for (server_name, entry) in &cache.servers {
            let def = self.config.mcp_servers.get(server_name);
            let ch = def.map(config::compute_server_config_hash).unwrap_or(0);
            if entry.config_hash != ch {
                continue;
            }
            let tools: Vec<McpToolInfo> = entry
                .tools
                .iter()
                .map(|t| McpToolInfo {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: if t.input_schema.is_null() {
                        serde_json::json!({"type": "object", "properties": {}})
                    } else {
                        t.input_schema.clone()
                    },
                })
                .collect();
            if !tools.is_empty() {
                tool_cache.insert(server_name.clone(), tools);
            }
        }
    }

    /// Bootstrap direct tools — checks which servers have directTools configured
    /// but no cached metadata yet, and logs a hint. Does NOT block startup on
    /// network connections. The first connection (via `mcp({{ connect: ... }})`
    /// or `mcp({{ server: ... }})`) populates the cache; on subsequent startups
    /// direct tools are available automatically.
    pub async fn bootstrap_direct_tools(&self) {
        let global_direct_tools = self
            .config
            .settings
            .as_ref()
            .is_some_and(|s| s.direct_tools);
        let missing_cache: Vec<String> = self
            .config
            .mcp_servers
            .iter()
            .filter(|(server_name, entry)| {
                let has_direct = match entry.direct_tools.as_ref() {
                    Some(v) if v.is_boolean() => v.as_bool().unwrap_or(false),
                    Some(v) if v.is_array() => true,
                    None => global_direct_tools,
                    Some(_) => false,
                };
                if !has_direct {
                    return false;
                }
                let config_hash = config::compute_server_config_hash(entry);
                !has_valid_cache(server_name, config_hash)
            })
            .map(|(name, _)| name.clone())
            .collect();

        if !missing_cache.is_empty() {
            eprintln!(
                "MCP: direct tools configured for {} but no cached metadata yet. \
                 Connect once via the mcp proxy tool, then restart.",
                missing_cache.join(", ")
            );
        }
    }
}

impl Extension for McpExtension {
    fn name(&self) -> Cow<'static, str> {
        "mcp".into()
    }

    fn tools(&self) -> Vec<ToolWithMeta> {
        let mut tools: Vec<ToolWithMeta> = Vec::new();

        // The proxy mcp tool is always available
        tools.push(ToolWithMeta {
            tool: Box::new(McpProxyTool {
                config: self.config.clone(),
                manager: self.manager.clone(),
                tool_cache: self.tool_cache.clone(),
            }),
            snippet: "MCP gateway - connect to MCP servers and call their tools. Non-MCP Pi tools should be called directly, not through mcp.",
            guidelines: &[
                "Use mcp to connect to external MCP tool servers",
                "Direct tools for configured servers can be called directly without mcp",
                "The proxy tool handles connect, list, search, describe, and call operations",
            ],
            prepare_arguments: None,
            before_tool_call: None,
            after_tool_call: None,
            renderer: Some(std::sync::Arc::new(McpProxyToolRenderer)),
        });

        // Add direct tools for servers with directTools enabled.
        // Per-server directTools takes precedence; falls back to global setting.
        let global_direct_tools = self
            .config
            .settings
            .as_ref()
            .is_some_and(|s| s.direct_tools);
        let cache = load_cache();
        let prefix_mode = self
            .config
            .settings
            .as_ref()
            .map(|s| s.tool_prefix.as_str())
            .unwrap_or("server");

        for (server_name, entry) in &self.config.mcp_servers {
            let direct = entry.direct_tools.as_ref();
            let has_direct = match direct {
                Some(v) if v.is_boolean() => v.as_bool().unwrap_or(false),
                Some(v) if v.is_array() => true,
                None => global_direct_tools,
                Some(_) => false,
            };

            if !has_direct {
                continue;
            }

            // Collect tool names for this server.
            // When directTools is an array, use those names directly (no cache needed).
            // When directTools is true, fall back to cached metadata.
            let tool_names: Vec<String> = match direct {
                Some(v) if v.is_array() => v
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
                _ => {
                    // Need cache to know tool names
                    let config_hash = config::compute_server_config_hash(entry);
                    if !has_valid_cache(server_name, config_hash) {
                        continue;
                    }
                    cache
                        .servers
                        .get(server_name)
                        .map(|s| &s.tools)
                        .into_iter()
                        .flatten()
                        .map(|ct| ct.name.clone())
                        .collect()
                }
            };

            if tool_names.is_empty() {
                continue;
            }

            // Look up cached metadata for descriptions/schemas if available
            let cached_tools: Vec<&CachedTool> = cache
                .servers
                .get(server_name)
                .map(|s| s.tools.iter().collect())
                .unwrap_or_default();
            for tool_name in &tool_names {
                let prefixed = format_tool_name(tool_name, server_name, prefix_mode);

                // Use cached metadata if available, otherwise provide defaults
                let (description, input_schema) = cached_tools
                    .iter()
                    .find(|ct| ct.name == *tool_name)
                    .map(|ct| {
                        let desc = ct
                            .description
                            .clone()
                            .unwrap_or_else(|| "MCP tool".to_string());
                        let schema = if ct.input_schema.is_null() {
                            serde_json::json!({"type": "object", "properties": {}})
                        } else {
                            ct.input_schema.clone()
                        };
                        (desc, schema)
                    })
                    .unwrap_or_else(|| {
                        (
                            format!("MCP tool: {} on {}", tool_name, server_name),
                            serde_json::json!({"type": "object", "properties": {}}),
                        )
                    });

                tools.push(ToolWithMeta {
                    tool: Box::new(McpDirectTool {
                        server_name: server_name.clone(),
                        original_name: tool_name.clone(),
                        display_name: prefixed.clone(),
                        description,
                        input_schema,
                        manager: self.manager.clone(),
                    }),
                    snippet: "MCP direct tool",
                    guidelines: &[],
                    prepare_arguments: None,
                    before_tool_call: None,
                    after_tool_call: None,
                    renderer: Some(std::sync::Arc::new(McpToolRenderer::new(&prefixed))),
                });
            }
        }

        tools
    }
}

// ═══════════════════════════════════════════════════════════════════
// Proxy `mcp` Tool
// ═══════════════════════════════════════════════════════════════════

/// The unified `mcp` proxy tool — a gateway to all MCP servers.
///
/// Supports operations:
/// - `{ }` — show status
/// - `{ server: "name" }` — list tools from server
/// - `{ tool: "name", args: '{"key": "val"}' }` — call a tool
/// - `{ connect: "server-name" }` — connect to a server
/// - `{ describe: "tool_name" }` — show tool details
/// - `{ search: "query" }` — search tools by name/description
/// - `{ action: "ui-messages" }` — retrieve UI session messages (stub)
/// - `{ action: "auth-start", server: "name" }` — start OAuth (stub)
/// - `{ action: "auth-complete", server: "name", args: '{"redirectUrl":"..."}' }` — complete OAuth (stub)
struct McpProxyTool {
    config: McpConfig,
    manager: Arc<Mutex<ServerManager>>,
    tool_cache: Arc<Mutex<HashMap<String, Vec<McpToolInfo>>>>,
}

impl McpProxyTool {
    /// Ensure a server is connected (lazy connect).
    async fn ensure_connected(&self, name: &str) -> bool {
        let mut manager = self.manager.lock().await;
        manager.ensure_connected(name).await
    }

    /// Cache the tools for a server after successful connection.
    async fn cache_tools(&self, server_name: &str) {
        let manager = self.manager.lock().await;
        let client = manager.get_client(server_name);
        drop(manager);

        if let Some(client) = client {
            let client = client.lock().await;
            if let Ok(tools) = client.list_tools().await {
                let config_hash = self
                    .config
                    .mcp_servers
                    .get(server_name)
                    .map(config::compute_server_config_hash)
                    .unwrap_or(0);

                let mut tool_cache = self.tool_cache.lock().await;
                tool_cache.insert(server_name.to_string(), tools.clone());
                drop(tool_cache);

                update_cache_entry(server_name, config_hash, &tools);
            }
        }
    }

    /// Call a tool on a connected server.
    async fn call_tool(
        &self,
        server_name: &str,
        tool_name: &str,
        args: serde_json::Value,
    ) -> Result<(Vec<Content>, bool), String> {
        let manager = self.manager.lock().await;
        let client = manager.get_client(server_name);
        drop(manager);

        let client = match client {
            Some(c) => c,
            None => return Err(format!("Server '{}' is not connected", server_name)),
        };

        let client = client.lock().await;
        let result = client
            .call_tool(tool_name, args)
            .await
            .map_err(|e| format!("MCP call failed: {}", e))?;

        let is_error = result.is_error;
        let content: Vec<Content> = result
            .content
            .into_iter()
            .map(|c| match c {
                McpContent::Text { text } => Content::Text { text },
                McpContent::Image { data, mime_type } => Content::Image { data, mime_type },
            })
            .collect();

        Ok((content, is_error))
    }

    /// Format search results as a text response.
    fn format_search_results(query: &str, matches: &[(String, McpToolInfo)]) -> String {
        let mut text = format!(
            "Found {} tool{} matching \"{}\":\n\n",
            matches.len(),
            if matches.len() == 1 { "" } else { "s" },
            query
        );

        for (server_name, tool) in matches {
            text.push_str(&format!(
                "{} @ {}\n  {}\n",
                tool.name,
                server_name,
                tool.description.as_deref().unwrap_or("(no description)")
            ));

            let schema = &tool.input_schema;
            if !schema.is_null()
                && schema.is_object()
                && let Some(props) = schema.get("properties").and_then(|p| p.as_object())
                && !props.is_empty()
            {
                let required: std::collections::HashSet<&str> = schema
                    .get("required")
                    .and_then(|r| r.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                    .unwrap_or_default();

                text.push_str("    Parameters:\n");
                for (prop_name, prop_schema) in props {
                    let is_req = required.contains(prop_name.as_str());
                    let type_str = prop_schema
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("any");
                    let desc = prop_schema
                        .get("description")
                        .and_then(|d| d.as_str())
                        .unwrap_or("");
                    text.push_str(&format!(
                        "    - {} ({}){} {}\n",
                        prop_name,
                        type_str,
                        if is_req { " *required*" } else { "" },
                        if desc.is_empty() {
                            String::new()
                        } else {
                            format!("- {}", desc)
                        }
                    ));
                }
            }
            text.push('\n');
        }

        text.trim().to_string()
    }

    /// Execute the status operation: list all configured servers and their status.
    async fn execute_status(&self) -> ToolResult {
        let manager = self.manager.lock().await;
        let tool_cache = self.tool_cache.lock().await;

        let mut lines = vec![format!(
            "MCP: {} servers configured",
            self.config.mcp_servers.len()
        )];
        lines.push(String::new());

        for name in self.config.mcp_servers.keys() {
            let status = manager.status(name);
            let tool_count = tool_cache.get(name).map(|v| v.len()).unwrap_or(0);
            let status_str = match status {
                Some(server::ConnectionStatus::Connected) => "✓ connected",
                Some(server::ConnectionStatus::Idle) => "○ idle",
                Some(server::ConnectionStatus::Failed) => "✗ failed",
                None => "○ not connected",
            };
            lines.push(format!("{} {} ({} tools)", status_str, name, tool_count));
        }

        if !self.config.mcp_servers.is_empty() {
            lines.push(String::new());
            lines.push(
                "mcp({ server: \"name\" }) to list tools, mcp({ search: \"...\" }) to search"
                    .to_string(),
            );
        }

        ToolResult {
            content: vec![Content::Text {
                text: lines.join("\n"),
            }],
            details: serde_json::json!({"mode": "status"}),
        }
    }

    /// Execute the list operation: list tools for a specific server.
    async fn execute_list(&self, server_name: &str) -> ToolResult {
        // Ensure connected
        let connected = {
            let mut manager = self.manager.lock().await;
            manager.ensure_connected(server_name).await
        };

        if connected {
            // Cache tools after connecting
            let manager = self.manager.lock().await;
            let client = manager.get_client(server_name);
            drop(manager);

            if let Some(client) = client {
                let client = client.lock().await;
                if let Ok(tools) = client.list_tools().await {
                    let config_hash = self
                        .config
                        .mcp_servers
                        .get(server_name)
                        .map(config::compute_server_config_hash)
                        .unwrap_or(0);

                    let mut tool_cache = self.tool_cache.lock().await;
                    tool_cache.insert(server_name.to_string(), tools.clone());
                    drop(tool_cache);

                    update_cache_entry(server_name, config_hash, &tools);
                }
            }
        }

        let tool_cache = self.tool_cache.lock().await;
        let tools = tool_cache.get(server_name);

        match tools {
            Some(tool_list) if !tool_list.is_empty() => {
                let mut lines = vec![format!("{} ({} tools):", server_name, tool_list.len())];
                lines.push(String::new());

                for tool in tool_list {
                    let desc = tool.description.as_deref().unwrap_or("");
                    let truncated = if desc.len() > 80 {
                        format!("{}...", &desc[..77])
                    } else {
                        desc.to_string()
                    };
                    lines.push(format!("- {}", tool.name));
                    if !truncated.is_empty() {
                        lines.push(format!("  {}", truncated));
                    }
                }

                ToolResult {
                    content: vec![Content::Text {
                        text: lines.join("\n"),
                    }],
                    details: serde_json::json!({"mode": "list", "server": server_name, "tools": tool_list.len()}),
                }
            }
            _ => {
                if self.config.mcp_servers.contains_key(server_name) {
                    ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "Server \"{}\" has no tools (or hasn't been connected yet). Use mcp({{ connect: \"{}\" }}) to connect.",
                                server_name, server_name
                            ),
                        }],
                        details: serde_json::json!({"mode": "list", "error": "no_tools", "server": server_name}),
                    }
                } else {
                    ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "Server \"{}\" not found. Use mcp({{}}) to see available servers.",
                                server_name
                            ),
                        }],
                        details: serde_json::json!({"mode": "list", "error": "not_found"}),
                    }
                }
            }
        }
    }

    /// Execute the search operation.
    async fn execute_search(
        &self,
        query: &str,
        regex: bool,
        filter_server: Option<&str>,
    ) -> ToolResult {
        let tool_cache = self.tool_cache.lock().await;
        let query_lower = query.to_lowercase();

        let matches: Vec<(String, McpToolInfo)> = tool_cache
            .iter()
            .filter(|(server_name, _)| filter_server.is_none_or(|s| server_name.as_str() == s))
            .flat_map(|(server_name, tools)| {
                let ql = query_lower.clone();
                tools.iter().filter_map(move |tool| {
                    let name_match = if regex {
                        regex::Regex::new(query)
                            .ok()
                            .is_some_and(|re| re.is_match(&tool.name))
                    } else {
                        tool.name.to_lowercase().contains(&ql)
                    };

                    let desc_match = tool.description.as_ref().is_some_and(|desc| {
                        if regex {
                            regex::Regex::new(query)
                                .ok()
                                .is_some_and(|re| re.is_match(desc))
                        } else {
                            desc.to_lowercase().contains(&ql)
                        }
                    });

                    if name_match || desc_match {
                        Some((server_name.clone(), tool.clone()))
                    } else {
                        None
                    }
                })
            })
            .take(MAX_SEARCH_RESULTS)
            .collect();

        drop(tool_cache);

        if matches.is_empty() {
            return ToolResult {
                content: vec![Content::Text {
                    text: format!("No tools matching \"{}\"", query),
                }],
                details: serde_json::json!({"mode": "search", "matches": [], "query": query}),
            };
        }

        let text = McpProxyTool::format_search_results(query, &matches);
        let count = matches.len();
        ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::json!({
                "mode": "search",
                "matches": matches.iter().map(|(s, t)| serde_json::json!({"server": s, "tool": t.name})).collect::<Vec<_>>(),
                "count": count,
                "query": query,
            }),
        }
    }

    /// Execute the describe operation.
    async fn execute_describe(&self, tool_name: &str) -> ToolResult {
        let tool_cache = self.tool_cache.lock().await;

        for (server_name, tools) in tool_cache.iter() {
            for tool in tools {
                if tool.name == tool_name {
                    let prefix = self
                        .config
                        .settings
                        .as_ref()
                        .map(|s| s.tool_prefix.as_str())
                        .unwrap_or("server");
                    let full_name = format_tool_name(&tool.name, server_name, prefix);

                    let mut lines = vec![
                        full_name,
                        format!("Server: {}", server_name),
                        String::new(),
                        tool.description
                            .clone()
                            .unwrap_or_else(|| "(no description)".to_string()),
                        String::new(),
                    ];

                    let schema = &tool.input_schema;
                    if !schema.is_null() && schema.is_object() {
                        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
                            if props.is_empty() {
                                lines.push("Parameters: (none)".to_string());
                            } else {
                                lines.push("Parameters:".to_string());
                                let required: std::collections::HashSet<&str> = schema
                                    .get("required")
                                    .and_then(|r| r.as_array())
                                    .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                                    .unwrap_or_default();

                                for (prop_name, prop_schema) in props {
                                    let type_str = prop_schema
                                        .get("type")
                                        .and_then(|t| t.as_str())
                                        .unwrap_or("any");
                                    let desc = prop_schema
                                        .get("description")
                                        .and_then(|d| d.as_str())
                                        .unwrap_or("");
                                    let req = if required.contains(prop_name.as_str()) {
                                        " *required*"
                                    } else {
                                        ""
                                    };
                                    lines.push(format!(
                                        "  - {} ({}){}{}",
                                        prop_name,
                                        type_str,
                                        req,
                                        if desc.is_empty() {
                                            String::new()
                                        } else {
                                            format!(" - {}", desc)
                                        }
                                    ));
                                }
                            }
                        } else {
                            lines.push("Parameters: (empty schema)".to_string());
                        }
                    } else {
                        lines.push("Parameters: (none)".to_string());
                    }

                    return ToolResult {
                        content: vec![Content::Text {
                            text: lines.join("\n"),
                        }],
                        details: serde_json::json!({
                            "mode": "describe",
                            "server": server_name,
                            "tool": tool_name,
                        }),
                    };
                }
            }
        }

        ToolResult {
            content: vec![Content::Text {
                text: format!(
                    "Tool \"{}\" not found. Use mcp({{ search: \"...\" }}) to find tools.",
                    tool_name
                ),
            }],
            details: serde_json::json!({"mode": "describe", "error": "not_found"}),
        }
    }

    /// Execute the connect operation.
    async fn execute_connect(&self, server_name: &str) -> ToolResult {
        if !self.config.mcp_servers.contains_key(server_name) {
            return ToolResult {
                content: vec![Content::Text {
                    text: format!(
                        "Server \"{}\" not found. Use mcp({{}}) to see available servers.",
                        server_name
                    ),
                }],
                details: serde_json::json!({"mode": "connect", "error": "not_found"}),
            };
        }

        let connected = self.ensure_connected(server_name).await;
        if connected {
            self.cache_tools(server_name).await;

            // Touch the server to mark it as recently used
            let mut manager = self.manager.lock().await;
            manager.touch(server_name);
            drop(manager);

            // List tools to show results
            self.execute_list(server_name).await
        } else {
            ToolResult {
                content: vec![Content::Text {
                    text: format!(
                        "Failed to connect to \"{}\". Check the server config.",
                        server_name
                    ),
                }],
                details: serde_json::json!({"mode": "connect", "error": "connect_failed", "server": server_name}),
            }
        }
    }

    /// Execute the tool call operation.
    async fn execute_call(
        &self,
        tool_name: &str,
        args_str: Option<&str>,
        server_override: Option<&str>,
    ) -> ToolResult {
        // Parse args JSON if provided
        let parsed_args: serde_json::Value = args_str
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(serde_json::json!({}));

        // Find the server and original tool name
        let prefix_mode = self
            .config
            .settings
            .as_ref()
            .map(|s| s.tool_prefix.as_str())
            .unwrap_or("server");

        let (server_name, original_name) = if let Some(srv) = server_override {
            // Server specified — lookup tool by original name
            (srv.to_string(), tool_name.to_string())
        } else {
            // No server — search all
            let tool_cache = self.tool_cache.lock().await;
            let mut found = None;
            for (srv, tools) in tool_cache.iter() {
                for tool in tools {
                    let prefixed = format_tool_name(&tool.name, srv, prefix_mode);
                    if prefixed == tool_name || tool.name == tool_name {
                        found = Some((srv.clone(), tool.name.clone()));
                        break;
                    }
                }
                if found.is_some() {
                    break;
                }
            }
            match found {
                Some(f) => f,
                None => {
                    return ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "Tool \"{}\" not found. Use mcp({{ search: \"...\" }}) to find tools.",
                                tool_name
                            ),
                        }],
                        details: serde_json::json!({"mode": "call", "error": "tool_not_found"}),
                    };
                }
            }
        };

        // Ensure connected
        if !self.ensure_connected(&server_name).await {
            return ToolResult {
                content: vec![Content::Text {
                    text: format!(
                        "Server \"{}\" is not available. Use mcp({{ connect: \"{}\" }}) to connect.",
                        server_name, server_name
                    ),
                }],
                details: serde_json::json!({"mode": "call", "error": "server_unavailable"}),
            };
        }

        // Touch the server
        {
            let mut manager = self.manager.lock().await;
            manager.touch(&server_name);
        }

        // Call the tool
        match self
            .call_tool(&server_name, &original_name, parsed_args)
            .await
        {
            Ok((content, is_error)) => {
                let text: String = content
                    .iter()
                    .filter_map(|c| {
                        if let Content::Text { text } = c {
                            Some(text.as_str())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");

                if is_error {
                    ToolResult {
                        content: vec![Content::Text {
                            text: format!("Error: {}", text),
                        }],
                        details: serde_json::json!({"mode": "call", "error": "tool_error", "server": server_name}),
                    }
                } else {
                    ToolResult {
                        content: vec![Content::Text { text }],
                        details: serde_json::json!({"mode": "call", "server": server_name, "tool": original_name}),
                    }
                }
            }
            Err(e) => ToolResult {
                content: vec![Content::Text {
                    text: format!("Failed to call tool: {}", e),
                }],
                details: serde_json::json!({"mode": "call", "error": "call_failed", "server": server_name}),
            },
        }
    }
}

#[async_trait::async_trait]
impl AgentTool for McpProxyTool {
    fn name(&self) -> &str {
        "mcp"
    }

    fn label(&self) -> &str {
        "mcp"
    }

    fn description(&self) -> &str {
        "MCP gateway - connect to MCP servers and call their tools. Non-MCP Pi tools should be called directly, not through mcp.\n\n\
         Direct tools available (call as normal tools): varies by configuration\n\n\
         Servers: varies by configuration\n\n\
         Usage:\n\
           mcp({ })                              → Show server status\n\
           mcp({ server: \"name\" })               → List tools from server\n\
           mcp({ search: \"query\" })              → Search MCP tools by name/description\n\
           mcp({ describe: \"tool_name\" })        → Show tool details and parameters\n\
           mcp({ connect: \"server-name\" })       → Connect to a server and refresh metadata\n\
           mcp({ tool: \"name\", args: '{\"key\": \"value\"}' })    → Call a tool (args is JSON string)\n\
           mcp({ action: \"ui-messages\" })        → Retrieve accumulated messages from completed UI sessions\n\
           mcp({ action: \"auth-start\", server: \"name\" })      → Start manual OAuth and get a browser URL\n\
           mcp({ action: \"auth-complete\", server: \"name\", args: '{\"redirectUrl\":\"...\"}' }) → Complete OAuth\n\n\
         Mode: action > tool (call) > connect > describe > search > server (list) > nothing (status)"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Server name for listing tools"
                },
                "tool": {
                    "type": "string",
                    "description": "Tool name to call"
                },
                "args": {
                    "type": "string",
                    "description": "JSON string of arguments for the tool call"
                },
                "connect": {
                    "type": "string",
                    "description": "Server name to connect to"
                },
                "describe": {
                    "type": "string",
                    "description": "Tool name to describe"
                },
                "search": {
                    "type": "string",
                    "description": "Search query for finding tools"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Treat search query as regex (default: false)"
                },
                "includeSchemas": {
                    "type": "boolean",
                    "description": "Include parameter schemas in search results (default: true)"
                },
                "action": {
                    "type": "string",
                    "description": "Action: 'ui-messages', 'auth-start', or 'auth-complete'"
                }
            }
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Determine mode: action > tool (call) > connect > describe > search > server (list) > nothing (status)
        if let Some(action) = params.get("action").and_then(|v| v.as_str()) {
            match action {
                "ui-messages" => {
                    return Ok(ToolResult {
                        content: vec![Content::Text {
                            text: "No UI session messages available. (UI sessions not yet implemented in this version.)".to_string(),
                        }],
                        details: serde_json::json!({"mode": "ui-messages", "sessions": 0}),
                    });
                }
                "auth-start" => {
                    let server_name = params.get("server").and_then(|v| v.as_str()).unwrap_or("");
                    if server_name.is_empty() {
                        return Err(ToolError::InvalidArgs(
                            "Missing 'server' argument for auth-start action".into(),
                        ));
                    }
                    return Ok(ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "OAuth authentication for \"{}\" is not yet implemented in this version. \
                                 Please start the server manually and configure authentication.",
                                server_name
                            ),
                        }],
                        details: serde_json::json!({"mode": "auth-start", "error": "not_implemented"}),
                    });
                }
                "auth-complete" => {
                    let server_name = params.get("server").and_then(|v| v.as_str()).unwrap_or("");
                    if server_name.is_empty() {
                        return Err(ToolError::InvalidArgs(
                            "Missing 'server' argument for auth-complete action".into(),
                        ));
                    }
                    return Ok(ToolResult {
                        content: vec![Content::Text {
                            text: format!(
                                "OAuth completion for \"{}\" is not yet implemented in this version.",
                                server_name
                            ),
                        }],
                        details: serde_json::json!({"mode": "auth-complete", "error": "not_implemented"}),
                    });
                }
                _ => {
                    return Err(ToolError::InvalidArgs(format!(
                        "Unknown action '{}'. Supported: ui-messages, auth-start, auth-complete",
                        action
                    )));
                }
            }
        }

        if let Some(tool_name) = params.get("tool").and_then(|v| v.as_str()) {
            let args_str = params.get("args").and_then(|v| v.as_str());
            let server_override = params.get("server").and_then(|v| v.as_str());
            return Ok(self
                .execute_call(tool_name, args_str, server_override)
                .await);
        }

        if let Some(server_name) = params.get("connect").and_then(|v| v.as_str()) {
            return Ok(self.execute_connect(server_name).await);
        }

        if let Some(tool_name) = params.get("describe").and_then(|v| v.as_str()) {
            return Ok(self.execute_describe(tool_name).await);
        }

        if let Some(query) = params.get("search").and_then(|v| v.as_str()) {
            let regex = params
                .get("regex")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let server_filter = params.get("server").and_then(|v| v.as_str());
            return Ok(self.execute_search(query, regex, server_filter).await);
        }

        if let Some(server_name) = params.get("server").and_then(|v| v.as_str()) {
            return Ok(self.execute_list(server_name).await);
        }

        // Default: status
        Ok(self.execute_status().await)
    }
}

// ═══════════════════════════════════════════════════════════════════
// Direct Tool Adapter
// ═══════════════════════════════════════════════════════════════════

/// A direct MCP tool adapter that wraps a remote MCP server tool.
struct McpDirectTool {
    server_name: String,
    original_name: String,
    display_name: String,
    description: String,
    input_schema: serde_json::Value,
    manager: Arc<Mutex<ServerManager>>,
}

#[async_trait::async_trait]
impl AgentTool for McpDirectTool {
    fn name(&self) -> &str {
        &self.display_name
    }

    fn label(&self) -> &str {
        &self.display_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.input_schema.clone()
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let mut manager = self.manager.lock().await;

        // Ensure connected
        if !manager.ensure_connected(&self.server_name).await {
            return Err(ToolError::Failed(format!(
                "Server '{}' is not available",
                self.server_name
            )));
        }

        manager.touch(&self.server_name);
        let client = manager.get_client(&self.server_name).ok_or_else(|| {
            ToolError::Failed(format!("Server '{}' has no client", self.server_name))
        })?;

        drop(manager);

        let client = client.lock().await;
        let result = client
            .call_tool(&self.original_name, params)
            .await
            .map_err(|e| ToolError::Failed(format!("MCP call failed: {}", e)))?;

        if result.is_error {
            let error_text = result
                .content
                .iter()
                .filter_map(|c| match c {
                    McpContent::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            return Err(ToolError::Failed(error_text));
        }

        let content: Vec<Content> = result
            .content
            .into_iter()
            .map(|c| match c {
                McpContent::Text { text } => Content::Text { text },
                McpContent::Image { data, mime_type } => Content::Image { data, mime_type },
            })
            .collect();

        Ok(ToolResult {
            content,
            details: serde_json::Value::Null,
        })
    }
}
