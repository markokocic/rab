use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::extension::ToolRenderer;
use yoagent::types::AgentTool;

use crate::agent::AgentSession;
use crate::agent::footer_data_provider::FooterDataProvider;
use crate::builtin::export;
use crate::extension::{CommandResult, Extension};
use crate::provider;
use crate::provider::ProviderRegistry;
use crate::provider::auth;
use yoagent::types::AgentMessage;

use crate::agent::ui::chat_editor::{ChatEditor, InputAction};

use crate::agent::ui::components::EditorComponent;
use crate::agent::ui::components::FooterComponent;
use crate::agent::ui::components::InfoMessageComponent;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::theme;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
use crate::tui::Component;
use crate::tui::TUI;
use crate::tui::focusable::Focusable;

pub mod events;
pub mod helpers;
pub(crate) use events::*;
pub(crate) use helpers::*;

/// Pending label changes accumulator (used by tree selector, flushed each frame).
pub type PendingLabelChanges = Rc<RefCell<Vec<(String, Option<String>)>>>;

/// Result from an overlay lifecycle — checked by the main loop after route_input.
#[derive(Debug, Clone)]
pub enum OverlayResult {
    /// User selected a model (provider/id string).
    ModelSelected(String),
    /// User accepted scoped model changes — persist to settings.
    ScopedModelsAccepted(Option<Vec<String>>),
    /// User cancelled — close overlay, no persist.
    ScopedModelsCancelled,
    /// User selected a provider for login.
    LoginProviderSelected(String),
    /// User provided an API key for login.
    LoginApiKeyProvided { provider: String, key: String },
    /// User selected an auth type for login.
    LoginAuthTypeSelected(AuthType),
    /// User selected a provider for logout.
    LogoutProviderSelected(String),
    /// User confirmed session import (carries the resolved path).
    ImportConfirmed(String),
    /// User cancelled session import.
    ImportCancelled,
    /// User selected a tree entry to navigate to.
    TreeNavigateTo(String),
    /// User cancelled tree navigation.
    TreeCancelled,
    /// User chose whether to summarize after tree entry selection.
    /// `custom_instructions` is set when user chose "Summarize with custom prompt".
    TreeSummarizeChoice {
        entry_id: String,
        summarize: bool,
        custom_instructions: Option<String>,
    },
    /// User wants to reopen the tree selector (from summarization prompt), carrying the entry to select.
    TreeReopen(String),
    /// User selected a message to fork from.
    ForkMessageSelected(String),
    /// User cancelled fork message selection.
    ForkCancelled,
    /// Generic dismiss (no action needed, close the overlay).
    Dismiss,
}

use crate::agent::ui::components::oauth_selector::AuthType;
use crate::agent::ui::theme::ThemeKey;
use crate::tui::components::Spacer;
use crate::tui::components::Text;
use crate::tui::terminal::{self, ProcessTerminal, TerminalTrait};
use crossterm::event::{KeyEvent, KeyEventKind};
use tokio::sync::mpsc;

/// Thinking level cycle order (matching pi's thinking level enum). Cycles from
/// highest to lowest so the first press from the default (max) goes to "high"
/// (a step down), not to "off".
const ALL_THINKING_LEVELS: &[&str] = &["max", "high", "medium", "low", "off"];

/// Get the available thinking levels for the current model, filtered by
/// the model's `thinkingLevelMap`. Matches pi's `getSupportedThinkingLevels`.
/// Levels mapped to `null` are unsupported. "max" requires explicit presence
/// in the map (other levels are available unless nulled).
fn available_thinking_levels(app: &App) -> Vec<&'static str> {
    // Try to read thinkingLevelMap from the resolved model
    let thinking_map: Option<std::collections::HashMap<String, serde_json::Value>> = app
        .registry
        .resolve(&app.model, Some(&app.current_provider))
        .ok()
        .and_then(|r| r.thinking_map);

    match thinking_map {
        Some(map) => ALL_THINKING_LEVELS
            .iter()
            .filter(|level| {
                let mapped = map.get(**level);
                // null means explicitly unsupported
                if matches!(mapped, Some(v) if v.is_null()) {
                    return false;
                }
                // Pi: "xhigh" / "max" require explicit presence in the map
                if **level == "max" {
                    return mapped.is_some();
                }
                true
            })
            .copied()
            .collect(),
        None => ALL_THINKING_LEVELS.to_vec(),
    }
}

/// Configuration for the UI app.
pub struct AppConfig {
    pub model: String,
    pub provider: String,
    pub system_prompt: String,
    pub extensions: Vec<Box<dyn Extension>>,
    pub cwd: PathBuf,
    pub thinking_level: Option<String>,
    pub available_models: Vec<String>,
    pub hide_thinking: bool,
    pub collapse_tool_output: bool,
    pub interactive: bool,
    pub settings: crate::settings::Settings,
    /// Context files (AGENTS.md / CLAUDE.md) loaded for the session.
    pub context_files: Vec<String>,

    /// Skills loaded for the session (used for /skill:name expansion).
    pub skills: Vec<yoagent::skills::Skill>,
    /// Skill directories to scan (for /reload support).
    pub skill_dirs: Vec<PathBuf>,
    /// Agent config directory (~/.rab/agent).
    pub agent_dir: PathBuf,
    /// Prompt templates loaded for the session (/name expansion).
    pub prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
    /// Prompt template directories to scan (for /reload support).
    pub prompt_template_dirs: Vec<PathBuf>,
    /// API key for yoagent provider.
    pub api_key: String,
    /// Provider registry for model resolution and provider dispatch.
    pub registry: Arc<ProviderRegistry>,
    /// If true, open the session picker immediately on startup (for --resume CLI).
    pub open_session_picker: bool,
}

/// Main application state.
pub struct App {
    cwd: PathBuf,
    model: String,
    current_provider: String,
    thinking_level: Option<String>,
    system_prompt: String,
    theme: RabTheme,

    /// Slash commands from all extensions.
    commands: Vec<(String, String)>,

    /// Available models for the model selector.
    available_models: Vec<String>,
    /// Provider registry for model resolution and provider dispatch.
    registry: Arc<ProviderRegistry>,

    /// Component-based chat area - mirrors pi's `this.chatContainer`.
    /// Components are added here in handle_agent_event instead of pushing to messages.
    pub chat_container: std::rc::Rc<std::cell::RefCell<crate::tui::Container>>,

    // ── Section components for the UI layout (written by compose_ui) ──
    /// Status text section (transient, dim).
    pub status_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,
    /// Working indicator section.
    pub working_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,

    /// The chat editor (shared ownership - App mutates, TUI.root renders).
    editor: Rc<RefCell<ChatEditor>>,

    /// Agent event channel.
    event_tx: mpsc::UnboundedSender<yoagent::types::AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<yoagent::types::AgentEvent>,

    /// Streaming state.
    is_streaming: bool,
    /// Pending agent submission (set by sync handle_input, consumed by async main loop).
    pending_submit: Option<String>,
    /// Pre-loaded messages for the next agent turn (drained from steer/follow-up queues).
    pending_preloaded_msgs: Option<Vec<yoagent::types::AgentMessage>>,
    /// Pending manual compaction (carries optional custom instructions).
    pending_compact: Option<Option<String>>,
    /// Pending auto-compaction check after AgentEnd (pi-compatible).
    pending_auto_compact: bool,
    /// The reused Agent (accumulates messages across turns, supports mid-turn steering).
    agent: Option<yoagent::agent::Agent>,
    /// Handle for the forwarding task that relays events from the agent's event
    /// receiver to the UI channel. The Agent stays in `app.agent` during streaming.
    forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Handle for the OAuth login task, aborted on quit to avoid background polling.
    oauth_join_handle: Option<tokio::task::JoinHandle<()>>,

    /// Provider ID of an in-flight OAuth login, used to perform post-login
    /// actions (registry refresh, model auto-selection) after the task completes.
    pending_oauth_provider: Option<String>,

    /// Display settings.
    hide_thinking: bool,
    collapse_tool_output: bool,
    /// Global toggle: expand all tool outputs (Ctrl+O). Inverted of collapse_tool_output.
    tools_expanded: bool,

    /// Timestamp of last Ctrl+C for double-press detection (pi-style).
    last_clear_time: std::time::Instant,

    /// Exit flag.
    should_quit: bool,

    /// Number of tool executions currently in-flight.
    /// Incremented on ToolExecutionStart, decremented on ToolExecutionEnd.
    /// Used to skip the 15s inactivity timeout while tools are running,
    /// since long-running tools (e.g. bash) may not emit progress events.
    pending_tool_executions: usize,

    /// Bash abort handle for bang (!) commands.
    bash_abort_handle: Option<tokio::task::AbortHandle>,

    /// Session persistence via AgentSession lifecycle layer.
    session: Option<AgentSession>,

    /// Footer (shared ownership - App mutates, TUI.root renders).
    footer: Rc<RefCell<Footer>>,

    /// Footer data provider (pull-based: git branch, extension statuses).
    footer_provider: Rc<RefCell<FooterDataProvider>>,

    /// Pending tool executions keyed by tool call ID.
    /// Used to update ToolExecComponent when ToolResult arrives (pi's `pendingTools` Map).
    pending_tools: HashMap<String, Weak<RefCell<crate::agent::ui::components::ToolExecComponent>>>,

    /// Start times for pending tool calls, keyed by tool call ID.
    /// Used to compute duration for bash and other tools.
    tool_call_start_times: HashMap<String, std::time::Instant>,

    /// Receivers for async invalidation notifications (edit tool preview).
    /// Polled on each render cycle to trigger re-render of tool components.
    invalidate_rxs: Vec<tokio::sync::mpsc::UnboundedReceiver<()>>,

    /// Streaming assistant message component (pi's `streamingComponent`).
    /// Created on first TextDelta, updated in-place, cleared on TurnEnd/AgentEnd.
    streaming_component:
        Option<Weak<RefCell<crate::agent::ui::components::AssistantMessageComponent>>>,

    /// Working indicator.
    working: WorkingIndicator,

    /// Transient status text (pi-style: replaces previous status, not added to chat).
    status_text: Option<String>,

    /// Pending command result that needs TUI access (overlays etc.).
    /// Set by handle_slash_command, consumed in the main loop where TUI is available.
    pending_command_result: Option<CommandResult>,

    /// Overlay result signal — set by overlay callbacks, checked by main loop.
    overlay_result_signal: Rc<RefCell<Option<OverlayResult>>>,

    /// Pending scoped model changes from ScopedModelsSelector (session-only, no close).
    pending_scoped_ids: Rc<RefCell<Option<Vec<String>>>>,

    /// Pending settings changes from SettingsSelector: (id, new_value).
    /// Checked and consumed by the main loop after overlay input processing.
    pending_settings_change: Rc<RefCell<Option<(String, String)>>>,

    /// Agent tools (for tool execution).
    /// Extensions.
    extensions: Arc<Vec<Box<dyn Extension>>>,
    /// Skills loaded for the session (/skill:name expansion).
    skills: Vec<yoagent::skills::Skill>,
    /// Skill directories to scan (for /reload support).
    skill_dirs: Vec<PathBuf>,
    /// Agent config directory (~/.rab/agent).
    agent_dir: PathBuf,
    /// Context file paths (AGENTS.md / CLAUDE.md) loaded for the session.
    context_files: Vec<String>,
    /// Prompt template directories to scan (for /reload support).
    prompt_template_dirs: Vec<PathBuf>,
    /// Prompt templates loaded for the session (/name expansion).
    prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
    /// API key for yoagent provider.
    api_key: String,

    /// Auto-compact toggle state.
    auto_compact: bool,

    /// Settings reference for persisting toggle changes.
    settings: crate::settings::Settings,

    /// Header component (welcome/onboarding). Stored as `Rc<RefCell>` so
    /// handle_tools_expand can toggle its expanded state (matching pi's
    /// behavior where setToolsExpanded expands both the header and all
    /// expandable chat children).
    header: Rc<RefCell<crate::agent::ui::components::HeaderComponent>>,

    /// Scoped model IDs for cycling (null = all enabled).
    scoped_model_ids: Option<Vec<String>>,

    /// Session picker state (Some = picker is active).
    session_picker: Option<crate::agent::ui::components::SessionPicker>,

    /// Pending messages section (pi-style: shows queued steer/follow-up messages
    /// between chat_container and status_section, with a spacer before).
    pub pending_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,

    /// Tracks the number of children in `chat_container` after the last
    /// status message was added (pi-style `lastStatusSpacer`/`lastStatusText`).
    /// Used by `show_status()` to replace consecutive status messages in-place
    /// instead of appending indefinitely.
    last_status_len: Option<usize>,
    /// Pending label changes from the tree selector (accumulated, flushed each frame).
    pending_label_changes: PendingLabelChanges,
    /// Stop-requested flag shared with agent's before_turn callback.
    /// Set by /stop, cleared when a new agent turn starts.
    stop_requested: Arc<AtomicBool>,
    /// Messages queued via /nextTurn (delivered at the start of the next agent run).
    next_turn_queue: Vec<yoagent::types::AgentMessage>,
    /// Messages saved from steer/follow-up queues when stop was requested.
    /// Restored as preloaded on the next agent run.
    saved_queued_msgs: Vec<yoagent::types::AgentMessage>,
    // ── Message rendering cache (avoids re-rendering messages every frame) ──
    // Cache fields removed - messages now rendered via Components in chat_container.
}

impl App {
    fn new(config: AppConfig, session: AgentSession) -> Self {
        let mut agent_session = session;
        let resolved = config
            .registry
            .resolve(&config.model, Some(&config.provider))
            .ok();
        let model_config = resolved
            .as_ref()
            .map(|r| r.model_config.clone())
            .unwrap_or_else(|| crate::agent::base_model_config(&config.model));
        let context_window = model_config.context_window;
        let rab_compat = resolved.as_ref().map(|r| r.rab_compat.clone());
        agent_session.set_compaction_config(
            config.api_key.clone(),
            &config.model,
            context_window as u64,
            Some(model_config),
            rab_compat,
        );
        agent_session.set_registry(config.registry.clone());
        agent_session.set_auto_compact(config.settings.get_auto_compact());
        if let Some(ref cc) = config.settings.compaction {
            agent_session.apply_compaction_config(cc);
        }
        let (tx, rx) = mpsc::unbounded_channel();
        use crate::agent::ui::theme::current_theme;
        let theme = current_theme().clone();

        let mut editor = ChatEditor::new(&theme, config.cwd.clone());

        // Collect slash commands with argument completion callbacks
        use crate::tui::autocomplete::AutocompleteItem as AutoAutocompleteItem;
        use crate::tui::autocomplete::SlashCommand as AutoSlashCommand;
        let mut auto_commands: Vec<AutoSlashCommand> = config
            .extensions
            .iter()
            .flat_map(|e| e.commands())
            .map(|cmd| {
                let handler = cmd.handler;
                AutoSlashCommand {
                    name: cmd.name,
                    description: Some(cmd.description),
                    argument_hint: None,
                    argument_completions: None,
                    get_argument_completions: Some(std::sync::Arc::new(
                        move |prefix: &str| -> Vec<AutoAutocompleteItem> {
                            handler
                                .argument_completions(prefix)
                                .into_iter()
                                .map(|item| AutoAutocompleteItem {
                                    value: item.value,
                                    label: item.label,
                                    description: item.description,
                                })
                                .collect()
                        },
                    )),
                }
            })
            .collect();

        // Register /skill:name commands for autocomplete (pi-compatible)
        for skill in &config.skills {
            let cmd_name = format!("skill:{}", skill.name);
            auto_commands.push(AutoSlashCommand {
                name: cmd_name,
                description: Some(skill.description.clone()),
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            });
        }

        // Register prompt template commands for autocomplete (pi-compatible)
        for template in &config.prompt_templates {
            auto_commands.push(AutoSlashCommand {
                name: template.name.clone(),
                description: Some(template.description.clone()),
                argument_hint: template.argument_hint.clone(),
                argument_completions: None,
                get_argument_completions: None,
            });
        }
        editor.set_slash_commands(auto_commands);

        // Keep commands list for help overlay and unknown-command display.
        let mut commands: Vec<(String, String)> = config
            .extensions
            .iter()
            .flat_map(|e| e.commands())
            .map(|c| (c.name, c.description))
            .collect();

        // Add skill commands (pi-compatible: /skill:name is an implicit command)
        for skill in &config.skills {
            commands.push((format!("skill:{}", skill.name), skill.description.clone()));
        }

        // Add prompt template commands (pi-compatible: /name is an implicit command)
        for template in &config.prompt_templates {
            commands.push((template.name.clone(), template.description.clone()));
        }

        let editor = Rc::new(RefCell::new(editor));

        let footer_provider = Rc::new(RefCell::new(FooterDataProvider::new(config.cwd.clone())));

        let mut footer = Footer::new(
            config.cwd.to_string_lossy().to_string(),
            footer_provider.clone(),
        );
        footer.set_context_window(context_window as u64);

        // Set available provider count for footer display
        footer_provider
            .borrow_mut()
            .set_available_provider_count(config.registry.count_providers());

        // Record initial model/thinking in session if not already present
        // so refresh_from_session can pick them up.
        {
            let has_model_entry = !agent_session
                .session()
                .find_entries("model_change")
                .is_empty();
            if !has_model_entry {
                agent_session.on_model_change(&config.provider, &config.model);
            }
            let has_thinking_entry = !agent_session
                .session()
                .find_entries("thinking_level_change")
                .is_empty();
            if !has_thinking_entry && let Some(ref level) = config.thinking_level {
                agent_session.on_thinking_level_change(level);
            }
        }

        let footer = Rc::new(RefCell::new(footer));

        // Load session messages
        let context = agent_session.session().build_context();
        let history_messages = context.messages.clone();

        // Build chat_container from AgentMessages directly (matching pi's renderSessionContext).
        // Adjacent toolCall content + toolResult messages are paired into single
        // ToolExecComponent so reloaded sessions look identical to live execution.
        let cwd_string = config.cwd.to_string_lossy().to_string();

        // Collect context file paths for header resource display (pi-style loaded resources).
        let context_file_paths: Vec<String> = config
            .context_files
            .iter()
            .map(|s| {
                // Shorten paths for display (relative to cwd or home)
                if let Some(rel) = s.strip_prefix(&cwd_string) {
                    if rel.is_empty() {
                        s.clone()
                    } else {
                        format!("./{}", rel.trim_start_matches('/'))
                    }
                } else if let Some(home) =
                    std::env::var_os("HOME").and_then(|h| h.into_string().ok())
                    && let Some(rel) = s.strip_prefix(&home)
                {
                    if rel.is_empty() {
                        s.clone()
                    } else {
                        format!("~/{}", rel.trim_start_matches('/'))
                    }
                } else {
                    s.clone()
                }
            })
            .collect();
        let skill_names: Vec<String> = config.skills.iter().map(|s| s.name.clone()).collect();
        let template_names: Vec<String> = config
            .prompt_templates
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let extension_names: Vec<(String, bool)> = config
            .extensions
            .iter()
            .map(|e| {
                let enabled = crate::extension::is_extension_enabled(e.as_ref(), &config.settings);
                (e.name().to_string(), enabled)
            })
            .collect();
        // Custom theme names (excluding built-in dark/light), matching pi's showLoadedResources
        let theme_names: Vec<String> = crate::agent::ui::theme::get_available_themes()
            .into_iter()
            .filter(|n| n != "dark" && n != "light")
            .collect();

        let chat_container =
            std::rc::Rc::new(std::cell::RefCell::new(crate::tui::Container::new()));
        {
            let mut chat = chat_container.borrow_mut();
            rebuild_chat_from_messages(
                &mut chat,
                &history_messages,
                &cwd_string,
                config.hide_thinking,
                config.collapse_tool_output,
                &config.extensions,
            );
        }

        let verbose = config.settings.verbose;

        let mut result = Self {
            cwd: config.cwd,
            model: config.model,
            current_provider: config.provider,
            thinking_level: config.thinking_level,
            system_prompt: config.system_prompt,
            theme,
            commands,
            available_models: config.available_models,
            registry: config.registry.clone(),
            chat_container,
            pending_tools: HashMap::new(),
            tool_call_start_times: HashMap::new(),
            invalidate_rxs: Vec::new(),
            streaming_component: None,

            pending_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            status_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            working_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            editor,
            event_tx: tx,
            event_rx: rx,
            is_streaming: false,
            pending_submit: None,
            pending_preloaded_msgs: None,
            pending_compact: None,
            pending_auto_compact: false,
            agent: None,
            forward_handle: None,
            oauth_join_handle: None,
            pending_oauth_provider: None,
            pending_command_result: if config.open_session_picker {
                Some(CommandResult::OpenSessionSelector)
            } else {
                None
            },
            overlay_result_signal: Rc::new(RefCell::new(None)),
            pending_scoped_ids: Rc::new(RefCell::new(None)),
            pending_settings_change: Rc::new(RefCell::new(None)),
            hide_thinking: config.hide_thinking,
            collapse_tool_output: config.collapse_tool_output,
            tools_expanded: !config.collapse_tool_output,
            last_clear_time: std::time::Instant::now(),

            should_quit: false,
            pending_tool_executions: 0,
            bash_abort_handle: None,
            session: Some(agent_session),
            footer,
            footer_provider,
            working: WorkingIndicator::new(),
            extensions: Arc::new(config.extensions),

            skills: config.skills,
            skill_dirs: config.skill_dirs,
            agent_dir: config.agent_dir,
            prompt_template_dirs: config.prompt_template_dirs,
            prompt_templates: config.prompt_templates,
            api_key: config.api_key,
            scoped_model_ids: config.settings.enabled_models.clone(),
            settings: config.settings,
            auto_compact: true,
            status_text: None,
            context_files: context_file_paths.clone(),
            header: Rc::new(RefCell::new(
                crate::agent::ui::components::HeaderComponent::new_with_expanded(
                    !config.collapse_tool_output || verbose,
                ),
            )),
            session_picker: None,
            last_status_len: None,
            pending_label_changes: Rc::new(RefCell::new(Vec::new())),
            stop_requested: Arc::new(AtomicBool::new(false)),
            next_turn_queue: Vec::new(),
            saved_queued_msgs: Vec::new(),
        };

        // Set resource data on header (pi-style loaded resources display)
        {
            let mut hdr = result.header.borrow_mut();
            hdr.set_resource_data(
                context_file_paths,
                skill_names,
                template_names,
                extension_names,
                theme_names,
            );
        }

        // Initialize footer stats and session name from session
        if let Some(ref mut s) = result.session {
            result.footer.borrow_mut().refresh_from_session(s.session());
        }

        result
    }

