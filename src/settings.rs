use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Settings schema matching pi's settings.json format.
/// API keys live in auth.json, not here.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub default_provider: Option<String>,

    #[serde(default)]
    pub default_model: Option<String>,

    #[serde(default)]
    pub default_thinking_level: Option<String>,

    #[serde(default)]
    pub tools: Vec<String>,

    #[serde(default)]
    pub exclude_tools: Vec<String>,

    #[serde(default)]
    pub theme: Option<String>,

    #[serde(default)]
    pub verbose: bool,

    /// Hide thinking blocks (Ctrl+T toggle). Persisted to settings.json.
    #[serde(default, rename = "hideThinkingBlock")]
    pub hide_thinking: Option<bool>,

    /// Collapse tool output (Ctrl+O toggle). Persisted to settings.json.
    #[serde(default, rename = "collapseToolOutput")]
    pub collapse_tool_output: Option<bool>,
}

impl Settings {
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
        }
    }

    /// Save settings to the global config path.
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::global_path()?;
        self.save_to(path)
    }

    /// Save settings to a specific path (for testing).
    pub fn save_to(&self, path: std::path::PathBuf) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(self)
            .with_context(|| format!("Failed to serialize settings to {}", path.display()))?;
        std::fs::write(&path, &content)
            .with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    /// Resolved model name (defaults to deepseek-v4-flash).
    pub fn model(&self) -> &str {
        self.default_model.as_deref().unwrap_or("deepseek-v4-flash")
    }
}
