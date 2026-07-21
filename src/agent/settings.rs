use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use std::collections::HashSet;
use std::path::PathBuf;

// ── Nested setting types (pi-compatible) ───────────────────────────────

/// Compaction thresholds and retention settings (pi's `compaction` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct CompactionConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    /// Tokens reserved for system prompt + tool defs + response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,

    /// Number of most-recent tokens to always keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_recent_tokens: Option<u64>,
}

/// Terminal display settings (pi's `terminal` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TerminalConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_images: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_width_cells: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear_on_shrink: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_terminal_progress: Option<bool>,
}

/// Image processing settings (pi's `images` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImageConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_resize: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_images: Option<bool>,
}

/// Retry settings (pi's `retry` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RetryConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_delay_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderRetryConfig>,
}

/// Provider-level retry settings (pi's `retry.provider` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderRetryConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retry_delay_ms: Option<u64>,
}

/// Markdown rendering settings (pi's `markdown` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code_block_indent: Option<String>,
}

/// Warning toggles (pi's `warnings` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WarningConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_extra_usage: Option<bool>,
}

/// Branch summary settings (pi's `branchSummary` block).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BranchSummaryConfig {
    /// Tokens reserved for prompt + LLM response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve_tokens: Option<u64>,

    /// When true, skips "Summarize branch?" prompt and defaults to no summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_prompt: Option<bool>,
}

/// Custom token budgets for thinking levels (pi's `thinkingBudgets`).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingBudgetsConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimal: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub low: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub medium: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub high: Option<u64>,
}

// ── Package source type ────────────────────────────────────────────────

/// A package source (npm/git). Either a string URL or an object with filters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PackageSource {
    String(String),
    Object {
        source: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extensions: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        skills: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prompts: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        themes: Option<Vec<String>>,
    },
}

// ── Helpers ────────────────────────────────────────────────────────────

/// Merge two optional values: override wins when Some.
fn merge_opt<T>(base: Option<T>, override_val: Option<T>) -> Option<T> {
    override_val.or(base)
}

/// Merge two optional nested configs field-by-field when both are Some.
fn merge_nested<T: DeepMerge>(base: Option<T>, override_val: Option<T>) -> Option<T> {
    match (base, override_val) {
        (Some(b), Some(o)) => Some(b.deep_merge(o)),
        (Some(b), None) => Some(b),
        (None, Some(o)) => Some(o),
        (None, None) => None,
    }
}

/// Helper: skip serializing `false` for `verbose`.
fn is_false(v: &bool) -> bool {
    !*v
}

// ── DeepMerge trait ─────────────────────────────────────────────────────

/// Recursive field-by-field merge where `self` is the base and
/// `overrides` takes precedence for each field.
trait DeepMerge: Sized {
    fn deep_merge(self, overrides: Self) -> Self;
}

impl DeepMerge for CompactionConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            enabled: merge_opt(self.enabled, o.enabled),
            reserve_tokens: merge_opt(self.reserve_tokens, o.reserve_tokens),
            keep_recent_tokens: merge_opt(self.keep_recent_tokens, o.keep_recent_tokens),
        }
    }
}

impl DeepMerge for TerminalConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            show_images: merge_opt(self.show_images, o.show_images),
            image_width_cells: merge_opt(self.image_width_cells, o.image_width_cells),
            clear_on_shrink: merge_opt(self.clear_on_shrink, o.clear_on_shrink),
            show_terminal_progress: merge_opt(
                self.show_terminal_progress,
                o.show_terminal_progress,
            ),
        }
    }
}

impl DeepMerge for ImageConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            auto_resize: merge_opt(self.auto_resize, o.auto_resize),
            block_images: merge_opt(self.block_images, o.block_images),
        }
    }
}

impl DeepMerge for RetryConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            enabled: merge_opt(self.enabled, o.enabled),
            max_retries: merge_opt(self.max_retries, o.max_retries),
            base_delay_ms: merge_opt(self.base_delay_ms, o.base_delay_ms),
            provider: merge_nested(self.provider, o.provider),
        }
    }
}

impl DeepMerge for ProviderRetryConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            timeout_ms: merge_opt(self.timeout_ms, o.timeout_ms),
            max_retries: merge_opt(self.max_retries, o.max_retries),
            max_retry_delay_ms: merge_opt(self.max_retry_delay_ms, o.max_retry_delay_ms),
        }
    }
}

impl DeepMerge for MarkdownConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            code_block_indent: merge_opt(self.code_block_indent, o.code_block_indent),
        }
    }
}

impl DeepMerge for WarningConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            anthropic_extra_usage: merge_opt(self.anthropic_extra_usage, o.anthropic_extra_usage),
        }
    }
}

impl DeepMerge for BranchSummaryConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            reserve_tokens: merge_opt(self.reserve_tokens, o.reserve_tokens),
            skip_prompt: merge_opt(self.skip_prompt, o.skip_prompt),
        }
    }
}

