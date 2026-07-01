use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::extension::ToolRenderer;
use yoagent::types::AgentTool;

use crate::agent::AgentSession;
use crate::agent::extension::{CommandResult, Extension};
use crate::agent::footer_data_provider::FooterDataProvider;
use crate::agent::session::SessionEntry;
use crate::auth;
use crate::builtin::export;
use crate::provider;
use crate::provider::ProviderRegistry;

use crate::agent::ui::chat_editor::{ChatEditor, InputAction};

use crate::agent::ui::components::EditorComponent;
use crate::agent::ui::components::FooterComponent;
use crate::agent::ui::components::InfoMessageComponent;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
use crate::builtin::commands::SessionInfoInternal;
use crate::tui::Component;
use crate::tui::TUI;
use crate::tui::focusable::Focusable;

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
}

use crate::agent::ui::components::oauth_selector::AuthType;
use crate::agent::ui::theme::ThemeKey;
use crate::tui::components::Spacer;
use crate::tui::components::Text;
use crate::tui::terminal::{self, ProcessTerminal, TerminalTrait};
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

/// Thinking level cycle order (matching pi's thinking level enum). Cycles from
/// highest to lowest so the first press from the default (xhigh) goes to "high"
/// (a step down), not to "off".
const ALL_THINKING_LEVELS: &[&str] = &["xhigh", "high", "medium", "low", "off"];

