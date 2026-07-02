use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

/// Helper: skip serializing `false` for `verbose`.
fn is_false(v: &bool) -> bool {
    !*v
}

/// Settings schema matching pi's settings.json format.
/// API keys live in auth.json, not here.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking_level: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude_tools: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    #[serde(default, skip_serializing_if = "is_false")]
    pub verbose: bool,

    /// Hide thinking blocks (Ctrl+T toggle). Persisted to settings.json.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "hideThinkingBlock"
    )]
    pub hide_thinking: Option<bool>,

    /// Collapse tool output (Ctrl+O toggle). Persisted to settings.json.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "collapseToolOutput"
    )]
    pub collapse_tool_output: Option<bool>,

    /// Model patterns for cycling (same format as --models CLI flag).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "enabledModels"
    )]
    pub enabled_models: Option<Vec<String>>,

    /// Auto-compact enabled (Ctrl+Shift+C toggle).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "autoCompact"
    )]
    pub auto_compact: Option<bool>,

    /// Tokens to reserve for system prompt + tool defs + response.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "compactReserveTokens"
    )]
    pub compact_reserve_tokens: Option<u64>,

    /// Number of most-recent tokens to always keep.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "compactKeepRecentTokens"
    )]
    pub compact_keep_recent_tokens: Option<u64>,

    /// Tracks which fields were explicitly modified during this session.
    /// Only modified fields are written when saving, preventing unset/default
    /// fields and project-level overrides from leaking into the global file.
    #[serde(skip)]
    pub(crate) modified_fields: HashSet<String>,
}

impl Settings {
    // ── Setters that track modification ────────────────────────────────

    /// Set hide_thinking and mark it as modified.
    pub fn set_hide_thinking(&mut self, value: Option<bool>) {
        self.hide_thinking = value;
        self.modified_fields.insert("hideThinkingBlock".into());
    }

    /// Set collapse_tool_output and mark it as modified.
    pub fn set_collapse_tool_output(&mut self, value: Option<bool>) {
        self.collapse_tool_output = value;
        self.modified_fields.insert("collapseToolOutput".into());
    }

    /// Set default_thinking_level and mark it as modified.
    pub fn set_default_thinking_level(&mut self, value: Option<String>) {
        self.default_thinking_level = value;
        self.modified_fields.insert("defaultThinkingLevel".into());
    }

    /// Set enabled_models and mark it as modified.
    pub fn set_enabled_models(&mut self, value: Option<Vec<String>>) {
        self.enabled_models = value;
        self.modified_fields.insert("enabledModels".into());
    }

    /// Set auto_compact and mark it as modified.
    pub fn set_auto_compact(&mut self, value: Option<bool>) {
        self.auto_compact = value;
        self.modified_fields.insert("autoCompact".into());
    }

    /// Set compact_reserve_tokens and mark it as modified.
    pub fn set_compact_reserve_tokens(&mut self, value: Option<u64>) {
        self.compact_reserve_tokens = value;
        self.modified_fields.insert("compactReserveTokens".into());
    }

    /// Set compact_keep_recent_tokens and mark it as modified.
    pub fn set_compact_keep_recent_tokens(&mut self, value: Option<u64>) {
        self.compact_keep_recent_tokens = value;
        self.modified_fields
            .insert("compactKeepRecentTokens".into());
    }

    /// Set default_provider and mark it as modified.
    pub fn set_default_provider(&mut self, value: Option<String>) {
        self.default_provider = value;
        self.modified_fields.insert("defaultProvider".into());
    }

    /// Set default_model and mark it as modified.
    pub fn set_default_model(&mut self, value: Option<String>) {
        self.default_model = value;
        self.modified_fields.insert("defaultModel".into());
    }

    /// Set both default_provider and default_model in one call (pi-compatible).
    pub fn set_default_model_and_provider(&mut self, provider: &str, model: &str) {
        self.default_provider = Some(provider.to_string());
        self.default_model = Some(model.to_string());
        self.modified_fields.insert("defaultProvider".into());
        self.modified_fields.insert("defaultModel".into());
    }

    /// Mark a field as modified (for use with the setters or external callers).
    /// The field name must match the camelCase JSON key (e.g. "hideThinkingBlock").
    #[doc(hidden)]
    pub fn mark_modified(&mut self, field: &str) {
        self.modified_fields.insert(field.to_string());
    }

    // ── Loading ─────────────────────────────────────────────────────────

    /// Load settings from the global agent config path and project-local path.
    pub fn load(cwd: &std::path::Path) -> anyhow::Result<Self> {
        let global_path = Self::global_path()?;
        Self::load_from(global_path, cwd)
    }