    /// Update the session info shared with CommandsExtension for /session display.
    /// Refresh git branch for footer display.
    /// Called on AgentStart to match pi's FooterDataProvider.onBranchChange.
    fn refresh_git_branch(&self) {
        self.footer_provider.borrow_mut().refresh_git_branch();
    }

    /// Clear all transient session state when switching to a new session.
    fn clear_session_state(&mut self) {
        self.chat_container.borrow_mut().clear();
        self.pending_section.borrow_mut().set_lines(vec![]);
        self.streaming_component = None;
        self.pending_tools.clear();
        self.tool_call_start_times.clear();
        self.pending_submit = None;
        self.pending_preloaded_msgs = None;
        self.next_turn_queue.clear();
        self.saved_queued_msgs.clear();
        self.stop_requested.store(false, Ordering::Relaxed);
    }

    /// Rebuild chat and agent messages from the current session context.
    /// Used after compaction to update the UI and keep the agent in sync.
    fn rebuild_from_session_context(&mut self) {
        if let Some(ref agent_session) = self.session {
            let context = agent_session.session().build_context();
            {
                let mut chat = self.chat_container.borrow_mut();
                rebuild_chat_from_messages(
                    &mut chat,
                    &context.messages,
                    &self.cwd.to_string_lossy(),
                    self.hide_thinking,
                    self.collapse_tool_output,
                    &self.extensions,
                );
            }
            if let Some(ref mut agent) = self.agent {
                agent.replace_messages(context.messages);
            }
        }
    }

    /// Record a model change in the session and refresh footer display.
    fn record_model_change(&mut self, model: &str) {
        if let Some(ref mut agent_session) = self.session {
            agent_session.on_model_change(&self.current_provider, model);
        }
        if let Some(ref session) = self.session {
            self.footer
                .borrow_mut()
                .refresh_from_session(session.session());
        }
    }

    /// Reload the provider registry from disk, updating `self.registry`.
    /// Shows a status message on failure.
    fn refresh_registry(&mut self) {
        match provider::ProviderRegistry::load(&provider::get_agent_dir()) {
            Ok(new_reg) => self.registry = Arc::new(new_reg),
            Err(e) => {
                self.status_text = Some(format!("Failed to refresh registry: {}", e));
            }
        }
    }

    /// Propagate `hide_thinking` to all chat container children and the streaming component.
    fn propagate_hide_thinking(&mut self) {
        let hide = self.hide_thinking;
        {
            let mut chat = self.chat_container.borrow_mut();
            for child in chat.children_mut().iter_mut() {
                child.set_hide_thinking(hide);
            }
        }
        if let Some(weak) = self.streaming_component.as_ref().and_then(|w| w.upgrade()) {
            weak.borrow_mut().set_hide_thinking(hide);
        }
    }

    /// Switch to a different session: open the file, clear state, rebuild chat.
    fn switch_to_session(&mut self, new_session: AgentSession) {
        let ctx = new_session.session().build_context();
        self.clear_session_state();
        rebuild_chat_from_messages(
            &mut self.chat_container.borrow_mut(),
            &ctx.messages,
            &self.cwd.to_string_lossy(),
            self.hide_thinking,
            self.collapse_tool_output,
            &self.extensions,
        );
        // Refresh footer cached stats for the switched-to session
        self.footer
            .borrow_mut()
            .refresh_from_session(new_session.session());

        self.session = Some(new_session);
        self.agent = None;
    }
}

