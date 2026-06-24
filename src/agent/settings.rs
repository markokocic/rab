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
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
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
    pub fn save(&mut self) -> anyhow::Result<()> {
        if self.modified_fields.is_empty() {
            return Ok(());
        }
        let path = Self::global_path()?;
        self.save_to(path)
    }

    /// Save only the modified fields to a specific path (for testing).
    /// Uses atomic write (temp file + rename) to prevent partial writes.
    pub fn save_to(&mut self, path: std::path::PathBuf) -> anyhow::Result<()> {
        if self.modified_fields.is_empty() {
            return Ok(());
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Read existing file content (if any)
        let mut current: serde_json::Value = if path.exists() {
            let content = std::fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            serde_json::from_str(&content)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
        } else {
            serde_json::Value::Object(serde_json::Map::new())
        };

        // Serialize self - with skip_serializing_if, only explicitly-set
        // (non-default) fields appear in the output.
        let self_value = serde_json::to_value(&*self)
            .with_context(|| format!("Failed to serialize settings to {}", path.display()))?;

        // Apply only the modified fields on top of the existing file content.
        if let (Some(current_obj), Some(self_obj)) =
            (current.as_object_mut(), self_value.as_object())
        {
            for key in &self.modified_fields {
                if let Some(value) = self_obj.get(key) {
                    // Field is set (non-default) - write it
                    current_obj.insert(key.clone(), value.clone());
                } else {
                    // Field was un-set (returned to default) - remove from file
                    current_obj.remove(key);
                }
            }
        }

        let content = serde_json::to_string_pretty(&current)
            .with_context(|| format!("Failed to serialize settings to {}", path.display()))?;

        // Atomic write: write to temp file, then rename to prevent partial writes.
        let tmp_path = path.with_extension("json.tmp");
        std::fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write {}", tmp_path.display()))?;
        std::fs::rename(&tmp_path, &path).with_context(|| {
            format!(
                "Failed to rename {} to {}",
                tmp_path.display(),
                path.display()
            )
        })?;

        // Clear modified fields after a successful write so that a later
        // save of a *different* field does not re-apply this field's value
        // (which may have been manually edited in the file in between).
        self.modified_fields.clear();
        Ok(())
    }

    /// Resolved model name (defaults to deepseek-v4-flask).
    pub fn model(&self) -> &str {
        self.default_model.as_deref().unwrap_or("deepseek-v4-flash")
    }
}
