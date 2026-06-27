//! Server lifecycle manager — lazy connection, idle timeout, keep-alive.
//! Mirrors pi-mcp-adapter's McpLifecycleManager + McpServerManager pattern.

use crate::extensions::mcp::types::ServerEntry;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Instant;
use tokio::sync::Mutex;
use yoagent::mcp::McpClient;
use yoagent::mcp::McpTransport;
use yoagent::mcp::types::*;

// ---------------------------------------------------------------------------
// SSE-aware HTTP transport — handles servers that return SSE events (e.g. exa)
// instead of plain JSON-RPC responses. Falls back to direct JSON parsing
// for servers that return plain JSON-RPC.
// ---------------------------------------------------------------------------

/// HTTP transport that handles both SSE (Server-Sent Events) and direct JSON-RPC responses.
///
/// Modern MCP servers (exa, etc.) return SSE events like:
/// ```text
/// event: message
/// data: {"jsonrpc":"2.0","result":{...},"id":1}
///
/// ```
/// This transport parses those events and extracts the JSON-RPC response.
struct SseHttpTransport {
    client: reqwest::Client,
    base_url: String,
    headers: Vec<(String, String)>,
    /// Session ID returned by the server (Streamable HTTP).
    session_id: StdMutex<Option<String>>,
}

impl SseHttpTransport {
    fn new(url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url: url.trim_end_matches('/').to_string(),
            headers: Vec::new(),
            session_id: StdMutex::new(None),
        }
    }

    fn with_headers(mut self, headers: Option<&std::collections::HashMap<String, String>>) -> Self {
        if let Some(h) = headers {
            for (k, v) in h {
                self.headers.push((k.clone(), v.clone()));
            }
        }
        self
    }

    /// Parse an SSE response body to extract JSON-RPC responses.
    fn parse_sse_response(body: &str) -> Result<JsonRpcResponse, McpError> {
        // Try direct JSON parse first (for old-style HTTP transport)
        if let Ok(r) = serde_json::from_str::<JsonRpcResponse>(body) {
            return Ok(r);
        }

        // SSE format: split by double newlines, look for `data:` lines
        for event in body.split("\n\n") {
            let event = event.trim();
            if event.is_empty() {
                continue;
            }
            // Find the data line
            for line in event.lines() {
                if let Some(data) = line
                    .strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
                {
                    let data = data.trim();
                    if data.starts_with('{')
                        && let Ok(r) = serde_json::from_str::<JsonRpcResponse>(data)
                    {
                        return Ok(r);
                    }
                }
            }
        }

        Err(McpError::Transport(format!(
            "Cannot parse SSE response: {}",
            body.chars().take(200).collect::<String>()
        )))
    }
}

#[async_trait]
impl McpTransport for SseHttpTransport {
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let mut req = self
            .client
            .post(&self.base_url)
            // Streamable HTTP requires the client to accept both formats
            .header("Accept", "application/json, text/event-stream")
            .json(&request);

        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }

        // Include session ID if we have one (Streamable HTTP)
        if let Ok(guard) = self.session_id.lock()
            && let Some(ref sid) = *guard
        {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("HTTP error: {}", e)))?;

        let status = resp.status();

        // Capture session ID from response headers (Streamable HTTP)
        // reqwest normalizes header names to lowercase
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
            && let Ok(mut guard) = self.session_id.lock()
            && guard.is_none()
        {
            *guard = Some(sid.to_string());
        }

        let body = resp
            .text()
            .await
            .map_err(|e| McpError::Transport(format!("Failed to read response: {}", e)))?;

        if status.is_success() || status == 202 {
            Self::parse_sse_response(&body)
        } else {
            Err(McpError::Transport(format!(
                "HTTP {} from server: {}",
                status,
                body.chars().take(200).collect::<String>()
            )))
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

/// Connection status for a server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Successfully connected and ready.
    Connected,
    /// Disconnected after idle timeout.
    Idle,
    /// Connection failed or server unreachable.
    Failed,
}

/// A managed server connection.
struct ServerConnection {
    entry: ServerEntry,
    client: Option<Arc<Mutex<McpClient>>>,
    status: ConnectionStatus,
    last_used: Instant,
    last_failure: Option<Instant>,
    config_hash: u64,
}

/// Manages all MCP server connections with lazy connection, idle timeout, and health checks.
pub struct ServerManager {
    servers: HashMap<String, ServerConnection>,
    global_idle_timeout: std::time::Duration,
}

impl ServerManager {
    pub fn new(global_idle_timeout_minutes: u64) -> Self {
        Self {
            servers: HashMap::new(),
            global_idle_timeout: std::time::Duration::from_secs(global_idle_timeout_minutes * 60),
        }
    }

    /// Register a server definition (from config). Does not connect.
    pub fn register(&mut self, name: &str, entry: ServerEntry, config_hash: u64) {
        self.servers
            .entry(name.to_string())
            .or_insert_with(|| ServerConnection {
                entry,
                client: None,
                status: ConnectionStatus::Idle,
                last_used: Instant::now(),
                last_failure: None,
                config_hash,
            });
    }