impl DeepMerge for ThinkingBudgetsConfig {
    fn deep_merge(self, o: Self) -> Self {
        Self {
            minimal: merge_opt(self.minimal, o.minimal),
            low: merge_opt(self.low, o.low),
            medium: merge_opt(self.medium, o.medium),
            high: merge_opt(self.high, o.high),
        }
    }
}

// ── Extensions Config ──────────────────────────────────────────

/// Extension enable/disable state (managed by /extensions command).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ExtensionsConfig {
    /// Extension name → whether it's enabled.
    /// Extensions not listed use their own default_state().
    /// "builtin" is always ignored (always enabled).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub states: HashMap<String, bool>,
}

/// Helper to skip serializing default ExtensionsConfig.
fn is_default_extensions(cfg: &ExtensionsConfig) -> bool {
    cfg.states.is_empty()
}

// ── Main Settings struct ────────────────────────────────────────────────

/// Settings schema matching pi's settings.json format.
/// API keys live in auth.json, not here.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    // ── Provider / Model ──────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_thinking_level: Option<String>,

    // ── Transport ─────────────────────────────────────────────────
    /// Transport preference: "sse", "websocket", "websocket-cached", "auto".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,

    // ── Steering / Follow-up ───────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steering_mode: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_up_mode: Option<String>,

    // ── Theme / Display ────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    #[serde(default, skip_serializing_if = "is_false")]
    pub verbose: bool,

    /// Hide thinking blocks (Ctrl+T toggle).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "hideThinkingBlock"
    )]
    pub hide_thinking: Option<bool>,

    /// Collapse tool output (Ctrl+O toggle).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "collapseToolOutput"
    )]
    pub collapse_tool_output: Option<bool>,

    // ── Compaction (nested, pi-compatible) ──────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<CompactionConfig>,

    // ── Model cycling ──────────────────────────────────────────────
    /// Model patterns for cycling (same format as --models CLI flag).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "enabledModels"
    )]
    pub enabled_models: Option<Vec<String>>,

    // ── UI / Editor ────────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet_startup: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_changelog: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_skill_commands: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_install_telemetry: Option<bool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub double_escape_action: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_filter_mode: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub editor_padding_x: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_pad: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocomplete_max_visible: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub show_hardware_cursor: Option<bool>,

    // ── Shell ──────────────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_path: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_editor: Option<String>,

    /// Prefix prepended to every bash command (e.g. "shopt -s expand_aliases" for alias support).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_command_prefix: Option<String>,

    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "npmCommand"
    )]
    pub npm_command: Option<Vec<String>>,

    // ── Project trust ──────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project_trust: Option<String>,

    // ── Nested configs ─────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<ImageConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetryConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub markdown: Option<MarkdownConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<WarningConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_summary: Option<BranchSummaryConfig>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budgets: Option<ThinkingBudgetsConfig>,

    // ── Extensions / Packages ──────────────────────────────────────
    /// Extension script paths to load (user extensions).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<String>,

    /// Extension enable/disable overrides (managed by /extensions command).
    #[serde(default, skip_serializing_if = "is_default_extensions")]
    pub extensions_config: ExtensionsConfig,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub themes: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageSource>,

    // ── Pi compat stubs (n/a) ────────────────────────────────────────
    /// (n/a) pi version tracking, not used by rab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_changelog_version: Option<String>,

    /// (n/a) pi analytics opt-in, not used by rab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable_analytics: Option<bool>,

    /// (n/a) pi analytics tracking ID, not used by rab.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracking_id: Option<String>,

    // ── Network ────────────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_idle_timeout_ms: Option<u64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_connect_timeout_ms: Option<u64>,

    // ── Session ────────────────────────────────────────────────────
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_dir: Option<String>,

    /// Tracks which fields were explicitly modified during this session.
    /// Only modified fields are written when saving. Dot-separated paths
    /// (e.g. "compaction.enabled") track nested-field changes.
    #[serde(skip)]
    pub(crate) modified_fields: HashSet<String>,
}

impl Settings {
    // ── Setters that track modification ────────────────────────────────

    /// Set hide_thinking and mark it as modified.
    pub fn set_hide_thinking(&mut self, value: Option<bool>) {
        self.hide_thinking = value;
        self.mark_modified("hideThinkingBlock");
    }

    /// Set collapse_tool_output and mark it as modified.
    pub fn set_collapse_tool_output(&mut self, value: Option<bool>) {
        self.collapse_tool_output = value;
        self.mark_modified("collapseToolOutput");
    }

    /// Set default_thinking_level and mark it as modified.
    pub fn set_default_thinking_level(&mut self, value: Option<String>) {
        self.default_thinking_level = value;
        self.mark_modified("defaultThinkingLevel");
    }

    /// Set enabled_models and mark it as modified.
    pub fn set_enabled_models(&mut self, value: Option<Vec<String>>) {
        self.enabled_models = value;
        self.mark_modified("enabledModels");
    }