/// Run the interactive UI.
pub async fn run(config: AppConfig, session: AgentSession) -> anyhow::Result<()> {
    // Initialize theme system
    crate::agent::ui::theme::init_theme(Some("dark"), false);

    let mut term = ProcessTerminal::new();
    let mut stdout = std::io::stdout();

    // Main-screen mode (like pi) - no alternate screen, no clear.
    // Content writes from current cursor position (after shell prompt).
    // Terminal scrolls naturally, editor/footer appear at the bottom.
    term.start(&mut stdout)?;
    term.hide_cursor(&mut stdout)?;
    term.set_color_scheme_notifications(&mut stdout, true)?;
    crate::tui::terminal::start_stdin_reader();

    let mut tui = TUI::new();
    tui.set_clear_on_shrink(false);
    let mut app = App::new(config, session);

    // Focus the editor so it emits the cursor marker for Screen tracking
    {
        let editor_rc = app.editor.clone();
        tui.register_editor_focus(Box::new(move |focused| {
            editor_rc.borrow_mut().editor.set_focused(focused);
        }));
    }
    // Register cursor callback for immediate show/hide on overlay lifecycle
    tui.register_cursor_callback(Box::new(move |visible| {
        use std::io::Write;
        if visible {
            let _ = write!(std::io::stdout(), "\x1b[?25h");
        } else {
            let _ = write!(std::io::stdout(), "\x1b[?25l");
        }
        let _ = std::io::stdout().flush();
    }));
    tui.set_focus(crate::tui::FocusTarget::Editor);

    // Set up the component tree in TUI.root (matching pi's TUI.extend(Container))
    // Order: header → chat_container (messages) → pending → status → working → spacer → editor → footer
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(
            app.header.clone() as Rc<RefCell<dyn Component>>,
        ),
    ));
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.chat_container.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.pending_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.status_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.working_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    // Pi-compatible: Spacer(1) between working indicator and editor (widgetContainerAbove)
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(EditorComponent(app.editor.clone())));
    tui.add_child(std::boxed::Box::new(FooterComponent(app.footer.clone())));

    // Initialize editor border color
    app.editor.borrow_mut().update_border_color(
        app.thinking_level.as_deref(),
        &app.theme as &dyn crate::tui::Theme,
    );

    // Cache terminal dimensions to avoid expensive syscall on every frame.
    // Only re-query when a resize event is detected or periodically.
    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut dirty = true; // force initial render

    loop {
        // Drain agent events FIRST so state (is_streaming, pending_auto_compact) is
        // up-to-date before handle_input checks it. Prevents races where a terminal
        // event arrives in the same cycle as AgentEnd — handle_input would see stale
        // is_streaming=true and steer the message instead of starting a new turn.
        let mut had_event = false;
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
            had_event = true;
        }
        if had_event {
            dirty = true;
        }

        // Drain terminal events (non-blocking — stdin reader runs on a
        // separate thread). The stdin thread is already decoupled from the
        // main loop, so we just drain whatever has arrived since last check.
        loop {
            match terminal::try_recv_terminal_event() {
                Some(terminal::TerminalEvent::Key(key)) => {
                    // Ignore key release events (crossterm on Windows reports both
                    // Press and Release for every keystroke — processing both would
                    // duplicate every character).
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    // TUI overlay routing first (overlays get first crack at input)
                    if !tui.route_input(&key) {
                        handle_input(&mut app, &mut tui, &mut term, &key);
                    }
                }
                Some(terminal::TerminalEvent::Paste(content)) => {
                    // Route to focused overlay first (e.g. Input in settings),
                    // fall back to the main Editor.
                    if !tui.route_paste(&content) {
                        app.editor.borrow_mut().editor.handle_paste(&content);
                    }
                }
                Some(terminal::TerminalEvent::Resize(w, h)) => {
                    app.editor.borrow_mut().editor.set_terminal_rows(h as usize);
                    tui.set_dimensions(w as usize, h as usize);
                }
                None => break,
            }
            dirty = true;
        }

        // Check pending scoped model changes (session-only, from on_change callback).
        // Pi-compatible: only set scoped models when fewer than all models are enabled.
        // Empty list or all models = no filter (None).
        if let Some(ids) = app.pending_scoped_ids.borrow_mut().take() {
            let auth_count = app.registry.list_authenticated_model_ids().len();
            if ids.is_empty() || ids.len() >= auth_count {
                app.scoped_model_ids = None;
            } else {
                app.scoped_model_ids = Some(ids);
            }
            dirty = true;
        }

        // Check pending settings changes from SettingsSelector.
        // Applied immediately without closing the overlay.
        {
            let change = app.pending_settings_change.borrow_mut().take();
            if let Some((id, new_value)) = change {
                apply_settings_change(&mut app, &id, &new_value);
                dirty = true;
            }
        }

        // Flush pending label changes from tree selector to session (without closing overlay).
        if tui.has_overlays() {
            let changes = app
                .pending_label_changes
                .borrow_mut()
                .drain(..)
                .collect::<Vec<_>>();
            for (entry_id, label) in changes {
                if let Some(ref mut session) = app.session {
                    let _ = session
                        .session_mut()
                        .append_label_change(&entry_id, label.as_deref());
                }
            }
        }

        // Check overlay result signal
        if tui.has_overlays() {
            let result = app.overlay_result_signal.borrow_mut().take();
            if let Some(result) = result {
                tui.pop_overlay();
                match result {
                    OverlayResult::ModelSelected(full_id) => {
                        if !full_id.is_empty() {
                            let (provider, model_id) = full_id
                                .split_once('/')
                                .map(|(p, m)| (p.to_string(), m.to_string()))
                                .unwrap_or_else(|| (String::new(), full_id.clone()));
                            app.current_provider = provider.clone();
                            app.model = model_id.clone();
                            app.record_model_change(&model_id);
                            // Persist model/provider to settings (pi-compatible)
                            if !provider.is_empty() {
                                app.settings
                                    .set_default_model_and_provider(&provider, &model_id);
                            } else {
                                app.settings.set_default_model(Some(model_id.clone()));
                            }
                            if let Err(e) = app.settings.save() {
                                app.status_text =
                                    Some(format!("Failed to save default model: {}", e));
                            } else {
                                app.status_text = Some(format!("Model: {}", full_id));
                            }
                        }
                    }
                    OverlayResult::ScopedModelsAccepted(ids) => {
                        match ids {
                            Some(ids)
                                if !ids.is_empty()
                                    && ids.len()
                                        < app.registry.list_authenticated_model_ids().len() =>
                            {
                                app.scoped_model_ids = Some(ids.clone());
                                // Persist to settings
                                app.settings.set_enabled_models(Some(ids));
                                if let Err(e) = app.settings.save() {
                                    app.status_text =
                                        Some(format!("Failed to save model scope: {}", e));
                                } else {
                                    app.status_text = Some("Model scope saved to settings".into());
                                }
                            }
                            _ => {
                                // All enabled or none = clear scoped models and settings
                                app.scoped_model_ids = None;
                                app.settings.set_enabled_models(None);
                                if let Err(e) = app.settings.save() {
                                    app.status_text =
                                        Some(format!("Failed to save model scope: {}", e));
                                } else if ids.is_some() {
                                    app.status_text = Some("Model scope saved to settings".into());
                                }
                            }
                        }
                    }
                    OverlayResult::ScopedModelsCancelled => {
                        // Just close the overlay, don't persist anything.
                    }
                    OverlayResult::LoginAuthTypeSelected(auth_type) => {
                        // User selected auth type — show provider selector filtered by type
                        show_login_provider_selector(&mut app, &mut tui, Some(auth_type));
                    }
                    OverlayResult::LoginProviderSelected(provider_id) => {
                        // Check if this is an OAuth provider
                        if crate::provider::oauth::get(&provider_id).is_some() {
                            // OAuth login flow
                            show_oauth_login_dialog(&mut app, &mut tui, &provider_id);
                        } else {
                            // API key login flow
                            show_api_key_login_dialog(&mut app, &mut tui, &provider_id);
                        }
                    }
                    OverlayResult::LoginApiKeyProvided { provider, key } => {
                        // Check for OAuth login failure prefix
                        if let Some(err_msg) = key.strip_prefix("OAUTH_LOGIN_FAILED:") {
                            app.status_text = Some(format!("OAuth login failed: {}", err_msg));
                        } else {
                            match auth::login(&provider, &key) {
                                Ok(_) => {
                                    app.status_text = Some(format!("Logged in to {}", provider));
                                    app.refresh_registry();
                                    complete_login(&mut app, &provider, AuthType::ApiKey);
                                }
                                Err(e) => {
                                    app.status_text = Some(format!("Login failed: {}", e));
                                }
                            }
                        }
                    }
                    OverlayResult::LogoutProviderSelected(provider_id) => {
                        match auth::logout(Some(&provider_id)) {
                            Ok(true) => {
                                app.status_text = Some(format!("Logged out from {}", provider_id));
                                app.refresh_registry();
                            }
                            Ok(false) => {
                                app.status_text =
                                    Some(format!("No credentials for {}", provider_id));
                            }
                            Err(e) => {
                                app.status_text = Some(format!("Logout failed: {}", e));
                            }
                        }
                    }
                    OverlayResult::ImportConfirmed(path) => {
                        let result = (|| -> Result<PathBuf, String> {
                            let resolved = crate::builtin::resolve_path(&path, &app.cwd);
                            if !resolved.exists() {
                                return Err(format!("File not found: {}", resolved.display()));
                            }

                            // Get the session directory from the current session (pi-compatible)
                            let session_dir = app
                                .session
                                .as_ref()
                                .map(|s| s.session_dir().to_path_buf())
                                .unwrap_or_else(|| {
                                    crate::agent::session::get_default_session_dir(&app.cwd)
                                });

                            // Ensure session directory exists
                            std::fs::create_dir_all(&session_dir)
                                .map_err(|e| format!("Failed to create session dir: {}", e))?;

                            // Copy the file to the session directory (pi-compatible)
                            let dest = session_dir.join(
                                resolved
                                    .file_name()
                                    .unwrap_or_else(|| std::ffi::OsStr::new("session.jsonl")),
                            );
                            if dest != resolved {
                                std::fs::copy(&resolved, &dest)
                                    .map_err(|e| format!("Failed to copy session file: {}", e))?;
                            }

                            let agent_session = crate::agent::AgentSession::open(
                                &dest,
                                Some(&session_dir),
                                Some(&app.cwd),
                            );
                            app.working.stop();
                            app.status_text = None;
                            app.switch_to_session(agent_session);
                            Ok(dest)
                        })();

                        match result {
                            Ok(path) => {
                                show_status(
                                    &mut app,
                                    format!(
                                        "✓ Imported and switched to session: {}",
                                        crate::builtin::shorten_path(&path.to_string_lossy())
                                    ),
                                );
                            }
                            Err(msg) => {
                                show_status(&mut app, format!("✗ {}", msg));
                            }
                        }
                    }
                    OverlayResult::ImportCancelled => {
                        show_status(&mut app, "Import cancelled.");
                    }
                    OverlayResult::ForkMessageSelected(entry_id) => {
                        // User selected a message to fork from
                        let source_path = app
                            .session
                            .as_ref()
                            .and_then(|s| s.session().session_file());
                        let session_dir =
                            app.session.as_ref().map(|s| s.session_dir().to_path_buf());
                        let cwd = app.cwd.clone();

                        match (source_path, session_dir) {
                            (Some(source), Some(ref target_dir)) => {
                                match crate::agent::session::fork_session(
                                    source,
                                    target_dir,
                                    Some(&entry_id),
                                    None,
                                ) {
                                    Ok(new_id) => {
                                        let dir_entries = std::fs::read_dir(target_dir).ok();
                                        let new_path = dir_entries.and_then(|entries| {
                                            entries
                                                .flatten()
                                                .find(|e| {
                                                    let filename = e.file_name();
                                                    filename.to_string_lossy().contains(&new_id)
                                                })
                                                .map(|e| e.path())
                                        });

                                        match new_path {
                                            Some(ref path) => {
                                                let new_session = crate::agent::AgentSession::open(
                                                    path,
                                                    None,
                                                    Some(&cwd),
                                                );
                                                app.switch_to_session(new_session);

                                                let styled = app.theme.fg(
                                                    "accent",
                                                    &format!(
                                                        "✓ Forked session: {}",
                                                        path.display()
                                                    ),
                                                );
                                                chat_add(
                                                    &mut app,
                                                    std::boxed::Box::new(Text::new(
                                                        styled, 1, 1, None,
                                                    )),
                                                );
                                            }
                                            None => {
                                                let msg = format!(
                                                    "Fork created but new file not found: {}",
                                                    new_id
                                                );
                                                show_status(&mut app, msg);
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        let msg = format!("Fork failed: {}", e);
                                        show_status(&mut app, msg.clone());
                                    }
                                }
                            }
                            _ => {
                                let msg = "No active session to fork".to_string();
                                show_status(&mut app, msg.clone());
                            }
                        }
                    }
                    OverlayResult::ForkCancelled => {
                        // Just close
                    }
                    OverlayResult::Dismiss => {
                        // Just close
                    }
                    OverlayResult::TreeNavigateTo(entry_id) => {
                        // User selected an entry — check if it's the current leaf
                        let current_leaf =
                            app.session.as_ref().and_then(|s| s.session().get_leaf_id());
                        if current_leaf.as_deref() == Some(&entry_id) {
                            app.status_text = Some("Already at this point".to_string());
                        } else {
                            // Show summarization choice prompt (matching pi's showExtensionSelector)
                            show_summarization_prompt(&mut app, &mut tui, &entry_id);
                        }
                    }
                    OverlayResult::TreeCancelled => {
                        // Just close
                    }
                    OverlayResult::TreeSummarizeChoice {
                        entry_id,
                        summarize,
                        custom_instructions,
                    } => {
                        // Navigate with or without summary
                        if summarize {
                            if let Some(ref mut session) = app.session {
                                match session
                                    .set_branch(&entry_id, custom_instructions.as_deref())
                                    .await
                                {
                                    Ok(_) => {
                                        app.status_text =
                                            Some("Navigated to selected point".to_string());
                                        app.rebuild_from_session_context();
                                    }
                                    Err(e) => {
                                        app.status_text = Some(format!("Navigation error: {}", e));
                                    }
                                }
                            }
                        } else {
                            // No summary — just move the leaf
                            if let Some(ref mut session) = app.session {
                                match session.session_mut().set_leaf_id(&entry_id) {
                                    Ok(_) => {
                                        app.status_text = Some(
                                            "Navigated to selected point (no summary)".to_string(),
                                        );
                                        app.rebuild_from_session_context();
                                    }
                                    Err(e) => {
                                        app.status_text = Some(format!("Navigation error: {}", e));
                                    }
                                }
                            }
                        }
                    }
                    OverlayResult::TreeReopen(_entry_id) => {
                        show_status(&mut app, "Session tree view temporarily disabled.");
                    }
                }
                dirty = true;
            }
        }

        // Re-drain agent events that arrived during terminal event processing.
        // AgentEnd (which sets is_streaming=false) can land between the initial
        // drain above and the user hitting Enter — processing terminal events
        // can take real time (edit operations, overlays, etc). Without this,
        // submit_message may see a stale is_streaming=true and incorrectly try
        // to steer a finished agent.
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
            dirty = true;
        }

        // Recover Agent state BEFORE submitting any new prompt or running
        // auto-compact. This ensures agent.finish() restores messages from
        // the completed JoinHandle first, so that subsequent
        // replace_messages calls (from handle_auto_compact) don't get
        // overwritten.
        if app.forward_handle.as_ref().is_some_and(|h| h.is_finished()) {
            app.forward_handle.take();
            if let Some(ref mut agent) = app.agent {
                // The JoinHandle is resolved, so this returns instantly.
                agent.finish().await;
            }
        }

        // Clean up completed OAuth handle
        if app
            .oauth_join_handle
            .as_ref()
            .is_some_and(|h| h.is_finished())
        {
            app.oauth_join_handle.take();

            // OAuth task finished — check if credentials were saved and if so,
            // refresh registry and auto-select a model (matching API key login flow).
            // Also add a persistent chat message so the user sees the result
            // even after the status bar text gets overwritten.
            let oauth_provider = app.pending_oauth_provider.take();
            if let Some(ref provider_id) = oauth_provider
                && let Ok(Some(auth::AuthCredential::Oauth { .. })) =
                    auth::read_credential(provider_id)
            {
                let provider_name = app
                    .registry
                    .list_providers()
                    .into_iter()
                    .find(|(id, _)| id == provider_id)
                    .map(|(_, name)| name)
                    .unwrap_or_else(|| provider_id.clone());
                let msg = format!("✓ Logged in to {} via OAuth", provider_name);
                app.status_text = Some(msg.clone());
                show_status(&mut app, &msg);
                app.refresh_registry();
                complete_login(
                    &mut app,
                    provider_id,
                    crate::agent::ui::components::oauth_selector::AuthType::OAuth,
                );
            } else if oauth_provider.is_some() {
                // OAuth task finished but no credential saved (login failed).
                // The error message was already shown as status_text; persist it to chat.
                let err_msg = app.status_text.clone().unwrap_or_default();
                if !err_msg.is_empty() {
                    show_status(&mut app, &err_msg);
                }
            }
        }

        // Handle pending agent submission (async).
        // During streaming, submit_message uses agent.steer() directly so
        // pending_submit is only set for the idle path. Processed here as
        // soon as is_streaming becomes false and the agent is truly idle.
        if !app.is_streaming {
            let agent_ready = app.agent.as_ref().is_none_or(|a| !a.is_streaming());
            if agent_ready && let Some(text) = app.pending_submit.take() {
                let preloaded = app.pending_preloaded_msgs.take();
                start_agent_loop(&mut app, text, preloaded).await;
                dirty = true;
            }
        }

        // Handle pending manual compaction (async)
        if let Some(custom_instructions) = app.pending_compact.take() {
            handle_compact_command(&mut app, custom_instructions).await;
            dirty = true;
        }

        // Pi-compatible: auto-compaction check after agent ends.
        // Runs after agent.finish() to ensure replace_messages in
        // handle_auto_compact doesn't get overwritten.
        if app.pending_auto_compact {
            app.pending_auto_compact = false;
            handle_auto_compact(&mut app).await;
            dirty = true;
        }

        // Handle pending command results that need TUI access (overlays, etc.)
        if let Some(result) = app.pending_command_result.take() {
            match result {
                CommandResult::ShowHelp => {
                    show_help_overlay(&mut app, &mut tui);
                }
                CommandResult::OpenSessionSelector => {
                    // Open session picker with current-project context
                    let mut picker = crate::agent::ui::components::SessionPicker::new();
                    let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
                    picker.load_sessions_with_cwd(Some(&app.cwd), session_dir.as_deref());
                    app.session_picker = Some(picker);
                    app.status_text = None;
                }
                CommandResult::OpenModelSelector => {
                    open_model_selector(&mut app, &mut tui);
                }
                CommandResult::OpenSettings => {
                    open_settings(&mut app, &mut tui);
                }
                CommandResult::OpenExtensions => {
                    open_extensions_selector(&mut app, &mut tui);
                }
                CommandResult::ScopedModels => {
                    open_scoped_models_selector(&mut app, &mut tui);
                }
                CommandResult::Login {
                    ref provider,
                    ref api_key,
                } => {
                    if let (Some(provider), Some(key)) = (provider, api_key) {
                        handle_login(&mut app, provider, Some(key));
                    } else if let Some(provider) = provider {
                        // Provider specified, no key — show API key prompt
                        show_api_key_login_dialog(&mut app, &mut tui, provider);
                    } else {
                        // No provider — determine if auth type selector is needed
                        show_auth_type_or_provider_selector(&mut app, &mut tui);
                    }
                }
                CommandResult::Logout { provider } => match provider {
                    Some(p) => handle_logout(&mut app, Some(&p)),
                    None => show_logout_provider_selector(&mut app, &mut tui),
                },
                CommandResult::ImportSession { path } => {
                    let resolved = crate::builtin::resolve_path(&path, &app.cwd);
                    if !resolved.exists() {
                        show_status(
                            &mut app,
                            format!("✗ File not found: {}", resolved.display()),
                        );
                    } else {
                        let display_path = resolved.display().to_string();
                        let signal = app.overlay_result_signal.clone();
                        let path_for_confirm = path.clone();
                        let mut confirm =
                            Box::new(crate::agent::ui::components::ConfirmOverlay::new(
                                "Import Session",
                                format!("Replace current session with {}?", display_path),
                            ));
                        confirm.on_confirm({
                            let signal = signal.clone();
                            move || {
                                *signal.borrow_mut() =
                                    Some(OverlayResult::ImportConfirmed(path_for_confirm));
                            }
                        });
                        confirm.on_cancel({
                            let signal = signal.clone();
                            move || {
                                *signal.borrow_mut() = Some(OverlayResult::ImportCancelled);
                            }
                        });
                        tui.show_overlay(confirm, Default::default());
                    }
                }
                CommandResult::SessionTree => {
                    show_status(&mut app, "Session tree view temporarily disabled.");
                }
                CommandResult::ForkSession { message_id: None } => {
                    // Show user message selector for forking
                    let user_messages: Vec<
                        crate::agent::ui::components::fork_selector::UserMessageItem,
                    > = app
                        .session
                        .as_ref()
                        .map(|s| {
                            s.session()
                                .get_entries()
                                .iter()
                                .filter_map(|entry| {
                                    if let Some(llm_msg) = entry.message.as_llm() {
                                        if llm_msg.role() == "user" {
                                            Some((
                                                entry.id.clone(),
                                                AgentMessage::Llm(llm_msg.clone()),
                                            ))
                                        } else {
                                            None
                                        }
                                    } else {
                                        None
                                    }
                                })
                                .enumerate()
                                .map(|(i, (id, msg))| {
                                    crate::agent::ui::components::fork_selector::UserMessageItem {
                                        id,
                                        text: crate::agent::types::message_text(&msg),
                                        index: i,
                                        total: 0, // will be set after collect
                                    }
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();

                    let total = user_messages.len();
                    let user_messages: Vec<_> = user_messages
                        .into_iter()
                        .map(|mut m| {
                            m.total = total;
                            m
                        })
                        .collect();

                    if user_messages.is_empty() {
                        show_status(&mut app, "No messages to fork from".to_string());
                    } else {
                        let signal_select = app.overlay_result_signal.clone();
                        let signal_cancel = app.overlay_result_signal.clone();
                        let mut selector =
                            crate::agent::ui::components::fork_selector::ForkSelector::new(
                                user_messages,
                            );
                        selector.on_select = Some(Box::new(move |entry_id| {
                            *signal_select.borrow_mut() =
                                Some(OverlayResult::ForkMessageSelected(entry_id));
                        }));
                        selector.on_cancel = Some(Box::new(move || {
                            *signal_cancel.borrow_mut() = Some(OverlayResult::ForkCancelled);
                        }));
                        tui.show_positioned_overlay(
                            Box::new(selector),
                            crate::tui::OverlayPosition::Bottom,
                        );
                    }
                }
                _ => {}
            }
            dirty = true;
        }

        // Poll async invalidation receivers (edit tool preview, etc.)
        app.invalidate_rxs.retain_mut(|rx| {
            if rx.try_recv().is_ok() {
                dirty = true;
                true
            } else {
                !rx.is_closed()
            }
        });

        // Check terminal size only when we're about to render
        // (avoids expensive ioctl syscall on idle frames)
        if dirty && let Ok((w, h)) = term.size() {
            app.editor.borrow_mut().editor.set_terminal_rows(h as usize);
            cols = w;
            rows = h;
        }

        // Tick the working indicator - sets dirty when spinner advances
        if app.working.tick() {
            dirty = true;
        }

        // Tick active tool timers (bash elapsed display, matching pi's setInterval(1000))
        let mut tools_to_remove: Vec<String> = Vec::new();
        for (id, weak) in app.pending_tools.iter() {
            if let Some(comp) = weak.upgrade() {
                if comp.borrow_mut().tick_timer() {
                    dirty = true;
                }
            } else {
                tools_to_remove.push(id.clone());
            }
        }
        for id in tools_to_remove {
            app.pending_tools.remove(&id);
        }

        // Compose and render only when state has changed
        if dirty {
            compose_ui(&mut app, cols as usize);
            tui.set_dimensions(cols as usize, rows as usize);
            tui.render(cols as usize, rows as usize, &mut stdout)?;
            dirty = false;
        }

        // Idle backpressure: sleep briefly so we don't busy-wait when idle.
        // Active frames (dirty, streaming, working spinner) run at ~60fps;
        // idle frames pace at ~20fps to save CPU/battery.
        tokio::time::sleep(if dirty || app.is_streaming || app.working.active {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(50)
        })
        .await;

        // Pi: clear transient status after rendering
        app.status_text = None;

        if app.should_quit {
            // Abort any in-flight OAuth login task
            if let Some(handle) = app.oauth_join_handle.take() {
                handle.abort();
            }
            break;
        }
    }

    // Cleanup - move cursor past all rendered content so the shell prompt
    // appears on a fresh line after the footer (matching pi's stop() behavior).
    tui.finalize(&mut stdout)?;
    term.set_color_scheme_notifications(&mut stdout, false)?;
    term.show_cursor(&mut stdout)?;
    term.stop(&mut stdout)?;

    Ok(())
}

/// Update UI section components from app state.
/// Each section is a child of TUI.root rendered in the correct order.
///
/// Layout (top to bottom):
///   header → chat_container (messages) → pending (queued steer/follow-up) → status → working → spacer → editor → footer
fn compose_ui(app: &mut App, width: usize) {
    // ── Session picker ──
    if let Some(ref picker) = app.session_picker {
        let (_lines, _cursor_y) = picker.render(width, &app.theme as &dyn crate::tui::Theme);
        // Clear chat container when picker is active
        app.chat_container.borrow_mut().clear();
        app.pending_section.borrow_mut().set_lines(vec![]);
        app.status_section.borrow_mut().set_lines(vec![]);
        app.working_section.borrow_mut().set_lines(vec![]);
        return;
    }

    // ── Transient status text (pi-style: replaces previous status, not added to chat) ──
    let mut status_lines = Vec::new();
    if let Some(ref status) = app.status_text {
        let line = app.theme.fg_key(ThemeKey::Dim, &format!(" {}", status));
        status_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
    }
    app.status_section.borrow_mut().set_lines(status_lines);

    // ── Pending messages section (pi-style pendingMessagesContainer) ──
    // Shows queued steer/follow-up/nextTurn messages between chat and status.
    let mut pending_lines = Vec::new();
    let has_next_turn = !app.next_turn_queue.is_empty();
    let has_saved = !app.saved_queued_msgs.is_empty();
    let has_pending = if let Some(ref agent) = app.agent {
        agent.steering_queue_len() > 0
            || agent.follow_up_queue_len() > 0
            || app.pending_submit.is_some()
            || has_next_turn
            || has_saved
    } else {
        app.pending_submit.is_some() || has_next_turn || has_saved
    };
    if has_pending {
        // Blank line separator (pi's Spacer(1) before pending section)
        pending_lines.push(String::new());

        // Show next-turn queue (queued while idle via /nextTurn)
        for msg in &app.next_turn_queue {
            let text = crate::agent::types::message_text(msg);
            let preview = if text.len() > width.saturating_sub(14) {
                format!("{}…", &text[..width.saturating_sub(14)])
            } else {
                text
            };
            if !preview.is_empty() {
                let line = app
                    .theme
                    .fg_key(ThemeKey::Dim, &format!(" Next turn: {}", preview));
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
            }
        }

        // Show saved queued messages (from stop-requested)
        for msg in &app.saved_queued_msgs {
            let text = crate::agent::types::message_text(msg);
            let preview = if text.len() > width.saturating_sub(14) {
                format!("{}…", &text[..width.saturating_sub(14)])
            } else {
                text
            };
            if !preview.is_empty() {
                let line = app
                    .theme
                    .fg_key(ThemeKey::Dim, &format!(" Saved: {}", preview));
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
            }
        }

        // Show pending_submit (idle path, before agent loop starts)
        if let Some(ref msg) = app.pending_submit {
            let preview = if msg.len() > width.saturating_sub(12) {
                format!("{}…", &msg[..width.saturating_sub(12)])
            } else {
                msg.clone()
            };
            let line = app
                .theme
                .fg_key(ThemeKey::Dim, &format!(" 📝 queued: {}", preview));
            pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
        }

        // Show agent's internal steer/follow-up queues (pi-style)
        if let Some(ref agent) = app.agent {
            for msg in agent.steering_queue_snapshot() {
                let text = crate::agent::types::message_text(&msg);
                let preview = if text.len() > width.saturating_sub(14) {
                    format!("{}…", &text[..width.saturating_sub(14)])
                } else {
                    text
                };
                if !preview.is_empty() {
                    let line = app
                        .theme
                        .fg_key(ThemeKey::Dim, &format!(" Steering: {}", preview));
                    pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
                }
            }
            for msg in agent.follow_up_queue_snapshot() {
                let text = crate::agent::types::message_text(&msg);
                let preview = if text.len() > width.saturating_sub(14) {
                    format!("{}…", &text[..width.saturating_sub(14)])
                } else {
                    text
                };
                if !preview.is_empty() {
                    let line = app
                        .theme
                        .fg_key(ThemeKey::Dim, &format!(" Follow-up: {}", preview));
                    pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
                }
            }

            // Dequeue hint (pi-style)
            let dequeue_keys = crate::tui::keybindings::get_keybindings()
                .get_keys(crate::tui::keybindings::ACTION_APP_MESSAGE_DEQUEUE);
            if !dequeue_keys.is_empty() {
                let hint = app.theme.fg_key(
                    ThemeKey::Dim,
                    &format!(" ↳ {} to edit all queued messages", dequeue_keys[0]),
                );
                pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&hint, width));
            }
        }
    }
    app.pending_section.borrow_mut().set_lines(pending_lines);

    // ── Working indicator (pi-style: blank line + spinner before editor) ──
    let mut working_lines = Vec::new();
    let wl = app.working.render(width);
    working_lines.extend(wl);
    app.working_section.borrow_mut().set_lines(working_lines);
}

// Helper: create an AgentMessage for a user text input (used for steer/follow_up).
fn user_agent_message(text: &str) -> yoagent::types::AgentMessage {
    yoagent::types::AgentMessage::Llm(yoagent::types::Message::User {
        content: vec![yoagent::types::Content::Text {
            text: text.to_string(),
        }],
        timestamp: yoagent::types::now_ms(),
    })
}

/// Handle keyboard input. Mirrors pi's InteractiveMode key dispatch:
///
/// 1. Overlays handled via TUI.route_input - checked first in event loop
/// 2. ChatEditor::handle_input checks app-level keys and returns InputAction
/// 3. App.rs matches on InputAction to perform side effects
///
/// This keeps text-editing logic in the Editor component (via ChatEditor)
/// and app-level side effects (aborting agents, toggling settings, etc.) here.
fn handle_input(app: &mut App, tui: &mut TUI, term: &mut ProcessTerminal, key: &KeyEvent) {
    // ── Session picker input handling ──
    if app.session_picker.is_some() {
        handle_session_picker_input(app, key);
        return;
    }

    // ── Check if any TUI overlay is active (help, model selector, etc.) ──
    // Only pop when Escape is pressed and no overlay consumed it.
    // Overlay components return false for Escape which reaches here —
    // we pop the overlay instead of routing to the editor.
    if tui.has_overlays() && matches!(key.code, crossterm::event::KeyCode::Esc) {
        tui.pop_overlay();
        return;
    }
    if tui.has_overlays() {
        // Overlay didn't handle this key and it's not Escape — just ignore.
        return;
    }

    // ── Route input to root container children (header, etc.) ──
    // Root children (header → chat_container → pending → etc.) get a chance
    // to handle input before the editor. Components that don't consume the
    // event return false so it flows through to the editor.
    if tui.handle_input(key) {
        return;
    }

    // ── Dispatch to ChatEditor (mirrors pi's CustomEditor.handleInput) ──
    // Borrow the editor in a let binding so the RefMut drops before we mutate App.
    let action = app.editor.borrow_mut().handle_input(key);
    match action {
        InputAction::Handled => {}
        InputAction::Escape => {
            // Pi-style: abort streaming or bash, else clear editor
            if app.is_streaming {
                interrupt_streaming(app);
            } else {
                app.editor.borrow_mut().editor.set_text("");
            }
        }
        InputAction::Clear => {
            handle_clear(app);
        }
        InputAction::Exit => {
            app.should_quit = true;
        }
        InputAction::ThinkingCycle => {
            handle_thinking_cycle(app);
        }
        InputAction::ModelSelector => {
            open_model_selector(app, tui);
        }
        InputAction::ModelCycleForward => {
            handle_model_cycle(app, 1);
        }
        InputAction::ModelCycleBackward => {
            handle_model_cycle(app, -1);
        }
        InputAction::ToggleThinking => {
            app.hide_thinking = !app.hide_thinking;
            // Propagate to ALL existing components in chat container (matching pi)
            app.propagate_hide_thinking();
            // Persist only the affected field (incremental save)
            app.settings.set_hide_thinking(Some(app.hide_thinking));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save thinking visibility: {}", e));
            }
            show_status(
                app,
                if app.hide_thinking {
                    "Thinking blocks: hidden".to_string()
                } else {
                    "Thinking blocks: visible".to_string()
                },
            );
        }
        InputAction::ToolsExpand => {
            handle_tools_expand(app);
        }
        InputAction::EditorExternal => {
            handle_editor_external(app, tui, term);
        }
        InputAction::Help => {
            show_help_overlay(app, tui);
        }
        InputAction::Submit(text) => {
            submit_message(app, text);
        }
        InputAction::FollowUp(text) => {
            handle_follow_up(app, text);
        }
        InputAction::Dequeue => {
            // Restore queued message back to editor (pi's app.message.dequeue)
            if let Some(msg) = app.pending_submit.take() {
                app.editor.borrow_mut().editor.set_text(&msg);
                app.status_text = Some("Queued message restored to editor".into());
            } else {
                app.status_text = Some("No queued message".into());
            }
        }
        InputAction::CompactToggle => {
            handle_compact_toggle(app);
        }
        InputAction::SessionResume => {
            app.pending_command_result = Some(CommandResult::OpenSessionSelector);
        }
    }
}