    /// Ensure a server is connected (lazy connect). Returns true if connected/available.
    pub async fn ensure_connected(&mut self, name: &str) -> bool {
        // Check if we have a cached connection that's still alive
        if let Some(conn) = self.servers.get(name)
            && conn.status == ConnectionStatus::Connected
            && conn.client.is_some()
        {
            // Touch last_used so idle timer resets
            if let Some(c) = self.servers.get_mut(name) {
                c.last_used = Instant::now();
            }
            return true;
        }

        // Need to connect
        let entry = match self.servers.get(name) {
            Some(e) => e.entry.clone(),
            None => return false,
        };

        let client = match &entry.url {
            Some(url) => {
                // Use SSE-aware HTTP transport instead of the plain yoagent one
                let transport =
                    Box::new(SseHttpTransport::new(url).with_headers(entry.headers.as_ref()));
                let mut c = McpClient::from_transport(transport);
                c.initialize().await.map(|_| c)
            }
            None => {
                let env = entry.env.as_ref().cloned();
                let cmd = entry.command.as_deref().unwrap_or("npx");
                McpClient::connect_stdio(cmd, &to_str_slice(&entry.args), env).await
            }
        };

        match client {
            Ok(c) => {
                let c = Arc::new(Mutex::new(c));
                if let Some(conn) = self.servers.get_mut(name) {
                    conn.client = Some(c);
                    conn.status = ConnectionStatus::Connected;
                    conn.last_used = Instant::now();
                    conn.last_failure = None;
                }
                true
            }
            Err(e) => {
                eprintln!("MCP: failed to connect to '{}': {}", name, e);
                if let Some(conn) = self.servers.get_mut(name) {
                    conn.status = ConnectionStatus::Failed;
                    conn.last_failure = Some(Instant::now());
                    conn.client = None;
                }
                false
            }
        }
    }

    /// Get a connected client for a server (must call ensure_connected first).
    pub fn get_client(&self, name: &str) -> Option<Arc<Mutex<McpClient>>> {
        self.servers.get(name).and_then(|c| c.client.clone())
    }

    /// Get the connection status for a server.
    pub fn status(&self, name: &str) -> Option<ConnectionStatus> {
        self.servers.get(name).map(|c| c.status.clone())
    }

    /// Mark a connection as failed after a tool call error.
    pub fn mark_failed(&mut self, name: &str) {
        if let Some(conn) = self.servers.get_mut(name) {
            conn.status = ConnectionStatus::Failed;
            conn.last_failure = Some(Instant::now());
            conn.client = None;
        }
    }

    /// Touch a server (update last_used timestamp, e.g. after successful tool call).
    pub fn touch(&mut self, name: &str) {
        if let Some(conn) = self.servers.get_mut(name) {
            conn.last_used = Instant::now();
            if conn.status == ConnectionStatus::Failed && conn.last_failure.is_some() {
                let backoff = std::time::Duration::from_secs(60);
                if conn.last_failure.unwrap().elapsed() > backoff {
                    conn.status = ConnectionStatus::Idle;
                    conn.last_failure = None;
                }
            }
        }
    }

    /// Disconnect a server (idle shutdown).
    pub async fn disconnect(&mut self, name: &str) {
        if let Some(conn) = self.servers.get_mut(name) {
            if let Some(ref client) = conn.client {
                let _ = client.lock().await.close().await;
            }
            conn.client = None;
            conn.status = ConnectionStatus::Idle;
        }
    }

    /// Close all connections (on session shutdown).
    pub async fn close_all(&mut self) {
        let names: Vec<String> = self.servers.keys().cloned().collect();
        for name in &names {
            self.disconnect(name).await;
        }
    }

    /// Get the idle timeout for a server (per-server override or global default).
    pub fn idle_timeout(&self, name: &str) -> std::time::Duration {
        if let Some(conn) = self.servers.get(name) {
            idle_timeout_for(conn, self.global_idle_timeout)
        } else {
            self.global_idle_timeout
        }
    }

    /// Check for idle servers and disconnect them.
    pub async fn sweep_idle(&mut self) {
        let now = Instant::now();
        let idle_names: Vec<String> = self
            .servers
            .iter()
            .filter(|(_name, conn)| {
                if conn.status != ConnectionStatus::Connected {
                    return false;
                }
                let timeout = idle_timeout_for(conn, self.global_idle_timeout);
                now.duration_since(conn.last_used) > timeout
            })
            .map(|(name, _)| name.clone())
            .collect();

        for name in &idle_names {
            self.disconnect(name).await;
        }
    }

    /// Get a list of all registered server names.
    pub fn server_names(&self) -> Vec<String> {
        self.servers.keys().cloned().collect()
    }

    /// Check if a server should be connected eagerly at startup.
    pub fn should_connect_eagerly(&self, name: &str) -> bool {
        self.servers
            .get(name)
            .is_some_and(|c| matches!(c.entry.lifecycle.as_deref(), Some("eager" | "keep-alive")))
    }

    /// Get the config hash for a server.
    pub fn config_hash(&self, name: &str) -> Option<u64> {
        self.servers.get(name).map(|c| c.config_hash)
    }
}

fn to_str_slice(args: &[String]) -> Vec<&str> {
    args.iter().map(|s| s.as_str()).collect()
}

/// Compute idle timeout for a server connection.
fn idle_timeout_for(conn: &ServerConnection, global: std::time::Duration) -> std::time::Duration {
    if let Some(t) = conn.entry.idle_timeout {
        return std::time::Duration::from_secs(t * 60);
    }
    // keep-alive servers have no idle timeout
    if conn.entry.lifecycle.as_deref() == Some("keep-alive") {
        return std::time::Duration::MAX;
    }
    global
}
