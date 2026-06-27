//! Server lifecycle manager — lazy connection, idle timeout, keep-alive.
//! Mirrors pi-mcp-adapter's McpLifecycleManager + McpServerManager pattern.

use crate::extensions::mcp::types::ServerEntry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use yoagent::mcp::McpClient;

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
            Some(url) => McpClient::connect_http(url).await,
            None => {
                let env = entry.env.as_ref().cloned();
                McpClient::connect_stdio(&entry.command, &to_str_slice(&entry.args), env).await
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