// =============================================================================
// New action handlers (pi-compatible)
// =============================================================================

/// Handle Ctrl+C: clear editor (double-press within 500ms = exit).
fn handle_clear(app: &mut App) {
    let now = std::time::Instant::now();
    let elapsed = now.duration_since(app.last_clear_time);
    app.last_clear_time = now;

    if app.is_streaming {
        interrupt_streaming(app);
    } else if elapsed.as_millis() < 500 {
        // Double Ctrl+C within 500ms = exit (pi-style)
        app.should_quit = true;
    } else {
        app.editor.borrow_mut().editor.set_text("");
        app.status_text = Some("Cleared".into());
    }
}

/// Cycle thinking level through the levels available for the current model.
fn handle_thinking_cycle(app: &mut App) {
    if app.available_models.is_empty() && app.model.is_empty() {
        app.status_text = Some("No model selected".into());
        return;
    }

    let levels = available_thinking_levels(app);
    if levels.is_empty() {
        return;
    }

    let current = app.thinking_level.as_deref().unwrap_or("off");
    let next = match levels.iter().position(|&l| l == current) {
        Some(pos) => levels[(pos + 1) % levels.len()],
        None => "off",
    };

    app.thinking_level = Some(next.to_string());
    app.editor
        .borrow_mut()
        .update_border_color(Some(next), &app.theme as &dyn crate::tui::Theme);
    app.settings
        .set_default_thinking_level(Some(next.to_string()));
    if let Err(e) = app.settings.save() {
        app.status_text = Some(format!("Failed to save thinking level: {}", e));
    }
    // Record the change in the session and refresh footer
    if let Some(ref mut agent_session) = app.session {
        agent_session.on_thinking_level_change(next);
    }
    if let Some(ref s) = app.session {
        app.footer.borrow_mut().refresh_from_session(s.session());
    }
    show_status(app, format!("Thinking level: {}", next));
}

/// Cycle model forward (dir=1) or backward (dir=-1).
/// If scoped models are set, cycles through those only (matching pi's cycleModel).
fn handle_model_cycle(app: &mut App, dir: isize) {
    // Determine the model pool: scoped models if set, otherwise all available
    // from authenticated providers.
    let authenticated_models = app.registry.list_authenticated_model_ids();
    let model_pool: Vec<String> = if let Some(ref scoped) = app.scoped_model_ids
        && !scoped.is_empty()
    {
        // Scoped model IDs are "provider/id" — extract just the model id part.
        // We match against app.model (which is just a model id string).
        scoped
            .iter()
            .filter_map(|full_id| {
                let (_provider, model_id) = full_id.split_once('/')?;
                if authenticated_models.iter().any(|m| m == model_id) {
                    Some(model_id.to_string())
                } else {
                    None
                }
            })
            .collect()
    } else {
        authenticated_models
    };

    let n = model_pool.len();
    if n == 0 {
        app.status_text = Some("No models available".into());
        return;
    }

    let current_idx = model_pool.iter().position(|m| m == &app.model);

    let next_idx = match current_idx {
        Some(idx) => (idx as isize + dir).rem_euclid(n as isize) as usize,
        None => 0,
    };

    let model = model_pool[next_idx].clone();
    app.model = model.clone();
    app.current_provider = app
        .registry
        .provider_for_model(&model, Some(&app.current_provider))
        .unwrap_or_default();
    app.record_model_change(&model);
    // Persist model/provider to settings (pi-compatible)
    let provider = &app.current_provider;
    if !provider.is_empty() {
        app.settings
            .set_default_model_and_provider(provider, &model);
    } else {
        app.settings.set_default_model(Some(model.clone()));
    }
    if let Err(e) = app.settings.save() {
        eprintln!("Warning: failed to save default model: {}", e);
    }
    show_status(app, format!("Model: {}", app.model));
}

/// Toggle all tool output expansion (Ctrl+O).
/// Mirrors pi's `toggleToolOutputExpansion()` which iterates all chat_container
/// children and calls `setExpanded()` on `Expandable` components.
fn handle_tools_expand(app: &mut App) {
    app.tools_expanded = !app.tools_expanded;
    app.collapse_tool_output = !app.tools_expanded;

    // Expand/collapse header (welcome/onboarding) - matching pi's setToolsExpanded
    // which expands both the active header and all expandable chat children.
    app.header.borrow_mut().set_expanded(app.tools_expanded);

    // Propagate to all children in chat_container
    let mut chat = app.chat_container.borrow_mut();
    for child in chat.children_mut().iter_mut() {
        child.set_expanded(app.tools_expanded);
    }
    drop(chat);

    app.settings
        .set_collapse_tool_output(Some(app.collapse_tool_output));
    if let Err(e) = app.settings.save() {
        app.status_text = Some(format!("Failed to save tool output setting: {}", e));
    }
    show_status(
        app,
        if app.tools_expanded {
            "Tool output: expanded".to_string()
        } else {
            "Tool output: collapsed".to_string()
        },
    );
}

/// Open external editor ($VISUAL / $EDITOR) for current editor content.
/// Suspends the TUI (disables raw mode), runs the editor, then resumes.
fn handle_editor_external(app: &mut App, tui: &mut TUI, term: &mut ProcessTerminal) {
    let editor_cmd = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_default();

    if editor_cmd.is_empty() {
        app.status_text = Some("No editor configured. Set $VISUAL or $EDITOR.".into());
        return;
    }

    let tmp_dir = std::env::temp_dir();
    let tmp_file = tmp_dir.join(format!(
        "rab-editor-{}.md",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let current_text = app.editor.borrow().editor.get_text();
    if let Err(e) = std::fs::write(&tmp_file, &current_text) {
        app.status_text = Some(format!("Failed to write temp file: {}", e));
        return;
    }

    let parts: Vec<&str> = editor_cmd.split(' ').collect();
    let (editor, args) = parts.split_first().unwrap_or((&"", &[]));

    // ── Suspend TUI ──
    app.status_text = Some(format!("Opening {} ...", editor_cmd));
    let mut suspend_buf = Vec::new();
    let _ = term.stop(&mut suspend_buf);
    let _ = term.show_cursor(&mut suspend_buf);
    if !suspend_buf.is_empty() {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(&suspend_buf);
        let _ = handle.flush();
    }

    // Stop the stdin reader thread (uses poll() with timeout, exits cleanly).
    crate::tui::terminal::stop_stdin_reader();
    crate::tui::terminal::join_stdin_reader();

    // ── Run editor ──
    let status = std::process::Command::new(editor)
        .args(args)
        .arg(&tmp_file)
        .status();

    // ── Resume TUI ──
    let mut resume_buf = Vec::new();
    let _ = term.start(&mut resume_buf);
    let _ = term.hide_cursor(&mut resume_buf);
    if !resume_buf.is_empty() {
        let stdout = std::io::stdout();
        let mut handle = stdout.lock();
        let _ = handle.write_all(&resume_buf);
        let _ = handle.flush();
    }
    // Restart stdin reader (after raw mode is active)
    crate::tui::terminal::start_stdin_reader();
    // Force full redraw
    tui.request_render();

    match status {
        Ok(status) if status.success() => {
            if let Ok(new_content) = std::fs::read_to_string(&tmp_file) {
                let trimmed = new_content.trim_end_matches('\n').to_string();
                app.editor.borrow_mut().editor.set_text(&trimmed);
                app.editor.borrow_mut().check_autocomplete();
            }
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some("Editor closed".into());
        }
        Ok(_) => {
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some("Editor exited with non-zero status".into());
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_file);
            app.status_text = Some(format!("Failed to launch editor: {}", e));
        }
    }
}

/// Toggle auto-compact indicator (Ctrl+Shift+C).
/// Pi-compatible: syncs with AgentSession and persists to settings.
fn handle_compact_toggle(app: &mut App) {
    app.auto_compact = !app.auto_compact;
    app.footer.borrow_mut().set_auto_compact(app.auto_compact);

    // Sync with AgentSession (pi-compatible: compaction_settings.enabled)
    if let Some(ref mut s) = app.session {
        s.set_auto_compact(app.auto_compact);
    }

    // Persist to settings
    app.settings.set_auto_compact(Some(app.auto_compact));
    if let Err(e) = app.settings.save() {
        eprintln!("Warning: failed to save auto_compact setting: {}", e);
    }

    app.status_text = Some(if app.auto_compact {
        "Auto-compact: on".into()
    } else {
        "Auto-compact: off".into()
    });
}

/// Queue a follow-up message (Alt+Enter) during streaming.
/// Uses yoagent's native `follow_up()` — the agent loop's outer loop
/// picks it up naturally after the current inner loop finishes.
pub fn handle_follow_up(app: &mut App, text: String) {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return;
    }

    if app.is_streaming && app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
        let follow_msg = user_agent_message(&trimmed);
        if let Some(ref agent) = app.agent {
            agent.follow_up(follow_msg);
            app.status_text = Some("Follow-up queued — will send when agent finishes".into());
        }
        return;
    }

    // Agent is idle but has queued messages — drain and submit as batch.
    // This mirrors pi's continue() which drains steer/follow-up queues
    // when the last message is assistant.
    if let Some(ref agent) = app.agent
        && !agent.is_streaming()
        && (agent.steering_queue_len() > 0 || agent.follow_up_queue_len() > 0)
    {
        let mut msgs = agent.take_steering_queue();
        msgs.extend(agent.take_follow_up_queue());
        msgs.push(user_agent_message(&trimmed));
        app.pending_preloaded_msgs = Some(msgs);
        app.pending_submit = Some(trimmed);
        app.status_text = Some("Queued messages + follow-up will be sent next".into());
        return;
    }

    // Not streaming — submit directly
    if app.is_streaming {
        app.is_streaming = false;
    }
    submit_message(app, trimmed);
}

/// Interrupt streaming agent and restore queued messages to editor.
fn interrupt_streaming(app: &mut App) {
    // Cooperatively cancel the running agent loop (fires cancel token)
    if let Some(ref agent) = app.agent {
        agent.abort();
    }
    // Kill the forwarding task
    if let Some(handle) = app.forward_handle.take() {
        handle.abort();
    }
    if let Some(handle) = app.bash_abort_handle.take() {
        handle.abort();
    }
    // Drop the agent — its tools were moved into the aborted loop and are lost.
    // A fresh agent will be created from session on the next turn.
    app.agent = None;
    app.is_streaming = false;
    app.working.stop();
    app.footer.borrow_mut().set_streaming(false);

    // Rebuild chat from session (authoritative store after abort)
    if let Some(ref s) = app.session {
        let ctx = s.session().build_context();
        let mut chat = app.chat_container.borrow_mut();
        rebuild_chat_from_messages(
            &mut chat,
            &ctx.messages,
            &app.cwd.to_string_lossy(),
            app.hide_thinking,
            app.collapse_tool_output,
            &app.extensions,
        );
    }

    app.status_text = Some("Interrupted".into());
}

// ── Auth helpers ─────────────────────────────────────

/// Handle a login command result. If `api_key` is provided, stores it immediately
/// and performs post-login completion (model auto-selection, registry refresh).
fn handle_login(app: &mut App, provider: &str, api_key: Option<&str>) {
    let provider = if provider.is_empty() {
        "opencode-go"
    } else {
        provider
    };
    if let Some(key) = api_key {
        match auth::login(provider, key) {
            Ok(_) => {
                app.refresh_registry();
                // Post-login completion
                complete_login(
                    app,
                    provider,
                    crate::agent::ui::components::oauth_selector::AuthType::ApiKey,
                );
            }
            Err(e) => show_status(app, format!("Login failed: {}", e)),
        }
    } else {
        show_status(app, format!("Usage: /login {} <api-key>", provider));
    }
}

/// Handle a logout command result.
fn handle_logout(app: &mut App, provider: Option<&str>) {
    match auth::logout(provider) {
        Ok(true) => {
            let msg = provider
                .map(|p| format!("Logged out from {}", p))
                .unwrap_or_else(|| "Logged out from all providers".into());
            show_status(app, msg);
        }
        Ok(false) => {
            let msg = provider
                .map(|p| format!("No credentials for {}", p))
                .unwrap_or_else(|| "No credentials found".into());
            show_status(app, msg);
        }
        Err(e) => {
            show_status(app, format!("Logout failed: {}", e));
        }
    }
}

/// Show the login provider selector overlay, optionally filtered by auth type.
/// Shows all available providers from the registry for the user to pick one.
fn show_login_provider_selector(app: &mut App, tui: &mut TUI, auth_type: Option<AuthType>) {
    use crate::agent::ui::components::oauth_selector::{
        AuthSelectorProvider, AuthType, OAuthSelector, SelectorMode,
    };

    let all_providers = app.registry.list_providers();

    // Build the provider list, including OAuth providers from the OAuth registry
    let mut providers: Vec<AuthSelectorProvider> = Vec::new();

    // Add API key providers
    for (id, name) in all_providers {
        let is_oauth_provider = crate::provider::oauth::get(&id).is_some();
        match auth_type {
            Some(AuthType::ApiKey) => {
                // Skip OAuth-only providers (those not in models.json)
                if !is_oauth_provider {
                    providers.push(AuthSelectorProvider {
                        id,
                        name,
                        auth_type: AuthType::ApiKey,
                    });
                }
            }
            Some(AuthType::OAuth) => {
                // Only include OAuth providers
                if is_oauth_provider {
                    providers.push(AuthSelectorProvider {
                        id,
                        name,
                        auth_type: AuthType::OAuth,
                    });
                }
            }
            None => {
                providers.push(AuthSelectorProvider {
                    id,
                    name,
                    auth_type: if is_oauth_provider {
                        AuthType::OAuth
                    } else {
                        AuthType::ApiKey
                    },
                });
            }
        }
    }

    // Also add OAuth providers that aren't in models.json (e.g. only in OAuth registry)
    if auth_type != Some(AuthType::ApiKey) {
        for oauth_id in crate::provider::oauth::list_ids() {
            if !providers.iter().any(|p| p.id == oauth_id)
                && let Some(provider) = crate::provider::oauth::get(&oauth_id)
            {
                providers.push(AuthSelectorProvider {
                    id: oauth_id,
                    name: provider.name().to_string(),
                    auth_type: AuthType::OAuth,
                });
            }
        }
    }

    // Sort alphabetically by name for consistent display.
    providers.sort_by_key(|a| a.name.to_lowercase());

    if providers.is_empty() {
        app.status_text = Some(match auth_type {
            Some(AuthType::OAuth) => "No subscription providers available.".into(),
            Some(AuthType::ApiKey) => "No API key providers available.".into(),
            None => "No providers available.".into(),
        });
        return;
    }

    let signal = app.overlay_result_signal.clone();
    let mut selector = OAuthSelector::new(
        providers,
        |provider_id| app.registry.auth_status_for_provider(provider_id),
        SelectorMode::Login,
    );

    selector.on_select(move |provider_id: String| {
        *signal.borrow_mut() = Some(OverlayResult::LoginProviderSelected(provider_id));
    });
    selector.on_cancel(|| {});

    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Show the API key input dialog for a specific provider.
/// Uses LoginDialog which matches pi's LoginDialogComponent.
fn show_api_key_login_dialog(app: &mut App, tui: &mut TUI, provider_id: &str) {
    use crate::agent::ui::components::LoginDialog;

    // Find the provider name from the registry
    let provider_name = app
        .registry
        .list_providers()
        .into_iter()
        .find(|(id, _)| id == provider_id)
        .map(|(_, name)| name)
        .unwrap_or_else(|| provider_id.to_string());

    let mut dialog = LoginDialog::new(provider_id.to_string(), provider_name.clone());

    let signal = app.overlay_result_signal.clone();
    let provider_id_clone = provider_id.to_string();

    dialog.on_submit(move |api_key: String| {
        *signal.borrow_mut() = Some(OverlayResult::LoginApiKeyProvided {
            provider: provider_id_clone,
            key: api_key,
        });
    });

    dialog.on_cancel(|| {});

    dialog.show_prompt("Enter API key:", Some("sk-..."));

    tui.show_positioned_overlay(Box::new(dialog), crate::tui::OverlayPosition::Bottom);
}

/// Show the OAuth login dialog for a specific provider.
/// Matches pi's showLoginDialog for OAuth providers.
fn show_oauth_login_dialog(app: &mut App, tui: &mut TUI, provider_id: &str) {
    let provider_name = app
        .registry
        .list_providers()
        .into_iter()
        .find(|(id, _)| id == provider_id)
        .map(|(_, name)| name)
        .unwrap_or_else(|| {
            crate::provider::oauth::get(provider_id)
                .map(|p| p.name().to_string())
                .unwrap_or_else(|| provider_id.to_string())
        });

    app.status_text = Some(format!("Starting OAuth login for {}…", provider_name));
    tui.pop_overlay(); // close the provider selector overlay

    // Send progress updates through the agent event channel.
    // ProgressMessage with empty tool_name sets app.status_text (visible to user).
    let tx = app.event_tx.clone();
    let pid = provider_id.to_string();
    let pname = provider_name.clone();

    let tx2 = tx.clone();
    let tx3 = tx.clone();
    let tx4 = tx.clone();

    app.pending_oauth_provider = Some(pid.clone());

    let handle = tokio::spawn(async move {
        let oauth_provider = match crate::provider::oauth::get(&pid) {
            Some(p) => p,
            None => {
                let _ = tx.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!(
                        "OAuth login failed: No OAuth provider registered for '{}'",
                        pid
                    ),
                });
                return;
            }
        };

        let mut callbacks = crate::provider::oauth::OAuthLoginCallbacks {
            on_device_code: Box::new(move |info: crate::provider::oauth::DeviceCodeInfo| {
                let device_msg = format!(
                    "Open {} and enter code: {}",
                    info.verification_uri, info.user_code
                );
                // Show as status AND as a persistent chat message via ToolExecutionEnd
                let _ = tx.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: device_msg,
                });
            }),
            on_prompt: Box::new(
                move |prompt: crate::provider::oauth::OAuthPrompt| match prompt {
                    crate::provider::oauth::OAuthPrompt::Text {
                        message,
                        placeholder: _,
                        allow_empty: _,
                    } => {
                        // Log the prompt so users see it; empty response = default (github.com)
                        let _ = tx2.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("{} (empty = github.com)", message),
                        });
                        // For now, accept empty — GitHub Enterprise users need to
                        // set enterprise_url in credentials manually or via config.
                        Ok(String::new())
                    }
                },
            ),
            on_progress: Box::new(move |msg: String| {
                let _ = tx3.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!("[OAuth] {}", msg),
                });
            }),
            signal: None,
        };

        match oauth_provider.login(&mut callbacks).await {
            Ok(credentials) => {
                let cred = crate::provider::auth::AuthCredential::Oauth {
                    access: credentials.access.clone(),
                    refresh: Some(credentials.refresh.clone()),
                    expires: Some(credentials.expires),
                    enterprise_url: credentials.enterprise_url.clone(),
                };
                match crate::provider::auth::login_oauth(&pid, &cred) {
                    Ok(_) => {
                        let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("✓ Logged in to {} via OAuth", pname),
                        });
                    }
                    Err(e) => {
                        let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                            tool_call_id: String::new(),
                            tool_name: String::new(),
                            text: format!("Failed to save OAuth credentials: {}", e),
                        });
                    }
                }
            }
            Err(e) => {
                let _ = tx4.send(yoagent::types::AgentEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: String::new(),
                    text: format!("OAuth login failed: {}", e),
                });
            }
        }
    });
    app.oauth_join_handle = Some(handle);
}

