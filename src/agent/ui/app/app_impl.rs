//! The `App` struct, `AppConfig`, constructor, and main event loop.
//!
//! This is the heart of the interactive UI: state, lifecycle, and rendering.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::agent::AgentSession;
use crate::agent::footer_data_provider::FooterDataProvider;
use crate::agent::ui::chat_editor::ChatEditor;
use crate::agent::ui::components::EditorComponent;
use crate::agent::ui::components::FooterComponent;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
use crate::extension::{CommandResult, Extension};
use crate::provider;
use crate::provider::ProviderRegistry;
use crate::tui::Component;
use crate::tui::TUI;
use crate::tui::components::RcRefCellComponent;
use crate::tui::components::Spacer;
use crate::tui::terminal::{self, ProcessTerminal, TerminalTrait};
use crossterm::event::KeyEventKind;
use tokio::sync::mpsc;

use super::agent::handle_auto_compact;
use super::agent::handle_compact_command;
use super::agent::start_agent_loop;
use super::auth::{
    complete_login, handle_login, show_api_key_login_dialog, show_auth_type_or_provider_selector,
    show_login_provider_selector, show_logout_provider_selector, show_oauth_login_dialog,
};
use super::chat::chat_add;
use super::chat::rebuild_chat_from_messages;
use super::chat::show_status;
use super::events::handle_agent_event;
use super::handlers::compose_ui;
use super::handlers::handle_input;
use super::overlays::{
    apply_settings_change, open_extensions_selector, open_model_selector,
    open_scoped_models_selector, open_settings, show_help_overlay, show_summarization_prompt,
};
use super::types::{OverlayResult, PendingLabelChanges};
use crate::tui::components::Text;
use crate::tui::focusable::Focusable;

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
    pub cwd: PathBuf,
    pub model: String,
    pub current_provider: String,
    pub thinking_level: Option<String>,
    pub system_prompt: String,
    pub theme: RabTheme,

    /// Slash commands from all extensions.
    pub commands: Vec<(String, String)>,

    /// Available models for the model selector.
    pub available_models: Vec<String>,
    /// Provider registry for model resolution and provider dispatch.
    pub registry: Arc<ProviderRegistry>,

    /// Component-based chat area - mirrors pi's `this.chatContainer`.
    /// Components are added here in handle_agent_event instead of pushing to messages.
    pub chat_container: Rc<RefCell<crate::tui::Container>>,

    // ── Section components for the UI layout (written by compose_ui) ──
    /// Status text section (transient, dim).
    pub status_section: Rc<RefCell<crate::tui::components::DynamicLines>>,
    /// Working indicator section.
    pub working_section: Rc<RefCell<crate::tui::components::DynamicLines>>,

    /// The chat editor (shared ownership - App mutates, TUI.root renders).
    pub editor: Rc<RefCell<ChatEditor>>,

    /// Agent event channel.
    pub event_tx: mpsc::UnboundedSender<yoagent::types::AgentEvent>,
    pub event_rx: mpsc::UnboundedReceiver<yoagent::types::AgentEvent>,

    /// Streaming state.
    pub is_streaming: bool,
    /// Pending agent submission (set by sync handle_input, consumed by async main loop).
    pub pending_submit: Option<String>,
    /// Pre-loaded messages for the next agent turn (drained from steer/follow-up queues).
    pub pending_preloaded_msgs: Option<Vec<yoagent::types::AgentMessage>>,
    /// Pending manual compaction (carries optional custom instructions).
    pub pending_compact: Option<Option<String>>,
    /// Pending auto-compaction check after AgentEnd (pi-compatible).
    pub pending_auto_compact: bool,
    /// The reused Agent (accumulates messages across turns, supports mid-turn steering).
    pub agent: Option<yoagent::agent::Agent>,
    /// Handle for the forwarding task that relays events from the agent's event
    /// receiver to the UI channel. The Agent stays in `app.agent` during streaming.
    pub forward_handle: Option<tokio::task::JoinHandle<()>>,

    /// Handle for the OAuth login task, aborted on quit to avoid background polling.
    pub oauth_join_handle: Option<tokio::task::JoinHandle<()>>,

    /// Provider ID of an in-flight OAuth login.
    pub pending_oauth_provider: Option<String>,

    /// Display settings.
    pub hide_thinking: bool,
    pub collapse_tool_output: bool,
    /// Global toggle: expand all tool outputs (Ctrl+O). Inverted of collapse_tool_output.
    pub tools_expanded: bool,

    /// Timestamp of last Ctrl+C for double-press detection (pi-style).
    pub last_clear_time: std::time::Instant,

    /// Exit flag.
    pub should_quit: bool,

    /// Number of tool executions currently in-flight.
    pub pending_tool_executions: usize,

    /// Bash abort handle for bang (!) commands.
    pub bash_abort_handle: Option<tokio::task::AbortHandle>,

    /// Session persistence via AgentSession lifecycle layer.
    pub session: Option<AgentSession>,

    /// Footer (shared ownership - App mutates, TUI.root renders).
    pub footer: Rc<RefCell<Footer>>,

    /// Footer data provider (pull-based: git branch, extension statuses).
    pub footer_provider: Rc<RefCell<FooterDataProvider>>,

    /// Pending tool executions keyed by tool call ID.
    pub pending_tools:
        HashMap<String, Weak<RefCell<crate::agent::ui::components::ToolExecComponent>>>,

    /// Start times for pending tool calls, keyed by tool call ID.
    pub tool_call_start_times: HashMap<String, std::time::Instant>,

    /// Receivers for async invalidation notifications (edit tool preview).
    pub invalidate_rxs: Vec<tokio::sync::mpsc::UnboundedReceiver<()>>,

    /// Streaming assistant message component (pi's `streamingComponent`).
    pub streaming_component:
        Option<Weak<RefCell<crate::agent::ui::components::AssistantMessageComponent>>>,

    /// Working indicator.
    pub working: WorkingIndicator,

    /// Transient status text (pi-style: replaces previous status, not added to chat).
    pub status_text: Option<String>,

    /// Pending command result that needs TUI access (overlays etc.).
    pub pending_command_result: Option<CommandResult>,

    /// Overlay result signal — set by overlay callbacks, checked by main loop.
    pub overlay_result_signal: Rc<RefCell<Option<OverlayResult>>>,

    /// Pending scoped model changes from ScopedModelsSelector (session-only, no close).
    pub pending_scoped_ids: Rc<RefCell<Option<Vec<String>>>>,

    /// Pending settings changes from SettingsSelector.
    pub pending_settings_change: Rc<RefCell<Option<(String, String)>>>,

    /// Extensions.
    pub extensions: Arc<Vec<Box<dyn Extension>>>,
    /// Skills loaded for the session (/skill:name expansion).
    pub skills: Vec<yoagent::skills::Skill>,
    /// Skill directories to scan (for /reload support).
    pub skill_dirs: Vec<PathBuf>,
    /// Agent config directory (~/.rab/agent).
    pub agent_dir: PathBuf,
    /// Context file paths (AGENTS.md / CLAUDE.md) loaded for the session.
    pub context_files: Vec<String>,
    /// Prompt template directories to scan (for /reload support).
    pub prompt_template_dirs: Vec<PathBuf>,
    /// Prompt templates loaded for the session (/name expansion).
    pub prompt_templates: Vec<crate::agent::prompt_templates::PromptTemplate>,
    /// API key for yoagent provider.
    pub api_key: String,

    /// Auto-compact toggle state.
    pub auto_compact: bool,

    /// Settings reference for persisting toggle changes.
    pub settings: crate::settings::Settings,

    /// Header component (welcome/onboarding).
    pub header: Rc<RefCell<crate::agent::ui::components::HeaderComponent>>,

    /// Scoped model IDs for cycling (null = all enabled).
    pub scoped_model_ids: Option<Vec<String>>,

    /// Session picker state (Some = picker is active).
    pub session_picker: Option<crate::agent::ui::components::SessionPicker>,

    /// Pending messages section (pi-style: shows queued steer/follow-up messages).
    pub pending_section: Rc<RefCell<crate::tui::components::DynamicLines>>,

    /// Tracks the number of children in `chat_container` after the last
    /// status message was added.
    pub last_status_len: Option<usize>,
    /// Pending label changes from the tree selector (accumulated, flushed each frame).
    pub pending_label_changes: PendingLabelChanges,
    /// Stop-requested flag shared with agent's before_turn callback.
    pub stop_requested: Arc<AtomicBool>,
    /// Messages queued via /nextTurn (delivered at the start of the next agent run).
    pub next_turn_queue: Vec<yoagent::types::AgentMessage>,
    /// Messages saved from steer/follow-up queues when stop was requested.
    pub saved_queued_msgs: Vec<yoagent::types::AgentMessage>,
}