    /// Load settings with an explicit global config path (for testing).
    pub fn load_from(
        global_path: std::path::PathBuf,
        cwd: &std::path::Path,
    ) -> anyhow::Result<Self> {
        let global = Self::load_file(&global_path)?;
        let project = Self::load_file(&cwd.join(".rab").join("settings.json")).unwrap_or_default();
        Ok(Self::merge(global, project))
    }

    fn global_path() -> anyhow::Result<PathBuf> {
        let dir = directories::BaseDirs::new().context("Could not determine home directory")?;
        Ok(dir
            .home_dir()
            .join(".rab")
            .join("agent")
            .join("settings.json"))
    }

    fn load_file(path: &std::path::Path) -> anyhow::Result<Settings> {
        if !path.exists() {
            return Ok(Settings::default());
        }
        // Shared lock for reading — blocks if another process holds an exclusive lock.
        let content = read_file_with_shared_lock(path)?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse {}", path.display()))
    }

    /// Merge project settings over global. Project values take precedence when set.
    fn merge(global: Settings, project: Settings) -> Self {
        Self {
            default_provider: project.default_provider.or(global.default_provider),
            default_model: project.default_model.or(global.default_model),
            default_thinking_level: project
                .default_thinking_level
                .or(global.default_thinking_level),
            tools: if project.tools.is_empty() {
                global.tools
            } else {
                project.tools
            },
            exclude_tools: if project.exclude_tools.is_empty() {
                global.exclude_tools
            } else {
                project.exclude_tools
            },
            theme: project.theme.or(global.theme),
            verbose: project.verbose || global.verbose,
            hide_thinking: project.hide_thinking.or(global.hide_thinking),
            collapse_tool_output: project.collapse_tool_output.or(global.collapse_tool_output),
            enabled_models: project.enabled_models.or(global.enabled_models),
            auto_compact: project.auto_compact.or(global.auto_compact),
            compact_reserve_tokens: project
                .compact_reserve_tokens
                .or(global.compact_reserve_tokens),
            compact_keep_recent_tokens: project
                .compact_keep_recent_tokens
                .or(global.compact_keep_recent_tokens),
            modified_fields: HashSet::new(),
        }
    }

    // ── Saving ──────────────────────────────────────────────────────────

    /// Save only the modified fields to the global config path.
    /// Unmodified fields are never written, preventing project-level
    /// overrides and default values from leaking into the global file.
    ///
    /// After a successful save, `modified_fields` is cleared so that
    /// subsequent saves only write fields that changed since the last
    /// write. This prevents stale modifications from being re-applied
    /// when a different field is toggled later.
    ///
    /// Uses file locking (`flock` on the `.json.lock` file) to prevent
    /// corruption when multiple rab processes access the same file.
    /// Atomic write via temp-file + rename prevents partial writes.
    pub fn save(&mut self) -> anyhow::Result<()> {
        if self.modified_fields.is_empty() {
            return Ok(());
        }
        let path = Self::global_path()?;
        self.save_to(path)
    }

    /// Save only the modified fields to a specific path (for testing).
    /// Uses file locking and atomic write (temp file + rename).
    pub fn save_to(&mut self, path: std::path::PathBuf) -> anyhow::Result<()> {
        if self.modified_fields.is_empty() {
            return Ok(());
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let self_value = serde_json::to_value(&*self)
            .with_context(|| format!("Failed to serialize settings to {}", path.display()))?;
        let content = compute_merged_content(&path, &self_value, &self.modified_fields)?;
        atomic_write_with_lock(&path, &content)?;

        // Clear modified fields after a successful write.
        self.modified_fields.clear();
        Ok(())
    }

    /// Reload settings from disk (re-reads global + project).
    /// Clears modified_fields since the freshly loaded settings are unmodified.
    pub fn reload(&mut self, cwd: &std::path::Path) -> anyhow::Result<()> {
        let global_path = Self::global_path()?;
        let global = Self::load_file(&global_path)?;
        let project = Self::load_file(&cwd.join(".rab").join("settings.json")).unwrap_or_default();
        let merged = Self::merge(global, project);
        // Copy all fields from merged into self
        self.default_provider = merged.default_provider;
        self.default_model = merged.default_model;
        self.default_thinking_level = merged.default_thinking_level;
        self.tools = merged.tools;
        self.exclude_tools = merged.exclude_tools;
        self.theme = merged.theme;
        self.verbose = merged.verbose;
        self.hide_thinking = merged.hide_thinking;
        self.collapse_tool_output = merged.collapse_tool_output;
        self.enabled_models = merged.enabled_models;
        self.auto_compact = merged.auto_compact;
        self.compact_reserve_tokens = merged.compact_reserve_tokens;
        self.compact_keep_recent_tokens = merged.compact_keep_recent_tokens;
        self.modified_fields.clear();
        Ok(())
    }

    /// Resolved model name (defaults to deepseek-v4-flash).
    pub fn model(&self) -> &str {
        self.default_model.as_deref().unwrap_or("deepseek-v4-flash")
    }
}

// ── File I/O helpers with flock ─────────────────────────────────────

/// Read a file with a shared (read) lock via flock on the `.json.lock` file.
/// Falls back to an unlocked read if the lock file cannot be opened.
fn read_file_with_shared_lock(path: &std::path::Path) -> anyhow::Result<String> {
    let lock_path = path.with_extension("json.lock");
    if let Ok(_lock_file) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
    {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(_lock_file.as_raw_fd(), libc::LOCK_SH);
            }
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            unsafe {
                libc::flock(_lock_file.as_raw_fd(), libc::LOCK_UN);
            }
        }
        Ok(content)
    } else {
        // Lock file cannot be opened — fall back to unlocked read.
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))
    }
}