/// Show the auth type selector overlay ("Use a subscription" vs "Use an API key").
/// Matches pi's showLoginAuthTypeSelector behavior.
fn show_auth_type_selector(app: &mut App, tui: &mut TUI) {
    // Build simple two-option selector
    let signal = app.overlay_result_signal.clone();
    let _theme = crate::agent::ui::theme::current_theme().clone();

    let mut items = vec![crate::tui::components::select_list::SelectItem::new(
        "api_key",
        "Use an API key",
    )];
    // Add OAuth option if any OAuth providers are registered
    let has_oauth = !crate::provider::oauth::list_ids().is_empty();
    if has_oauth {
        items.push(crate::tui::components::select_list::SelectItem::new(
            "oauth",
            "Use a subscription",
        ));
    }

    let filtered_indices: Vec<usize> = (0..items.len()).collect();
    let selected_index: usize = 0;

    struct AuthTypeOverlay {
        items: Vec<crate::tui::components::select_list::SelectItem>,
        selected_index: usize,
        filtered_indices: Vec<usize>,
        signal: std::rc::Rc<std::cell::RefCell<Option<OverlayResult>>>,
    }

    impl crate::tui::Component for AuthTypeOverlay {
        fn render(&mut self, width: usize) -> Vec<String> {
            let theme = crate::agent::ui::theme::current_theme();
            let mut lines = Vec::new();

            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));
            lines.push(String::new());
            lines.push(format!(
                "  {}",
                theme.bold(&theme.fg_key(ThemeKey::Accent, "Select authentication method:"))
            ));
            lines.push(String::new());

            for (i, &item_idx) in self.filtered_indices.iter().enumerate() {
                let item = &self.items[item_idx];
                let is_selected = i == self.selected_index;
                let prefix = if is_selected {
                    theme.fg_key(ThemeKey::Accent, "→ ")
                } else {
                    "  ".to_string()
                };
                let text = if is_selected {
                    theme.fg_key(ThemeKey::Accent, &item.label)
                } else {
                    theme.fg_key(ThemeKey::Text, &item.label)
                };
                lines.push(format!("{}{}", prefix, text));
            }

            lines.push(String::new());
            lines.push(format!("  {}", theme.dim("Enter: select · Esc: cancel")));
            lines.push(String::new());
            lines.push(theme.dim(&"─".repeat(width.saturating_sub(2))));

            lines
        }

        fn handle_input(&mut self, key: &crossterm::event::KeyEvent) -> bool {
            let kb = crate::tui::keybindings::get_keybindings();

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_UP) {
                if self.filtered_indices.is_empty() {
                    return true;
                }
                self.selected_index = if self.selected_index == 0 {
                    self.filtered_indices.len() - 1
                } else {
                    self.selected_index - 1
                };
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_DOWN) {
                if self.filtered_indices.is_empty() {
                    return true;
                }
                self.selected_index = if self.selected_index >= self.filtered_indices.len() - 1 {
                    0
                } else {
                    self.selected_index + 1
                };
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CONFIRM) {
                if let Some(&idx) = self.filtered_indices.get(self.selected_index) {
                    let value = self.items[idx].value.clone();
                    let auth_type = match value.as_str() {
                        "oauth" => AuthType::OAuth,
                        _ => AuthType::ApiKey,
                    };
                    *self.signal.borrow_mut() =
                        Some(OverlayResult::LoginAuthTypeSelected(auth_type));
                }
                return true;
            }

            if kb.matches(key, crate::tui::keybindings::ACTION_SELECT_CANCEL) {
                // Cancel — just close overlay
                return true;
            }

            false
        }
    }

    let overlay = AuthTypeOverlay {
        items,
        selected_index,
        filtered_indices,
        signal: signal.clone(),
    };

    tui.show_positioned_overlay(Box::new(overlay), crate::tui::OverlayPosition::Bottom);
}

/// Show auth type selector or go directly to provider list depending on
/// which auth types are available. Matches pi's logic.
fn show_auth_type_or_provider_selector(app: &mut App, tui: &mut TUI) {
    let providers = app.registry.list_providers();
    if providers.is_empty() {
        app.status_text = Some("No providers available for login.".into());
        return;
    }
    // Check if any OAuth providers are registered (from OAuth registry)
    let has_oauth = !crate::provider::oauth::list_ids().is_empty();
    let has_api_key = providers.iter().any(|(_, _)| true);
    if has_oauth && has_api_key {
        show_auth_type_selector(app, tui);
    } else if has_oauth {
        show_login_provider_selector(app, tui, Some(AuthType::OAuth));
    } else {
        show_login_provider_selector(app, tui, Some(AuthType::ApiKey));
    }
}

/// Show the logout provider selector overlay.
/// Shows only providers with stored credentials (matching pi's getLogoutProviderOptions).
fn show_logout_provider_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::oauth_selector::{
        AuthSelectorProvider, AuthType, OAuthSelector, SelectorMode,
    };

    // Get providers that have stored credentials
    let logged_in = auth::list_logged_in().unwrap_or_default();

    if logged_in.is_empty() {
        app.status_text = Some(
            "No stored credentials to remove. /logout only removes credentials saved by /login; \
             environment variables and models.json config are unchanged."
                .into(),
        );
        return;
    }

    let mut providers: Vec<AuthSelectorProvider> = logged_in
        .into_iter()
        .filter_map(|id| {
            app.registry
                .list_providers()
                .into_iter()
                .find(|(pid, _)| pid == &id)
                .map(|(pid, name)| AuthSelectorProvider {
                    id: pid,
                    name,
                    auth_type: AuthType::ApiKey,
                })
        })
        .collect();

    // Sort alphabetically by name for consistent display.
    providers.sort_by_key(|a| a.name.to_lowercase());

    if providers.is_empty() {
        // Providers with stored credentials may not be in registry anymore
        app.status_text = Some("No registered providers with stored credentials.".into());
        return;
    }

    let signal = app.overlay_result_signal.clone();
    let mut selector = OAuthSelector::new(
        providers,
        |provider_id| app.registry.auth_status_for_provider(provider_id),
        SelectorMode::Logout,
    );

    selector.on_select(move |provider_id: String| {
        *signal.borrow_mut() = Some(OverlayResult::LogoutProviderSelected(provider_id));
    });
    selector.on_cancel(|| {});

    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Post-login completion: auto-select default model for the provider.
/// Matches pi's completeProviderAuthentication logic for API key login.
fn complete_login(app: &mut App, provider_id: &str, _auth_type: AuthType) {
    // Try to select the default model for this provider
    let available_models = app.registry.list_model_provider_tuples();
    let provider_models: Vec<&str> = available_models
        .iter()
        .filter(|(pid, _, _)| pid == provider_id)
        .map(|(_, mid, _)| mid.as_str())
        .collect();

    if provider_models.is_empty() {
        app.status_text = Some(format!(
            "Saved API key for {provider_id}. No models available for this provider. Use /model to select a model."
        ));
        return;
    }

    // If current model is unknown or doesn't belong to this provider, select first available
    let current_provider = app
        .registry
        .provider_for_model(&app.model, Some(&app.current_provider))
        .unwrap_or_default();

    if current_provider != provider_id || !app.available_models.contains(&app.model) {
        let first_model = provider_models[0];
        app.model = first_model.to_string();
        app.current_provider = provider_id.to_string();
        let model = app.model.clone();
        app.record_model_change(&model);
        app.status_text = Some(format!(
            "Saved API key for {provider_id}. Selected {first_model}."
        ));
    } else {
        app.status_text = Some(format!("Saved API key for {provider_id}."));
    }
}

/// Open the model selector overlay.
fn open_model_selector(app: &mut App, tui: &mut TUI) {
    let current = app.model.clone();

    // Build (provider, model_id, name) tuples from authenticated providers only.
    // This matches pi's behavior of showing only models from configured providers.
    let all_tuples: Vec<(String, String, String)> = app.registry.list_model_provider_tuples();
    let all_models: Vec<(String, String, String)> = all_tuples
        .into_iter()
        .filter(|(provider, _, _)| app.registry.provider_has_auth(provider))
        .collect();

    let scoped_ids = app.scoped_model_ids.clone().unwrap_or_default();

    let signal = app.overlay_result_signal.clone();
    let current_provider = app
        .registry
        .provider_for_model(&current, Some(&app.current_provider))
        .unwrap_or_else(|| "unknown".to_string());
    let current_full_id = format!("{}/{}", current_provider, current);

    let callbacks = crate::agent::ui::model_selector::ModelSelectorCallbacks {
        on_select: Box::new({
            let signal = signal.clone();
            move |full_id: String| {
                *signal.borrow_mut() = Some(OverlayResult::ModelSelected(full_id));
            }
        }),
        on_cancel: Box::new(|| {}), // No-op: overlay is popped by handle_input returning false
    };

    let selector = crate::agent::ui::model_selector::ModelSelector::new(
        all_models,
        scoped_ids,
        current_full_id,
        callbacks,
    );
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Open the settings overlay.
fn open_settings(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::settings_selector::{SettingsCallbacks, SettingsSelector};

    let available_themes = theme::get_available_themes();

    let items = SettingsSelector::build_items(
        app.auto_compact,
        app.hide_thinking,
        app.collapse_tool_output,
        &app.thinking_level,
        &app.theme.name,
        &available_themes,
        &app.settings.default_provider,
        &app.model,
        &app.settings.transport,
        &app.settings.steering_mode,
        &app.settings.follow_up_mode,
        &app.settings.quiet_startup,
        &app.settings.collapse_changelog,
        &app.settings.enable_skill_commands,
        &app.settings.enable_install_telemetry,
        &app.settings.double_escape_action,
        &app.settings.tree_filter_mode,
        &app.settings.show_hardware_cursor,
        &app.settings.editor_padding_x,
        &app.settings.output_pad,
        &app.settings.autocomplete_max_visible,
        app.settings.verbose,
        &app.settings.default_project_trust,
        &app.settings.http_idle_timeout_ms,
        &app.settings
            .terminal
            .as_ref()
            .and_then(|t| t.clear_on_shrink),
        &app.settings
            .terminal
            .as_ref()
            .and_then(|t| t.show_terminal_progress),
        &app.settings
            .warnings
            .as_ref()
            .and_then(|w| w.anthropic_extra_usage),
        &app.settings.shell_command_prefix,
        &app.settings.shell_path,
        &app.settings.external_editor,
        &app.settings.http_proxy,
        &app.settings.session_dir,
    );

    let change_signal = app.pending_settings_change.clone();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = SettingsCallbacks {
        on_change: Box::new({
            let cs = change_signal.clone();
            move |id: String, new_value: String| {
                *cs.borrow_mut() = Some((id, new_value));
            }
        }),
        on_cancel: Box::new({
            let signal = close_signal.clone();
            move || {
                *signal.borrow_mut() = Some(OverlayResult::Dismiss);
            }
        }),
    };

    let selector = SettingsSelector::new(items, callbacks);
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

/// Open the extension configuration overlay.
fn open_extensions_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::extensions_selector::{
        ExtensionInfo, ExtensionsCallbacks, ExtensionsSelector,
    };

    // Build ExtensionInfo for each loaded extension
    let extensions: Vec<ExtensionInfo> = app
        .extensions
        .iter()
        .map(|ext| {
            let tools = ext.tools();
            let tool_names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
            let commands = ext.commands();
            let command_names: Vec<String> = commands.iter().map(|c| c.name.clone()).collect();
            let skills = ext.skills();
            let skill_names: Vec<String> = skills.skills().iter().map(|s| s.name.clone()).collect();

            let name = ext.name().to_string();
            let default_state = ext.default_state();

            // Determine current enabled state
            let enabled = crate::extension::is_extension_enabled(ext.as_ref(), &app.settings);

            ExtensionInfo {
                name,
                default_state,
                enabled,
                tool_names,
                command_names,
                skill_names,
            }
        })
        .collect();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = ExtensionsCallbacks {
        on_toggle: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move |name: String, enabled: bool| {
                let app = unsafe { &mut *app_ptr };
                // Update in-memory settings
                app.settings.set_extension_enabled(&name, enabled);

                // Rebuild system prompt, commands, and skills from current extensions
                refresh_agent_config(app);
            }
        }),
        on_save_global: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move || {
                let app = unsafe { &mut *app_ptr };
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save extensions globally: {}", e));
                } else {
                    app.status_text = Some("Extensions saved globally".into());
                }
            }
        }),
        on_save_project: Box::new({
            let app_ptr: *mut App = app as *mut App;
            move || {
                let app = unsafe { &mut *app_ptr };
                if let Err(e) = app.settings.save_to_project(&app.cwd) {
                    app.status_text = Some(format!("Failed to save extensions in project: {}", e));
                } else {
                    app.status_text = Some("Extensions saved in project".into());
                }
            }
        }),
        on_cancel: Box::new({
            let signal = close_signal.clone();
            move || {
                *signal.borrow_mut() = Some(OverlayResult::Dismiss);
            }
        }),
    };

    let selector = ExtensionsSelector::new(extensions, callbacks);
    use crate::tui::overlay::{OverlayAnchor, OverlayOptions, SizeValue};
    tui.show_overlay(
        Box::new(selector),
        OverlayOptions {
            width: Some(SizeValue::Percent(100.0)),
            anchor: Some(OverlayAnchor::BottomLeft),
            offset_y: Some(-5),
            ..Default::default()
        },
    );
}

/// Refresh all agent-facing configuration (system prompt, tools, commands,
/// skills, autocomplete) from the current extension enablement state.
///
/// This is the single method for rebuilding everything that depends on which
/// extensions are enabled. Called by:
/// - `/extensions` toggle (extension enablement changed)
/// - `/reload` handler (files on disk may have changed)
fn refresh_agent_config(app: &mut App) {
    fn is_enabled(ext: &dyn Extension, settings: &crate::settings::Settings) -> bool {
        crate::extension::is_extension_enabled(ext, settings)
    }

    // ── Collect tools from enabled extensions ────────────────────
    let enabled_exts: Vec<&dyn Extension> = app
        .extensions
        .iter()
        .filter(|ext| is_enabled(ext.as_ref(), &app.settings))
        .map(|b| b.as_ref())
        .collect();

    let all_tools: Vec<crate::extension::ToolDefinition> =
        enabled_exts.iter().flat_map(|ext| ext.tools()).collect();
    let tool_snippets: Vec<crate::agent::ToolSnippet> = all_tools
        .iter()
        .map(|twm| crate::agent::ToolSnippet {
            name: twm.name().to_string(),
            description: twm.snippet.to_string(),
        })
        .collect();

    let tool_guidelines: Vec<String> = all_tools
        .iter()
        .flat_map(|twm| twm.guidelines.iter().copied())
        .map(|s| s.to_string())
        .collect();

    let has_read_tool = tool_snippets.iter().any(|t| t.name == "read");

    // ── Re-register hooks from enabled extensions ────────────────
    use crate::extension::HookRegistration;
    let enabled_hooks: Vec<HookRegistration> = enabled_exts
        .iter()
        .flat_map(|ext| ext.tool_hooks())
        .collect();
    crate::extension::clear_tool_hooks();
    crate::extension::register_tool_hooks(&enabled_hooks);

    // ── Collect skills: disk-loaded + enabled extensions ─────────
    let mut all_skills = yoagent::skills::SkillSet::load(&app.skill_dirs).unwrap_or_default();
    for ext in &enabled_exts {
        all_skills.merge(ext.skills());
    }
    app.skills = all_skills.skills().to_vec();

    // ── Rebuild commands list and editor autocomplete ────────────
    app.commands = enabled_exts
        .iter()
        .flat_map(|e| e.commands())
        .map(|c| (c.name, c.description))
        .collect();
    for skill in &app.skills {
        app.commands
            .push((format!("skill:{}", skill.name), skill.description.clone()));
    }
    for template in &app.prompt_templates {
        app.commands
            .push((template.name.clone(), template.description.clone()));
    }

    // Rebuild editor autocomplete
    {
        use crate::tui::autocomplete::AutocompleteItem as AutoAutocompleteItem;
        use crate::tui::autocomplete::SlashCommand as AutoSlashCommand;

        let mut auto_commands: Vec<AutoSlashCommand> = enabled_exts
            .iter()
            .flat_map(|e| e.commands())
            .map(|cmd| {
                let handler = cmd.handler;
                AutoSlashCommand {
                    name: cmd.name,
                    description: Some(cmd.description),
                    argument_hint: None,
                    argument_completions: None,
                    get_argument_completions: Some(std::sync::Arc::new(
                        move |prefix: &str| -> Vec<AutoAutocompleteItem> {
                            handler
                                .argument_completions(prefix)
                                .into_iter()
                                .map(|item| AutoAutocompleteItem {
                                    value: item.value,
                                    label: item.label,
                                    description: item.description,
                                })
                                .collect()
                        },
                    )),
                }
            })
            .collect();

        // Re-register /skill:name commands
        for skill in &app.skills {
            let cmd_name = format!("skill:{}", skill.name);
            auto_commands.push(AutoSlashCommand {
                name: cmd_name,
                description: Some(skill.description.clone()),
                argument_hint: None,
                argument_completions: None,
                get_argument_completions: None,
            });
        }

        // Re-register prompt template commands
        for template in &app.prompt_templates {
            auto_commands.push(AutoSlashCommand {
                name: template.name.clone(),
                description: Some(template.description.clone()),
                argument_hint: template.argument_hint.clone(),
                argument_completions: None,
                get_argument_completions: None,
            });
        }

        app.editor.borrow_mut().set_slash_commands(auto_commands);
    }

    // ── Reload context files and system prompts from disk ────────
    let context_files = crate::agent::context_files::load_context_files(&app.cwd, &app.agent_dir);

    let custom_system_md = {
        let project_path = app.cwd.join(".rab").join("SYSTEM.md");
        if project_path.exists() {
            std::fs::read_to_string(&project_path).ok()
        } else {
            let global_path = app.agent_dir.join("SYSTEM.md");
            if global_path.exists() {
                std::fs::read_to_string(&global_path).ok()
            } else {
                None
            }
        }
    };
    let append_system_md = {
        let project_path = app.cwd.join(".rab").join("APPEND_SYSTEM.md");
        if project_path.exists() {
            std::fs::read_to_string(&project_path).ok()
        } else {
            let global_path = app.agent_dir.join("APPEND_SYSTEM.md");
            if global_path.exists() {
                std::fs::read_to_string(&global_path).ok()
            } else {
                None
            }
        }
    };

    // Update header context file display names
    let context_file_list: Vec<String> = context_files
        .iter()
        .map(|cf| crate::util::paths::format_for_display(&cf.path, &app.cwd))
        .collect();
    app.context_files = context_file_list;

    // ── Build system prompt ──────────────────────────────────────
    let new_system_prompt = crate::agent::SystemPromptBuilder::new()
        .tool_snippets(tool_snippets)
        .guidelines(tool_guidelines)
        .context_files(context_files)
        .custom_prompt(custom_system_md)
        .append_prompt(append_system_md)
        .skills(all_skills)
        .has_read_tool(has_read_tool)
        .cwd(&app.cwd)
        .build();
    app.system_prompt = new_system_prompt;
}