impl App {
    pub fn new(config: AppConfig, session: AgentSession) -> Self {
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

        // Build chat_container from AgentMessages directly.
        let cwd_string = config.cwd.to_string_lossy().to_string();

        // Collect context file paths for header resource display.
        let context_file_paths: Vec<String> = config
            .context_files
            .iter()
            .map(|s| {
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
        let theme_names: Vec<String> = crate::agent::ui::theme::get_available_themes()
            .into_iter()
            .filter(|n| n != "dark" && n != "light")
            .collect();

        let chat_container = Rc::new(RefCell::new(crate::tui::Container::new()));
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

            pending_section: Rc::new(RefCell::new(crate::tui::components::DynamicLines::new())),
            status_section: Rc::new(RefCell::new(crate::tui::components::DynamicLines::new())),
            working_section: Rc::new(RefCell::new(crate::tui::components::DynamicLines::new())),
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

    /// Refresh git branch for footer display.
    pub fn refresh_git_branch(&self) {
        self.footer_provider.borrow_mut().refresh_git_branch();
    }

    /// Clear all transient session state when switching to a new session.
    pub fn clear_session_state(&mut self) {
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
    pub fn rebuild_from_session_context(&mut self) {
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
    pub fn record_model_change(&mut self, model: &str) {
        if let Some(ref mut agent_session) = self.session {
            agent_session.on_model_change(&self.current_provider, model);
        }
        if let Some(ref session) = self.session {
            self.footer
                .borrow_mut()
                .refresh_from_session(session.session());
        }
    }

    /// Reload the provider registry from disk.
    pub fn refresh_registry(&mut self) {
        match provider::ProviderRegistry::load(&provider::get_agent_dir()) {
            Ok(new_reg) => self.registry = Arc::new(new_reg),
            Err(e) => {
                self.status_text = Some(format!("Failed to refresh registry: {}", e));
            }
        }
    }

    /// Propagate `hide_thinking` to all chat container children and the streaming component.
    pub fn propagate_hide_thinking(&mut self) {
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
    pub fn switch_to_session(&mut self, new_session: AgentSession) {
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

    // Set up the component tree in TUI.root
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(RcRefCellComponent(
        app.header.clone() as Rc<RefCell<dyn Component>>
    )));
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(RcRefCellComponent(
        app.chat_container.clone() as Rc<RefCell<dyn crate::tui::Component>>,
    )));
    tui.add_child(std::boxed::Box::new(RcRefCellComponent(
        app.pending_section.clone() as Rc<RefCell<dyn crate::tui::Component>>,
    )));
    tui.add_child(std::boxed::Box::new(RcRefCellComponent(
        app.status_section.clone() as Rc<RefCell<dyn crate::tui::Component>>,
    )));
    tui.add_child(std::boxed::Box::new(RcRefCellComponent(
        app.working_section.clone() as Rc<RefCell<dyn crate::tui::Component>>,
    )));
    tui.add_child(std::boxed::Box::new(Spacer::new(1)));
    tui.add_child(std::boxed::Box::new(EditorComponent(app.editor.clone())));
    tui.add_child(std::boxed::Box::new(FooterComponent(app.footer.clone())));

    // Initialize editor border color
    app.editor.borrow_mut().update_border_color(
        app.thinking_level.as_deref(),
        &app.theme as &dyn crate::tui::Theme,
    );

    let mut cols: u16 = 80;
    let mut rows: u16 = 24;
    let mut dirty = true;

    loop {
        // Drain agent events FIRST
        let mut had_event = false;
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
            had_event = true;
        }
        if had_event {
            dirty = true;
        }

        // Drain terminal events
        loop {
            match terminal::try_recv_terminal_event() {
                Some(terminal::TerminalEvent::Key(key)) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if !tui.route_input(&key) {
                        handle_input(&mut app, &mut tui, &mut term, &key);
                    }
                }
                Some(terminal::TerminalEvent::Paste(content)) => {
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

        // Check pending scoped model changes
        if let Some(ids) = app.pending_scoped_ids.borrow_mut().take() {
            let auth_count = app.registry.list_authenticated_model_ids().len();
            if ids.is_empty() || ids.len() >= auth_count {
                app.scoped_model_ids = None;
            } else {
                app.scoped_model_ids = Some(ids);
            }
            dirty = true;
        }

        // Check pending settings changes from SettingsSelector
        {
            let change = app.pending_settings_change.borrow_mut().take();
            if let Some((id, new_value)) = change {
                apply_settings_change(&mut app, &id, &new_value);
                dirty = true;
            }
        }

        // Flush pending label changes from tree selector to session
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
                    OverlayResult::ScopedModelsAccepted(ids) => match ids {
                        Some(ids)
                            if !ids.is_empty()
                                && ids.len()
                                    < app.registry.list_authenticated_model_ids().len() =>
                        {
                            app.scoped_model_ids = Some(ids.clone());
                            app.settings.set_enabled_models(Some(ids));
                            if let Err(e) = app.settings.save() {
                                app.status_text =
                                    Some(format!("Failed to save model scope: {}", e));
                            } else {
                                app.status_text = Some("Model scope saved to settings".into());
                            }
                        }
                        _ => {
                            app.scoped_model_ids = None;
                            app.settings.set_enabled_models(None);
                            if let Err(e) = app.settings.save() {
                                app.status_text =
                                    Some(format!("Failed to save model scope: {}", e));
                            } else if ids.is_some() {
                                app.status_text = Some("Model scope saved to settings".into());
                            }
                        }
                    },
                    OverlayResult::ScopedModelsCancelled => {}
                    OverlayResult::LoginAuthTypeSelected(auth_type) => {
                        show_login_provider_selector(&mut app, &mut tui, Some(auth_type));
                    }
                    OverlayResult::LoginProviderSelected(provider_id) => {
                        if crate::provider::oauth::get(&provider_id).is_some() {
                            show_oauth_login_dialog(&mut app, &mut tui, &provider_id);
                        } else {
                            show_api_key_login_dialog(&mut app, &mut tui, &provider_id);
                        }
                    }
                    OverlayResult::LoginApiKeyProvided { provider, key } => {
                        if let Some(err_msg) = key.strip_prefix("OAUTH_LOGIN_FAILED:") {
                            app.status_text = Some(format!("OAuth login failed: {}", err_msg));
                        } else {
                            match crate::provider::auth::login(&provider, &key) {
                                Ok(_) => {
                                    app.status_text = Some(format!("Logged in to {}", provider));
                                    app.refresh_registry();
                                    handle_login(&mut app, &provider, Some(&key));
                                }
                                Err(e) => {
                                    app.status_text = Some(format!("Login failed: {}", e));
                                }
                            }
                        }
                    }
                    OverlayResult::LogoutProviderSelected(provider_id) => {
                        match crate::provider::auth::logout(Some(&provider_id)) {
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
                            let session_dir = app
                                .session
                                .as_ref()
                                .map(|s| s.session_dir().to_path_buf())
                                .unwrap_or_else(|| {
                                    crate::agent::session::get_default_session_dir(&app.cwd)
                                });
                            std::fs::create_dir_all(&session_dir)
                                .map_err(|e| format!("Failed to create session dir: {}", e))?;
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
                    OverlayResult::ForkCancelled => {}
                    OverlayResult::Dismiss => {}
                    OverlayResult::TreeNavigateTo(entry_id) => {
                        let current_leaf =
                            app.session.as_ref().and_then(|s| s.session().get_leaf_id());
                        if current_leaf.as_deref() == Some(&entry_id) {
                            app.status_text = Some("Already at this point".to_string());
                        } else {
                            show_summarization_prompt(&mut app, &mut tui, &entry_id);
                        }
                    }
                    OverlayResult::TreeCancelled => {}
                    OverlayResult::TreeSummarizeChoice {
                        entry_id,
                        summarize,
                        custom_instructions,
                    } => {
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

        // Re-drain agent events that arrived during terminal event processing
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
            dirty = true;
        }

        // Recover Agent state BEFORE submitting any new prompt or running auto-compact
        if app.forward_handle.as_ref().is_some_and(|h| h.is_finished()) {
            app.forward_handle.take();
            if let Some(ref mut agent) = app.agent {
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

            let oauth_provider = app.pending_oauth_provider.take();
            if let Some(ref provider_id) = oauth_provider
                && let Ok(Some(crate::provider::auth::AuthCredential::Oauth { .. })) =
                    crate::provider::auth::read_credential(provider_id)
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
                let auth_type = crate::agent::ui::components::oauth_selector::AuthType::OAuth;
                complete_login(&mut app, provider_id, auth_type);
            } else if oauth_provider.is_some() {
                let err_msg = app.status_text.clone().unwrap_or_default();
                if !err_msg.is_empty() {
                    show_status(&mut app, &err_msg);
                }
            }
        }

        // Handle pending agent submission (async)
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

        // Pi-compatible: auto-compaction check after agent ends
        if app.pending_auto_compact {
            app.pending_auto_compact = false;
            handle_auto_compact(&mut app).await;
            dirty = true;
        }

        // Handle pending command results that need TUI access
        if let Some(result) = app.pending_command_result.take() {
            match result {
                CommandResult::ShowHelp => {
                    show_help_overlay(&mut app, &mut tui);
                }
                CommandResult::OpenSessionSelector => {
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
                        show_api_key_login_dialog(&mut app, &mut tui, provider);
                    } else {
                        show_auth_type_or_provider_selector(&mut app, &mut tui);
                    }
                }
                CommandResult::Logout { provider } => match provider {
                    Some(p) => {
                        crate::provider::auth::logout(Some(&p)).ok();
                    }
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
                                                yoagent::types::AgentMessage::Llm(llm_msg.clone()),
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
                                        total: 0,
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
        if dirty && let Ok((w, h)) = term.size() {
            app.editor.borrow_mut().editor.set_terminal_rows(h as usize);
            cols = w;
            rows = h;
        }

        // Tick the working indicator
        if app.working.tick() {
            dirty = true;
        }

        // Tick active tool timers (bash elapsed display)
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

        tokio::time::sleep(if dirty || app.is_streaming || app.working.active {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(50)
        })
        .await;

        app.status_text = None;

        if app.should_quit {
            if let Some(handle) = app.oauth_join_handle.take() {
                handle.abort();
            }
            break;
        }
    }

    tui.finalize(&mut stdout)?;
    term.set_color_scheme_notifications(&mut stdout, false)?;
    term.show_cursor(&mut stdout)?;
    term.stop(&mut stdout)?;

    Ok(())
}