/// Get the available thinking levels for the current model, filtered by
/// the model's `thinkingLevelMap`. Levels mapped to `null` are unsupported.
fn available_thinking_levels(app: &App) -> Vec<&'static str> {
    // Try to read thinkingLevelMap from the resolved model
    let thinking_map: Option<std::collections::HashMap<String, Option<serde_json::Value>>> = app
        .registry
        .resolve(&app.model, Some(&app.current_provider))
        .ok()
        .and_then(|r| {
            r.model_config
                .headers
                .get("_rab_thinking_map")
                .and_then(|json| serde_json::from_str(json).ok())
        });

    match thinking_map {
        Some(map) => ALL_THINKING_LEVELS
            .iter()
            .filter(|level| {
                if **level == "off" {
                    return true; // off is always available
                }
                // If the level is in the map and maps to null, it's unsupported
                !matches!(map.get(**level), Some(None))
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
    pub settings: crate::agent::settings::Settings,
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
    /// Session info Arc for /session command (shared with CommandsExtension).
    pub session_info: Option<std::sync::Arc<std::sync::Mutex<Option<SessionInfoInternal>>>>,
    /// API key for yoagent provider.
    pub api_key: String,
    /// Provider registry for model resolution and provider dispatch.
    pub registry: Arc<ProviderRegistry>,
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

    /// Chat scroll offset (lines scrolled up from bottom).
    scroll_offset: usize,

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
    /// Session info updater for /session command.
    session_info: Option<std::sync::Arc<std::sync::Mutex<Option<SessionInfoInternal>>>>,

    /// Auto-compact toggle state.
    auto_compact: bool,

    /// Settings reference for persisting toggle changes.
    settings: crate::agent::settings::Settings,

    /// Header component (welcome/onboarding). Stored as `Rc<RefCell>` so
    /// handle_tools_expand can toggle its expanded state (matching pi's
    /// behavior where setToolsExpanded expands both the header and all
    /// expandable chat children).
    header: Rc<RefCell<crate::agent::ui::components::HeaderComponent>>,

    /// Scoped model IDs for cycling (null = all enabled).
    scoped_model_ids: Option<Vec<String>>,

    /// Session picker state (Some = picker is active).
    session_picker: Option<crate::agent::ui::components::SessionPicker>,

    /// Tracks the number of children in `chat_container` after the last
    /// status message was added (pi-style `lastStatusSpacer`/`lastStatusText`).
    /// Used by `show_status()` to replace consecutive status messages in-place
    /// instead of appending indefinitely.
    last_status_len: Option<usize>,
    /// Pending label changes from the tree selector (accumulated, flushed each frame).
    pending_label_changes: PendingLabelChanges,
    // ── Message rendering cache (avoids re-rendering messages every frame) ──
    // Cache fields removed - messages now rendered via Components in chat_container.
}

impl App {
    fn new(config: AppConfig, session: AgentSession) -> Self {
        let mut agent_session = session;
        let model_config = config
            .registry
            .resolve(&config.model, Some(&config.provider))
            .ok()
            .map(|r| r.model_config.clone())
            .unwrap_or_else(|| {
                let mut mc = crate::agent::base_model_config(&config.model);
                mc.context_window =
                    crate::agent::compaction::get_model_context_window(&config.model) as u32;
                mc
            });
        agent_session.set_compaction_config(
            config.api_key.clone(),
            &config.model,
            crate::agent::compaction::get_model_context_window(&config.model),
            Some(model_config),
        );
        agent_session.set_registry(config.registry.clone());
        agent_session.set_auto_compact(config.settings.auto_compact.unwrap_or(true));
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
        footer.set_context_window(crate::agent::compaction::get_model_context_window(
            &config.model,
        ));

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
        let context = agent_session.session().build_session_context();
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
        let extension_names: Vec<String> = config
            .extensions
            .iter()
            .map(|e| e.name().to_string())
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
            pending_compact: None,
            pending_auto_compact: false,
            agent: None,
            forward_handle: None,
            oauth_join_handle: None,
            pending_oauth_provider: None,
            pending_command_result: None,
            overlay_result_signal: Rc::new(RefCell::new(None)),
            pending_scoped_ids: Rc::new(RefCell::new(None)),
            hide_thinking: config.hide_thinking,
            collapse_tool_output: config.collapse_tool_output,
            tools_expanded: !config.collapse_tool_output,
            scroll_offset: 0,
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
            session_info: config.session_info,
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

        // Initial session info for /session command
        result.update_session_info();

        // Initialize footer stats and session name from session
        if let Some(ref mut s) = result.session {
            result.footer.borrow_mut().refresh_from_session(s.session());
        }

        result
    }

    /// Update the session info shared with CommandsExtension for /session display.
    fn update_session_info(&self) {
        if let Some(ref session) = self.session
            && let Some(ref info) = self.session_info
        {
            let si = crate::builtin::commands::compute_session_info(session.session());
            if let Ok(mut guard) = info.lock() {
                *guard = Some(si);
            }
        }
    }

    /// Refresh git branch for footer display.
    /// Called on AgentStart to match pi's FooterDataProvider.onBranchChange.
    fn refresh_git_branch(&self) {
        self.footer_provider.borrow_mut().refresh_git_branch();
    }

    /// Clear all transient session state when switching to a new session.
    fn clear_session_state(&mut self) {
        self.chat_container.borrow_mut().clear();
        self.streaming_component = None;
        self.pending_tools.clear();
        self.tool_call_start_times.clear();
        self.pending_submit = None;
    }

    /// Rebuild chat and agent messages from the current session context.
    /// Used after compaction to update the UI and keep the agent in sync.
    fn rebuild_from_session_context(&mut self) {
        if let Some(ref agent_session) = self.session {
            let context = agent_session.session().build_session_context();
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
        let ctx = new_session.session().build_session_context();
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
        self.update_session_info();
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
    // Disable clear_on_shrink to avoid full redraws during streaming
    // (content grows/shrinks frequently as pending text is flushed).
    tui.set_clear_on_shrink(false);
    let mut app = App::new(config, session);

    // Focus the editor so it emits the cursor marker for Screen tracking
    app.editor.borrow_mut().editor.set_focused(true);

    // Set up the component tree in TUI.root (matching pi's TUI.extend(Container))
    // Order: header → chat_container (messages) → pending → status → queued → working → editor → footer
    tui.root.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(
            app.header.clone() as Rc<RefCell<dyn Component>>,
        ),
    ));
    tui.root.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.chat_container.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.status_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.working_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root
        .add_child(std::boxed::Box::new(EditorComponent(app.editor.clone())));
    tui.root
        .add_child(std::boxed::Box::new(FooterComponent(app.footer.clone())));

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

        // Check overlay result signal (set by overlay callbacks when user selects/cancels).
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
                            app.current_provider = provider;
                            app.model = model_id.clone();
                            app.record_model_change(&model_id);
                            app.status_text = Some(format!("Model: {}", full_id));
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
                                .map(|s| s.session_manager().session_dir().to_path_buf())
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
                                chat_info(
                                    &mut app,
                                    format!(
                                        "✓ Imported and switched to session: {}",
                                        crate::builtin::shorten_path(&path.to_string_lossy())
                                    ),
                                );
                            }
                            Err(msg) => {
                                chat_info(&mut app, format!("✗ {}", msg));
                            }
                        }
                    }
                    OverlayResult::ImportCancelled => {
                        chat_info(&mut app, "Import cancelled.");
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
                                match session.session_mut().set_leaf_id(Some(&entry_id)) {
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
                    OverlayResult::TreeReopen(entry_id) => {
                        // Re-show the tree selector (user cancelled from summarization prompt)
                        if let Some(ref session) = app.session {
                            let tree = session.session_manager().get_tree();
                            let leaf_id = session.session().get_leaf_id();
                            let signal_select = app.overlay_result_signal.clone();
                            let signal_cancel = app.overlay_result_signal.clone();
                            let label_signal = app.pending_label_changes.clone();
                            let mut tree_selector = crate::agent::ui::components::TreeSelector::new(
                                tree,
                                leaf_id,
                                rows as usize,
                                None,
                            );
                            // Restore cursor to the entry the user had selected
                            if !entry_id.is_empty() {
                                tree_selector.set_initial_selection(&entry_id);
                            }
                            tree_selector.on_select = Some(Box::new(move |eid| {
                                *signal_select.borrow_mut() =
                                    Some(OverlayResult::TreeNavigateTo(eid));
                            }));
                            tree_selector.on_cancel = Some(Box::new(move || {
                                *signal_cancel.borrow_mut() = Some(OverlayResult::TreeCancelled);
                            }));
                            tree_selector.on_label_change = Some(Box::new(move |eid, label| {
                                label_signal.borrow_mut().push((eid, label));
                            }));
                            tui.show_top_overlay(Box::new(tree_selector));
                        }
                    }
                }
            }
            dirty = true;
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
                chat_info(&mut app, &msg);
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
                    chat_info(&mut app, &err_msg);
                }
            }
        }

        // Handle pending agent submission (async).
        // During streaming, submit_message uses agent.steer() directly so
        // pending_submit is only set for the idle path. Processed here as
        // soon as is_streaming becomes false.
        if !app.is_streaming
            && let Some(text) = app.pending_submit.take()
        {
            start_agent_loop(&mut app, text).await;
            dirty = true;
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
                    // Open session picker
                    let mut picker = crate::agent::ui::components::SessionPicker::new();
                    let repo = crate::agent::DefaultSessionRepo::new();
                    picker.load_sessions(&repo);
                    app.session_picker = Some(picker);
                    app.status_text = None;
                }
                CommandResult::OpenModelSelector => {
                    open_model_selector(&mut app, &mut tui);
                }
                CommandResult::OpenSettings => {
                    chat_info(&mut app, "Settings menu - not yet implemented.");
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
                        chat_info(
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
                    // Show the tree selector overlay
                    if let Some(ref session) = app.session {
                        let tree = session.session_manager().get_tree();
                        let leaf_id = session.session().get_leaf_id();
                        let signal_select = app.overlay_result_signal.clone();
                        let signal_cancel = app.overlay_result_signal.clone();
                        let label_signal = app.pending_label_changes.clone();
                        let mut tree_selector = crate::agent::ui::components::TreeSelector::new(
                            tree,
                            leaf_id,
                            rows as usize,
                            None,
                        );
                        tree_selector.on_select = Some(Box::new(move |entry_id| {
                            *signal_select.borrow_mut() =
                                Some(OverlayResult::TreeNavigateTo(entry_id));
                        }));
                        tree_selector.on_cancel = Some(Box::new(move || {
                            *signal_cancel.borrow_mut() = Some(OverlayResult::TreeCancelled);
                        }));
                        tree_selector.on_label_change = Some(Box::new(move |entry_id, label| {
                            label_signal.borrow_mut().push((entry_id, label));
                        }));
                        use crate::tui::focusable::Focusable;
                        tree_selector.set_focused(true);
                        tui.show_top_overlay(Box::new(tree_selector));
                    } else {
                        chat_info(&mut app, "No active session.");
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
            // Update section components from compose_ui
            compose_ui(&mut app, cols as usize);
            tui.set_dimensions(cols as usize, rows as usize);
            tui.render(cols as usize, rows as usize, &mut stdout)?;
            dirty = false;
        }

        // Idle backpressure: sleep briefly so we don't busy-wait when idle.
        // Active frames (dirty, streaming, working spinner) run at ~60fps;
        // idle frames pace at ~20fps to save CPU/battery.
        tokio::time::sleep(if dirty || app.is_streaming || app.working.should_show() {
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
///   header → chat_container (messages) → pending → status → queued → working → editor → footer
fn compose_ui(app: &mut App, width: usize) {
    // ── Session picker ──
    if let Some(ref picker) = app.session_picker {
        let (_lines, _cursor_y) = picker.render(width, &app.theme as &dyn crate::tui::Theme);
        // Clear chat container when picker is active
        app.chat_container.borrow_mut().clear();
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

    // ── Queued message indicator (pi-style: shows queued messages during streaming) ──
    if app.is_streaming {
        // Show pending_submit if set (idle path, before agent loop starts)
        if let Some(ref msg) = app.pending_submit {
            let preview = if msg.len() > 60 {
                format!("{}…", &msg[..60])
            } else {
                msg.clone()
            };
            let line = app
                .theme
                .fg_key(ThemeKey::Dim, &format!(" 📝 queued: {}", preview));
            status_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
        }
    }
    app.status_section.borrow_mut().set_lines(status_lines);

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
    if tui.root.handle_input(key) {
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
    } else {
        // Not streaming — submit directly
        if app.is_streaming {
            app.is_streaming = false;
        }
        submit_message(app, trimmed);
    }
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
        let ctx = s.session().build_session_context();
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
            Err(e) => chat_info(app, format!("Login failed: {}", e)),
        }
    } else {
        chat_info(app, format!("Usage: /login {} <api-key>", provider));
    }
}

/// Handle a logout command result.
fn handle_logout(app: &mut App, provider: Option<&str>) {
    match auth::logout(provider) {
        Ok(true) => {
            let msg = provider
                .map(|p| format!("Logged out from {}", p))
                .unwrap_or_else(|| "Logged out from all providers".into());
            chat_info(app, msg);
        }
        Ok(false) => {
            let msg = provider
                .map(|p| format!("No credentials for {}", p))
                .unwrap_or_else(|| "No credentials found".into());
            chat_info(app, msg);
        }
        Err(e) => {
            chat_info(app, format!("Logout failed: {}", e));
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

    tui.show_top_overlay(Box::new(selector));
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

    tui.show_top_overlay(Box::new(dialog));
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
                let cred = crate::auth::AuthCredential::Oauth {
                    access: credentials.access.clone(),
                    refresh: Some(credentials.refresh.clone()),
                    expires: Some(credentials.expires),
                    enterprise_url: credentials.enterprise_url.clone(),
                };
                match crate::auth::login_oauth(&pid, &cred) {
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

    tui.show_top_overlay(Box::new(overlay));
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

    tui.show_top_overlay(Box::new(selector));
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
    tui.show_top_overlay(Box::new(selector));
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
    tui.show_top_overlay(Box::new(selector));
}

fn show_help_overlay(app: &mut App, tui: &mut TUI) {
    let mut overlay = crate::agent::ui::help::HelpOverlay::new(&app.theme);
    overlay.set_commands(app.commands.clone());
    tui.show_overlay(Box::new(overlay), Default::default());
}

/// Submit or queue a user message.
/// When streaming, sets pending_submit which is deferred until the current
/// turn finishes (the main loop skips start_agent_loop while is_streaming).
/// When idle, starts a new agent loop immediately.
fn submit_message(app: &mut App, message: String) {
    app.scroll_offset = 0;
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

/// Build a fresh Agent with the given messages and app configuration.
/// Uses the provider registry to resolve the model and dispatch to the right provider.
#[allow(clippy::too_many_arguments)]
fn build_fresh_agent(
    registry: &ProviderRegistry,
    model: &str,
    api_key: &str,
    system_prompt: &str,
    thinking_level: yoagent::types::ThinkingLevel,
    messages: Vec<yoagent::types::AgentMessage>,
    extensions: &[Box<dyn Extension>],
    default_provider: Option<&str>,
) -> yoagent::agent::Agent {
    use yoagent::provider::model::ApiProtocol;

    let resolved = registry.resolve(model, default_provider).ok();
    let mc = resolved
        .as_ref()
        .map(|r| r.model_config.clone())
        .unwrap_or_else(|| crate::agent::base_model_config(model));
    let api_key = resolved
        .as_ref()
        .map(|r| r.api_key.as_str())
        .unwrap_or(api_key);

    let tools: Vec<Box<dyn yoagent::types::AgentTool>> = extensions
        .iter()
        .flat_map(|ext| ext.tools())
        .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
        .collect();

    let agent = match mc.api {
        ApiProtocol::OpenAiCompletions => {
            yoagent::agent::Agent::new(crate::provider::openai_compat::RabOpenAiCompatProvider)
        }
        ApiProtocol::AnthropicMessages => {
            yoagent::agent::Agent::new(crate::provider::anthropic::RabAnthropicProvider)
        }
        ApiProtocol::OpenAiResponses => {
            yoagent::agent::Agent::new(yoagent::provider::OpenAiResponsesProvider)
        }
        ApiProtocol::GoogleGenerativeAi => {
            yoagent::agent::Agent::new(yoagent::provider::GoogleProvider)
        }
        _ => yoagent::agent::Agent::new(yoagent::provider::OpenAiCompatProvider),
    };

    agent
        .with_model(model)
        .with_api_key(api_key)
        .with_model_config(mc)
        .with_system_prompt(system_prompt)
        .with_thinking(thinking_level)
        .with_messages(messages)
        .with_tools(tools)
        .without_context_management()
}

/// Map rab's thinking level string to yoagent's ThinkingLevel enum.
fn map_thinking_level(level: Option<&str>) -> yoagent::types::ThinkingLevel {
    match level {
        Some("off") => yoagent::types::ThinkingLevel::Off,
        Some("low") => yoagent::types::ThinkingLevel::Low,
        Some("medium") => yoagent::types::ThinkingLevel::Medium,
        Some("high") | Some("xhigh") => yoagent::types::ThinkingLevel::High,
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
async fn start_agent_loop(app: &mut App, message: String) {
    if app.session.is_none() {
        return;
    }

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);

    let thinking = map_thinking_level(app.thinking_level.as_deref());

    // Build or reuse agent. On the first turn the session has no messages;
    // on subsequent turns the reused agent already has messages restored
    // by agent.finish() — no need to sync from session here.
    let msgs = app
        .session
        .as_ref()
        .map(|s| s.session().build_session_context().messages)
        .unwrap_or_default();

    // Record model/thinking changes in the session before borrowing agent
    let model = app.model.clone();
    app.record_model_change(&model);
    if let Some(ref mut session) = app.session {
        session.on_thinking_level_change(app.thinking_level.as_deref().unwrap_or("off"));
    }

    let agent: &mut yoagent::agent::Agent = match &mut app.agent {
        Some(existing) => {
            // Reuse existing agent — messages are already correct from
            // agent.finish(). Compaction sync is handled separately by
            // handle_auto_compact / handle_compact_command.
            existing
        }
        None => {
            let preferred = if !app.current_provider.is_empty() {
                Some(app.current_provider.as_str())
            } else {
                app.settings.default_provider.as_deref()
            };
            app.agent = Some(build_fresh_agent(
                &app.registry,
                &app.model,
                &app.api_key,
                &app.system_prompt,
                thinking,
                msgs,
                &app.extensions,
                preferred,
            ));
            // SAFETY: we just set app.agent to Some(...)
            app.agent.as_mut().unwrap()
        }
    };

    // Start the turn: agent.prompt() spawns the loop internally, keeps the
    // Agent in scope, and returns a receiver for streaming events.
    let mut rx = agent.prompt(message).await;

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
        chat_info(app, "No active session to compact".to_string());
        return;
    }

    let agent_session = app.session.as_mut().unwrap();

    app.working.start();

    match agent_session
        .run_manual_compact(custom_instructions.as_deref())
        .await
    {
        Ok(_summary) => {
            app.working.stop();
            app.status_text = None;
            app.rebuild_from_session_context();
            show_status(app, "Compaction completed".to_string());
        }
        Err(e) => {
            app.working.stop();
            app.status_text = None;
            chat_info(app, format!("Compaction failed: {}", e));
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

    // Find the command handler first (before mutable borrow on app)
    for ext in app.extensions.iter() {
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
                        chat_info(app, format!("Error executing /{}: {}", cmd_name, e));
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
            chat_info(app, msg.clone());
        }
        CommandResult::Quit => {
            app.should_quit = true;
        }
        CommandResult::ModelChanged(model) => {
            app.model = model.clone();
            app.current_provider = app
                .registry
                .provider_for_model(&model, Some(&app.current_provider))
                .unwrap_or_default();
            app.record_model_change(&model);
            app.status_text = Some(format!("Model: {}", model));
        }
        CommandResult::ShowHelp => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::Reloaded => {
            app.refresh_registry();

            // Refresh cached model list from the updated registry.
            {
                let models = app.registry.list_models();
                app.available_models = models.clone();
                for ext in app.extensions.iter() {
                    if let Some(cmd) = ext
                        .as_any()
                        .downcast_ref::<crate::builtin::commands::CommandsExtension>()
                    {
                        cmd.set_available_models(models.clone());
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
                    app.auto_compact = app.settings.auto_compact.unwrap_or(true);
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

            // 4. Reload skills from disk (pi-compatible)
            let new_skill_set =
                yoagent::skills::SkillSet::load(&app.skill_dirs).unwrap_or_default();
            app.skills = new_skill_set.skills().to_vec();
            reload_parts.push("skills");

            // 5. Reload prompt templates from disk (pi-compatible)
            app.prompt_templates =
                crate::agent::prompt_templates::load_prompt_templates(&app.prompt_template_dirs);
            // Only report if there are any template dirs configured
            if !app.prompt_template_dirs.is_empty() {
                reload_parts.push("prompts");
            }

            // 5. Reload context files (AGENTS.md / CLAUDE.md) and system prompt (pi-compatible)
            let context_files =
                crate::agent::context_files::load_context_files(&app.cwd, &app.agent_dir);
            // Load SYSTEM.md: project `.rab/SYSTEM.md` first, then global
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
            // Load APPEND_SYSTEM.md: project `.rab/APPEND_SYSTEM.md` first, then global
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

            // Rebuild tool snippets from current extensions
            let all_tools: Vec<crate::agent::extension::ToolDefinition> =
                app.extensions.iter().flat_map(|ext| ext.tools()).collect();
            let tool_snippets: Vec<crate::agent::ToolSnippet> = all_tools
                .iter()
                .map(|twm| crate::agent::ToolSnippet {
                    name: twm.name().to_string(),
                    description: twm.snippet.to_string(),
                })
                .collect();
            let has_read_tool = tool_snippets.iter().any(|t| t.name == "read");

            let new_system_prompt = crate::agent::SystemPromptBuilder::new()
                .tool_snippets(tool_snippets)
                .context_files(context_files.clone())
                .custom_prompt(custom_system_md)
                .append_prompt(append_system_md)
                .skills(new_skill_set)
                .has_read_tool(has_read_tool)
                .cwd(&app.cwd)
                .build();
            app.system_prompt = new_system_prompt;

            // Store context files for header resource display
            let context_file_list: Vec<String> = context_files
                .iter()
                .map(|cf| {
                    let cwd_str = app.cwd.to_string_lossy();
                    if let Some(rel) = cf.path.to_string_lossy().strip_prefix(&cwd_str as &str) {
                        if rel.is_empty() {
                            cf.path.to_string_lossy().to_string()
                        } else {
                            format!("./{}", rel.trim_start_matches('/'))
                        }
                    } else if let Some(home) =
                        std::env::var_os("HOME").and_then(|h| h.into_string().ok())
                        && let Some(rel) = cf.path.to_string_lossy().strip_prefix(&home)
                    {
                        if rel.is_empty() {
                            cf.path.to_string_lossy().to_string()
                        } else {
                            format!("~/{}", rel.trim_start_matches('/'))
                        }
                    } else {
                        cf.path.to_string_lossy().to_string()
                    }
                })
                .collect();
            app.context_files = context_file_list.clone();
            // Update header resource data
            {
                let skill_names: Vec<String> = app.skills.iter().map(|s| s.name.clone()).collect();
                let template_names: Vec<String> = app
                    .prompt_templates
                    .iter()
                    .map(|t| t.name.clone())
                    .collect();
                let extension_names: Vec<String> = app
                    .extensions
                    .iter()
                    .map(|e| e.name().to_string())
                    .collect();
                let theme_names: Vec<String> = crate::agent::ui::theme::get_available_themes()
                    .into_iter()
                    .filter(|n| n != "dark" && n != "light")
                    .collect();
                app.header.borrow_mut().set_resource_data(
                    context_file_list,
                    skill_names,
                    template_names,
                    extension_names,
                    theme_names,
                );
            }
            reload_parts.push("system prompt");
            reload_parts.push("context files");

            // 6. Rebuild slash commands and commands list with updated skills
            {
                use crate::tui::autocomplete::SlashCommand as AutoSlashCommand;
                let mut auto_commands: Vec<AutoSlashCommand> =
                    app.extensions
                        .iter()
                        .flat_map(|e| e.commands())
                        .map(|cmd| {
                            let handler = cmd.handler;
                            AutoSlashCommand {
                                name: cmd.name,
                                description: Some(cmd.description),
                                argument_hint: None,
                                argument_completions: None,
                                get_argument_completions: Some(
                                    std::sync::Arc::new(
                                        move |prefix: &str| -> Vec<
                                            crate::tui::autocomplete::AutocompleteItem,
                                        > {
                                            handler
                                                .argument_completions(prefix)
                                                .into_iter()
                                                .map(|item| {
                                                    crate::tui::autocomplete::AutocompleteItem {
                                                        value: item.value,
                                                        label: item.label,
                                                        description: item.description,
                                                    }
                                                })
                                                .collect()
                                        },
                                    ),
                                ),
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

            // Rebuild commands list for help overlay
            app.commands = app
                .extensions
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

            // 7. Notify extensions that reload is complete (pi-compatible: session_start)
            for ext in app.extensions.iter() {
                ext.on_session_start("reload");
            }

            chat_info(app, format!("{} reloaded.", reload_parts.join(", ")));
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
        CommandResult::SessionInfo {
            session_id,
            file_path,
            name,
            message_count,
            user_messages,
            assistant_messages,
            tool_calls,
            tool_results,
            total_tokens,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            cost,
        } => {
            let name_display = name
                .or_else(|| {
                    app.session
                        .as_ref()
                        .and_then(|s| s.session().session_name())
                })
                .unwrap_or_else(|| "unnamed".to_string());
            let file_display = file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "in-memory".to_string());
            let sid = if session_id.is_empty() {
                app.session
                    .as_ref()
                    .map(|s| s.session().session_id())
                    .unwrap_or_default()
            } else {
                session_id
            };

            let total_messages = message_count;

            // Build info display matching pi's handleSessionCommand
            let mut info = format!(
                "Session Info\n\n\
                 Name: {name_display}\n\
                 File: {file_display}\n\
                 ID: {sid}\n\
                 \n\
                 Messages\n\
                 User: {user_messages}\n\
                 Assistant: {assistant_messages}\n\
                 Tool Calls: {tool_calls}\n\
                 Tool Results: {tool_results}\n\
                 Total: {total_messages}\n\
                 \n\
                 Tokens\n\
                 Input: {}\n\
                 Output: {}",
                format_number(input_tokens),
                format_number(output_tokens),
            );
            if cache_read_tokens > 0 {
                info += &format!("\nCache Read: {}", format_number(cache_read_tokens));
            }
            if cache_write_tokens > 0 {
                info += &format!("\nCache Write: {}", format_number(cache_write_tokens));
            }
            info += &format!("\nTotal: {}", format_number(total_tokens));

            if cost > 0.0 {
                info += &format!("\n\nCost\nTotal: {:.4}", cost);
            }

            // Parent session (fork chain)
            if let Some(ref asession) = app.session
                && let Some(file_path) = asession.session().session_file().as_ref()
                && let Some(h) = crate::agent::session::read_session_header(file_path)
                && let Some(ref parent) = h.parent_session
            {
                info += &format!("\n\nParent: {}", parent);
            }

            chat_info(app, info.clone());
        }
        CommandResult::OpenSessionSelector => {
            // Load and display available sessions
            use crate::agent::SessionRepo;
            let repo = crate::agent::DefaultSessionRepo::new();
            let sessions = repo.list_all(None);

            if sessions.is_empty() {
                let msg = "No sessions found.".to_string();
                chat_info(app, msg.clone());
            } else {
                let mut info = format!("Available Sessions ({} total)\n\n", sessions.len());
                for (i, s) in sessions.iter().take(20).enumerate() {
                    let name = s.name.as_deref().unwrap_or("unnamed");
                    let cwd_short = s.cwd.rsplit('/').next().unwrap_or(&s.cwd);
                    info += &format!(
                        "{}. {}  [{}]  {} msgs\n   {}\n\n",
                        i + 1,
                        name,
                        fmt_time_short(&s.created),
                        s.message_count,
                        cwd_short,
                    );
                }
                if sessions.len() > 20 {
                    info += &format!("... and {} more sessions\n", sessions.len() - 20);
                }
                info += "Use /resume to open the interactive picker";

                chat_info(app, info.clone());
            }
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
                .and_then(|s| s.session().session_name());
            if let Some(ref stored) = stored_name
                && stored != &name
            {
                chat_info(
                    app,
                    format!("Session name normalized from {:?} to {:?}", name, stored),
                );
            }

            chat_info(
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

            // Update session info and footer (refresh_from_session picks up the new name)
            app.update_session_info();
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
                    chat_info(app, format!("✓ Session exported to: {}", display));
                }
                Err(msg) => {
                    chat_info(app, format!("✗ {}", msg));
                }
            }
        }
        result @ CommandResult::ImportSession { .. } => {
            // Needs TUI overlay (confirmation) - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::ShareSession => {
            let msg = "Share session - not yet implemented.".to_string();
            chat_info(app, msg.clone());
        }
        CommandResult::CopyLastMessage => {
            // Get last assistant message text (pi-compatible)
            let text = app.session.as_ref().and_then(|s| {
                let entries = s.session().get_entries();
                entries.iter().rev().find_map(|entry| {
                    if let SessionEntry::Message(m) = entry
                        && matches!(
                                &m.message,
                                yoagent::types::AgentMessage::Llm(
                                    yoagent::types::Message::Assistant {
                                        stop_reason, ..
                                    },
                                ) if *stop_reason != yoagent::types::StopReason::Aborted
                                    || !crate::agent::types::message_text(&m.message)
                                        .trim()
                                        .is_empty()
                        )
                    {
                        let text = crate::agent::types::message_text(&m.message);
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            return Some(trimmed.to_string());
                        }
                    }
                    None
                })
            });

            let text = match text {
                Some(t) => t,
                None => {
                    chat_info(app, "No agent messages to copy yet.");
                    return;
                }
            };

            // Pi-compatible clipboard copy (includes OSC 52 fallback)
            copy_to_clipboard(&text);
            chat_info(app, "Copied last agent message to clipboard");
        }
        CommandResult::ShowChangelog => {
            let msg = "Changelog - not yet implemented.".to_string();
            chat_info(app, msg.clone());
        }
        CommandResult::ForkSession { message_id } => {
            // Clone the session info before modifying app.session
            let source_path = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_file());
            let session_dir = app.session.as_ref().map(|s| s.session_dir().to_path_buf());
            let cwd = app.cwd.clone();

            match (source_path, session_dir) {
                (Some(ref source), Some(ref target_dir)) => {
                    match crate::agent::session::fork_session(
                        source,
                        target_dir,
                        message_id.as_deref(),
                        None,
                    ) {
                        Ok(new_id) => {
                            // Find the new session file
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
                                    // Open the new session and replace the current one
                                    let new_session =
                                        crate::agent::AgentSession::open(path, None, Some(&cwd));
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
                                    let msg =
                                        format!("Fork created but new file not found: {}", new_id);
                                    chat_info(app, msg);
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Fork failed: {}", e);
                            chat_info(app, msg.clone());
                        }
                    }
                }
                _ => {
                    let msg = "No active session to fork".to_string();
                    chat_info(app, msg.clone());
                }
            }
        }
        CommandResult::CloneSession => {
            let msg = "Clone session - not yet implemented.".to_string();
            chat_info(app, msg.clone());
        }
        CommandResult::SessionTree => {
            // Needs TUI overlay — defer
            app.pending_command_result = Some(result);
        }
        CommandResult::TrustDecision { decision } => {
            let msg = format!("Trust decision '{}' saved.", decision);
            chat_info(app, msg.clone());
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
            // If streaming, interrupt first
            if app.is_streaming {
                interrupt_streaming(app);
            }
            app.pending_compact = Some(custom_instructions);
        }
    }
}

/// Look up a tool renderer by name from extensions (bundled in ToolDefinition.renderer).
fn find_tool_renderer(
    extensions: &[Box<dyn crate::agent::extension::Extension>],
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
    extensions: &[Box<dyn crate::agent::extension::Extension>],
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
            if let Some(text) = crate::agent::types::message_extension_text(msg) {
                if !chat.children().is_empty() {
                    chat.add_child(std::boxed::Box::new(Spacer::new(1)));
                }
                chat.add_child(std::boxed::Box::new(InfoMessageComponent::new(text)));
            }
        }
    }
}

/// Add a Component to chat_container with a spacer before it if chat_container is not empty.
/// Mirrors pi's `addMessageToChat()` which adds `new Spacer(1)` before each message
/// when `this.chatContainer.children.length > 0`.
pub fn chat_add(app: &mut App, component: std::boxed::Box<dyn Component>) {
    let mut chat = app.chat_container.borrow_mut();
    if !chat.children().is_empty() {
        chat.add_child(std::boxed::Box::new(Spacer::new(1)));
    }
    chat.add_child(component);
}

/// Convenience shortcut: add an InfoMessageComponent to chat.
pub fn chat_info(app: &mut App, msg: impl Into<String>) {
    chat_add(
        app,
        std::boxed::Box::new(InfoMessageComponent::new(msg.into())),
    );
}

/// Add an AssistantMessageComponent with a preceding spacer.
fn add_assistant_message(chat: &mut crate::tui::Container, text: &str, hide_thinking: bool) {
    if !chat.children().is_empty() {
        chat.add_child(std::boxed::Box::new(Spacer::new(1)));
    }
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

    tui.show_top_overlay(Box::new(prompt));
}

/// Show a status message in the chat (pi-style `showStatus`).
///
/// If the last two children of `chat_container` are from a previous status
/// (spacer + InfoMessageComponent), they are replaced in-place rather than
/// appending new entries. This prevents multiple consecutive status messages
/// from accumulating at the end of the chat session.
fn show_status(app: &mut App, message: String) {
    let mut chat = app.chat_container.borrow_mut();
    // Check if previous status children are still the last in the container
    if let Some(prev_len) = app.last_status_len
        && chat.len() == prev_len
        && prev_len >= 2
    {
        chat.pop_child(); // info message
        chat.pop_child(); // spacer
    }
    app.last_status_len = None;
    drop(chat);

    // Add the new status
    let mut chat = app.chat_container.borrow_mut();
    if !chat.children().is_empty() {
        chat.add_child(std::boxed::Box::new(Spacer::new(1)));
    }
    chat.add_child(std::boxed::Box::new(InfoMessageComponent::new(message)));
    app.last_status_len = Some(chat.len());
}

/// Concatenate all Text content from a slice of Content values.
fn extract_text_content(content: &[yoagent::types::Content]) -> String {
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

/// Handle agent events from the channel.
///
/// Delegates persistence to `AgentSession::on_agent_event()` (single source of truth)
/// and only handles display/UI logic here. This mirrors pi's single _handleAgentEvent
/// that all modes share — the mode-agnostic persistence lives on AgentSession, and each
/// mode adds display on top.
fn handle_agent_event(app: &mut App, event: yoagent::types::AgentEvent) {
    // ── Persistence: delegate to the shared handler (single source of truth) ──
    // Handle with &event before the display match consumes it.
    {
        let ev = &event;
        if let E::MessageEnd { message } = ev {
            if crate::agent::types::message_is_user(message)
                && let Some(ref mut s) = app.session
            {
                s.reset_overflow_recovery();
            }
            if crate::agent::types::message_error(message).is_none()
                && !crate::agent::types::message_is_system_stop(message)
                && let Some(ref mut s) = app.session
            {
                s.on_agent_event(ev);
            }
        }
        if let E::ToolExecutionEnd { tool_call_id, .. } = ev
            && tool_call_id != "__bang__"
            && let Some(ref mut s) = app.session
        {
            s.on_agent_event(ev);
        }
        if let E::AgentEnd { .. } = ev
            && let Some(ref mut s) = app.session
        {
            s.on_agent_event(ev);
        }
    }

    // ── Display logic (consumes owned event) ──
    use yoagent::types::AgentEvent as E;
    match event {
        E::AgentStart => {
            app.is_streaming = true;
            app.working.start();
            app.refresh_git_branch();
        }
        E::TurnStart => {}
        E::MessageStart { message } => {
            // Add user messages to chat when the agent loop processes them.
            // Covers both the initial prompt (non-streaming) and
            // steered/follow-up messages queued during streaming.
            if crate::agent::types::message_is_user(&message) {
                let text = crate::agent::types::message_text(&message);
                if !text.is_empty() {
                    chat_add(
                        app,
                        std::boxed::Box::new(
                            crate::agent::ui::components::UserMessageComponent::new(&text),
                        ),
                    );
                }
            }
        }
        E::MessageUpdate { delta, .. } => {
            use yoagent::types::StreamDelta;
            match delta {
                StreamDelta::Text { delta } => {
                    if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                        weak.borrow_mut().append_text(&delta);
                    } else {
                        use crate::tui::components::rc_ref_cell_component::RcRefCellComponent;
                        let comp = Rc::new(RefCell::new(
                            crate::agent::ui::components::AssistantMessageComponent::new(&delta),
                        ));
                        if app.hide_thinking {
                            comp.borrow_mut().set_hide_thinking(true);
                        }
                        app.streaming_component = Some(Rc::downgrade(&comp));
                        app.chat_container
                            .borrow_mut()
                            .add_child(std::boxed::Box::new(RcRefCellComponent(comp)));
                    }
                }
                StreamDelta::Thinking { delta } => {
                    if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                        weak.borrow_mut()
                            .add_thinking(&delta, app.thinking_level.clone());
                    } else {
                        use crate::tui::components::rc_ref_cell_component::RcRefCellComponent;
                        let mut comp =
                            crate::agent::ui::components::AssistantMessageComponent::new("");
                        comp.add_thinking(&delta, app.thinking_level.clone());
                        if app.hide_thinking {
                            comp.set_hide_thinking(true);
                        }
                        let comp = Rc::new(RefCell::new(comp));
                        app.streaming_component = Some(Rc::downgrade(&comp));
                        app.chat_container
                            .borrow_mut()
                            .add_child(std::boxed::Box::new(RcRefCellComponent(comp)));
                    }
                }
                StreamDelta::ToolCallDelta { .. } => {}
            }
        }
        E::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => {
            app.pending_tool_executions += 1;
            app.streaming_component = None;
            let name = tool_name;
            let renderer = find_tool_renderer(&app.extensions, &name);
            let started_at = std::time::Instant::now();
            let (invalidate_tx, invalidate_rx) =
                crate::agent::ui::components::ToolExecComponent::make_invalidation_channel();
            app.invalidate_rxs.push(invalidate_rx);
            let comp: Rc<RefCell<_>> = {
                let mut tool = crate::agent::ui::components::ToolExecComponent::new(
                    &name,
                    renderer,
                    args.clone(),
                    app.cwd.to_string_lossy().to_string(),
                    tool_call_id.clone(),
                );
                tool.set_started_at(std::time::Instant::now());
                tool.set_invalidate_tx(invalidate_tx);
                Rc::new(RefCell::new(tool))
            };
            comp.borrow_mut().set_expanded(app.tools_expanded);
            app.pending_tools
                .insert(tool_call_id.clone(), Rc::downgrade(&comp));
            app.tool_call_start_times
                .insert(tool_call_id.clone(), started_at);
            chat_add(
                app,
                std::boxed::Box::new(crate::agent::ui::components::RcToolExec(comp)),
            );
        }
        E::ToolExecutionUpdate {
            tool_call_id,
            partial_result,
            ..
        } => {
            // Forward partial results to the pending tool component (live streaming).
            let partial_text = extract_text_content(&partial_result.content);
            if !partial_text.is_empty()
                && let Some(weak) = app.pending_tools.get(&tool_call_id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut().append_output(&partial_text);
            }
        }
        E::ToolExecutionEnd {
            tool_call_id,
            tool_name: _,
            result,
            is_error,
        } => {
            app.pending_tool_executions = app.pending_tool_executions.saturating_sub(1);
            let content = extract_text_content(&result.content);
            if let Some(weak) = app.pending_tools.get(&tool_call_id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut()
                    .set_result_with_details(&content, is_error, Some(result.details));
                app.tool_call_start_times.remove(&tool_call_id);
            }
        }
        E::ProgressMessage {
            text, tool_name, ..
        } => {
            // Bang (") command progress feeds into pending_tools["__bang__"]
            if let Some(weak) = app.pending_tools.get("__bang__")
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut().append_output(&text);
            } else if tool_name.is_empty() {
                // General progress message (not tool-specific) — show as status
                app.status_text = Some(text.trim().to_string());
            }
        }
        E::TurnEnd { message, .. } => {
            app.streaming_component = None;
            // Surface provider errors carried by the turn's final message.
            if let Some(err) = crate::agent::types::message_error(&message) {
                chat_info(app, format!("Provider error: {}", err));
            }
        }
        E::AgentEnd { messages } => {
            app.streaming_component = None;
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            // Refresh footer cached stats from session at turn end (pull-based)
            if let Some(ref s) = app.session {
                app.footer.borrow_mut().refresh_from_session(s.session());
            }
            // Pi-compatible: schedule auto-compaction check after agent ends.
            // check_auto_compact() is called asynchronously in the main loop.
            app.pending_auto_compact = app.auto_compact;
            // Detect silent stops / provider errors: surface any assistant message
            // that ended without visible output (empty content or provider error).
            // Provider errors with error_message set were never forwarded as
            // MessageEnd events (the provider returned Err() without streaming),
            // so they must be surfaced here.
            for msg in messages.iter().rev() {
                if let Some(yoagent::types::Message::Assistant {
                    content,
                    stop_reason,
                    error_message,
                    ..
                }) = msg.as_llm()
                    && stop_reason != &yoagent::types::StopReason::ToolUse
                {
                    if let Some(err) = error_message {
                        chat_info(app, format!("Provider error: {}", err));
                        break;
                    }
                    // Check for any visible content: non-empty text or tool calls.
                    // Thinking blocks alone don't count as visible output
                    // (they may be hidden or just cut-off thoughts).
                    let has_visible = content.iter().any(|c| match c {
                        yoagent::types::Content::Text { text } => !text.trim().is_empty(),
                        yoagent::types::Content::ToolCall { .. } => true,
                        _ => false,
                    });
                    if !has_visible {
                        chat_info(
                            app,
                            "The agent returned an empty response. \
                                 This can happen when the provider's context \
                                 limit is exceeded or the model declined to \
                                 respond. Try sending a new message."
                                .to_string(),
                        );
                        break;
                    }
                }
            }
        }
        E::MessageEnd { message } => {
            // Special cases: persist as extension (excluded from LLM context).
            // Normal persistence handled by if-let above before the display match.
            if let Some(err) = crate::agent::types::message_error(&message) {
                chat_info(app, err.to_string());
                let ext = crate::agent::types::extension_message("error", err, true);
                if let Some(ref mut s) = app.session {
                    s.persist_extension_message(&ext);
                }
            } else if crate::agent::types::message_is_system_stop(&message) {
                let text = crate::agent::types::message_text(&message);
                chat_info(app, text.clone());
                if let Some(ref mut s) = app.session {
                    let ext = crate::agent::types::extension_message("system_stop", text, true);
                    s.persist_extension_message(&ext);
                }
            } else if crate::agent::types::message_is_extension(&message) {
                // Extension messages: display in chat (persisted by on_agent_event).
                if let Some(text) = crate::agent::types::message_extension_text(&message) {
                    chat_info(app, text);
                }
            }
        }
        E::InputRejected { reason } => {
            let msg = format!("Input rejected: {}", reason);
            chat_info(app, msg);
        }
    }
}

/// Parse a ! or !! bang command from input.
fn parse_bang_command(input: &str) -> Option<(String, bool)> {
    if let Some(rest) = input.strip_prefix("!!") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), true))
        }
    } else if let Some(rest) = input.strip_prefix('!') {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), false))
        }
    } else {
        None
    }
}