/// Apply a setting change from the settings menu.
/// Updates app state and persists to settings.
fn apply_settings_change(app: &mut App, id: &str, new_value: &str) {
    match id {
        "autocompact" => {
            app.auto_compact = new_value == "true";
            app.footer.borrow_mut().set_auto_compact(app.auto_compact);
            if let Some(ref mut s) = app.session {
                s.set_auto_compact(app.auto_compact);
            }
            app.settings.set_auto_compact(Some(app.auto_compact));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save auto-compact: {}", e));
            }
        }
        "hide-thinking" => {
            app.hide_thinking = new_value == "true";
            app.propagate_hide_thinking();
            app.settings.set_hide_thinking(Some(app.hide_thinking));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save hide thinking: {}", e));
            }
        }
        "collapse-tool-output" => {
            app.collapse_tool_output = new_value == "true";
            app.tools_expanded = !app.collapse_tool_output;
            app.header.borrow_mut().set_expanded(app.tools_expanded);
            {
                let mut chat = app.chat_container.borrow_mut();
                for child in chat.children_mut().iter_mut() {
                    child.set_expanded(app.tools_expanded);
                }
            }
            app.settings
                .set_collapse_tool_output(Some(app.collapse_tool_output));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save tool output setting: {}", e));
            }
        }
        "thinking-level" => {
            let level = if new_value == "off" {
                None
            } else {
                Some(new_value.to_string())
            };
            app.thinking_level = level.clone();
            app.footer.borrow_mut().set_thinking_level(level.clone());
            app.settings.set_default_thinking_level(level);
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save thinking level: {}", e));
            }
        }
        "theme" => {
            if theme::set_theme(new_value).is_ok() {
                app.theme = theme::current_theme().clone();
                // Persist to settings + save
                app.settings.theme = Some(new_value.to_string());
                app.settings.mark_modified("theme");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save theme: {}", e));
                }
                app.status_text = Some(format!("Theme: {}", new_value));
            }
        }
        "transport" => {
            app.settings.transport = Some(new_value.to_string());
            app.settings.mark_modified("transport");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save transport: {}", e));
            }
        }
        "steering-mode" => {
            app.settings.steering_mode = Some(new_value.to_string());
            app.settings.mark_modified("steeringMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save steering mode: {}", e));
            }
        }
        "follow-up-mode" => {
            app.settings.follow_up_mode = Some(new_value.to_string());
            app.settings.mark_modified("followUpMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save follow-up mode: {}", e));
            }
        }
        "quiet-startup" => {
            app.settings.quiet_startup = Some(new_value == "true");
            app.settings.mark_modified("quietStartup");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save quiet startup: {}", e));
            }
        }
        "collapse-changelog" => {
            app.settings.collapse_changelog = Some(new_value == "true");
            app.settings.mark_modified("collapseChangelog");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save collapse changelog: {}", e));
            }
        }
        "verbose" => {
            app.settings.verbose = new_value == "true";
            app.settings.mark_modified("verbose");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save verbose: {}", e));
            }
        }
        "double-escape-action" => {
            app.settings.double_escape_action = Some(new_value.to_string());
            app.settings.mark_modified("doubleEscapeAction");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save double-escape action: {}", e));
            }
        }
        "tree-filter-mode" => {
            app.settings.tree_filter_mode = Some(new_value.to_string());
            app.settings.mark_modified("treeFilterMode");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save tree filter mode: {}", e));
            }
        }
        "show-hardware-cursor" => {
            app.settings.show_hardware_cursor = Some(new_value == "true");
            app.settings.mark_modified("showHardwareCursor");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save hardware cursor: {}", e));
            }
        }
        "editor-padding" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.editor_padding_x = Some(v);
                app.settings.mark_modified("editorPaddingX");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save editor padding: {}", e));
                }
            }
        }
        "output-padding" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.output_pad = Some(v);
                app.settings.mark_modified("outputPad");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save output padding: {}", e));
                }
            }
        }
        "autocomplete-max-visible" => {
            if let Ok(v) = new_value.parse::<i32>() {
                app.settings.autocomplete_max_visible = Some(v);
                app.settings.mark_modified("autocompleteMaxVisible");
                if let Err(e) = app.settings.save() {
                    app.status_text = Some(format!("Failed to save autocomplete max: {}", e));
                }
            }
        }
        "skill-commands" => {
            app.settings.enable_skill_commands = Some(new_value == "true");
            app.settings.mark_modified("enableSkillCommands");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save skill commands: {}", e));
            }
        }
        "install-telemetry" => {
            app.settings.enable_install_telemetry = Some(new_value == "true");
            app.settings.mark_modified("enableInstallTelemetry");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save install telemetry: {}", e));
            }
        }
        "default-project-trust" => {
            app.settings.default_project_trust = Some(new_value.to_string());
            app.settings.mark_modified("defaultProjectTrust");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save project trust: {}", e));
            }
        }
        "http-idle-timeout" => {
            let ms = match new_value {
                "30 sec" => 30_000u64,
                "1 min" => 60_000,
                "2 min" => 120_000,
                "5 min" => 300_000,
                "disabled" => 0,
                _ => 30_000,
            };
            app.settings.http_idle_timeout_ms = Some(ms);
            app.settings.mark_modified("httpIdleTimeoutMs");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save HTTP idle timeout: {}", e));
            }
        }
        "clear-on-shrink" => {
            let val = new_value == "true";
            let terminal = app.settings.terminal.get_or_insert_with(Default::default);
            terminal.clear_on_shrink = Some(val);
            app.settings
                .mark_nested_modified("terminal", "clearOnShrink");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save clear on shrink: {}", e));
            }
        }
        "terminal-progress" => {
            let val = new_value == "true";
            let terminal = app.settings.terminal.get_or_insert_with(Default::default);
            terminal.show_terminal_progress = Some(val);
            app.settings
                .mark_nested_modified("terminal", "showTerminalProgress");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save terminal progress: {}", e));
            }
        }
        "warnings-anthropic-extra-usage" => {
            let val = new_value == "true";
            let warnings = app.settings.warnings.get_or_insert_with(Default::default);
            warnings.anthropic_extra_usage = Some(val);
            app.settings
                .mark_nested_modified("warnings", "anthropicExtraUsage");
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save warning setting: {}", e));
            }
        }
        _ => {}
    }
}

/// Open the scoped-models selector overlay.
fn open_scoped_models_selector(app: &mut App, tui: &mut TUI) {
    use crate::agent::ui::components::scoped_models_selector::{
        ModelsCallbacks, ModelsConfig, ScopedModelsSelector,
    };

    // Build (provider, model_id, name) tuples from authenticated providers only.
    let all_tuples: Vec<(String, String, String)> = app.registry.list_model_provider_tuples();
    let all_models: Vec<(String, String, String)> = all_tuples
        .into_iter()
        .filter(|(provider, _, _)| app.registry.provider_has_auth(provider))
        .collect();

    let current_enabled = app.scoped_model_ids.clone();
    let change_signal = app.pending_scoped_ids.clone();
    let close_signal = app.overlay_result_signal.clone();

    let callbacks = ModelsCallbacks {
        on_change: Box::new(move |enabled_ids: Option<Vec<String>>| {
            // Session-only update — does NOT close the overlay.
            *change_signal.borrow_mut() = Some(enabled_ids.unwrap_or_default());
        }),
        on_persist: Box::new({
            let cs = close_signal.clone();
            move |enabled_ids: Option<Vec<String>>| {
                *cs.borrow_mut() = Some(OverlayResult::ScopedModelsAccepted(enabled_ids));
            }
        }),
        on_cancel: Box::new(move || {
            *close_signal.borrow_mut() = Some(OverlayResult::ScopedModelsCancelled);
        }),
    };

    let config = ModelsConfig {
        all_models,
        enabled_model_ids: current_enabled,
    };

    let selector = ScopedModelsSelector::new(config, callbacks);
    tui.show_positioned_overlay(Box::new(selector), crate::tui::OverlayPosition::Bottom);
}

fn show_help_overlay(app: &mut App, tui: &mut TUI) {
    let mut overlay = crate::agent::ui::help::HelpOverlay::new(&app.theme);
    overlay.set_commands(app.commands.clone());
    overlay.set_dismiss_signal(app.overlay_result_signal.clone());
    tui.show_overlay(Box::new(overlay), Default::default());
}

/// Submit or queue a user message.
/// When streaming, sets pending_submit which is deferred until the current
/// turn finishes (the main loop skips start_agent_loop while is_streaming).
/// When idle, starts a new agent loop immediately.
fn submit_message(app: &mut App, message: String) {
    let trimmed = message.trim().to_string();

    // Don't submit empty messages (pi-style)
    if trimmed.is_empty() {
        return;
    }

    // Step 1: Expand /skill:name [args] (pi-style: skill before template)
    let after_skill = if trimmed.starts_with("/skill:") {
        expand_skill_command(&trimmed, &app.skills)
    } else {
        trimmed.clone()
    };

    // Step 2: Expand prompt templates (/name) on the result (pi-compatible order)
    let expanded =
        crate::agent::prompt_templates::expand_prompt_template(&after_skill, &app.prompt_templates);

    // If anything expanded (skill or template), submit the expanded content
    if expanded != after_skill || after_skill != trimmed {
        // Handle streaming for expanded content (same logic as below)
        if app.is_streaming && app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
            let steer_msg = user_agent_message(&expanded);
            if let Some(ref agent) = app.agent {
                agent.steer(steer_msg);
                app.status_text = Some("Skill/template steering message sent".into());
            }
            return;
        }
        if app.is_streaming {
            // Stale streaming flag — reset
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
        }
        app.pending_submit = Some(expanded);
        return;
    }

    // Handle /commands (need TUI from app for overlays)
    if trimmed.starts_with('/') {
        handle_slash_command(app, &trimmed);
        return;
    }

    // Handle ! and !! bang commands
    if let Some((cmd, _exclude)) = parse_bang_command(&trimmed) {
        handle_bang_command(app, cmd);
        return;
    }

    if app.is_streaming {
        // When streaming, queue via steer(). The agent loop picks it up
        // between tool calls or after the current assistant turn, then
        // continues processing. Do NOT add to chat here — MessageStart
        // handler adds it when the agent loop processes the queued message.
        if app.agent.as_ref().is_some_and(|a| a.is_streaming()) {
            let steer_msg = user_agent_message(&trimmed);
            if let Some(ref agent) = app.agent {
                agent.steer(steer_msg);
                app.status_text = Some("Steering message sent — will be processed next".into());
            }
            // Reset overflow recovery for the steer'd message
            if let Some(ref mut s) = app.session {
                s.reset_overflow_recovery();
            }
            return; // Don't set pending_submit — agent loop handles this
        } else {
            // Stale streaming flag — agent task finished but is_streaming
            // not reset. Fall through to normal submission path.
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
        }
    }

    // Pi-compatible: reset overflow recovery state at the start of each turn
    if let Some(ref mut s) = app.session {
        s.reset_overflow_recovery();
    }

    // Queue for async start in the main loop
    app.pending_submit = Some(trimmed);
}

/// Build a `yoagent::RetryConfig` from rab's user-facing settings.
fn retry_config_from_settings(settings: &crate::settings::Settings) -> yoagent::RetryConfig {
    let Some(r) = &settings.retry else {
        return yoagent::RetryConfig::default();
    };

    if r.enabled == Some(false) {
        return yoagent::RetryConfig::none();
    }

    let max_retries = r.max_retries.map(|v| v as usize).unwrap_or(3);
    let initial_delay_ms = r.base_delay_ms.unwrap_or(1000);
    let max_delay_ms = r
        .provider
        .as_ref()
        .and_then(|p| p.max_retry_delay_ms)
        .unwrap_or(30_000);

    yoagent::RetryConfig {
        max_retries,
        initial_delay_ms,
        max_delay_ms,
        ..yoagent::RetryConfig::default()
    }
}

/// Build a fresh Agent from the App's current configuration.
fn build_fresh_agent(
    app: &App,
    api_key: &str,
    messages: Vec<yoagent::types::AgentMessage>,
) -> yoagent::agent::Agent {
    use yoagent::provider::model::ApiProtocol;

    let preferred = if !app.current_provider.is_empty() {
        Some(app.current_provider.as_str())
    } else {
        app.settings.default_provider.as_deref()
    };

    let resolved = app.registry.resolve(&app.model, preferred).ok();
    let mut mc = resolved
        .as_ref()
        .map(|r| r.model_config.clone())
        .unwrap_or_else(|| crate::agent::base_model_config(&app.model));
    let api_key = resolved
        .as_ref()
        .map(|r| r.api_key.as_str())
        .filter(|k| !k.is_empty())
        .unwrap_or(api_key);

    // Inject provider attribution/session headers (pi-compatible).
    let session_id = app.session.as_ref().map(|s| s.session_id());
    let enable_telemetry = app.settings.enable_install_telemetry.unwrap_or(false);
    crate::provider::inject_provider_attribution_headers(
        &mut mc,
        session_id.as_deref(),
        enable_telemetry,
    );

    let rab_compat = resolved
        .as_ref()
        .map(|r| r.rab_compat.clone())
        .unwrap_or_default();

    let tools: Vec<Box<dyn yoagent::types::AgentTool>> = app
        .extensions
        .iter()
        .filter(|ext| crate::extension::is_extension_enabled(ext.as_ref(), &app.settings))
        .flat_map(|ext| ext.tools())
        .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
        .collect();

    let agent = match mc.api {
        ApiProtocol::OpenAiCompletions => yoagent::agent::Agent::from_provider(
            crate::provider::openai_compat::RabOpenAiCompatProvider::new(rab_compat),
            mc.clone(),
        ),
        ApiProtocol::AnthropicMessages => yoagent::agent::Agent::from_provider(
            crate::provider::anthropic::RabAnthropicProvider,
            mc.clone(),
        ),
        ApiProtocol::OpenAiResponses => yoagent::agent::Agent::from_config(mc.clone()),
        ApiProtocol::GoogleGenerativeAi => yoagent::agent::Agent::from_config(mc.clone()),
        _ => yoagent::agent::Agent::from_config(mc.clone()),
    };

    let retry_config = retry_config_from_settings(&app.settings);
    let thinking_level = map_thinking_level(app.thinking_level.as_deref());

    let context_window = mc.context_window;
    let execution_limits = yoagent::context::ExecutionLimits {
        max_total_tokens: usize::MAX,
        max_turns: usize::MAX,
        max_duration: std::time::Duration::from_secs(u64::MAX),
    };
    let context_config = yoagent::context::ContextConfig::from_context_window(context_window);

    agent
        .with_api_key(api_key)
        .with_system_prompt(&app.system_prompt)
        .with_thinking(thinking_level)
        .with_retry_config(retry_config)
        .with_messages(messages)
        .with_tools(tools)
        .with_context_config(context_config)
        .with_execution_limits(execution_limits)
        .on_before_turn({
            let stop_flag = app.stop_requested.clone();
            move |_, _| !stop_flag.load(Ordering::Relaxed)
        })
}

/// Map rab's thinking level string to yoagent's ThinkingLevel enum.
fn map_thinking_level(level: Option<&str>) -> yoagent::types::ThinkingLevel {
    match level {
        Some("off") => yoagent::types::ThinkingLevel::Off,
        Some("low") => yoagent::types::ThinkingLevel::Low,
        Some("medium") => yoagent::types::ThinkingLevel::Medium,
        Some("high") | Some("max") | Some("xhigh") => yoagent::types::ThinkingLevel::High,
        _ => yoagent::types::ThinkingLevel::High,
    }
}

/// Start an agent turn asynchronously. Called from the main loop only when
/// the agent is idle (the main loop guards with `!app.is_streaming`).
/// Reuses the existing agent across turns (single-agent model) so that
/// steer/follow-up queues and in-flight tool state survive across turns.
/// If no agent exists yet (first turn), creates a fresh one.
/// Messages are always synced from the session (error-filtered source) at
/// the start of each turn to avoid leaking transient provider errors.
async fn start_agent_loop(
    app: &mut App,
    message: String,
    preloaded: Option<Vec<yoagent::types::AgentMessage>>,
) {
    if app.session.is_none() {
        return;
    }

    // Reset stop flag — new turn starting
    app.stop_requested.store(false, Ordering::Relaxed);

    // Compose preloaded messages from all sources
    let mut all_preloaded: Vec<yoagent::types::AgentMessage> = Vec::new();
    // 1. Next-turn queue (queued while idle via /nextTurn)
    all_preloaded.append(&mut app.next_turn_queue);
    // 2. Saved queued messages from a previous stop-requested
    all_preloaded.append(&mut app.saved_queued_msgs);
    // 3. Explicit preloaded (from steer/follow-up drain at idle)
    if let Some(msgs) = preloaded {
        all_preloaded.extend(msgs);
    }

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);

    // Build or reuse agent. On the first turn the session has no messages;
    // on subsequent turns the reused agent already has messages restored
    // by agent.finish() — no need to sync from session here.
    let msgs = app
        .session
        .as_ref()
        .map(|s| s.session().build_context().messages)
        .unwrap_or_default();

    // Record model/thinking changes in the session before borrowing agent
    let model = app.model.clone();
    app.record_model_change(&model);
    if let Some(ref mut session) = app.session {
        session.on_thinking_level_change(app.thinking_level.as_deref().unwrap_or("off"));
    }

    // Refresh OAuth token if expired (e.g. GitHub Copilot tokens live ~15 min).
    // This covers both the first turn (token expired before rab started) and
    // subsequent turns (token expired mid-session).
    let fresh_oauth_key = {
        let provider = app.current_provider.clone();
        if crate::provider::oauth::is_built_in(&provider) {
            crate::provider::auth::refresh_oauth_token(&provider).await
        } else {
            None
        }
    };

    let agent: &mut yoagent::agent::Agent = match &mut app.agent {
        Some(existing) => {
            // Reuse existing agent — messages are already correct from
            // agent.finish(). Compaction sync is handled separately by
            // handle_auto_compact / handle_compact_command.
            // Update api_key in case the OAuth token was just refreshed.
            if let Some(ref key) = fresh_oauth_key {
                existing.api_key = key.clone();
            }
            existing
        }
        None => {
            let fallback_key = fresh_oauth_key.as_deref().unwrap_or(&app.api_key);
            app.agent = Some(build_fresh_agent(app, fallback_key, msgs));
            // SAFETY: we just set app.agent to Some(...)
            app.agent.as_mut().unwrap()
        }
    };

    // Apply steering/follow-up queue modes from settings (pi-compatible)
    if let Some(ref mode_str) = app.settings.steering_mode {
        let mode = match mode_str.as_str() {
            "all" => yoagent::agent::QueueMode::All,
            _ => yoagent::agent::QueueMode::OneAtATime,
        };
        agent.set_steering_mode(mode);
    }
    if let Some(ref mode_str) = app.settings.follow_up_mode {
        let mode = match mode_str.as_str() {
            "all" => yoagent::agent::QueueMode::All,
            _ => yoagent::agent::QueueMode::OneAtATime,
        };
        agent.set_follow_up_mode(mode);
    }

    // Start the turn.
    // When preloaded messages are provided, use prompt_messages so they're
    // all injected in one turn; the main message is appended to the list.
    let user_msg = user_agent_message(&message);
    let mut rx = if !all_preloaded.is_empty() {
        all_preloaded.push(user_msg);
        agent.prompt_messages(all_preloaded).await
    } else {
        agent.prompt(message).await
    };

    // Forward events from the agent's receiver to the UI channel.
    // This runs concurrently while the agent loop processes the turn.
    let tx = app.event_tx.clone();
    let handle = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    app.forward_handle = Some(handle);
}

/// Handle manual compaction asynchronously.
/// Called from the main loop when pending_compact is set.
async fn handle_compact_command(app: &mut App, custom_instructions: Option<String>) {
    if app.session.is_none() {
        show_status(app, "No active session to compact".to_string());
        return;
    }

    // Pi-compatible: disconnect from agent and abort streaming before compact.
    // This ensures compact runs in a consistent state (pi's compact() calls
    // _disconnectFromAgent() + abort() as its first internal steps).
    if app.is_streaming {
        interrupt_streaming(app);
    }

    let agent_session = app.session.as_mut().unwrap();

    app.working.start();

    match agent_session
        .run_manual_compact(custom_instructions.as_deref())
        .await
    {
        Ok(summary) => {
            app.working.stop();
            app.status_text = None;
            if summary.is_empty() {
                // Nothing was compacted — check why (matching pi)
                let entries = agent_session.session().get_entries();
                let reason = if entries.is_empty() {
                    "Nothing to compact (session too small)"
                } else if false {
                    "Already compacted"
                } else {
                    "Nothing to compact (session too small)"
                };
                show_status(app, reason.to_string());
            } else {
                app.rebuild_from_session_context();
                show_status(app, "Compaction completed".to_string());
            }
        }
        Err(e) => {
            app.working.stop();
            app.status_text = None;
            show_status(app, format!("Compaction failed: {}", e));
        }
    }
}