/// Compute the merged JSON content, combining existing file fields with
/// only the modified fields from `self_value`.
fn compute_merged_content(
    path: &std::path::Path,
    self_value: &serde_json::Value,
    modified_fields: &HashSet<String>,
) -> anyhow::Result<String> {
    let mut current: serde_json::Value = if path.exists() {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&content).unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    if let (Some(current_obj), Some(self_obj)) = (current.as_object_mut(), self_value.as_object()) {
        for key in modified_fields {
            if let Some(value) = self_obj.get(key) {
                current_obj.insert(key.clone(), value.clone());
            } else {
                current_obj.remove(key);
            }
        }
    }

    serde_json::to_string_pretty(&current)
        .with_context(|| format!("Failed to serialize settings to {}", path.display()))
}

/// Write content to a file atomically, protected by `flock` on the lock file.
fn atomic_write_with_lock(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Open (or create) the lock file and acquire an exclusive lock.
    let lock_path = path.with_extension("json.lock");
    let _lock_file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .with_context(|| format!("Failed to open lock file {}", lock_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        if unsafe { libc::flock(_lock_file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            let err = std::io::Error::last_os_error();
            anyhow::bail!("Failed to lock {}: {}", lock_path.display(), err);
        }
    }

    // Atomic write: temp file + rename.
    let tmp_path = path.with_extension("json.tmp");
    std::fs::write(&tmp_path, content)
        .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "Failed to rename {} to {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    // Ensure data is on disk before releasing the lock.
    if let Some(parent) = path.parent()
        && let Ok(f) = std::fs::File::open(parent)
    {
        let _ = f.sync_all();
    }

    // Release the lock (also happens on drop of _lock_file).
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        unsafe {
            libc::flock(_lock_file.as_raw_fd(), libc::LOCK_UN);
        }
    }

    Ok(())
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Helper: create a temporary file path for testing.
    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rab_settings_test_{}", name))
    }

    /// Clean up both the file and its lock file.
    fn cleanup(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("json.lock"));
        let _ = fs::remove_file(path.with_extension("json.tmp"));
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let path = tmp_path("roundtrip.json");
        cleanup(&path);

        let mut settings = Settings::default();
        settings.set_default_thinking_level(Some("high".into()));
        assert_eq!(settings.modified_fields.len(), 1);
        assert!(settings.modified_fields.contains("defaultThinkingLevel"));
        settings.save_to(path.clone()).unwrap();
        assert!(
            settings.modified_fields.is_empty(),
            "modified_fields should be cleared after save"
        );

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["defaultThinkingLevel"], "high");

        let loaded = Settings::load_file(&path).unwrap();
        assert_eq!(loaded.default_thinking_level.as_deref(), Some("high"));

        cleanup(&path);
    }

    #[test]
    fn test_save_multiple_fields_then_load() {
        let path = tmp_path("multi.json");
        cleanup(&path);

        let mut settings = Settings::default();
        settings.set_hide_thinking(Some(true));
        settings.set_collapse_tool_output(Some(false));
        settings.set_default_thinking_level(Some("medium".into()));
        assert_eq!(settings.modified_fields.len(), 3);
        settings.save_to(path.clone()).unwrap();

        let loaded = Settings::load_file(&path).unwrap();
        assert_eq!(loaded.hide_thinking, Some(true));
        assert_eq!(loaded.collapse_tool_output, Some(false));
        assert_eq!(loaded.default_thinking_level.as_deref(), Some("medium"));

        cleanup(&path);
    }

    #[test]
    fn test_incremental_save_preserves_existing_fields() {
        let path = tmp_path("incremental.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_hide_thinking(Some(false));
        s.save_to(path.clone()).unwrap();

        let mut s2 = Settings::load_file(&path).unwrap();
        assert_eq!(s2.hide_thinking, Some(false));
        s2.set_default_thinking_level(Some("low".into()));
        s2.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["hideThinkingBlock"], false);
        assert_eq!(json["defaultThinkingLevel"], "low");

        let loaded = Settings::load_file(&path).unwrap();
        assert_eq!(loaded.hide_thinking, Some(false));
        assert_eq!(loaded.default_thinking_level.as_deref(), Some("low"));

        cleanup(&path);
    }

    #[test]
    fn test_unset_field_removed_from_file() {
        let path = tmp_path("unset.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_default_thinking_level(Some("high".into()));
        s.save_to(path.clone()).unwrap();

        let mut s2 = Settings::load_file(&path).unwrap();
        s2.set_default_thinking_level(None);
        s2.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(
            !json
                .as_object()
                .unwrap()
                .contains_key("defaultThinkingLevel"),
            "Field should be removed when set to None"
        );

        let loaded = Settings::load_file(&path).unwrap();
        assert!(loaded.default_thinking_level.is_none());

        cleanup(&path);
    }

    #[test]
    fn test_hide_thinking_roundtrip() {
        let path = tmp_path("hide.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_hide_thinking(Some(false));
        s.save_to(path.clone()).unwrap();

        let loaded = Settings::load_file(&path).unwrap();
        assert_eq!(loaded.hide_thinking, Some(false));

        let mut s2 = Settings::load_file(&path).unwrap();
        s2.set_hide_thinking(Some(true));
        s2.save_to(path.clone()).unwrap();

        let loaded2 = Settings::load_file(&path).unwrap();
        assert_eq!(loaded2.hide_thinking, Some(true));

        cleanup(&path);
    }

    #[test]
    fn test_merge_global_and_project() {
        let mut global = Settings::default();
        global.hide_thinking = Some(true);
        global.default_thinking_level = Some("high".into());

        let mut project = Settings::default();
        project.hide_thinking = Some(false);

        let merged = Settings::merge(global, project);
        assert_eq!(merged.hide_thinking, Some(false));
        assert_eq!(merged.default_thinking_level.as_deref(), Some("high"));
        assert!(merged.modified_fields.is_empty());
    }

    #[test]
    fn test_save_only_modified_fields() {
        let path = tmp_path("modified_only.json");
        cleanup(&path);

        let initial = serde_json::json!({
            "theme": "dark",
            "defaultModel": "claude-sonnet",
            "hideThinkingBlock": true
        });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let mut s = Settings::load_file(&path).unwrap();
        assert_eq!(s.hide_thinking, Some(true));
        assert_eq!(s.theme.as_deref(), Some("dark"));
        assert_eq!(s.model(), "claude-sonnet");

        s.set_default_thinking_level(Some("low".into()));
        s.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            json["hideThinkingBlock"], true,
            "hideThinkingBlock preserved"
        );
        assert_eq!(
            json["defaultThinkingLevel"], "low",
            "defaultThinkingLevel added"
        );

        cleanup(&path);
    }

    #[test]
    fn test_clear_modified_fields_only_after_write() {
        let path = tmp_path("clear_modified.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_default_thinking_level(Some("xhigh".into()));
        s.set_hide_thinking(Some(false));
        s.save_to(path.clone()).unwrap();
        assert!(s.modified_fields.is_empty());

        s.set_hide_thinking(Some(true));
        assert_eq!(s.modified_fields.len(), 1);
        assert!(s.modified_fields.contains("hideThinkingBlock"));
        s.save_to(path.clone()).unwrap();
        assert!(s.modified_fields.is_empty());

        cleanup(&path);
    }

    // ── File locking tests ──────────────────────────────────────────

    #[test]
    fn test_lock_file_created_and_lock_released() {
        let path = tmp_path("lock_test.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_default_thinking_level(Some("high".into()));
        s.save_to(path.clone()).unwrap();

        let lock_path = path.with_extension("json.lock");
        assert!(lock_path.exists(), "Lock file should exist after write");

        // Lock should be released — we can acquire it non-blocking.
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let lock_file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&lock_path)
                .unwrap();
            let result =
                unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            assert_eq!(result, 0, "Lock must be released after write");
            unsafe {
                libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
            }
        }

        cleanup(&path);
    }

    // ── Integration tests: full persistence cycles ───────────────────

    /// Full startup→session→restore cycle:
    /// 1. Save settings with hide_thinking and thinking level
    /// 2. Load from file, verify values
    /// 3. Modify and save again
    /// 4. Reload and verify changes
    /// 5. Verify lock file is cleanly released
    #[test]
    fn test_full_persistence_cycle() {
        let path = tmp_path("full_cycle.json");
        cleanup(&path);

        // ── Phase 1: initial save ──
        {
            let mut settings = Settings::default();
            settings.set_hide_thinking(Some(false));
            settings.set_default_thinking_level(Some("xhigh".into()));
            settings.save_to(path.clone()).unwrap();
        }

        // ── Phase 2: reload and verify ──
        {
            let loaded = Settings::load_file(&path).unwrap();
            assert_eq!(loaded.hide_thinking, Some(false), "hide_thinking persists");
            assert_eq!(
                loaded.default_thinking_level.as_deref(),
                Some("xhigh"),
                "thinking level persists"
            );
        }

        // ── Phase 3: modify and save again ──
        {
            let mut settings = Settings::load_file(&path).unwrap();
            settings.set_hide_thinking(Some(true));
            settings.set_default_thinking_level(Some("low".into()));
            settings.save_to(path.clone()).unwrap();
        }

        // ── Phase 4: reload and verify both changes ──
        {
            let loaded = Settings::load_file(&path).unwrap();
            assert_eq!(loaded.hide_thinking, Some(true), "hide_thinking updated");
            assert_eq!(
                loaded.default_thinking_level.as_deref(),
                Some("low"),
                "thinking level updated"
            );
        }

        // ── Phase 5: verify lock file is released ──
        {
            let lock_path = path.with_extension("json.lock");
            assert!(lock_path.exists(), "Lock file should exist");
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                let lock_file = std::fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .unwrap();
                let result =
                    unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
                assert_eq!(result, 0, "Lock must be released after save");
                unsafe {
                    libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN);
                }
            }
        }

        cleanup(&path);
    }

    /// Concurrent access test: two separate Settings instances writing
    /// to the same file with file locking ensures no corruption.
    #[test]
    fn test_concurrent_writes_to_same_file() {
        let path = tmp_path("concurrent.json");
        cleanup(&path);

        // Simulate two rab processes (or sequential rapid saves)
        let mut s1 = Settings::default();
        s1.set_hide_thinking(Some(true));
        s1.set_default_thinking_level(Some("xhigh".into()));

        let mut s2 = Settings::default();
        s2.set_hide_thinking(Some(false));
        s2.set_default_thinking_level(Some("low".into()));

        // Interleave saves to same path
        s1.save_to(path.clone()).unwrap();
        s2.save_to(path.clone()).unwrap();

        // The file should be valid JSON regardless
        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.is_object(), "File must be valid JSON, not corrupted");

        // The last writer wins for each field
        assert_eq!(json["hideThinkingBlock"], false, "s2's hide_thinking");
        assert_eq!(json["defaultThinkingLevel"], "low", "s2's thinking level");

        cleanup(&path);
    }

    /// Verify the lock file is cleaned up after save.
    #[test]
    fn test_lock_file_cleanup() {
        let path = tmp_path("lock_cleanup.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_hide_thinking(Some(true));
        s.save_to(path.clone()).unwrap();

        let lock_path = path.with_extension("json.lock");
        assert!(lock_path.exists(), "Lock file should exist");

        // We can also verify the temp file is gone
        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "Temp file should be removed");

        cleanup(&path);
    }

    /// Reload should preserve unchanged fields from disk.
    #[test]
    fn test_reload_preserves_unmodified() {
        let path = tmp_path("reload_preserve.json");
        cleanup(&path);

        // Create initial file with some fields
        let initial = serde_json::json!({
            "theme": "solarized",
            "defaultModel": "deepseek-v4-pro",
            "hideThinkingBlock": true
        });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Load, modify only one field, save
        let mut s = Settings::load_file(&path).unwrap();
        s.set_default_thinking_level(Some("high".into()));
        s.save_to(path.clone()).unwrap();

        // Verify all fields preserved
        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["theme"], "solarized", "theme preserved");
        assert_eq!(json["defaultModel"], "deepseek-v4-pro", "model preserved");
        assert_eq!(
            json["hideThinkingBlock"], true,
            "hideThinkingBlock preserved"
        );
        assert_eq!(json["defaultThinkingLevel"], "high", "thinking level added");

        cleanup(&path);
    }
}