/// Format a number with locale-style thousands separators (e.g. 1234 -> "1,234").
fn format_number(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

/// Format a DateTime for short display (YYYY-MM-DD HH:MM).
fn fmt_time_short(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M").to_string()
}

// ── Skills utilities (moved inline from skills.rs) ─────────────────

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn strip_frontmatter(content: &str) -> String {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content.to_string();
    }
    let remaining = &content[3..];
    let end = match remaining.find("---") {
        Some(pos) => pos,
        None => return content.to_string(),
    };
    let body_start = 3 + end + 3;
    content[body_start..].trim().to_string()
}

fn read_skill_body(file_path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
    Some(strip_frontmatter(&content))
}

fn format_skill_invocation(skill: &yoagent::skills::Skill, extra: Option<&str>) -> Option<String> {
    let body = read_skill_body(&skill.file_path)?;
    let block = format!(
        r#"<skill name="{}" location="{}">
References are relative to {}.

{}
</skill>"#,
        xml_escape(&skill.name),
        xml_escape(&skill.file_path.to_string_lossy()),
        xml_escape(&skill.base_dir.to_string_lossy()),
        body
    );
    Some(match extra {
        Some(instr) if !instr.is_empty() => format!("{}\n\n{}", block, instr),
        _ => block,
    })
}