/// Pi-compatible: auto-compaction check after agent ends.
/// Calls `check_auto_compact()` on the session. If compaction was performed,
/// rebuilds the chat from the updated session context and updates agent state.
async fn handle_auto_compact(app: &mut App) {
    if app.session.is_none() {
        return;
    }

    let agent_session = app.session.as_mut().unwrap();

    match agent_session.check_auto_compact().await {
        Ok(true) => {
            app.rebuild_from_session_context();
            // Refresh footer stats (token counts may have changed)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
            app.status_text = Some("Auto-compaction completed".to_string());
        }
        Ok(false) => {
            // No compaction needed — nothing to do
        }
        Err(e) => {
            eprintln!("Warning: Auto-compaction failed: {}", e);
            app.status_text = Some(format!("Auto-compaction skipped: {}", e));
        }
    }
}

/// Handle keyboard input for the session picker.
fn handle_session_picker_input(app: &mut App, key: &crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;

    let Some(ref mut picker) = app.session_picker else {
        return;
    };

    // Handle rename mode input first
    if picker.is_rename_mode() {
        match key.code {
            KeyCode::Esc => {
                picker.cancel_rename();
            }
            KeyCode::Enter => {
                picker.handle_rename_char('\n');
                // Process pending rename — open the target session, write name, drop
                if let Some((path, name)) = picker.take_pending_rename() {
                    let mut session = crate::agent::session::Session::open(&path, Some(&app.cwd));
                    session.append_session_info(&name);
                    app.status_text = Some(format!("Session renamed to: {}", name));
                }
            }
            KeyCode::Char(c) => {
                picker.handle_rename_char(c);
            }
            KeyCode::Backspace => {
                picker.handle_rename_char('\x7f');
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            app.session_picker = None;
            app.status_text = None;
        }
        KeyCode::Enter => {
            if let Some(path) = picker.selected_path() {
                let path = path.clone();
                app.session_picker = None;
                app.status_text = None;
                // Delegate to the shared SessionSwitched handler
                app.pending_command_result = Some(CommandResult::SessionSwitched { path });
            }
        }
        KeyCode::Up => {
            picker.select_prev();
        }
        KeyCode::Down => {
            picker.select_next();
        }
        KeyCode::Char('/') => {
            picker.set_filter("");
        }
        KeyCode::Char(c) if c == 'r' || c == 'R' => {
            // Start rename mode for selected session
            picker.start_rename();
        }
        KeyCode::Char(c) => {
            let mut filter = picker.filter().to_string();
            filter.push(c);
            picker.set_filter(&filter);
        }
        KeyCode::Backspace => {
            let mut filter = picker.filter().to_string();
            filter.pop();
            picker.set_filter(&filter);
        }
        _ => {}
    }
}

/// Handle slash commands by dispatching through extension command handlers.
/// For commands that need TUI access (overlays), the result is stored in
/// `pending_command_result` and consumed in the main loop where TUI is available.
/// Simple results (Info, Quit, etc.) are handled immediately.
fn handle_slash_command(app: &mut App, input: &str) {
    let (cmd_name, args) = match input.split_once(' ') {
        Some((cmd, rest)) => (cmd.trim_start_matches('/'), rest),
        None => (input.trim_start_matches('/'), ""),
    };

    // Helper: is the extension that owns this command enabled?
    fn is_ext_enabled(ext: &dyn Extension, settings: &crate::settings::Settings) -> bool {
        crate::extension::is_extension_enabled(ext, settings)
    }

    // Find the command handler first (before mutable borrow on app)
    for ext in app.extensions.iter() {
        if !is_ext_enabled(ext.as_ref(), &app.settings) {
            continue;
        }
        for cmd in ext.commands() {
            if cmd.name == cmd_name {
                // Execute the handler here while we have immutably borrowed app,
                // then use the result after dropping the borrow.
                let result = cmd.handler.execute(args);
                match result {
                    Ok(result) => {
                        // Drop the iterator borrow before mutating app
                        drop((ext, cmd));
                        handle_command_result(app, result);
                        return;
                    }
                    Err(e) => {
                        drop((ext, cmd));
                        show_status(app, format!("Error executing /{}: {}", cmd_name, e));
                        return;
                    }
                }
            }
        }
    }

    // Unknown command
    let available: Vec<&str> = app.commands.iter().map(|(n, _)| n.as_str()).collect();
    app.status_text = Some(format!(
        "Unknown command: /{}. Available: {}",
        cmd_name,
        available.join(", ")
    ));
}

/// Handle a CommandResult from a slash command.
/// Simple results are applied immediately; overlay-requiring ones
/// are stored in `pending_command_result` for the main loop.
fn handle_command_result(app: &mut App, result: CommandResult) {
    match result {
        CommandResult::Info(msg) => {
            show_status(app, msg.clone());
        }
        CommandResult::Quit => {
            app.should_quit = true;
        }
        CommandResult::ModelChanged(model) => {
            // Handle "provider/model" format (e.g., "deepseek/deepseek-v4-flash")
            if let Some((provider, model_id)) = model.split_once('/') {
                let provider = provider.trim();
                let model_id = model_id.trim();
                app.current_provider = provider.to_string();
                app.model = model_id.to_string();
            } else {
                app.model = model.clone();
                app.current_provider = app
                    .registry
                    .provider_for_model(&model, Some(&app.current_provider))
                    .unwrap_or_default();
            }
            let model_ref = app.model.clone();
            app.record_model_change(&model_ref);
            app.status_text = Some(format!("Model: {}/{}", app.current_provider, app.model));
        }
        CommandResult::ShowHelp => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenSessionSelector => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::Reloaded => {
            app.refresh_registry();

            // Refresh cached model list from the updated registry.
            {
                let models = app.registry.list_models();
                let provider_models: Vec<(String, String)> = app
                    .registry
                    .list_model_provider_tuples()
                    .into_iter()
                    .map(|(p, m, _)| (p, m))
                    .collect();
                app.available_models = models.clone();
                for ext in app.extensions.iter() {
                    if let Some(cmd) = ext
                        .as_any()
                        .downcast_ref::<crate::builtin::extension::BuiltinExtension>()
                    {
                        cmd.set_available_models(models.clone());
                        cmd.set_provider_models(provider_models.clone());
                        break;
                    }
                }
            }

            // 0. Notify extensions of imminent shutdown (pi-compatible: session_shutdown)
            for ext in app.extensions.iter() {
                ext.on_session_shutdown("reload");
            }

            // 1. Reload settings from disk (pi-compatible)
            let mut reload_parts: Vec<&str> = Vec::new();
            match app.settings.reload(&app.cwd) {
                Err(e) => {
                    app.status_text = Some(format!("Failed to reload settings: {}", e));
                }
                Ok(()) => {
                    reload_parts.push("settings");
                    // Apply reloaded settings to runtime state
                    if let Some(level) = app.settings.default_thinking_level.clone() {
                        app.thinking_level = Some(level.clone());
                        if let Some(ref mut s) = app.session {
                            s.on_thinking_level_change(&level);
                        }
                        if let Some(ref s) = app.session {
                            app.footer.borrow_mut().refresh_from_session(s.session());
                        }
                    }
                    app.hide_thinking = app.settings.hide_thinking.unwrap_or(true);
                    app.propagate_hide_thinking();
                    app.editor.borrow_mut().update_border_color(
                        app.thinking_level.as_deref(),
                        &app.theme as &dyn crate::tui::Theme,
                    );

                    // Apply reloaded auto_compact setting
                    app.auto_compact = app.settings.get_auto_compact();
                    if let Some(ref mut s) = app.session {
                        s.set_auto_compact(app.auto_compact);
                    }
                    app.footer.borrow_mut().set_auto_compact(app.auto_compact);

                    // Apply reloaded collapse_tool_output setting
                    app.collapse_tool_output = app.settings.collapse_tool_output.unwrap_or(false);
                    app.tools_expanded = !app.collapse_tool_output;

                    // 2. Re-apply theme from reloaded settings (pi-compatible)
                    if let Some(ref theme_name) = app.settings.theme
                        && crate::agent::ui::theme::set_theme(theme_name).is_ok()
                    {
                        app.theme = crate::agent::ui::theme::current_theme().clone();
                        reload_parts.push("theme");
                    }
                }
            }

            // 3. Reload keybindings from disk (pi-compatible)
            let mut kb = crate::tui::keybindings::Keybindings::with_defaults();
            if let Some(home) = directories::BaseDirs::new()
                .map(|d| d.home_dir().join(".rab").join("keybindings.json"))
                && home.exists()
            {
                match crate::tui::keybindings::Keybindings::load(&home) {
                    Ok(custom) => kb.merge(custom),
                    Err(e) => {
                        app.status_text = Some(format!("Failed to load keybindings: {}", e));
                    }
                }
            }
            crate::tui::keybindings::init_keybindings(kb);
            reload_parts.push("keybindings");

            // 4. Skills are reloaded inside rebuild_ext_state below
            // 5. Reload prompt templates from disk (pi-compatible)
            app.prompt_templates =
                crate::agent::prompt_templates::load_prompt_templates(&app.prompt_template_dirs);
            // Only report if there are any template dirs configured
            if !app.prompt_template_dirs.is_empty() {
                reload_parts.push("prompts");
            }

            // 5. Reload context files and rebuild system prompt
            refresh_agent_config(app);
            reload_parts.push("skills");
            {
                let skill_names: Vec<String> = app.skills.iter().map(|s| s.name.clone()).collect();
                let template_names: Vec<String> = app
                    .prompt_templates
                    .iter()
                    .map(|t| t.name.clone())
                    .collect();
                let extension_names: Vec<(String, bool)> = app
                    .extensions
                    .iter()
                    .map(|e| {
                        let enabled =
                            crate::extension::is_extension_enabled(e.as_ref(), &app.settings);
                        (e.name().to_string(), enabled)
                    })
                    .collect();
                let theme_names: Vec<String> = crate::agent::ui::theme::get_available_themes()
                    .into_iter()
                    .filter(|n| n != "dark" && n != "light")
                    .collect();
                app.header.borrow_mut().set_resource_data(
                    app.context_files.clone(),
                    skill_names,
                    template_names,
                    extension_names,
                    theme_names,
                );
            }
            reload_parts.push("system prompt");
            reload_parts.push("context files");

            // 7. Notify extensions that reload is complete (pi-compatible: session_start)
            for ext in app.extensions.iter() {
                ext.on_session_start("reload");
            }

            show_status(app, format!("{} reloaded.", reload_parts.join(", ")));
        }
        CommandResult::NewSession => {
            // Matching pi's handleClearCommand:
            //   1. Stop loading animation
            //   2. Clear status container
            //   3. runtimeHost.newSession() -> session.new_session()
            //   4. renderCurrentSessionState() -> clear everything
            //   5. Add "✓ New session started" with accent color + spacer

            // Stop working indicator (matching pi's loadingAnimation.stop())
            app.working.stop();

            // Clear status section (matching pi's statusContainer.clear())
            app.status_text = None;

            // Create a new session via AgentSession (new ID, new file, resets tracked state)
            if let Some(ref mut agent_session) = app.session {
                agent_session.new_session();

                // Re-record current model/provider and thinking level in the fresh session
                // so the footer can pick them up via refresh_from_session.
                agent_session.on_model_change(&app.current_provider, &app.model);
                if let Some(ref level) = app.thinking_level {
                    agent_session.on_thinking_level_change(level);
                }
            }

            // Clear everything (matching pi's renderCurrentSessionState)
            app.agent = None;
            app.clear_session_state();

            // Refresh footer cached stats from the now-empty session
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }

            // Add "✓ New session started" with accent color, matching pi's
            // `new Text(theme.fg("accent", "✓ New session started"), 1, 1)`
            let styled = app.theme.fg("accent", "✓ New session started");
            chat_add(app, std::boxed::Box::new(Text::new(styled, 1, 1, None)));
        }
        CommandResult::SessionSwitched { path } => {
            let new_session = crate::agent::AgentSession::open(&path, None, Some(&app.cwd));
            app.switch_to_session(new_session);
            app.status_text = Some(format!("Switched to session: {}", path.display()));
        }
        CommandResult::SessionInfo { .. } => {
            // Compute from the live session on demand.
            let info = app
                .session
                .as_ref()
                .map(|s| crate::agent::session::format_session_info(s.session()))
                .unwrap_or_else(|| "No active session".to_string());
            show_status(app, info);
        }
        CommandResult::SessionNamed { name } => {
            // Persist name in session
            if let Some(ref mut s) = app.session {
                s.session_mut().append_session_info(&name);
            }

            // Check if name was normalized (pi-compatible normalization warning)
            let stored_name = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_name().map(|n| n.to_string()));
            if let Some(ref stored) = stored_name
                && stored != &name
            {
                show_status(
                    app,
                    format!("Session name normalized from {:?} to {:?}", name, stored),
                );
            }

            show_status(
                app,
                format!(
                    "Session name set: {}",
                    stored_name.as_deref().unwrap_or(&name)
                ),
            );

            app.status_text = Some(format!(
                "Session name set: {}",
                stored_name.as_deref().unwrap_or(&name)
            ));

            // Refresh footer (refresh_from_session picks up the new name)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
        }
        CommandResult::OpenModelSelector => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenSettings => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::OpenExtensions => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ScopedModels => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ExportSession { path } => {
            // Get session reference
            let result = (|| -> Result<PathBuf, String> {
                let agent_session = app.session.as_ref().ok_or("No active session")?;
                let session = agent_session.session();
                let system_prompt = Some(app.system_prompt.as_str());
                let theme = crate::agent::ui::theme::current_theme();
                let theme_name = Some(theme.name.as_str());

                let output_path = if path.as_ref().is_some_and(|p| p.ends_with(".jsonl")) {
                    export::export_to_jsonl(session, &app.cwd, path.as_deref())
                        .map_err(|e| format!("Export failed: {}", e))?
                } else {
                    export::export_to_html(
                        session,
                        system_prompt,
                        &app.cwd,
                        path.as_deref(),
                        theme_name,
                    )
                    .map_err(|e| format!("Export failed: {}", e))?
                };

                Ok(output_path)
            })();

            match result {
                Ok(path) => {
                    let display = crate::builtin::shorten_path(path.to_string_lossy().as_ref());
                    show_status(app, format!("✓ Session exported to: {}", display));
                }
                Err(msg) => {
                    show_status(app, format!("✗ {}", msg));
                }
            }
        }
        result @ CommandResult::ImportSession { .. } => {
            // Needs TUI overlay (confirmation) - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ShareSession => {
            let msg = "Share session - not yet implemented.".to_string();
            show_status(app, msg.clone());
        }
        CommandResult::CopyLastMessage => {
            // Get last assistant message text (pi-compatible)
            let text = app.session.as_ref().and_then(|s| {
                let entries = s.session().get_entries();
                entries.iter().rev().find_map(|entry| {
                    #[allow(clippy::collapsible_if)]
                    if let Some(yoagent::types::Message::Assistant { stop_reason, .. }) =
                        entry.message.as_llm()
                    {
                        if *stop_reason != yoagent::types::StopReason::Aborted
                            || !crate::agent::types::message_text(&AgentMessage::Llm(
                                entry.message.as_llm().unwrap().clone(),
                            ))
                            .trim()
                            .is_empty()
                        {
                            let text = crate::agent::types::message_text(&AgentMessage::Llm(
                                entry.message.as_llm().unwrap().clone(),
                            ));
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                return Some(trimmed.to_string());
                            }
                        }
                    }
                    None
                })
            });

            let text = match text {
                Some(t) => t,
                None => {
                    show_status(app, "No agent messages to copy yet.");
                    return;
                }
            };

            // Pi-compatible clipboard copy (includes OSC 52 fallback)
            copy_to_clipboard(&text);
            show_status(app, "Copied last agent message to clipboard");
        }
        CommandResult::ShowChangelog => {
            let msg = "Changelog - not yet implemented.".to_string();
            show_status(app, msg.clone());
        }
        CommandResult::ForkSession { ref message_id } => {
            if message_id.is_none() {
                // No message ID provided — defer to show message selector overlay
                app.pending_command_result = Some(result);
            } else {
                // Clone the session info before modifying app.session
                let source_path = app
                    .session
                    .as_ref()
                    .and_then(|s| s.session().session_file());
                let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
                let cwd = app.cwd.clone();

                match (source_path, session_dir) {
                    (Some(source), Some(ref target_dir)) => {
                        match crate::agent::session::fork_session(
                            source,
                            target_dir,
                            message_id.as_deref(),
                            None,
                        ) {
                            Ok(new_id) => {
                                let dir_entries = std::fs::read_dir(target_dir).ok();
                                let new_path = dir_entries.and_then(|entries| {
                                    entries
                                        .flatten()
                                        .find(|e| {
                                            let filename = e.file_name();
                                            filename.to_string_lossy().contains(&new_id)
                                        })
                                        .map(|e| e.path())
                                });

                                match new_path {
                                    Some(ref path) => {
                                        let new_session = crate::agent::AgentSession::open(
                                            path,
                                            None,
                                            Some(&cwd),
                                        );
                                        app.switch_to_session(new_session);

                                        let styled = app.theme.fg(
                                            "accent",
                                            &format!("✓ Forked session: {}", path.display()),
                                        );
                                        chat_add(
                                            app,
                                            std::boxed::Box::new(Text::new(styled, 1, 1, None)),
                                        );
                                    }
                                    None => {
                                        let msg = format!(
                                            "Fork created but new file not found: {}",
                                            new_id
                                        );
                                        show_status(app, msg);
                                    }
                                }
                            }
                            Err(e) => {
                                let msg = format!("Fork failed: {}", e);
                                show_status(app, msg.clone());
                            }
                        }
                    }
                    _ => {
                        let msg = "No active session to fork".to_string();
                        show_status(app, msg.clone());
                    }
                }
            }
        }
        CommandResult::CloneSession => {
            // Clone the session at the current position (like fork with position "at").
            let source_path = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_file());
            let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
            let leaf_id = app.session.as_ref().and_then(|s| s.session().get_leaf_id());
            let cwd = app.cwd.clone();

            let leaf_id = match leaf_id {
                Some(id) if !id.is_empty() => id,
                _ => {
                    let msg = "Nothing to clone yet".to_string();
                    show_status(app, msg);
                    return;
                }
            };

            match (source_path, session_dir) {
                (Some(source), Some(ref target_dir)) => {
                    match crate::agent::session::fork_session(
                        source,
                        target_dir,
                        Some(&leaf_id),
                        Some("at"),
                    ) {
                        Ok(new_id) => {
                            let dir_entries = std::fs::read_dir(target_dir).ok();
                            let new_path = dir_entries.and_then(|entries| {
                                entries
                                    .flatten()
                                    .find(|e| {
                                        let filename = e.file_name();
                                        filename.to_string_lossy().contains(&new_id)
                                    })
                                    .map(|e| e.path())
                            });

                            match new_path {
                                Some(ref path) => {
                                    let new_session =
                                        crate::agent::AgentSession::open(path, None, Some(&cwd));
                                    app.switch_to_session(new_session);

                                    let styled = app.theme.fg(
                                        "accent",
                                        &format!("✓ Cloned session: {}", path.display()),
                                    );
                                    chat_add(
                                        app,
                                        std::boxed::Box::new(Text::new(styled, 1, 1, None)),
                                    );
                                }
                                None => {
                                    let msg =
                                        format!("Clone created but new file not found: {}", new_id);
                                    show_status(app, msg);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Clone failed: {}", e);
                            show_status(app, msg.clone());
                        }
                    }
                }
                _ => {
                    let msg = "No active session to clone".to_string();
                    show_status(app, msg.clone());
                }
            }
        }
        CommandResult::SessionTree => {
            // Needs TUI overlay — defer
            app.pending_command_result = Some(result);
        }
        CommandResult::TrustDecision { decision } => {
            let msg = format!("Trust decision '{}' saved.", decision);
            show_status(app, msg.clone());
        }
        CommandResult::Login {
            ref provider,
            ref api_key,
        } => {
            if let (Some(provider), Some(key)) = (provider, api_key) {
                handle_login(app, provider, Some(key));
            } else {
                // Needs prompt — defer
                app.pending_command_result = Some(result);
            }
        }
        CommandResult::Logout { ref provider } => {
            if let Some(p) = provider {
                handle_logout(app, Some(p));
            } else {
                // Needs provider selector — defer
                app.pending_command_result = Some(result);
            }
        }
        CommandResult::CompactSession(custom_instructions) => {
            app.pending_compact = Some(custom_instructions);
        }
        CommandResult::Stop => {
            app.stop_requested
                .store(true, std::sync::atomic::Ordering::Relaxed);
            app.status_text = Some("Stop requested — finishing current turn…".into());
        }
        CommandResult::NextTurn { text } => {
            if text.is_empty() {
                app.status_text = Some("Usage: /nextTurn <message>".into());
            } else {
                app.next_turn_queue.push(user_agent_message(&text));
                app.status_text = Some("Message queued for next turn".into());
            }
        }
    }
}