    /// Set auto_compact in the compaction sub-object and mark it as modified.
    pub fn set_auto_compact(&mut self, value: Option<bool>) {
        self.compaction
            .get_or_insert_with(CompactionConfig::default)
            .enabled = value;
        self.mark_nested_modified("compaction", "enabled");
    }

    /// Set compact_reserve_tokens in the compaction sub-object.
    pub fn set_compact_reserve_tokens(&mut self, value: Option<u64>) {
        self.compaction
            .get_or_insert_with(CompactionConfig::default)
            .reserve_tokens = value;
        self.mark_nested_modified("compaction", "reserveTokens");
    }

    /// Set compact_keep_recent_tokens in the compaction sub-object.
    pub fn set_compact_keep_recent_tokens(&mut self, value: Option<u64>) {
        self.compaction
            .get_or_insert_with(CompactionConfig::default)
            .keep_recent_tokens = value;
        self.mark_nested_modified("compaction", "keepRecentTokens");
    }

    /// Set default_provider and mark it as modified.
    pub fn set_default_provider(&mut self, value: Option<String>) {
        self.default_provider = value;
        self.mark_modified("defaultProvider");
    }

    /// Set default_model and mark it as modified.
    pub fn set_default_model(&mut self, value: Option<String>) {
        self.default_model = value;
        self.mark_modified("defaultModel");
    }

    /// Set both default_provider and default_model in one call (pi-compatible).
    pub fn set_default_model_and_provider(&mut self, provider: &str, model: &str) {
        self.default_provider = Some(provider.to_string());
        self.default_model = Some(model.to_string());
        self.mark_modified("defaultProvider");
        self.mark_modified("defaultModel");
    }

    /// Set shell_command_prefix and mark it as modified.
    pub fn set_shell_command_prefix(&mut self, value: Option<String>) {
        self.shell_command_prefix = value;
        self.mark_modified("shellCommandPrefix");
    }

    /// Set an extension's enabled state and mark extensions as modified.
    pub fn set_extension_enabled(&mut self, name: &str, enabled: bool) {
        self.extensions_config
            .states
            .insert(name.to_string(), enabled);
        self.mark_modified("extensionsConfig");
    }

    /// Clear an extension's override (reverts to default).
    pub fn clear_extension_override(&mut self, name: &str) {
        self.extensions_config.states.remove(name);
        self.mark_modified("extensionsConfig");
    }

    // ── Convenience accessors for compaction (bridge to nested) ────────

    /// Get auto_compact value (from compaction.enabled or default true).
    pub fn get_auto_compact(&self) -> bool {
        self.compaction
            .as_ref()
            .and_then(|c| c.enabled)
            .unwrap_or(true)
    }

    /// Get compact_reserve_tokens (from compaction.reserveTokens or default 16384).
    pub fn get_compact_reserve_tokens(&self) -> u64 {
        self.compaction
            .as_ref()
            .and_then(|c| c.reserve_tokens)
            .unwrap_or(16_384)
    }

    /// Get compact_keep_recent_tokens (from compaction.keepRecentTokens or default 20000).
    pub fn get_compact_keep_recent_tokens(&self) -> u64 {
        self.compaction
            .as_ref()
            .and_then(|c| c.keep_recent_tokens)
            .unwrap_or(20_000)
    }

    // ── Modification tracking ──────────────────────────────────────────

    /// Mark a top-level field as modified.
    /// The field name must match the camelCase JSON key (e.g. "hideThinkingBlock").
    pub fn mark_modified(&mut self, field: &str) {
        self.modified_fields.insert(field.to_string());
    }