fn expand_skill_command(text: &str, skills: &[yoagent::skills::Skill]) -> String {
    if !text.starts_with("/skill:") {
        return text.to_string();
    }
    let rest = &text[7..];
    let (skill_name, args) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
        None => (rest, ""),
    };
    match skills.iter().find(|s| s.name == skill_name) {
        Some(s) => format_skill_invocation(s, if args.is_empty() { None } else { Some(args) })
            .unwrap_or_else(|| text.to_string()),
        None => text.to_string(),
    }
}

/// Parse a skill block from text (pi-compatible).
/// Returns Some((name, body, user_message)) if the text is a skill block.
pub fn parse_skill_block(text: &str) -> Option<(&str, &str, Option<&str>)> {
    let text = text.trim();
    let after_open = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = after_open.split_once("\" location=\"")?;
    let (_location, rest) = rest.split_once("\">\n")?;
    // Find closing tag to extract body
    let close_tag = "\n</skill>";
    let content_end = rest.rfind(close_tag)?;
    let body = rest[..content_end].trim();
    let after_close = rest[content_end + close_tag.len()..].trim();
    let user_message = if after_close.is_empty() {
        None
    } else {
        Some(after_close)
    };
    Some((name, body, user_message))
}

/// Format a skill block for display (prettify XML into a readable form).
/// Returns None if the text is not a skill block.
pub fn format_skill_block_for_display(text: &str) -> Option<String> {
    let (name, body, user_message) = parse_skill_block(text)?;
    let mut result = String::new();
    // Markdown bold label: **[skill] name**
    result.push_str("**[");
    result.push_str("skill] ");
    result.push_str(name);
    result.push_str("**\n\n");
    // Body content
    result.push_str(body);
    result.push('\n');
    // Append user message if present
    if let Some(msg) = user_message {
        result.push_str("\n---\n");
        result.push_str(msg);
        result.push('\n');
    }
    Some(result)
}