/// Look up a tool renderer by name from extensions (bundled in ToolDefinition.renderer).
pub(super) fn find_tool_renderer(
    extensions: &[Box<dyn crate::extension::Extension>],
    name: &str,
) -> Option<Arc<dyn ToolRenderer>> {
    for ext in extensions {
        for tool in ext.tools() {
            if tool.name() == name {
                return tool.renderer;
            }
        }
    }
    None
}

/// Handle ! and !! bang commands.
/// Renders via ToolExecComponent with the bash renderer (same visual treatment
/// as LLM-invoked bash tool calls, eliminating the separate BashExecution split).
fn handle_bang_command(app: &mut App, command: String) {
    let cwd = app.cwd.clone();
    let tx = app.event_tx.clone();
    use yoagent::types::{AgentEvent as YoEvent, Content as YoContent, ToolResult as YoResult};

    let renderer = find_tool_renderer(&app.extensions, "bash");
    let mut tool = crate::agent::ui::components::ToolExecComponent::new(
        "bash",
        renderer,
        serde_json::json!({"command": command}),
        app.cwd.to_string_lossy().to_string(),
        "__bang__".to_string(),
    );
    tool.set_started_at(std::time::Instant::now());
    let (invalidate_tx, invalidate_rx) =
        crate::agent::ui::components::ToolExecComponent::make_invalidation_channel();
    app.invalidate_rxs.push(invalidate_rx);
    tool.set_invalidate_tx(invalidate_tx);
    tool.set_expanded(app.tools_expanded);
    let tool = Rc::new(RefCell::new(tool));
    app.pending_tools
        .insert("__bang__".to_string(), Rc::downgrade(&tool));
    chat_add(
        app,
        std::boxed::Box::new(crate::agent::ui::components::RcToolExec(tool)),
    );
    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);
    app.pending_tool_executions += 1;

    let handle = tokio::spawn(async move {
        struct Guard<'a> {
            tx: &'a mpsc::UnboundedSender<yoagent::types::AgentEvent>,
            sent: bool,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if !self.sent {
                    let _ = self.tx.send(YoEvent::AgentEnd { messages: vec![] });
                }
            }
        }
        let mut guard = Guard {
            tx: &tx,
            sent: false,
        };

        let send_progress = |text: &str| {
            let _ = tx.send(YoEvent::ProgressMessage {
                tool_call_id: "__bang__".to_string(),
                tool_name: "bash".into(),
                text: text.to_string(),
            });
        };

        let mut child = match tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .current_dir(&cwd)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(YoEvent::ToolExecutionEnd {
                    tool_call_id: "__bang__".to_string(),
                    tool_name: "bash".into(),
                    result: YoResult {
                        content: vec![YoContent::Text {
                            text: format!("Failed to execute: {:#}", e),
                        }],
                        details: serde_json::Value::Null,
                    },
                    is_error: true,
                });
                guard.sent = true;
                let _ = tx.send(YoEvent::AgentEnd { messages: vec![] });
                return;
            }
        };

        let mut all_output = String::new();
        // Stream stdout and stderr concurrently using tokio async reads
        use tokio::io::AsyncReadExt;
        let mut stdio = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let mut buf1 = [0u8; 4096];
        let mut buf2 = [0u8; 4096];
        let mut stdout_done = false;
        let mut stderr_done = false;

        loop {
            tokio::select! {
                result = stdio.read(&mut buf1), if !stdout_done => {
                    match result {
                        Ok(0) => stdout_done = true,
                        Ok(n) => {
                            if let Ok(text) = std::str::from_utf8(&buf1[..n]) {
                                all_output.push_str(text);
                                send_progress(text);
                            }
                        }
                        Err(_) => stdout_done = true,
                    }
                }
                result = stderr.read(&mut buf2), if !stderr_done => {
                    match result {
                        Ok(0) => stderr_done = true,
                        Ok(n) => {
                            if let Ok(text) = std::str::from_utf8(&buf2[..n]) {
                                all_output.push_str(text);
                                send_progress(text);
                            }
                        }
                        Err(_) => stderr_done = true,
                    }
                }
            }
            if stdout_done && stderr_done {
                break;
            }
        }

        // Wait for process to finish
        let status = child.wait().await;
        let is_error = match &status {
            Ok(s) => !s.success(),
            Err(_) => true,
        };
        let result = if all_output.trim().is_empty() {
            "(no output)".to_string()
        } else {
            all_output.trim().to_string()
        };

        let _ = tx.send(YoEvent::ToolExecutionEnd {
            tool_call_id: "__bang__".to_string(),
            tool_name: "bash".into(),
            result: YoResult {
                content: vec![YoContent::Text { text: result }],
                details: serde_json::Value::Null,
            },
            is_error,
        });
        guard.sent = true;
        let _ = tx.send(YoEvent::AgentEnd { messages: vec![] });
    });
    app.bash_abort_handle = Some(handle.abort_handle());
}

/// Rebuild the chat container from a slice of AgentMessages (pi's renderSessionContext).
/// Clears the container and re-adds all message components with spacers between them.
/// Adjacent tool calls and tool results are paired into single ToolExecComponent.
pub fn rebuild_chat_from_messages(
    chat: &mut crate::tui::Container,
    messages: &[yoagent::types::AgentMessage],
    cwd: &str,
    hide_thinking: bool,
    _collapse_tool_output: bool,
    extensions: &[Box<dyn crate::extension::Extension>],
) {
    chat.clear();
    use std::collections::HashMap;
    let mut pending_tool_components: HashMap<
        String,
        Rc<RefCell<crate::agent::ui::components::ToolExecComponent>>,
    > = HashMap::new();

    for msg in messages {
        if crate::agent::types::message_is_user(msg) {
            let text = crate::agent::types::message_text(msg);
            if text.is_empty() {
                continue;
            }
            // pi: add Spacer(1) before user messages when chat isn't empty
            if !chat.children().is_empty() {
                chat.add_child(std::boxed::Box::new(Spacer::new(1)));
            }
            chat.add_child(std::boxed::Box::new(
                crate::agent::ui::components::UserMessageComponent::new(text),
            ));
        } else if crate::agent::types::message_is_assistant(msg) {
            let text = crate::agent::types::message_text(msg);
            if let yoagent::types::AgentMessage::Llm(yoagent::types::Message::Assistant {
                content,
                ..
            }) = msg
            {
                let tcs = crate::agent::types::content_tool_calls(content);
                if !tcs.is_empty() {
                    // Assistant with tool calls — render text first
                    if !text.trim().is_empty() {
                        add_assistant_message(chat, &text, hide_thinking);
                    }
                    // Create ToolExecComponent for each tool call
                    for (id, name, args) in &tcs {
                        let renderer = find_tool_renderer(extensions, name);
                        let tool = crate::agent::ui::components::ToolExecComponent::new(
                            name,
                            renderer,
                            args.clone(),
                            cwd.to_string(),
                            id.clone(),
                        );
                        let tool = Rc::new(RefCell::new(tool));
                        chat.add_child(std::boxed::Box::new(
                            crate::agent::ui::components::RcToolExec(tool.clone()),
                        ));
                        pending_tool_components.insert(id.clone(), tool);
                    }
                } else if !text.trim().is_empty() {
                    // Plain text assistant
                    add_assistant_message(chat, &text, hide_thinking);
                }
            }
        } else if crate::agent::types::message_is_tool_result(msg) {
            let is_error = crate::agent::types::message_is_error(msg);
            let text = crate::agent::types::message_text(msg);
            if let Some(tc_id) = crate::agent::types::message_tool_call_id(msg)
                && let Some(tool) = pending_tool_components.remove(tc_id)
            {
                let clean = text
                    .strip_prefix("✓ ")
                    .or_else(|| text.strip_prefix("✗ "))
                    .unwrap_or(&text);
                let mut tool = tool.borrow_mut();
                tool.set_result_with_details(clean, is_error, None);
            }
        } else if crate::agent::types::message_is_extension(msg) {
            // Extension messages (info, error, system_stop) rendered as info text.
            // Pi-style: add Spacer(1) before extension info messages (matches showStatus).
            if let Some(text) = crate::agent::types::message_extension_text(msg) {
                if !chat.children().is_empty() {
                    chat.add_child(std::boxed::Box::new(Spacer::new(1)));
                }
                chat.add_child(std::boxed::Box::new(InfoMessageComponent::new(text)));
            }
        }
    }
}

/// Add a Component to chat_container directly, without any preceding Spacer.
/// Components that need a leading Spacer (user messages) handle it themselves,
/// matching pi's per-message-type spacing in `addMessageToChat()`.
pub fn chat_add(app: &mut App, component: std::boxed::Box<dyn Component>) {
    let mut chat = app.chat_container.borrow_mut();
    chat.add_child(component);
}

/// Add an AssistantMessageComponent. Matching pi, the component handles its own
/// leading spacing internally — no external Spacer is needed.
fn add_assistant_message(chat: &mut crate::tui::Container, text: &str, hide_thinking: bool) {
    let mut asst = crate::agent::ui::components::AssistantMessageComponent::new(text);
    if hide_thinking {
        asst.set_hide_thinking(true);
    }
    chat.add_child(std::boxed::Box::new(asst));
}

/// Show a summarization choice prompt after tree entry selection (matching pi's showExtensionSelector).
/// Shows "No summary", "Summarize", and "Summarize with custom prompt" options.
fn show_summarization_prompt(app: &mut App, tui: &mut TUI, _entry_id: &str) {
    use crate::tui::Component;
    use crate::tui::keybindings::{
        ACTION_EDITOR_DELETE_CHAR_BACKWARD, ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM,
        ACTION_SELECT_DOWN, ACTION_SELECT_UP, get_keybindings,
    };
    use crossterm::event::KeyEvent;
    use std::cell::RefCell;
    use std::rc::Rc;

    struct SummarizationPrompt {
        selected_index: usize,
        items: [&'static str; 3],
        signal: Rc<RefCell<Option<OverlayResult>>>,
        entry_id: String,
        edit_mode: bool,
        edit_text: String,
    }

    impl Component for SummarizationPrompt {
        fn render(&mut self, width: usize) -> Vec<String> {
            let theme = crate::agent::ui::theme::current_theme();
            let mut lines = Vec::new();

            lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));
            lines.push(String::new());
            lines.push(format!("  {}", theme.bold("Summarize branch?")));
            lines.push(String::new());

            if self.edit_mode {
                // Show editor for custom summarization instructions
                lines.push(format!(
                    "  {}",
                    theme.fg("muted", "Custom summarization instructions (Enter to submit, Shift/Ctrl+Enter for newline):")
                ));
                lines.push(String::new());
                // Render multi-line text content
                if self.edit_text.is_empty() {
                    lines.push(format!(
                        "  {}",
                        theme.fg("muted", "<type here, Enter for newline>")
                    ));
                } else {
                    for line in self.edit_text.lines() {
                        lines.push(format!("  {}", line));
                    }
                }
                lines.push(String::new());
                lines.push(format!(
                    "  {}",
                    theme.fg(
                        "muted",
                        "Enter: submit \u{00b7} Shift/Ctrl+Enter: newline \u{00b7} Esc: back"
                    )
                ));
            } else {
                for (i, item) in self.items.iter().enumerate() {
                    let prefix = if i == self.selected_index {
                        theme.fg("accent", "\u{203a} ")
                    } else {
                        "  ".to_string()
                    };
                    let text = if i == self.selected_index {
                        theme.fg("accent", item)
                    } else {
                        theme.text_color(item)
                    };
                    lines.push(format!("{}{}", prefix, text));
                }
                lines.push(String::new());
                lines.push(theme.fg(
                    "muted",
                    "  \u{2191}/\u{2193} navigate \u{00b7} Enter select \u{00b7} Esc back to tree",
                ));
            }

            lines
        }

        fn handle_input(&mut self, key: &KeyEvent) -> bool {
            let kb = get_keybindings();

            if self.edit_mode {
                if key.code == crossterm::event::KeyCode::Esc {
                    self.edit_mode = false;
                    return true;
                }
                // Enter submits
                if key.code == crossterm::event::KeyCode::Enter
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT)
                    && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
                    && !key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    let instructions = self.edit_text.trim().to_string();
                    let ci = if instructions.is_empty() {
                        None
                    } else {
                        Some(instructions)
                    };
                    *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                        entry_id: self.entry_id.clone(),
                        summarize: true,
                        custom_instructions: ci,
                    });
                    return true;
                }
                // Shift+Enter, Ctrl+Enter, or Ctrl+J inserts newline
                if (key.code == crossterm::event::KeyCode::Enter
                    && (key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::SHIFT)
                        || key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL)))
                    || (key.code == crossterm::event::KeyCode::Char('j')
                        && key
                            .modifiers
                            .contains(crossterm::event::KeyModifiers::CONTROL))
                {
                    self.edit_text.push('\n');
                    return true;
                }
                if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
                    self.edit_text.pop();
                    return true;
                }
                if let crossterm::event::KeyCode::Char(c) = key.code
                    && !c.is_control()
                {
                    self.edit_text.push(c);
                    return true;
                }
                return true;
            }

            if kb.matches(key, ACTION_SELECT_UP) {
                self.selected_index = if self.selected_index == 0 {
                    self.items.len() - 1
                } else {
                    self.selected_index - 1
                };
                return true;
            }

            if kb.matches(key, ACTION_SELECT_DOWN) {
                self.selected_index = if self.selected_index >= self.items.len() - 1 {
                    0
                } else {
                    self.selected_index + 1
                };
                return true;
            }

            if kb.matches(key, ACTION_SELECT_CONFIRM) {
                match self.selected_index {
                    0 => {
                        *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                            entry_id: self.entry_id.clone(),
                            summarize: false,
                            custom_instructions: None,
                        });
                    }
                    1 => {
                        *self.signal.borrow_mut() = Some(OverlayResult::TreeSummarizeChoice {
                            entry_id: self.entry_id.clone(),
                            summarize: true,
                            custom_instructions: None,
                        });
                    }
                    2 => {
                        self.edit_mode = true;
                        self.edit_text.clear();
                        return true;
                    }
                    _ => {}
                }
                return true;
            }

            if kb.matches(key, ACTION_SELECT_CANCEL) {
                *self.signal.borrow_mut() = Some(OverlayResult::TreeReopen(self.entry_id.clone()));
                return true;
            }

            false
        }

        fn invalidate(&mut self) {}
    }

    let entry_id = _entry_id.to_string();
    let prompt = SummarizationPrompt {
        selected_index: 0,
        items: ["No summary", "Summarize", "Summarize with custom prompt"],
        signal: app.overlay_result_signal.clone(),
        entry_id,
        edit_mode: false,
        edit_text: String::new(),
    };

    tui.show_positioned_overlay(Box::new(prompt), crate::tui::OverlayPosition::Bottom);
}

/// Show a status message in the chat (pi-style `showStatus`).
///
/// If the last two children of `chat_container` are from a previous status
/// (spacer + InfoMessageComponent), they are replaced in-place rather than
/// appending new entries. This prevents multiple consecutive status messages
/// from accumulating at the end of the chat session.
pub(super) fn show_status(app: &mut App, message: impl Into<String>) {
    let mut chat = app.chat_container.borrow_mut();
    // Check if previous status children are still the last in the container
    // (pi-style: last two are Spacer + Text, replaced in-place)
    if let Some(prev_len) = app.last_status_len
        && chat.len() == prev_len
        && prev_len >= 2
    {
        chat.pop_child(); // text / InfoMessageComponent
        chat.pop_child(); // Spacer
    }
    app.last_status_len = None;
    drop(chat);

    // Add the new status with a leading spacer (pi-style: Spacer + Text)
    let mut chat = app.chat_container.borrow_mut();
    chat.add_child(std::boxed::Box::new(Spacer::new(1)));
    chat.add_child(std::boxed::Box::new(InfoMessageComponent::new(message)));
    app.last_status_len = Some(chat.len());
}

/// Concatenate all Text content from a slice of Content values.
pub(super) fn extract_text_content(content: &[yoagent::types::Content]) -> String {
    content
        .iter()
        .filter_map(|c| {
            if let yoagent::types::Content::Text { text } = c {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("")
}

/// Try to copy text to the system clipboard using platform-specific tools.
/// Returns true if successful, false if no tool was available.
/// Falls back to OSC 52 escape sequence for remote sessions.
/// Mirrors pi's clipboard strategy exactly.
fn copy_to_clipboard(text: &str) -> bool {
    use std::io::Write;
    let mut copied = false;

    // macOS
    if !copied
        && std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut child| {
                let _ = child.stdin.take().map(|mut stdin| {
                    let _ = stdin.write_all(text.as_bytes());
                });
                child.wait().ok()
            })
            .is_some_and(|s| s.success())
    {
        copied = true;
    }

    // Windows
    if !copied
        && std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut child| {
                let _ = child.stdin.take().map(|mut stdin| {
                    let _ = stdin.write_all(text.as_bytes());
                });
                child.wait().ok()
            })
            .is_some_and(|s| s.success())
    {
        copied = true;
    }

    // Linux / Termux
    if !copied
        && std::env::var("TERMUX_VERSION").is_ok()
        && let Ok(mut child) = std::process::Command::new("termux-clipboard-set")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    {
        let _ = child.stdin.take().map(|mut stdin| {
            let _ = stdin.write_all(text.as_bytes());
        });
        copied = child.wait().ok().is_some_and(|s| s.success());
    }

    // Wayland: spawn wl-copy without waiting (it daemonizes, pi-compatible)
    if !copied
        && std::env::var("WAYLAND_DISPLAY").is_ok()
        && std::process::Command::new("which")
            .arg("wl-copy")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()
            .is_some_and(|s| s.success())
        && let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
    {
        let _ = child.stdin.take().map(|mut stdin| {
            let _ = stdin.write_all(text.as_bytes());
        });
        // Don't wait — wl-copy daemonizes (pi-compatible)
        copied = true;
    }

    // X11: try xclip, then xsel
    if !copied
        && std::process::Command::new("xclip")
            .arg("-selection")
            .arg("clipboard")
            .arg("-i")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut child| {
                let _ = child.stdin.take().map(|mut stdin| {
                    let _ = stdin.write_all(text.as_bytes());
                });
                child.wait().ok()
            })
            .is_some_and(|s| s.success())
    {
        copied = true;
    }

    if !copied
        && std::process::Command::new("xsel")
            .arg("--clipboard")
            .arg("--input")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()
            .and_then(|mut child| {
                let _ = child.stdin.take().map(|mut stdin| {
                    let _ = stdin.write_all(text.as_bytes());
                });
                child.wait().ok()
            })
            .is_some_and(|s| s.success())
    {
        copied = true;
    }

    // OSC 52 fallback: emit for remote sessions or when nothing copied
    let remote = std::env::var("SSH_CONNECTION").is_ok()
        || std::env::var("SSH_CLIENT").is_ok()
        || std::env::var("MOSH_CONNECTION").is_ok();

    if remote || !copied {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
        // Pi-compatible: skip OSC 52 for very large payloads (>100KB encoded)
        if encoded.len() <= 100_000 {
            let _ = writeln!(std::io::stdout(), "\x1b]52;c;{}\x07", encoded);
            let _ = std::io::stdout().flush();
            copied = true;
        }
    }

    copied
}