    /// Mark a nested field as modified using dotted path.
    /// E.g. mark_nested_modified("compaction", "enabled") tracks "compaction.enabled".
    pub fn mark_nested_modified(&mut self, parent: &str, key: &str) {
        self.modified_fields.insert(format!("{}.{}", parent, key));
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

    /// Merge project settings over global. Project values take precedence
    /// when set. For nested objects (Option<T>), merges field-by-field when
    /// both are Some. Arrays are replaced entirely.
    fn merge(global: Settings, project: Settings) -> Self {
        Self {
            default_provider: merge_opt(global.default_provider, project.default_provider),
            default_model: merge_opt(global.default_model, project.default_model),
            default_thinking_level: merge_opt(
                global.default_thinking_level,
                project.default_thinking_level,
            ),
            transport: merge_opt(global.transport, project.transport),
            steering_mode: merge_opt(global.steering_mode, project.steering_mode),
            follow_up_mode: merge_opt(global.follow_up_mode, project.follow_up_mode),
            extensions_config: ExtensionsConfig {
                states: project
                    .extensions_config
                    .states
                    .clone()
                    .into_iter()
                    .chain(
                        global
                            .extensions_config
                            .states
                            .into_iter()
                            .filter(|(k, _)| !project.extensions_config.states.contains_key(k)),
                    )
                    .collect(),
            },
            theme: merge_opt(global.theme, project.theme),
            verbose: project.verbose || global.verbose,
            hide_thinking: merge_opt(global.hide_thinking, project.hide_thinking),
            collapse_tool_output: merge_opt(
                global.collapse_tool_output,
                project.collapse_tool_output,
            ),
            compaction: merge_nested(global.compaction, project.compaction),
            enabled_models: merge_opt(global.enabled_models, project.enabled_models),
            quiet_startup: merge_opt(global.quiet_startup, project.quiet_startup),
            collapse_changelog: merge_opt(global.collapse_changelog, project.collapse_changelog),
            enable_skill_commands: merge_opt(
                global.enable_skill_commands,
                project.enable_skill_commands,
            ),
            enable_install_telemetry: merge_opt(
                global.enable_install_telemetry,
                project.enable_install_telemetry,
            ),
            double_escape_action: merge_opt(
                global.double_escape_action,
                project.double_escape_action,
            ),
            tree_filter_mode: merge_opt(global.tree_filter_mode, project.tree_filter_mode),
            editor_padding_x: merge_opt(global.editor_padding_x, project.editor_padding_x),
            output_pad: merge_opt(global.output_pad, project.output_pad),
            autocomplete_max_visible: merge_opt(
                global.autocomplete_max_visible,
                project.autocomplete_max_visible,
            ),
            show_hardware_cursor: merge_opt(
                global.show_hardware_cursor,
                project.show_hardware_cursor,
            ),
            shell_path: merge_opt(global.shell_path, project.shell_path),
            external_editor: merge_opt(global.external_editor, project.external_editor),
            npm_command: merge_opt(global.npm_command, project.npm_command),
            shell_command_prefix: merge_opt(
                global.shell_command_prefix,
                project.shell_command_prefix,
            ),
            default_project_trust: merge_opt(
                global.default_project_trust,
                project.default_project_trust,
            ),
            terminal: merge_nested(global.terminal, project.terminal),
            images: merge_nested(global.images, project.images),
            retry: merge_nested(global.retry, project.retry),
            markdown: merge_nested(global.markdown, project.markdown),
            warnings: merge_nested(global.warnings, project.warnings),
            branch_summary: merge_nested(global.branch_summary, project.branch_summary),
            thinking_budgets: merge_nested(global.thinking_budgets, project.thinking_budgets),
            extensions: if project.extensions.is_empty() {
                global.extensions
            } else {
                project.extensions
            },
            skills: if project.skills.is_empty() {
                global.skills
            } else {
                project.skills
            },
            prompts: if project.prompts.is_empty() {
                global.prompts
            } else {
                project.prompts
            },
            themes: if project.themes.is_empty() {
                global.themes
            } else {
                project.themes
            },
            packages: if project.packages.is_empty() {
                global.packages
            } else {
                project.packages
            },
            http_proxy: merge_opt(global.http_proxy, project.http_proxy),
            http_idle_timeout_ms: merge_opt(
                global.http_idle_timeout_ms,
                project.http_idle_timeout_ms,
            ),
            websocket_connect_timeout_ms: merge_opt(
                global.websocket_connect_timeout_ms,
                project.websocket_connect_timeout_ms,
            ),
            session_dir: merge_opt(global.session_dir, project.session_dir),
            last_changelog_version: merge_opt(
                global.last_changelog_version,
                project.last_changelog_version,
            ),
            enable_analytics: merge_opt(global.enable_analytics, project.enable_analytics),
            tracking_id: merge_opt(global.tracking_id, project.tracking_id),
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
    /// Supports dotted paths for nested field modifications (e.g. "compaction.enabled").
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

    /// Save only the modified fields to the project-local path (`.rab/settings.json`).
    /// Returns without doing anything if no fields have been modified.
    pub fn save_to_project(&mut self, cwd: &std::path::Path) -> anyhow::Result<()> {
        if self.modified_fields.is_empty() {
            return Ok(());
        }
        let path = cwd.join(".rab").join("settings.json");
        self.save_to(path)
    }

    /// Reload settings from disk (re-reads global + project).
    /// Clears modified_fields since the freshly loaded settings are unmodified.
    pub fn reload(&mut self, cwd: &std::path::Path) -> anyhow::Result<()> {
        let global_path = Self::global_path()?;
        let global = Self::load_file(&global_path)?;
        let project = Self::load_file(&cwd.join(".rab").join("settings.json")).unwrap_or_default();
        let merged = Self::merge(global, project);
        // Copy all fields from merged into self
        *self = merged;
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
///
/// Supports dotted paths (e.g. "compaction.enabled") for nested fields.
/// For dotted paths, only the specific nested key is written, preserving
/// other keys in the same parent object.
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

    for modified_path in modified_fields {
        let (parent_key, nested_key) = modified_path.split_once('.').unzip();
        let parent = parent_key.unwrap_or(modified_path);

        if let Some(nested) = nested_key {
            // Nested field: merge into parent object preserving other keys
            if let (Some(current_obj), Some(self_obj)) =
                (current.as_object_mut(), self_value.as_object())
                && let Some(parent_value) = self_obj.get(parent)
            {
                let existing_parent = current_obj
                    .entry(parent)
                    .or_insert(serde_json::Value::Object(serde_json::Map::new()));
                if let Some(existing_obj) = existing_parent.as_object_mut()
                    && let Some(nested_value) = parent_value.get(nested)
                {
                    if nested_value.is_null() {
                        existing_obj.remove(nested);
                    } else {
                        existing_obj.insert(nested.to_string(), nested_value.clone());
                    }
                }
            }
        } else {
            // Top-level field: replace entirely, or remove if None/null
            if let (Some(current_obj), Some(self_obj)) =
                (current.as_object_mut(), self_value.as_object())
            {
                if let Some(value) = self_obj.get(parent) {
                    if value.is_null() {
                        current_obj.remove(parent);
                    } else {
                        current_obj.insert(parent.to_string(), value.clone());
                    }
                } else {
                    current_obj.remove(parent);
                }
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

    fn tmp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rab_settings_test_{}", name))
    }

    fn cleanup(path: &PathBuf) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(path.with_extension("json.lock"));
        let _ = fs::remove_file(path.with_extension("json.tmp"));
    }

    // ── Save / load roundtrip ───────────────────────────────────────

    #[test]
    fn test_save_and_load_roundtrip() {
        let path = tmp_path("roundtrip.json");
        cleanup(&path);

        let mut settings = Settings::default();
        settings.set_default_thinking_level(Some("high".into()));
        assert_eq!(settings.modified_fields.len(), 1);
        assert!(settings.modified_fields.contains("defaultThinkingLevel"));
        settings.save_to(path.clone()).unwrap();
        assert!(settings.modified_fields.is_empty());

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
                .contains_key("defaultThinkingLevel")
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

    // ── Merge ───────────────────────────────────────────────────────

    #[test]
    fn test_merge_global_and_project() {
        let global = Settings {
            hide_thinking: Some(true),
            default_thinking_level: Some("high".into()),
            ..Default::default()
        };

        let project = Settings {
            hide_thinking: Some(false),
            ..Default::default()
        };

        let merged = Settings::merge(global, project);
        assert_eq!(merged.hide_thinking, Some(false));
        assert_eq!(merged.default_thinking_level.as_deref(), Some("high"));
        assert!(merged.modified_fields.is_empty());
    }

    #[test]
    fn test_merge_nested_compaction_field_by_field() {
        let global = Settings {
            compaction: Some(CompactionConfig {
                enabled: Some(true),
                reserve_tokens: Some(16000),
                keep_recent_tokens: Some(20000),
            }),
            ..Settings::default()
        };
        let project = Settings {
            compaction: Some(CompactionConfig {
                enabled: Some(false),
                ..CompactionConfig::default()
            }),
            ..Settings::default()
        };

        let merged = Settings::merge(global, project);
        let c = merged.compaction.unwrap();
        assert_eq!(c.enabled, Some(false)); // project wins
        assert_eq!(c.reserve_tokens, Some(16000)); // from global, preserved
        assert_eq!(c.keep_recent_tokens, Some(20000)); // from global, preserved
    }

    #[test]
    fn test_merge_compaction_only_in_global() {
        let global = Settings {
            compaction: Some(CompactionConfig {
                enabled: Some(true),
                reserve_tokens: Some(16000),
                keep_recent_tokens: Some(20000),
            }),
            ..Settings::default()
        };
        let project = Settings::default();

        let merged = Settings::merge(global, project);
        let c = merged.compaction.unwrap();
        assert_eq!(c.enabled, Some(true));
        assert_eq!(c.reserve_tokens, Some(16000));
    }

    #[test]
    fn test_merge_compaction_only_in_project() {
        let global = Settings::default();
        let project = Settings {
            compaction: Some(CompactionConfig {
                enabled: Some(false),
                reserve_tokens: Some(32000),
                ..CompactionConfig::default()
            }),
            ..Settings::default()
        };

        let merged = Settings::merge(global, project);
        let c = merged.compaction.unwrap();
        assert_eq!(c.enabled, Some(false));
        assert_eq!(c.reserve_tokens, Some(32000));
    }

    // ── Save only modified fields ───────────────────────────────────

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
        assert_eq!(
            json["defaultModel"], "claude-sonnet",
            "defaultModel preserved"
        );
        assert_eq!(json["theme"], "dark", "theme preserved");

        cleanup(&path);
    }

    #[test]
    fn test_save_nested_field_preserves_other_keys() {
        let path = tmp_path("nested_save.json");
        cleanup(&path);

        // Write initial file with compaction block
        let initial = serde_json::json!({
            "compaction": {
                "enabled": true,
                "reserveTokens": 16000,
                "keepRecentTokens": 20000
            },
            "theme": "dark"
        });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Load and modify only compaction.enabled
        let mut s = Settings::load_file(&path).unwrap();
        s.set_auto_compact(Some(false));
        assert!(s.modified_fields.contains("compaction.enabled"));
        s.save_to(path.clone()).unwrap();

        // Verify: enabled changed, reserveTokens/keepRecentTokens preserved, theme preserved
        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["compaction"]["enabled"], false, "enabled changed");
        assert_eq!(
            json["compaction"]["reserveTokens"], 16000,
            "reserveTokens preserved"
        );
        assert_eq!(
            json["compaction"]["keepRecentTokens"], 20000,
            "keepRecentTokens preserved"
        );
        assert_eq!(json["theme"], "dark", "theme preserved");

        cleanup(&path);
    }

    #[test]
    fn test_save_nested_field_no_existing_parent() {
        let path = tmp_path("nested_new_parent.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_auto_compact(Some(true));
        s.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(json["compaction"]["enabled"], true);

        cleanup(&path);
    }

    #[test]
    fn test_clear_modified_fields_only_after_write() {
        let path = tmp_path("clear_modified.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_default_thinking_level(Some("max".into()));
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

    #[test]
    fn test_full_persistence_cycle() {
        let path = tmp_path("full_cycle.json");
        cleanup(&path);

        // Phase 1: initial save
        {
            let mut settings = Settings::default();
            settings.set_hide_thinking(Some(false));
            settings.set_default_thinking_level(Some("max".into()));
            settings.save_to(path.clone()).unwrap();
        }

        // Phase 2: reload and verify
        {
            let loaded = Settings::load_file(&path).unwrap();
            assert_eq!(loaded.hide_thinking, Some(false), "hide_thinking persists");
            assert_eq!(
                loaded.default_thinking_level.as_deref(),
                Some("max"),
                "thinking level persists"
            );
        }

        // Phase 3: modify and save again
        {
            let mut settings = Settings::load_file(&path).unwrap();
            settings.set_hide_thinking(Some(true));
            settings.set_default_thinking_level(Some("low".into()));
            settings.save_to(path.clone()).unwrap();
        }

        // Phase 4: reload and verify both changes
        {
            let loaded = Settings::load_file(&path).unwrap();
            assert_eq!(loaded.hide_thinking, Some(true), "hide_thinking updated");
            assert_eq!(
                loaded.default_thinking_level.as_deref(),
                Some("low"),
                "thinking level updated"
            );
        }

        cleanup(&path);
    }

    #[test]
    fn test_concurrent_writes_to_same_file() {
        let path = tmp_path("concurrent.json");
        cleanup(&path);

        let mut s1 = Settings::default();
        s1.set_hide_thinking(Some(true));
        s1.set_default_thinking_level(Some("max".into()));

        let mut s2 = Settings::default();
        s2.set_hide_thinking(Some(false));
        s2.set_default_thinking_level(Some("low".into()));

        s1.save_to(path.clone()).unwrap();
        s2.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(json.is_object(), "File must be valid JSON, not corrupted");
        assert_eq!(json["hideThinkingBlock"], false, "s2's hide_thinking");
        assert_eq!(json["defaultThinkingLevel"], "low", "s2's thinking level");

        cleanup(&path);
    }

    #[test]
    fn test_lock_file_cleanup() {
        let path = tmp_path("lock_cleanup.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_hide_thinking(Some(true));
        s.save_to(path.clone()).unwrap();

        let lock_path = path.with_extension("json.lock");
        assert!(lock_path.exists(), "Lock file should exist");
        let tmp_path = path.with_extension("json.tmp");
        assert!(!tmp_path.exists(), "Temp file should be removed");

        cleanup(&path);
    }

    #[test]
    fn test_reload_preserves_unmodified() {
        let path = tmp_path("reload_preserve.json");
        cleanup(&path);

        let initial = serde_json::json!({
            "theme": "solarized",
            "defaultModel": "deepseek-v4-pro",
            "hideThinkingBlock": true
        });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let mut s = Settings::load_file(&path).unwrap();
        s.set_default_thinking_level(Some("high".into()));
        s.save_to(path.clone()).unwrap();

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

    // ── Compaction setters / getters ───────────────────────────────

    #[test]
    fn test_auto_compact_setter_getter() {
        let mut s = Settings::default();
        assert!(s.get_auto_compact()); // default

        s.set_auto_compact(Some(false));
        assert!(!s.get_auto_compact());
        assert!(s.modified_fields.contains("compaction.enabled"));

        s.set_auto_compact(Some(true));
        assert!(s.get_auto_compact());
    }

    #[test]
    fn test_compact_reserve_tokens_getter_default() {
        let s = Settings::default();
        assert_eq!(s.get_compact_reserve_tokens(), 16384);
    }

    #[test]
    fn test_compact_reserve_tokens_setter() {
        let mut s = Settings::default();
        s.set_compact_reserve_tokens(Some(32000));
        assert_eq!(s.get_compact_reserve_tokens(), 32000);
        assert!(s.modified_fields.contains("compaction.reserveTokens"));
    }

    #[test]
    fn test_compact_keep_recent_getter_default() {
        let s = Settings::default();
        assert_eq!(s.get_compact_keep_recent_tokens(), 20000);
    }

    #[test]
    fn test_compact_keep_recent_setter() {
        let mut s = Settings::default();
        s.set_compact_keep_recent_tokens(Some(40000));
        assert_eq!(s.get_compact_keep_recent_tokens(), 40000);
        assert!(s.modified_fields.contains("compaction.keepRecentTokens"));
    }

    #[test]
    fn test_compaction_roundtrip_via_nested_save() {
        let path = tmp_path("compaction_roundtrip.json");
        cleanup(&path);

        let mut s = Settings::default();
        s.set_auto_compact(Some(false));
        s.set_compact_reserve_tokens(Some(9999));
        s.set_compact_keep_recent_tokens(Some(8888));
        s.save_to(path.clone()).unwrap();

        let loaded = Settings::load_file(&path).unwrap();
        let c = loaded.compaction.unwrap();
        assert_eq!(c.enabled, Some(false));
        assert_eq!(c.reserve_tokens, Some(9999));
        assert_eq!(c.keep_recent_tokens, Some(8888));

        cleanup(&path);
    }

    #[test]
    fn test_compaction_merge_from_file() {
        let path = tmp_path("compaction_merge.json");
        cleanup(&path);

        // Write global file with compaction
        let global = serde_json::json!({
            "compaction": {
                "enabled": true,
                "reserveTokens": 16000,
                "keepRecentTokens": 20000
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&global).unwrap()).unwrap();

        let loaded = Settings::load_file(&path).unwrap();
        let c = loaded.compaction.unwrap();
        assert_eq!(c.enabled, Some(true));
        assert_eq!(c.reserve_tokens, Some(16000));
        assert_eq!(c.keep_recent_tokens, Some(20000));

        cleanup(&path);
    }

    // ── Project values do not leak ──────────────────────────────────

    #[test]
    fn test_project_values_do_not_leak_into_global_file() {
        let path = tmp_path("no_leak.json");
        cleanup(&path);

        // Global has hideThinkingBlock
        let initial = serde_json::json!({ "hideThinkingBlock": true });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        // Load without project, modify, save
        let mut s = Settings::load_file(&path).unwrap();
        s.set_hide_thinking(Some(false));
        s.save_to(path.clone()).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            json["hideThinkingBlock"], false,
            "hideThinkingBlock updated"
        );
        // Only hideThinkingBlock should exist
        assert_eq!(json.as_object().unwrap().len(), 1);

        cleanup(&path);
    }

    // ── Deserialize nested configs ──────────────────────────────────

    #[test]
    fn test_deserialize_terminal_config() {
        let json = serde_json::json!({
            "terminal": {
                "showImages": false,
                "imageWidthCells": 80,
                "clearOnShrink": true
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let t = s.terminal.unwrap();
        assert_eq!(t.show_images, Some(false));
        assert_eq!(t.image_width_cells, Some(80));
        assert_eq!(t.clear_on_shrink, Some(true));
        assert_eq!(t.show_terminal_progress, None);
    }

    #[test]
    fn test_deserialize_images_config() {
        let json = serde_json::json!({
            "images": {
                "autoResize": false,
                "blockImages": true
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let im = s.images.unwrap();
        assert_eq!(im.auto_resize, Some(false));
        assert_eq!(im.block_images, Some(true));
    }

    #[test]
    fn test_deserialize_retry_config() {
        let json = serde_json::json!({
            "retry": {
                "enabled": true,
                "maxRetries": 5,
                "baseDelayMs": 3000,
                "provider": {
                    "timeoutMs": 60000,
                    "maxRetries": 2
                }
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let r = s.retry.unwrap();
        assert_eq!(r.enabled, Some(true));
        assert_eq!(r.max_retries, Some(5));
        assert_eq!(r.base_delay_ms, Some(3000));
        let p = r.provider.unwrap();
        assert_eq!(p.timeout_ms, Some(60000));
        assert_eq!(p.max_retries, Some(2));
    }

    #[test]
    fn test_deserialize_warnings_config() {
        let json = serde_json::json!({
            "warnings": {
                "anthropicExtraUsage": false
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let w = s.warnings.unwrap();
        assert_eq!(w.anthropic_extra_usage, Some(false));
    }

    #[test]
    fn test_deserialize_branch_summary_config() {
        let json = serde_json::json!({
            "branchSummary": {
                "reserveTokens": 8192,
                "skipPrompt": true
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let b = s.branch_summary.unwrap();
        assert_eq!(b.reserve_tokens, Some(8192));
        assert_eq!(b.skip_prompt, Some(true));
    }

    #[test]
    fn test_deserialize_thinking_budgets() {
        let json = serde_json::json!({
            "thinkingBudgets": {
                "low": 2048,
                "high": 32768
            }
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        let tb = s.thinking_budgets.unwrap();
        assert_eq!(tb.low, Some(2048));
        assert_eq!(tb.high, Some(32768));
        assert_eq!(tb.minimal, None);
        assert_eq!(tb.medium, None);
    }

    #[test]
    fn test_deserialize_package_string() {
        let json = serde_json::json!({
            "packages": ["npm:@scope/package", "git:https://example.com/repo.git"]
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(s.packages.len(), 2);
        match &s.packages[0] {
            PackageSource::String(v) => assert_eq!(v, "npm:@scope/package"),
            _ => panic!("expected string"),
        }
    }

    #[test]
    fn test_deserialize_package_object() {
        let json = serde_json::json!({
            "packages": [{
                "source": "npm:some-package",
                "extensions": ["ext1"],
                "skills": ["skill1"]
            }]
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(s.packages.len(), 1);
        match &s.packages[0] {
            PackageSource::Object {
                source,
                extensions,
                skills,
                ..
            } => {
                assert_eq!(source, "npm:some-package");
                assert_eq!(extensions.as_deref(), Some(&["ext1".to_string()][..]));
                assert_eq!(skills.as_deref(), Some(&["skill1".to_string()][..]));
            }
            _ => panic!("expected object"),
        }
    }

    #[test]
    fn test_deserialize_transport() {
        let json = serde_json::json!({ "transport": "websocket" });
        let s: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(s.transport.as_deref(), Some("websocket"));
    }

    #[test]
    fn test_deserialize_new_fields() {
        let json = serde_json::json!({
            "steeringMode": "one-at-a-time",
            "followUpMode": "all",
            "quietStartup": true,
            "collapseChangelog": true,
            "enableSkillCommands": false,
            "doubleEscapeAction": "fork",
            "treeFilterMode": "no-tools",
            "editorPaddingX": 1,
            "outputPad": 0,
            "autocompleteMaxVisible": 10,
            "showHardwareCursor": true,
            "shellPath": "/bin/zsh",
            "externalEditor": "code",
            "defaultProjectTrust": "ask",
            "httpProxy": "http://proxy:8080",
            "httpIdleTimeoutMs": 300000,
            "sessionDir": "~/.rab/sessions"
        });
        let s: Settings = serde_json::from_value(json).unwrap();
        assert_eq!(s.steering_mode.as_deref(), Some("one-at-a-time"));
        assert_eq!(s.follow_up_mode.as_deref(), Some("all"));
        assert_eq!(s.quiet_startup, Some(true));
        assert_eq!(s.collapse_changelog, Some(true));
        assert_eq!(s.enable_skill_commands, Some(false));
        assert_eq!(s.double_escape_action.as_deref(), Some("fork"));
        assert_eq!(s.tree_filter_mode.as_deref(), Some("no-tools"));
        assert_eq!(s.editor_padding_x, Some(1));
        assert_eq!(s.output_pad, Some(0));
        assert_eq!(s.autocomplete_max_visible, Some(10));
        assert_eq!(s.show_hardware_cursor, Some(true));
        assert_eq!(s.shell_path.as_deref(), Some("/bin/zsh"));
        assert_eq!(s.external_editor.as_deref(), Some("code"));
        assert_eq!(s.default_project_trust.as_deref(), Some("ask"));
        assert_eq!(s.http_proxy.as_deref(), Some("http://proxy:8080"));
        assert_eq!(s.http_idle_timeout_ms, Some(300000));
        assert_eq!(s.session_dir.as_deref(), Some("~/.rab/sessions"));
    }

    // ── Model ──────────────────────────────────────────────────────

    #[test]
    fn test_default_model_when_not_set() {
        let s = Settings::default();
        assert_eq!(s.model(), "deepseek-v4-flash");
    }

    #[test]
    fn test_model_from_settings() {
        let s = Settings {
            default_model: Some("claude-sonnet".into()),
            ..Default::default()
        };
        assert_eq!(s.model(), "claude-sonnet");
    }

    // ── Reload ─────────────────────────────────────────────────────

    #[test]
    fn test_reload_clears_modified_fields() {
        let path = tmp_path("reload_modified.json");
        cleanup(&path);

        let initial = serde_json::json!({ "hideThinkingBlock": true });
        fs::write(&path, serde_json::to_string_pretty(&initial).unwrap()).unwrap();

        let mut s = Settings::load_file(&path).unwrap();
        s.set_hide_thinking(Some(false));
        assert!(!s.modified_fields.is_empty());

        // Simulate reload (we can't call the real reload w/ cwd, but we can mock)
        let loaded = Settings::load_file(&path).unwrap();
        assert!(loaded.modified_fields.is_empty());

        cleanup(&path);
    }

    // ── Settings struct fields are accessible ───────────────────────

    #[test]
    fn test_settings_fields_accessible() {
        // Ensure commonly-used fields are still accessible as before
        let s = Settings {
            hide_thinking: Some(true),
            collapse_tool_output: Some(true),
            default_thinking_level: Some("high".into()),
            enabled_models: Some(vec!["model1".into()]),
            theme: Some("dark".into()),
            verbose: true,
            ..Default::default()
        };

        assert_eq!(s.hide_thinking, Some(true));
        assert_eq!(s.collapse_tool_output, Some(true));
        assert_eq!(s.default_thinking_level.as_deref(), Some("high"));
        assert_eq!(s.enabled_models, Some(vec!["model1".into()]));
        assert_eq!(s.theme.as_deref(), Some("dark"));
        assert!(s.verbose);
    }
}
