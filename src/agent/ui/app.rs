use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::extension::{AgentTool, CommandResult, Extension};
use crate::agent::provider::{Provider, ToolDef};
use crate::agent::session::SessionManager;
use crate::agent::types::{AgentMessage, PendingMessageQueue, QueueMode, ToolExecutionMode, Usage};
use crate::agent::ui::chat_editor::{ChatEditor, InputAction};
use crate::agent::ui::components::EditorComponent;
use crate::agent::ui::components::FooterComponent;
use crate::agent::ui::components::InfoMessageComponent;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::messages::{DisplayMsg, session_messages_to_display};
use crate::agent::ui::model_selector::ModelSelector;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
use crate::agent::{AgentEvent, LoopConfig, run_agent_loop};
use crate::builtin::commands::SessionInfoInternal;
use crate::tui::Component;
use crate::tui::TUI;
use crate::tui::focusable::Focusable;

use crate::agent::ui::theme::ThemeKey;
use crate::tui::components::Spacer;
use crate::tui::components::Text;
use crate::tui::terminal::{self, ProcessTerminal, TerminalTrait};
use crossterm::event::KeyEvent;
use tokio::sync::mpsc;

/// Thinking level cycle order (matching pi's thinking level enum).
/// Thinking level cycle order. Cycles from highest to lowest so the first
/// press from the default (xhigh) goes to "high" (a step down), not to "off".
const THINKING_LEVELS: &[&str] = &["xhigh", "high", "medium", "low", "off"];

/// Configuration for the UI app.
pub struct AppConfig {
    pub model: String,
    pub system_prompt: String,
    pub tools: Vec<ToolDef>,
    pub agent_tools: Vec<Box<dyn AgentTool>>,
    pub extensions: Vec<Box<dyn Extension>>,
    pub provider: Box<dyn Provider>,
    pub cwd: PathBuf,
    pub thinking_level: Option<String>,
    pub git_branch: Option<String>,
    pub available_models: Vec<String>,
    pub hide_thinking: bool,
    pub collapse_tool_output: bool,
    pub interactive: bool,
    pub settings: crate::agent::settings::Settings,
    /// Context files (AGENTS.md / CLAUDE.md) loaded for the session.
    pub context_files: Vec<String>,
    /// Tool execution mode (parallel by default).
    pub tool_execution: ToolExecutionMode,
    /// Skills loaded for the session (used for /skill:name expansion).
    pub skills: Vec<crate::agent::Skill>,
    /// Whether the current model supports reasoning (for showing thinking level in footer).
    pub model_supports_reasoning: bool,
    /// Session info Arc for /session command (shared with CommandsExtension).
    pub session_info: Option<std::sync::Arc<std::sync::Mutex<Option<SessionInfoInternal>>>>,
}

/// Main application state.
pub struct App {
    cwd: PathBuf,
    model: String,
    thinking_level: Option<String>,
    system_prompt: String,
    provider: Arc<dyn Provider>,
    theme: RabTheme,

    /// Slash commands from all extensions.
    commands: Vec<(String, String)>,

    /// Available models for the model selector.
    available_models: Vec<String>,

    /// Conversation history (AgentMessage).
    conversation: Vec<AgentMessage>,

    /// Rendered display messages (legacy - being migrated to Components).
    messages: Vec<DisplayMsg>,

    /// Component-based chat area - mirrors pi's `this.chatContainer`.
    /// Components are added here in handle_agent_event instead of pushing to messages.
    pub chat_container: std::rc::Rc<std::cell::RefCell<crate::tui::Container>>,

    // ── Section components for the UI layout (written by compose_ui) ──
    /// Pending streaming text section.
    pub pending_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,
    /// Status text section (transient, dim).
    pub status_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,
    /// Queued messages section.
    pub queued_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,
    /// Working indicator section.
    pub working_section: std::rc::Rc<std::cell::RefCell<crate::tui::components::DynamicLines>>,

    /// The chat editor (shared ownership - App mutates, TUI.root renders).
    editor: Rc<RefCell<ChatEditor>>,

    /// Agent event channel.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<AgentEvent>,

    /// Streaming state.
    is_streaming: bool,
    pending_text: Option<String>,
    pending_thinking: Option<String>,

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

    /// Timestamp of last agent event received (used for streaming safety timeout).
    last_streaming_event: std::time::Instant,

    /// Token usage from last response.
    last_usage: Option<Usage>,

    /// Agent cancellation sender for Ctrl+C / ESC.
    cancel_tx: Option<tokio::sync::watch::Sender<bool>>,

    /// Bash abort handle for bang (!) commands.
    bash_abort_handle: Option<tokio::task::AbortHandle>,

    /// Session persistence.
    session: Option<SessionManager>,

    /// Footer (shared ownership - App mutates, TUI.root renders).
    footer: Rc<RefCell<Footer>>,

    /// Pending tool executions keyed by tool call ID.
    /// Used to update ToolExecComponent when ToolResult arrives (pi's `pendingTools` Map).
    pending_tools: HashMap<String, Weak<RefCell<crate::agent::ui::components::ToolExecComponent>>>,

    /// Start times for pending tool calls, keyed by tool call ID.
    /// Used to compute duration for bash and other tools.
    tool_call_start_times: HashMap<String, std::time::Instant>,

    /// Streaming assistant message component (pi's `streamingComponent`).
    /// Created on first TextDelta, updated in-place, cleared on TurnEnd/AgentEnd.
    streaming_component:
        Option<Weak<RefCell<crate::agent::ui::components::AssistantMessageComponent>>>,

    /// Active bash execution component (for bang commands - updated when result arrives).
    bash_component: Option<Weak<RefCell<crate::agent::ui::components::BashExecution>>>,

    /// Working indicator.
    working: WorkingIndicator,

    /// Transient status text (pi-style: replaces previous status, not added to chat).
    status_text: Option<String>,

    /// Pending command result that needs TUI access (overlays etc.).
    /// Set by handle_slash_command, consumed in the main loop where TUI is available.
    pending_command_result: Option<CommandResult>,

    /// Agent tools (for tool execution).
    agent_tools: Arc<Vec<Box<dyn AgentTool>>>,
    /// Extensions.
    extensions: Arc<Vec<Box<dyn Extension>>>,

    /// Steering queue: messages delivered after current turn's tool calls finish,
    /// before the next LLM call. Shared with the agent loop.
    steering_queue: Arc<std::sync::Mutex<PendingMessageQueue>>,
    /// Follow-up queue: messages delivered only after the agent has no more
    /// tool calls (fully idle). Shared with the agent loop.
    follow_up_queue: Arc<std::sync::Mutex<PendingMessageQueue>>,
    /// Tool execution mode (parallel by default).
    tool_execution: ToolExecutionMode,

    /// Skills loaded for the session (/skill:name expansion).
    skills: Vec<crate::agent::Skill>,
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
    // ── Message rendering cache (avoids re-rendering messages every frame) ──
    // Cache fields removed - messages now rendered via Components in chat_container.
}

impl App {
    fn new(config: AppConfig, session: SessionManager) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        use crate::agent::ui::theme::current_theme;
        let theme = current_theme().clone();

        let mut editor = ChatEditor::new(&theme, config.cwd.clone());

        // Collect slash commands with argument completion callbacks
        use crate::tui::autocomplete::AutocompleteItem as AutoAutocompleteItem;
        use crate::tui::autocomplete::SlashCommand as AutoSlashCommand;
        let auto_commands: Vec<AutoSlashCommand> = config
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
        editor.set_slash_commands(auto_commands);

        // Keep commands list for help overlay and unknown-command display.
        let commands: Vec<(String, String)> = config
            .extensions
            .iter()
            .flat_map(|e| e.commands())
            .map(|c| (c.name, c.description))
            .collect();

        let editor = Rc::new(RefCell::new(editor));

        let mut footer = Footer::new(config.cwd.to_string_lossy().to_string());
        footer.set_git_branch(config.git_branch.clone());
        footer.set_model(&config.model);
        footer.set_model_supports_reasoning(config.model_supports_reasoning);
        footer.set_thinking_level(config.thinking_level.clone());

        let footer = Rc::new(RefCell::new(footer));

        // Load session messages
        let context = session.build_session_context();
        let history_messages = context.messages.clone();
        let history_display = session_messages_to_display(&history_messages);

        // Startup info: context files, skills, tools (pi-style loaded resources listing)
        let mut startup_info: Vec<DisplayMsg> = Vec::new();

        let mut resource_parts: Vec<String> = Vec::new();

        if !config.context_files.is_empty() {
            let ctx = config.context_files.join(", ");
            resource_parts.push(format!("Context: {}", ctx));
        }

        if !config.skills.is_empty() {
            let skill_names: Vec<&str> = config.skills.iter().map(|s| s.name.as_str()).collect();
            resource_parts.push(format!("Skills: {}", skill_names.join(", ")));
        }

        if !resource_parts.is_empty() {
            startup_info.push(DisplayMsg::Info(resource_parts.join("  ·  ")));
        }

        // Combine startup info with history (legacy - for session saving / tests)
        let messages = if startup_info.is_empty() {
            history_display
        } else {
            let mut combined = startup_info;
            combined.push(DisplayMsg::Separator);
            combined.extend(history_display);
            combined
        };

        // Populate chat_container with initial messages (startup info + history).
        // Add spacers between messages matching pi's addMessageToChat behavior.
        let chat_container =
            std::rc::Rc::new(std::cell::RefCell::new(crate::tui::Container::new()));
        {
            let mut chat = chat_container.borrow_mut();
            for display_msg in &messages {
                if let Some(component) =
                    crate::agent::ui::components::display_msg_to_component(display_msg)
                {
                    if !chat.children().is_empty() {
                        chat.add_child(std::boxed::Box::new(crate::tui::components::Spacer::new(
                            1,
                        )));
                    }
                    chat.add_child(component);
                }
            }
        }

        let result = Self {
            cwd: config.cwd,
            model: config.model,
            thinking_level: config.thinking_level,
            system_prompt: config.system_prompt,
            provider: Arc::from(config.provider),
            theme,
            commands,
            available_models: config.available_models,
            conversation: history_messages,
            messages,
            chat_container,
            pending_tools: HashMap::new(),
            tool_call_start_times: HashMap::new(),
            streaming_component: None,
            bash_component: None,
            pending_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            status_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            queued_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            working_section: std::rc::Rc::new(std::cell::RefCell::new(
                crate::tui::components::DynamicLines::new(),
            )),
            editor,
            event_tx: tx,
            event_rx: rx,
            is_streaming: false,
            pending_text: None,
            pending_command_result: None,
            pending_thinking: None,
            hide_thinking: config.hide_thinking,
            collapse_tool_output: config.collapse_tool_output,
            tools_expanded: !config.collapse_tool_output,
            scroll_offset: 0,
            last_clear_time: std::time::Instant::now(),

            should_quit: false,
            last_streaming_event: std::time::Instant::now(),
            last_usage: None,
            cancel_tx: None,
            bash_abort_handle: None,
            session: Some(session),
            footer,
            working: WorkingIndicator::new(),
            agent_tools: Arc::new(config.agent_tools),
            extensions: Arc::new(config.extensions),
            steering_queue: Arc::new(std::sync::Mutex::new(PendingMessageQueue::new(
                QueueMode::OneAtATime,
            ))),
            follow_up_queue: Arc::new(std::sync::Mutex::new(PendingMessageQueue::new(
                QueueMode::OneAtATime,
            ))),
            tool_execution: config.tool_execution,
            skills: config.skills,
            session_info: config.session_info,
            settings: config.settings,
            auto_compact: true,
            status_text: None,
            header: Rc::new(RefCell::new(
                crate::agent::ui::components::HeaderComponent::new(),
            )),
        };

        // Initial session info for /session command
        result.update_session_info();

        result
    }

    /// Update the session info shared with CommandsExtension for /session display.
    fn update_session_info(&self) {
        if let Some(ref session) = self.session
            && let Some(ref info) = self.session_info
        {
            let si = crate::builtin::commands::compute_session_info(session);
            if let Ok(mut guard) = info.lock() {
                *guard = Some(si);
            }
        }
    }

    /// Refresh git branch for footer display.
    /// Called on AgentStart to match pi's FooterDataProvider.onBranchChange.
    fn refresh_git_branch(&self) {
        if let Ok(output) = std::process::Command::new("git")
            .args([
                "-C",
                &self.cwd.to_string_lossy(),
                "rev-parse",
                "--abbrev-ref",
                "HEAD",
            ])
            .output()
            && output.status.success()
        {
            let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !branch.is_empty() {
                self.footer.borrow_mut().set_git_branch(Some(branch));
            }
        }
    }
}

/// Run the interactive UI.
pub async fn run(config: AppConfig, session: SessionManager) -> anyhow::Result<()> {
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
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(
            app.header.clone() as Rc<RefCell<dyn Component>>,
        ),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.chat_container.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.pending_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.status_section.clone()
            as std::rc::Rc<std::cell::RefCell<dyn crate::tui::Component>>),
    ));
    tui.root.add_child(std::boxed::Box::new(
        crate::tui::components::RcRefCellComponent(app.queued_section.clone()
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
        // Poll for events (pi-style: process input before rendering)
        // Reduced poll frequency: 16ms active (~60fps), 50ms idle - terminal UI
        // doesn't benefit from >60fps and lower frequency saves CPU/battery.
        let timeout = if dirty || app.is_streaming || app.working.should_show() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(50)
        };

        if let Some(evt) = terminal::poll_terminal_event(Some(timeout))? {
            match evt {
                terminal::TerminalEvent::Key(key) => {
                    // TUI overlay routing first (overlays get first crack at input)
                    if !tui.route_input(&key) {
                        handle_input(&mut app, &mut tui, &key);
                    }
                }
                terminal::TerminalEvent::Paste(content) => {
                    // Route to focused overlay first (e.g. Input in settings),
                    // fall back to the main Editor.
                    if !tui.route_paste(&content) {
                        app.editor.borrow_mut().editor.handle_paste(&content);
                    }
                }
                terminal::TerminalEvent::Resize(w, h) => {
                    // Update editor's terminal height for dynamic max-visible-lines
                    app.editor.borrow_mut().editor.set_terminal_rows(h as usize);
                    tui.set_dimensions(w as usize, h as usize);
                }
            }
            dirty = true;
        }

        // Drain agent events (batch: process all pending events before rendering)
        let mut had_event = false;
        while let Ok(event) = app.event_rx.try_recv() {
            handle_agent_event(&mut app, event);
            had_event = true;
        }
        if had_event {
            dirty = true;
        }

        // Handle pending command results that need TUI access (overlays, etc.)
        if let Some(result) = app.pending_command_result.take() {
            match result {
                CommandResult::ShowHelp => {
                    show_help_overlay(&mut app, &mut tui);
                }
                CommandResult::OpenSessionSelector => {
                    // Session selector not yet implemented
                    app.messages.push(DisplayMsg::Info(
                        "Session selector - not yet implemented.".to_string(),
                    ));
                }
                CommandResult::OpenSettings => {
                    app.messages.push(DisplayMsg::Info(
                        "Settings menu - not yet implemented.".to_string(),
                    ));
                }
                CommandResult::ScopedModels => {
                    app.messages.push(DisplayMsg::Info(
                        "Scoped models - not yet implemented.".to_string(),
                    ));
                }
                CommandResult::Login { .. } => {
                    app.messages.push(DisplayMsg::Info(
                        "Login dialog - not yet implemented.".to_string(),
                    ));
                }
                _ => {}
            }
            dirty = true;
        }

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

        // Safety timeout: force stop streaming if no agent events for 60s.
        // Prevents the event loop from spinning at 16ms with `is_streaming`
        // stuck true after the agent task completes or panics without
        // delivering AgentEnd through the channel.
        if app.is_streaming
            && app.last_streaming_event.elapsed() > std::time::Duration::from_secs(60)
        {
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            app.status_text = Some("Streaming timed out - agent may have crashed".into());
            dirty = true;
        }

        // Pi: clear transient status after rendering
        app.status_text = None;

        if app.should_quit {
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
    // ── Pending (streaming) text ──
    let mut pending_lines = Vec::new();
    if let Some(ref text) = app.pending_text
        && !text.is_empty()
    {
        let inner = width.saturating_sub(2);
        for line in text.lines() {
            if line.is_empty() {
                pending_lines.push(String::new());
            } else {
                let wrapped = crate::tui::util::wrap_text_with_ansi(line, inner);
                for w in wrapped {
                    let line = format!(" {}", w);
                    pending_lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
                }
            }
        }
    }
    if let Some(ref text) = app.pending_thinking
        && !text.is_empty()
    {
        if app.hide_thinking {
            let content = format!(
                " {} ",
                app.theme
                    .italic(&app.theme.fg("thinking_text", "Thinking..."))
            );
            let padded = crate::agent::ui::messages::pad_to_width(&content, width);
            pending_lines.push(app.theme.bg("thinking_bg", &padded));
        } else {
            let level_color = app
                .thinking_level
                .as_deref()
                .and_then(crate::agent::ui::messages::thinking_level_color)
                .unwrap_or("thinking_text");
            for line in text.lines() {
                let content = format!(" {}", app.theme.italic(&app.theme.fg(level_color, line)));
                let padded = crate::agent::ui::messages::pad_to_width(&content, width);
                pending_lines.push(app.theme.bg("thinking_bg", &padded));
            }
        }
    }
    app.pending_section.borrow_mut().set_lines(pending_lines);

    // ── Transient status text (pi-style: replaces previous status, not added to chat) ──
    let mut status_lines = Vec::new();
    if let Some(ref status) = app.status_text {
        let line = app.theme.fg_key(ThemeKey::Dim, &format!(" {}", status));
        status_lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
    }
    app.status_section.borrow_mut().set_lines(status_lines);

    // ── Queued messages (pi-style: shown between chat and editor) ──
    // Shows both steering and follow-up queue contents.
    let mut queued_lines = Vec::new();
    let steer_count = {
        let q = app.steering_queue.lock().unwrap();
        q.len()
    };
    let follow_count = {
        let q = app.follow_up_queue.lock().unwrap();
        q.len()
    };
    if steer_count > 0 {
        let line = app.theme.fg(
            "dim",
            &format!(
                " ◷ {} steer message{} pending",
                steer_count,
                if steer_count == 1 { "" } else { "s" }
            ),
        );
        queued_lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
    }
    if follow_count > 0 {
        let line = app.theme.fg(
            "dim",
            &format!(
                " ◷ {} follow-up message{} pending",
                follow_count,
                if follow_count == 1 { "" } else { "s" }
            ),
        );
        queued_lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
    }
    if steer_count > 0 || follow_count > 0 {
        let hint = app.theme.fg_key(
            ThemeKey::Dim,
            " ↳ Esc to abort, Alt+↑ to restore follow-ups",
        );
        queued_lines.push(crate::agent::ui::messages::pad_to_width(&hint, width));
    }
    app.queued_section.borrow_mut().set_lines(queued_lines);

    // ── Working indicator (pi-style: blank line + spinner before editor) ──
    let mut working_lines = Vec::new();
    let wl = app.working.render(width);
    working_lines.extend(wl);
    app.working_section.borrow_mut().set_lines(working_lines);
}

/// Handle keyboard input. Mirrors pi's InteractiveMode key dispatch:
///
/// 1. Overlays handled via TUI.route_input - checked first in event loop
/// 2. ChatEditor::handle_input checks app-level keys and returns InputAction
/// 3. App.rs matches on InputAction to perform side effects
///
/// This keeps text-editing logic in the Editor component (via ChatEditor)
/// and app-level side effects (aborting agents, toggling settings, etc.) here.
fn handle_input(app: &mut App, tui: &mut TUI, key: &KeyEvent) {
    // ── Check if any TUI overlay is active (help, model selector, etc.) ──
    if tui.has_overlays() {
        tui.pop_overlay();
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
            {
                let mut chat = app.chat_container.borrow_mut();
                for child in chat.children_mut().iter_mut() {
                    child.set_hide_thinking(app.hide_thinking);
                }
            }
            // Update streaming component if it exists
            if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                weak.borrow_mut().set_hide_thinking(app.hide_thinking);
            }
            // Persist only the affected field (incremental save)
            app.settings.set_hide_thinking(Some(app.hide_thinking));
            if let Err(e) = app.settings.save() {
                app.status_text = Some(format!("Failed to save thinking visibility: {}", e));
            }
            chat_add(
                app,
                Box::new(InfoMessageComponent::new(if app.hide_thinking {
                    "Thinking blocks: hidden".to_string()
                } else {
                    "Thinking blocks: visible".to_string()
                })),
            );
        }
        InputAction::ToolsExpand => {
            handle_tools_expand(app);
        }
        InputAction::EditorExternal => {
            handle_editor_external(app);
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
            handle_dequeue(app);
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

/// Cycle thinking level: off → low → medium → high → xhigh → off
fn handle_thinking_cycle(app: &mut App) {
    if app.available_models.is_empty() && app.model.is_empty() {
        app.status_text = Some("No model selected".into());
        return;
    }

    let current = app.thinking_level.as_deref().unwrap_or("off");
    let next = match THINKING_LEVELS.iter().position(|&l| l == current) {
        Some(pos) => THINKING_LEVELS[(pos + 1) % THINKING_LEVELS.len()],
        None => "off",
    };

    app.thinking_level = Some(next.to_string());
    app.footer
        .borrow_mut()
        .set_thinking_level(Some(next.to_string()));
    app.editor
        .borrow_mut()
        .update_border_color(Some(next), &app.theme as &dyn crate::tui::Theme);
    app.settings
        .set_default_thinking_level(Some(next.to_string()));
    if let Err(e) = app.settings.save() {
        app.status_text = Some(format!("Failed to save thinking level: {}", e));
    }
    // Update provider's reasoning effort so API calls use the new level
    // (even if save failed, the current session should use the new level)
    app.provider.set_reasoning_effort(Some(next));
    app.status_text = Some(format!("Thinking level: {}", next));
}

/// Cycle model forward (dir=1) or backward (dir=-1).
fn handle_model_cycle(app: &mut App, dir: isize) {
    let n = app.available_models.len();
    if n == 0 {
        app.status_text = Some("No models available".into());
        return;
    }

    let current_idx = app.available_models.iter().position(|m| m == &app.model);

    let next_idx = match current_idx {
        Some(idx) => (idx as isize + dir).rem_euclid(n as isize) as usize,
        None => 0,
    };

    app.model = app.available_models[next_idx].clone();
    app.footer.borrow_mut().set_model(&app.model);
    // All rab models support reasoning (deepseek-v4-flash, deepseek-v4-pro).
    app.footer.borrow_mut().set_model_supports_reasoning(true);
    app.status_text = Some(format!("Model: {}", app.model));
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
    chat_add(
        app,
        Box::new(InfoMessageComponent::new(if app.tools_expanded {
            "Tool output: expanded".to_string()
        } else {
            "Tool output: collapsed".to_string()
        })),
    );
}

/// Open external editor ($VISUAL / $EDITOR) for current editor content.
fn handle_editor_external(app: &mut App) {
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

    // Fork and exec the editor
    let parts: Vec<&str> = editor_cmd.split(' ').collect();
    let (editor, args) = parts.split_first().unwrap_or((&"", &[]));

    // Stop TUI, run editor, resume
    // For simplicity, we use std::process::Command which blocks
    app.status_text = Some(format!("Opening {} ...", editor_cmd));

    // Use std::process since we need to block the async runtime
    let status = std::process::Command::new(editor)
        .args(args)
        .arg(&tmp_file)
        .status();

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

/// Queue a follow-up message (Alt+Enter). Pushes to follow-up queue during streaming
/// (delivered only after agent has no more tool calls). When idle, submits immediately.
fn handle_follow_up(app: &mut App, text: String) {
    if app.is_streaming {
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user(text));
        app.status_text = Some("Message queued - will send when agent finishes".into());
    } else {
        // Not streaming - submit immediately
        submit_message(app, text);
    }
}

/// Restore queued messages to editor (Alt+Up).
/// Restores from the follow-up queue (steering messages are consumed during streaming).
fn handle_dequeue(app: &mut App) {
    let mut queue = app.follow_up_queue.lock().unwrap();
    if queue.is_empty() {
        app.status_text = Some("No queued messages to restore".into());
        return;
    }

    let count = queue.len();
    let all = queue.drain_all();
    let restored: Vec<String> = all.iter().map(|m| m.content.clone()).collect();
    let text = restored.join("\n\n");
    app.editor.borrow_mut().editor.set_text(&text);
    app.editor.borrow_mut().check_autocomplete();
    app.status_text = Some(format!(
        "Restored {} queued message{}",
        count,
        if count == 1 { "" } else { "s" }
    ));
}

/// Toggle auto-compact indicator (Ctrl+Shift+C).
fn handle_compact_toggle(app: &mut App) {
    app.auto_compact = !app.auto_compact;
    app.footer.borrow_mut().set_auto_compact(app.auto_compact);
    app.status_text = Some(if app.auto_compact {
        "Auto-compact: on".into()
    } else {
        "Auto-compact: off".into()
    });
}

/// Interrupt streaming agent and restore queued messages to editor.
fn interrupt_streaming(app: &mut App) {
    if let Some(tx) = app.cancel_tx.take() {
        let _ = tx.send(true);
    }
    if let Some(handle) = app.bash_abort_handle.take() {
        handle.abort();
    }
    app.is_streaming = false;
    app.working.stop();
    app.footer.borrow_mut().set_streaming(false);

    // Restore follow-up queue messages to editor (steering are mid-stream, not restorable).
    // Use try_lock to avoid deadlock if the agent loop holds the mutex when abort is called.
    if let Ok(mut follow_up) = app.follow_up_queue.try_lock() {
        if !follow_up.is_empty() {
            let all = follow_up.drain_all();
            let text: Vec<String> = all.iter().map(|m| m.content.clone()).collect();
            app.editor.borrow_mut().editor.set_text(&text.join("\n\n"));
            app.queued_section.borrow_mut().set_lines(vec![]);
        }
        drop(follow_up);
    }

    if let Ok(mut steering) = app.steering_queue.try_lock() {
        steering.clear();
    }

    app.status_text = Some("Interrupted".into());
}

/// Open the model selector overlay.
fn open_model_selector(app: &mut App, tui: &mut TUI) {
    let models = app.available_models.clone();
    let current = app.model.clone();
    let selector = ModelSelector::new(models, &current, &app.theme);
    tui.show_overlay(Box::new(selector), Default::default());
}

fn show_help_overlay(app: &mut App, tui: &mut TUI) {
    let mut overlay = crate::agent::ui::help::HelpOverlay::new(&app.theme);
    overlay.set_commands(app.commands.clone());
    tui.show_overlay(Box::new(overlay), Default::default());
}

/// Submit or queue a user message. When streaming, pushes to the steering queue
/// (delivered after current turn's tool calls finish, before next LLM call).
/// When idle, starts a new agent loop immediately.
fn submit_message(app: &mut App, message: String) {
    app.scroll_offset = 0;
    let trimmed = message.trim().to_string();

    // Don't submit empty messages (pi-style)
    if trimmed.is_empty() {
        return;
    }

    // Handle /skill:name [args] expansion (pi-style: before command dispatch)
    if trimmed.starts_with("/skill:") {
        let expanded = crate::agent::skills::expand_skill_command(&trimmed, &app.skills);
        chat_add(
            app,
            std::boxed::Box::new(crate::agent::ui::components::UserMessageComponent::new(
                &expanded,
            )),
        );
        app.messages.push(DisplayMsg::User(expanded.clone()));
        if app.is_streaming {
            app.steering_queue
                .lock()
                .unwrap()
                .enqueue(AgentMessage::user(expanded));
            return;
        }
        start_agent_loop(app, expanded);
        return;
    }

    // Handle /commands (need TUI from app for overlays)
    if trimmed.starts_with('/') {
        // If TUI was stored on App, we'd use it here. For now, just handle basic commands.
        handle_slash_command(app, &trimmed);
        return;
    }

    // Handle ! and !! bang commands
    if let Some((cmd, _exclude)) = parse_bang_command(&trimmed) {
        handle_bang_command(app, cmd);
        return;
    }

    // Normal message submission to LLM
    // Add Component to chat_container with spacer
    chat_add(
        app,
        std::boxed::Box::new(crate::agent::ui::components::UserMessageComponent::new(
            &trimmed,
        )),
    );
    app.messages.push(DisplayMsg::User(trimmed.clone()));

    if app.is_streaming {
        // Steering: delivered after current turn's tool calls finish, before next LLM call
        app.steering_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user(trimmed));
        return;
    }

    start_agent_loop(app, trimmed);
}

/// Actually start an agent loop (not queued).
fn start_agent_loop(app: &mut App, message: String) {
    let provider = Arc::clone(&app.provider);
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tools = collect_tool_defs(app);
    let tx = app.event_tx.clone();
    let history = app.conversation.clone();
    let agent_tools = Arc::clone(&app.agent_tools);
    let extensions = Arc::clone(&app.extensions);
    let tool_execution = app.tool_execution;
    let steering_queue = Arc::clone(&app.steering_queue);
    let follow_up_queue = Arc::clone(&app.follow_up_queue);

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);
    app.pending_text = None;
    app.pending_thinking = None;

    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    app.cancel_tx = Some(cancel_tx);

    tokio::spawn(async move {
        let config = LoopConfig {
            model: model.clone(),
            system_prompt,
            tools,
            agent_tools: &agent_tools,
            extensions: &extensions,
            tool_execution,
            steering_queue: Some(&*steering_queue),
            follow_up_queue: Some(&*follow_up_queue),
            transform_context: None,
            prepare_next_turn: None,
            should_stop_after_turn: None,
        };

        let mut emit = |event: AgentEvent| {
            let _ = tx.send(event);
        };

        let prompt = AgentMessage::user(message);

        tokio::select! {
            result = run_agent_loop(vec![prompt], history, &config, &*provider, &mut emit) => {
                match result {
                    Ok(_) => {
                        // AgentEnd already sent by run_agent_loop on success
                    }
                    Err(e) => {
                        // Pi-style: create an error assistant message so the failure is always visible
                        let error_text = format!("Error: {:#}", e);
                        let _ = tx.send(AgentEvent::TextDelta {
                            delta: error_text.clone(),
                        });
                        let error_assistant = AgentMessage {
                            id: uuid::Uuid::new_v4().to_string(),
                            parent_id: None,
                            role: crate::agent::types::Role::Assistant,
                            content: error_text,
                            tool_calls: vec![],
                            tool_call_id: None,
                            usage: None,
                            is_error: true,
                            timestamp: chrono::Utc::now().timestamp_millis(),
                        };
                        let _ = tx.send(AgentEvent::AgentEnd {
                            messages: vec![error_assistant],
                        });
                    }
                }
            }
            _ = cancel_rx.changed() => {
                // Cancelled by user (ESC/Ctrl+C). Send a clean AgentEnd without error text.
                let _ = tx.send(AgentEvent::AgentEnd {
                    messages: Vec::new(),
                });
            }
        }
    });
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
                        app.messages.push(DisplayMsg::Info(format!(
                            "Error executing /{}: {}",
                            cmd_name, e
                        )));
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
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::Quit => {
            app.should_quit = true;
        }
        CommandResult::ModelChanged(model) => {
            app.model = model.clone();
            app.footer.borrow_mut().set_model(&model);
            app.status_text = Some(format!("Model: {}", model));
        }
        CommandResult::ShowHelp => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::Reloaded => {
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(
                    "Settings, extensions, and keybindings reloaded.".to_string(),
                )),
            );
            app.messages.push(DisplayMsg::Info(
                "Settings, extensions, and keybindings reloaded.".to_string(),
            ));
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

            // Create a new session in the SessionManager (new ID, new file)
            if let Some(ref mut session) = app.session {
                session.new_session(None);
            }

            // Clear everything (matching pi's renderCurrentSessionState)
            app.conversation.clear();
            app.messages.clear();
            app.chat_container.borrow_mut().clear();
            app.streaming_component = None;
            app.pending_text = None;
            app.pending_thinking = None;
            app.pending_tools.clear();
            app.tool_call_start_times.clear();

            // Add "✓ New session started" with accent color, matching pi's
            // `new Text(theme.fg("accent", "✓ New session started"), 1, 1)`
            let styled = app.theme.fg("accent", "✓ New session started");
            chat_add(app, std::boxed::Box::new(Text::new(styled, 1, 1, None)));
        }
        CommandResult::SessionSwitched { path } => {
            app.status_text = Some(format!("Switched to session: {}", path.display()));
        }
        CommandResult::SessionInfo {
            session_id,
            file_path,
            name,
            message_count: _,
            user_messages: _,
            assistant_messages: _,
            tool_calls: _,
            tool_results: _,
            total_tokens: _,
            input_tokens: _,
            output_tokens: _,
            cache_read_tokens: _,
            cache_write_tokens: _,
            cost: _,
        } => {
            // Compute live stats from app.conversation (always fresh)
            let name_display = name
                .as_deref()
                .or_else(|| app.session.as_ref().and_then(|s| s.session_name()))
                .unwrap_or("unnamed");
            let file_display = file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "in-memory".to_string());
            let sid = if session_id.is_empty() {
                app.session
                    .as_ref()
                    .map(|s| s.session_id().to_string())
                    .unwrap_or_default()
            } else {
                session_id
            };

            let user_messages = app
                .conversation
                .iter()
                .filter(|m| m.role == crate::agent::types::Role::User)
                .count();
            let assistant_messages = app
                .conversation
                .iter()
                .filter(|m| m.role == crate::agent::types::Role::Assistant)
                .count();
            let tool_results = app
                .conversation
                .iter()
                .filter(|m| m.role == crate::agent::types::Role::ToolResult)
                .count();
            let tool_calls: usize = app
                .conversation
                .iter()
                .flat_map(|m| m.tool_calls.iter())
                .count();
            let total_messages = user_messages + assistant_messages + tool_results;

            let mut input_tokens: u64 = 0;
            let mut output_tokens: u64 = 0;
            let mut cache_read_tokens: u64 = 0;
            let cost: f64 = 0.0;
            for msg in &app.conversation {
                if let Some(ref usage) = msg.usage {
                    input_tokens += usage.input_tokens.unwrap_or(0) as u64;
                    output_tokens += usage.output_tokens.unwrap_or(0) as u64;
                    cache_read_tokens += usage.cache_tokens.unwrap_or(0) as u64;
                }
            }
            let total_tokens = input_tokens + output_tokens + cache_read_tokens;

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
                 Output: {}\n\
                 Total: {}",
                format_number(input_tokens),
                format_number(output_tokens),
                format_number(total_tokens),
            );
            if cache_read_tokens > 0 {
                info += &format!("\nCache Read: {}", format_number(cache_read_tokens));
            }
            if cost > 0.0 {
                info += &format!("\n\nCost\nTotal: {:.4}", cost);
            }

            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(info.clone())),
            );
            app.messages.push(DisplayMsg::Info(info));
        }
        CommandResult::OpenSessionSelector => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::SessionNamed { name } => {
            app.status_text = Some(format!("Session name: {}", name));

            // Update session info for /session command
            app.update_session_info();
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
            let msg = if let Some(p) = path {
                format!("Export session to {} - not yet implemented.", p)
            } else {
                "Export session - not yet implemented (defaults to HTML).".to_string()
            };
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::ImportSession { path } => {
            let msg = format!("Import session from {} - not yet implemented.", path);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::ShareSession => {
            let msg = "Share session - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::CopyLastMessage => {
            let msg = "Copy last agent message to clipboard - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::ShowChangelog => {
            let msg = "Changelog - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::ForkSession { message_id } => {
            let msg = if let Some(id) = message_id {
                format!("Fork session at message {} - not yet implemented.", id)
            } else {
                "Fork session - not yet implemented.".to_string()
            };
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::CloneSession => {
            let msg = "Clone session - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::SessionTree => {
            let msg = "Session tree - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::TrustDecision { decision } => {
            let msg = format!("Trust decision '{}' saved.", decision);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::Login { provider: _ } => {
            // Needs TUI overlay - defer
            app.pending_command_result = Some(result);
        }
        CommandResult::Logout { provider } => {
            let prov = provider.as_deref().unwrap_or("all providers");
            let msg = format!("Logged out from {} - not yet implemented.", prov);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
        CommandResult::CompactSession => {
            let msg = "Compact session - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
            app.messages.push(DisplayMsg::Info(msg));
        }
    }
}

/// Handle ! and !! bang commands.
/// Renders via BashExecutionComponent (borders, spinner, expand/collapse)
/// matching pi's handleBashCommand.
fn handle_bang_command(app: &mut App, command: String) {
    let cwd = app.cwd.clone();
    let tx = app.event_tx.clone();

    // Add BashExecutionComponent to chat_container (track for result updates)
    let bash_comp = Rc::new(RefCell::new(
        crate::agent::ui::components::BashExecution::new(&command),
    ));
    bash_comp
        .borrow_mut()
        .set_started_at(std::time::Instant::now());
    app.bash_component = Some(Rc::downgrade(&bash_comp));
    chat_add(
        app,
        std::boxed::Box::new(crate::tui::components::RcRefCellComponent(bash_comp)),
    );
    app.messages
        .push(DisplayMsg::User(format!("! {}", command)));

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);

    let handle = tokio::spawn(async move {
        struct Guard<'a> {
            tx: &'a mpsc::UnboundedSender<AgentEvent>,
            sent: bool,
        }
        impl Drop for Guard<'_> {
            fn drop(&mut self) {
                if !self.sent {
                    let _ = self.tx.send(AgentEvent::AgentEnd { messages: vec![] });
                }
            }
        }
        let mut guard = Guard {
            tx: &tx,
            sent: false,
        };

        let started = std::time::Instant::now();
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
                let _ = tx.send(AgentEvent::ToolProgress {
                    content: format!("Failed to spawn: {}", e),
                    is_error: true,
                });
                let _ = tx.send(AgentEvent::ToolResult {
                    id: String::new(),
                    name: "bash".into(),
                    content: format!("Failed to execute: {:#}", e),
                    compact: None,
                    is_error: true,
                });
                guard.sent = true;
                let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
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
                                let _ = tx.send(AgentEvent::ToolProgress {
                                    content: text.to_string(),
                                    is_error: false,
                                });
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
                                let _ = tx.send(AgentEvent::ToolProgress {
                                    content: text.to_string(),
                                    is_error: false,
                                });
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
        let elapsed = started.elapsed();
        let is_error = match &status {
            Ok(s) => !s.success(),
            Err(_) => true,
        };
        let result = if all_output.trim().is_empty() {
            "(no output)".to_string()
        } else {
            all_output.trim().to_string()
        };

        let _ = tx.send(AgentEvent::ToolResult {
            id: String::new(),
            name: "bash".into(),
            content: format!(
                "$ {}\n\n{}\n\n[{}s]",
                command,
                result,
                elapsed.as_secs_f64()
            ),
            compact: None,
            is_error,
        });
        guard.sent = true;
        let _ = tx.send(AgentEvent::AgentEnd { messages: vec![] });
    });
    app.bash_abort_handle = Some(handle.abort_handle());
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

/// Handle agent events from the channel.
fn handle_agent_event(app: &mut App, event: AgentEvent) {
    match event {
        AgentEvent::AgentStart => {
            app.is_streaming = true;
            app.working.start();
            app.pending_text = None;
            app.pending_thinking = None;
            app.last_streaming_event = std::time::Instant::now();

            // Refresh git branch on each agent start (matches pi's FooterDataProvider)
            app.refresh_git_branch();
        }
        AgentEvent::TurnStart => {}
        AgentEvent::TextDelta { delta } => {
            app.last_streaming_event = std::time::Instant::now();
            // Progressive streaming: create or update AssistantMessageComponent in-place
            if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                weak.borrow_mut().append_text(&delta);
            } else {
                // First text delta - create streaming component
                // Direct addChild matching pi's `this.chatContainer.addChild(this.streamingComponent)`
                // without a spacer - the component handles its own leading Spacer(1) internally.
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
        AgentEvent::ThinkingDelta { delta } => {
            app.last_streaming_event = std::time::Instant::now();
            // Progressive thinking: add thinking block to streaming component
            if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                weak.borrow_mut()
                    .add_thinking(&delta, app.thinking_level.clone());
            } else {
                // First thinking delta without text - create component with just thinking
                // Direct addChild matching pi - no spacer, component handles its own spacing.
                use crate::tui::components::rc_ref_cell_component::RcRefCellComponent;
                let mut comp = crate::agent::ui::components::AssistantMessageComponent::new("");
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
        AgentEvent::ToolCall { id, name, args, .. } => {
            app.last_streaming_event = std::time::Instant::now();
            flush_all(app);
            // Clear streaming component so answer text from the next turn creates a new
            // assistant message component (below the tool execution, matching pi).
            app.streaming_component = None;
            // Look up tool renderer from agent tools
            let renderer = app
                .agent_tools
                .iter()
                .find(|t| t.name() == name)
                .and_then(|t| t.renderer());

            // Create a combined ToolExecComponent with renderer, handling per-tool setup
            let started_at = std::time::Instant::now();
            let comp = if name == "bash" {
                let mut tool = crate::agent::ui::components::ToolExecComponent::new(
                    &name,
                    renderer,
                    args.clone(),
                );
                tool.set_started_at(std::time::Instant::now());
                Rc::new(RefCell::new(tool))
            } else if name == "read" {
                let path = args
                    .get("file_path")
                    .or_else(|| args.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let mut comp = crate::agent::ui::components::ToolExecComponent::new(
                    &name,
                    app.agent_tools
                        .iter()
                        .find(|t| t.name() == name)
                        .and_then(|t| t.renderer()),
                    args.clone(),
                );
                comp.set_file_path(path.to_string());
                comp.set_started_at(std::time::Instant::now());
                Rc::new(RefCell::new(comp))
            } else {
                let mut tool = crate::agent::ui::components::ToolExecComponent::new(
                    &name,
                    renderer,
                    args.clone(),
                );
                tool.set_started_at(std::time::Instant::now());
                Rc::new(RefCell::new(tool))
            };
            app.pending_tools.insert(id.clone(), Rc::downgrade(&comp));
            app.tool_call_start_times.insert(id.clone(), started_at);
            chat_add(
                app,
                std::boxed::Box::new(crate::agent::ui::components::RcToolExec(comp)),
            );

            // Legacy path
            let args_str = serde_json::to_string(&args).unwrap_or_default();
            app.messages.push(DisplayMsg::ToolCall {
                name,
                args: args_str,
            });
        }
        AgentEvent::ToolCallArgsUpdate { id, args } => {
            // Progressive args update - re-render call header with new args
            if let Some(weak) = app.pending_tools.get(&id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut().set_args(args);
            }
        }
        AgentEvent::ToolResult {
            content,
            compact: _,
            is_error,
            name,
            id,
        } => {
            app.last_streaming_event = std::time::Instant::now();
            if let Some(weak) = app.pending_tools.remove(&id) {
                // Update existing ToolExecComponent
                if let Some(comp) = weak.upgrade() {
                    if name == "bash" {
                        // Bash tool result: set duration, exit code, truncation info
                        let comp = weak.upgrade().expect("weak still valid");
                        let mut comp = comp.borrow_mut();
                        if let Some(start) = app.tool_call_start_times.remove(&id) {
                            comp.set_final_duration(start.elapsed().as_secs_f64());
                        }
                        if is_error {
                            if content.contains("aborted") || content.contains("cancelled") {
                                comp.set_cancelled(true);
                            } else if let Some(code) = extract_exit_code(&content) {
                                comp.set_exit_code(code);
                            }
                        } else {
                            comp.set_exit_code(0);
                        }
                        if content.contains("Full output:")
                            && let Some(path) = extract_full_output_path(&content)
                        {
                            comp.set_truncated(true, Some(path));
                        }
                        comp.set_result(&content, is_error);
                    } else {
                        comp.borrow_mut().set_result(&content, is_error);
                    };
                }
            } else if name == "bash" {
                // Bash result (from bang command or LLM tool call without preceding ToolCall):
                // Update existing BashExecutionComponent or create new one
                if let Some(weak) = app.bash_component.as_ref().and_then(|w| w.upgrade()) {
                    let mut bash = weak.borrow_mut();
                    let output_lines: Vec<&str> = content.lines().skip(2).collect();
                    let output = output_lines.join("\n");
                    if !output.is_empty() {
                        bash.append_chunk(&output);
                    }
                    bash.set_duration_from_content(&content);
                    bash.set_complete(if is_error { 1 } else { 0 });
                    drop(bash);
                    app.bash_component = None;
                } else {
                    // No tracked component - create new BashExecutionComponent
                    let cmd = content
                        .lines()
                        .next()
                        .unwrap_or("")
                        .trim_start_matches("$ ")
                        .to_string();
                    let mut bash = crate::agent::ui::components::BashExecution::new(&cmd);
                    let output_lines: Vec<&str> = content.lines().skip(2).collect();
                    let output = output_lines.join("\n");
                    if !output.is_empty() {
                        bash.append_chunk(&output);
                    }
                    bash.set_duration_from_content(&content);
                    bash.set_complete(if is_error { 1 } else { 0 });
                    chat_add(app, std::boxed::Box::new(bash));
                }
            } else {
                // Non-bash tool result without preceding call (shouldn't happen):
                // Fall back to generic ToolResultComponent
                chat_add(
                    app,
                    std::boxed::Box::new(crate::agent::ui::components::ToolResultComponent::new(
                        &content, is_error,
                    )),
                );
            }
            // Legacy path
            app.messages.push(DisplayMsg::ToolResult {
                content,
                compact: None,
                is_error,
            });
        }
        AgentEvent::ToolProgress { content, is_error } => {
            app.last_streaming_event = std::time::Instant::now();
            // Stream partial bash output to the tracked bash component
            if let Some(weak) = app.bash_component.as_ref().and_then(|w| w.upgrade()) {
                let mut bash = weak.borrow_mut();
                bash.append_chunk(&content);
                if is_error {
                    bash.set_error(content);
                }
            }
        }
        AgentEvent::UserMessage { content } => {
            app.last_streaming_event = std::time::Instant::now();
            // A queued message was injected by the agent loop (from steering or follow-up queue)
            chat_add(
                app,
                std::boxed::Box::new(crate::agent::ui::components::UserMessageComponent::new(
                    &content,
                )),
            );
            app.messages.push(DisplayMsg::User(content));
        }
        AgentEvent::Aborted { reason } => {
            // Show abort/error text inline in the streaming component (matches pi)
            if let Some(weak) = app.streaming_component.as_ref().and_then(|w| w.upgrade()) {
                let err_text = format!("\n\n**Error:** {}", reason);
                weak.borrow_mut().append_text(&err_text);
            }
            app.last_streaming_event = std::time::Instant::now();
        }
        AgentEvent::TurnEnd => {
            flush_all(app);
            app.last_streaming_event = std::time::Instant::now();
            // Streaming component is complete - clear reference (text persists in chat)
            app.streaming_component = None;
        }
        AgentEvent::AgentEnd { ref messages } => {
            flush_all(app);
            app.streaming_component = None;
            app.bash_component = None;
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            app.cancel_tx = None;

            // Persist new messages to session and update conversation state
            // (user message is in prompts, not duplicated in app.conversation)
            if let Some(ref mut session) = app.session {
                for msg in messages {
                    session.append_message(msg);
                }
            }
            // Extend app.conversation so subsequent turns have full context
            for msg in messages {
                if !app.conversation.iter().any(|m| m.id == msg.id) {
                    app.conversation.push(msg.clone());
                }
            }
            if let Some(last) = messages.iter().rev().find(|m| m.usage.is_some()) {
                app.last_usage = last.usage.clone();
                app.footer
                    .borrow_mut()
                    .accumulate_usage(last.usage.as_ref().unwrap());
            }

            // Update session info for /session command
            app.update_session_info();

            // Note: follow-up messages are handled internally by the agent loop
            // (outer loop in run_agent_loop drains the follow-up queue).
            // Queued messages from when the agent was idle will be submitted
            // via the normal submit_message path on next user input.
        }
    }
}

fn flush_text(app: &mut App) {
    if let Some(text) = app.pending_text.take()
        && !text.is_empty()
    {
        // Add Component to chat_container with spacer
        let mut comp = crate::agent::ui::components::AssistantMessageComponent::new(&text);
        if app.hide_thinking {
            comp.set_hide_thinking(true);
        }
        chat_add(app, std::boxed::Box::new(comp));
        // Legacy path
        app.messages.push(DisplayMsg::AssistantText(text));
    }
}

fn flush_thinking(app: &mut App) {
    if let Some(text) = app.pending_thinking.take()
        && !text.is_empty()
    {
        // Add Component to chat_container with spacer
        let mut thinking = crate::agent::ui::components::AssistantMessageComponent::new("");
        thinking.add_thinking(&text, app.thinking_level.clone());
        if app.hide_thinking {
            thinking.set_hide_thinking(true);
        }
        chat_add(app, std::boxed::Box::new(thinking));
        // Legacy path
        app.messages.push(DisplayMsg::Thinking {
            text,
            level: app.thinking_level.clone(),
        });
    }
}

fn flush_all(app: &mut App) {
    flush_text(app);
    flush_thinking(app);
}

/// Collect tool definitions from the app's agent tools.
fn collect_tool_defs(app: &App) -> Vec<ToolDef> {
    let mut defs = Vec::new();
    for tool in app.agent_tools.iter() {
        if !defs.iter().any(|d: &ToolDef| d.name == tool.name()) {
            defs.push(ToolDef {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters(),
            });
        }
    }
    defs
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

/// Extract the exit code from a bash error result content.
/// Looks for patterns like "Command exited with code N".
fn extract_exit_code(content: &str) -> Option<i32> {
    if let Some(pos) = content.rfind("exited with code ") {
        let num_start = pos + "exited with code ".len();
        let rest = &content[num_start..];
        let num_str: String = rest
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '-')
            .collect();
        if !num_str.is_empty() {
            return num_str.parse().ok();
        }
    }
    None
}

/// Extract the full output path from a bash result content.
/// Looks for patterns like "Full output: /path/to/file".
fn extract_full_output_path(content: &str) -> Option<String> {
    if let Some(pos) = content.rfind("Full output: ") {
        let path_start = pos + "Full output: ".len();
        let rest = &content[path_start..];
        let path: String = rest.chars().take_while(|c| !c.is_whitespace()).collect();
        if !path.is_empty() {
            return Some(path);
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::provider::StreamEvent;
    use crate::agent::types::AgentMessage;
    use crate::agent::ui::messages::render_messages;
    use async_trait::async_trait;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use futures::Stream;
    use std::pin::Pin;
    use tempfile::tempdir;

    struct MockProvider;
    #[async_trait]
    impl Provider for MockProvider {
        async fn stream(
            &self,
            _model: &str,
            _system: &str,
            _msgs: &[AgentMessage],
            _tools: &[ToolDef],
        ) -> anyhow::Result<Pin<Box<dyn Stream<Item = StreamEvent> + Send>>> {
            unimplemented!()
        }
    }

    #[test]
    fn test_compose_ui_stable_line_count() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        let width = 80;

        // First compose
        let before = compose_ui_test(&mut app, width);
        // Type "/"
        let slash = KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE);
        app.editor.borrow_mut().editor.handle_input(&slash);
        // Second compose
        let after = compose_ui_test(&mut app, width);

        assert_eq!(
            before.len(),
            after.len(),
            "Line count changed from {} to {}",
            before.len(),
            after.len()
        );

        // Find the border lines and verify they exist in both
        let before_has_top = before.iter().any(|l| l.contains('─'));
        let before_has_bottom = before.iter().any(|l| l.contains('─'));
        let after_has_top = after.iter().any(|l| l.contains('─'));
        let after_has_bottom = after.iter().any(|l| l.contains('─'));

        assert!(before_has_top, "Before: missing top border");
        assert!(before_has_bottom, "Before: missing bottom border");
        assert!(after_has_top, "After: missing top border");
        assert!(after_has_bottom, "After: missing bottom border");

        // The changed line should be the same index in both
        for (i, (b, a)) in before.iter().zip(after.iter()).enumerate() {
            if b != a {
                eprintln!("Changed line {}: '{}' -> '{}'", i, b, a);
            }
        }
    }

    fn compose_ui_test(app: &mut App, width: usize) -> Vec<String> {
        let theme = &app.theme;
        let mut lines = Vec::new();

        // Header (matches compose_ui)
        // Model info, thinking level, token stats are shown in the footer.
        lines.push(theme.bold(&theme.fg_key(ThemeKey::Accent, "rab")));
        lines.push(String::new());

        let rendered = render_messages(
            &app.messages,
            width,
            app.hide_thinking,
            app.collapse_tool_output,
            theme,
        );

        // Apply scroll offset (matching compose_ui)
        let total = rendered.len();
        let scroll = app.scroll_offset.min(total.saturating_sub(1));
        let visible = if scroll > 0 {
            let indicator = theme.fg_key(ThemeKey::Dim, &format!(" ↑ {} more", scroll));
            lines.push(crate::agent::ui::messages::pad_to_width(&indicator, width));
            &rendered[scroll..]
        } else {
            &rendered[..]
        };
        lines.extend(visible.iter().cloned());

        // Pending (streaming) text - matches compose_ui
        if let Some(ref text) = app.pending_text
            && !text.is_empty()
        {
            let inner = width.saturating_sub(2);
            for line in text.lines() {
                if line.is_empty() {
                    lines.push(String::new());
                } else {
                    let wrapped = crate::tui::util::wrap_text_with_ansi(line, inner);
                    for w in wrapped {
                        let line = format!(" {}", w);
                        lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
                    }
                }
            }
        }
        if let Some(ref text) = app.pending_thinking
            && !text.is_empty()
            && !app.hide_thinking
        {
            let level_color = app
                .thinking_level
                .as_deref()
                .and_then(crate::agent::ui::messages::thinking_level_color)
                .unwrap_or("thinking_text");
            for line in text.lines() {
                let content = format!(" {}", theme.italic(&theme.fg(level_color, line)));
                let padded = crate::agent::ui::messages::pad_to_width(&content, width);
                lines.push(theme.bg("thinking_bg", &padded));
            }
        }

        // Queued messages - matches compose_ui
        let follow_msgs: Vec<String> = {
            let q = app.follow_up_queue.lock().unwrap();
            // Drain a copy: lock briefly, clone messages
            // For the compose_ui test helper, we just read the count
            if q.is_empty() {
                vec![]
            } else {
                // Show placeholder based on queue count
                vec![format!("◷ {} follow-up message(s) pending", q.len())]
            }
        };
        for msg in &follow_msgs {
            let line = theme.fg_key(ThemeKey::Dim, &format!(" {}", msg));
            lines.push(crate::agent::ui::messages::pad_to_width(&line, width));
        }
        if !follow_msgs.is_empty() {
            let hint = theme.fg_key(ThemeKey::Dim, " ↳ queued");
            lines.push(crate::agent::ui::messages::pad_to_width(&hint, width));
        }

        if !lines.is_empty() && !lines.last().is_none_or(|l| l.trim().is_empty()) {
            lines.push(String::new());
        }
        lines.extend(app.working.render(width));
        lines.extend(app.editor.borrow_mut().editor.render(width));
        lines.extend(app.footer.borrow_mut().render(width));
        lines
    }

    // ── New tests ──

    #[test]
    fn test_submit_queues_when_streaming() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);

        // Simulate streaming in progress
        app.is_streaming = true;
        app.follow_up_queue.lock().unwrap().clear();

        // Submit a message while streaming (Enter during streaming = steering)
        submit_message(&mut app, "hello".into());

        assert!(
            !app.steering_queue.lock().unwrap().is_empty(),
            "Message should be in steering queue when streaming"
        );
        assert!(
            app.is_streaming,
            "is_streaming should remain true after queuing"
        );
    }

    #[tokio::test]
    async fn test_submit_starts_loop_when_not_streaming() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.follow_up_queue.lock().unwrap().clear();

        // Submit a message while NOT streaming
        submit_message(&mut app, "hello".into());

        // Should have one user message (no startup info messages)
        assert_eq!(app.messages.len(), 1, "Should have just the user message");
        assert!(
            matches!(app.messages.last(), Some(DisplayMsg::User(_))),
            "Last message should be User"
        );
    }
    #[test]
    fn test_compose_ui_shows_queued_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("queued-msg-1"));
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("queued-msg-2"));

        let lines = compose_ui_test(&mut app, 80);

        let all = lines.join("\n");
        assert!(
            all.contains("follow-up"),
            "Compose UI should show follow-up count"
        );
        assert!(all.contains("2"), "Compose UI should show count of 2");
    }

    #[test]
    fn test_compose_ui_shows_pending_text() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.pending_text = Some("streaming text content".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            all.contains("streaming text"),
            "Compose UI should contain pending streaming text"
        );
    }

    #[test]
    fn test_compose_ui_shows_pending_thinking() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: false,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.pending_thinking = Some("thinking content".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            all.contains("thinking content"),
            "Compose UI should contain pending thinking text when not hidden"
        );
    }

    #[test]
    fn test_pending_thinking_hidden_when_hide_thinking() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.pending_thinking = Some("hidden thinking".into());

        let lines = compose_ui_test(&mut app, 80);
        let all = lines.join("\n");
        assert!(
            !all.contains("hidden thinking"),
            "Compose UI should NOT contain thinking content when hide_thinking is true"
        );
    }

    #[tokio::test]
    async fn test_agent_end_leaves_follow_up_queue() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        let msg = AgentMessage::user("next-msg");
        app.follow_up_queue.lock().unwrap().enqueue(msg.clone());
        app.is_streaming = true;
        app.working.start();

        // AgentEnd no longer processes follow-up queue - the agent loop handles it.
        // The queue should remain intact.
        handle_agent_event(&mut app, AgentEvent::AgentEnd { messages: vec![] });

        assert_eq!(
            app.follow_up_queue.lock().unwrap().len(),
            1,
            "Follow-up queue should NOT be processed by AgentEnd (loop handles it)"
        );
    }

    #[test]
    fn test_ctrl_c_interrupt_restores_queued_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("q1"));
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("q2"));
        app.is_streaming = true;

        // Simulate Ctrl+C
        let mut test_tui = crate::tui::TUI::new();
        handle_input(
            &mut app,
            &mut test_tui,
            &KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );

        assert!(
            app.follow_up_queue.lock().unwrap().is_empty(),
            "Queued messages should be cleared after interrupt"
        );
        assert!(
            app.editor.borrow().editor.get_text().contains("q1"),
            "Editor should contain restored queued messages"
        );
        assert!(
            app.editor.borrow().editor.get_text().contains("q2"),
            "Editor should contain both restored queued messages"
        );
    }

    #[test]
    fn test_render_messages_pads_assistant_text() {
        crate::agent::ui::theme::init_theme(Some("dark"), false);
        let theme = crate::agent::ui::theme::current_theme().clone();
        let msgs = vec![DisplayMsg::AssistantText("short line".into())];

        let width = 60;
        let lines = render_messages(&msgs, width, false, false, &theme);

        for (i, line) in lines.iter().enumerate() {
            let vw = crate::tui::util::visible_width(line);
            assert!(
                vw <= width,
                "Line {} has visible_width {} > width {}: {:?}",
                i,
                vw,
                width,
                line
            );
            // The line should be padded to exactly width (no undershoot)
            if !line.is_empty() {
                assert!(
                    vw >= width.saturating_sub(2),
                    "Line {} has visible_width {} < width-2 {}: {:?}",
                    i,
                    vw,
                    width.saturating_sub(2),
                    line
                );
            }
        }
    }

    #[test]
    fn test_queued_messages_rendered_in_compose_ui_line_count_is_stable() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);

        // No queued messages - compose
        let before = compose_ui_test(&mut app, 80);

        // Add queued messages
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("msg1"));
        let after = compose_ui_test(&mut app, 80);

        // Should have more lines with queued messages (count label and hint)
        assert!(
            after.len() > before.len(),
            "Line count should increase when queued messages are present"
        );

        // Queued messages appear between messages and editor.
        let after_text = after.join("\n");
        assert!(
            after_text.contains("follow-up"),
            "Output should contain follow-up queue info"
        );
    }

    // ── Conversation / send behavior (matches pi) ────────────────

    #[test]
    fn test_submit_message_does_not_add_to_conversation() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);

        // Mark as streaming so submit_message queues instead of spawning a tokio task
        app.is_streaming = true;
        let initial_len = app.conversation.len();

        // Submit a message while streaming (queues, no tokio::spawn)
        submit_message(&mut app, "test message".into());

        // Pi: submit_message sends to agent loop, which emits message_end for persistence.
        // The message is NOT added to app.conversation directly - it flows through AgentEnd.
        assert_eq!(
            app.conversation.len(),
            initial_len,
            "submit_message must not add to app.conversation (avoids double-send)"
        );

        // But the display message IS added immediately (so the UI shows it)
        assert!(
            app.messages
                .iter()
                .any(|m| matches!(m, DisplayMsg::User(t) if t == "test message")),
            "submit_message must add DisplayMsg::User for immediate display"
        );
    }

    #[test]
    fn test_agent_end_populates_conversation() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);

        // Simulate the messages that AgentEnd receives from run_agent_loop
        let user_msg = AgentMessage::user("hello");
        let assistant_msg = AgentMessage {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id: None,
            role: crate::agent::types::Role::Assistant,
            content: "Hello back".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: chrono::Utc::now().timestamp_millis(),
        };

        let agent_messages = vec![user_msg.clone(), assistant_msg.clone()];

        // Fire AgentEnd
        handle_agent_event(
            &mut app,
            AgentEvent::AgentEnd {
                messages: agent_messages,
            },
        );

        // Pi: all messages from the turn are added to conversation on AgentEnd.
        // The user message appears once (not duplicated), followed by assistant responses.
        assert_eq!(
            app.conversation.len(),
            2,
            "conversation should have user + assistant"
        );
        assert_eq!(app.conversation[0].content, "hello");
        assert_eq!(app.conversation[0].role, crate::agent::types::Role::User);
        assert_eq!(app.conversation[1].content, "Hello back");
        assert_eq!(
            app.conversation[1].role,
            crate::agent::types::Role::Assistant
        );
    }

    #[test]
    fn test_agent_end_no_duplicate_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);

        let config = AppConfig {
            model: "deepseek-v4-flash".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd: cwd.clone(),
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        };

        let mut app = App::new(config, session);

        // Simulate a message already in conversation (e.g. from a previous turn)
        let existing = AgentMessage::user("existing");
        let existing_id = existing.id.clone();
        app.conversation.push(existing);

        // AgentEnd fires with the SAME message id - should NOT duplicate
        let dup_msg = AgentMessage {
            id: existing_id,
            parent_id: None,
            role: crate::agent::types::Role::User,
            content: "existing".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            usage: None,
            is_error: false,
            timestamp: 0,
        };

        handle_agent_event(
            &mut app,
            AgentEvent::AgentEnd {
                messages: vec![dup_msg],
            },
        );

        // Should still have exactly 1 message (deduplicated by id)
        assert_eq!(
            app.conversation.len(),
            1,
            "AgentEnd must not duplicate messages already in conversation (pi-style)"
        );
    }

    // ── New actions tests ──

    #[test]
    fn test_handle_clear_when_streaming_interrupts() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);
        app.is_streaming = true;
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("q"));

        handle_clear(&mut app);

        assert!(!app.is_streaming, "Streaming should be interrupted");
        assert!(
            app.follow_up_queue.lock().unwrap().is_empty(),
            "Queued messages should be restored"
        );
    }

    #[test]
    fn test_handle_clear_not_streaming_clears_editor() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);
        app.is_streaming = false;
        app.editor.borrow_mut().editor.set_text("some text");
        // Set last_clear_time far in the past so double-press doesn't trigger
        app.last_clear_time = std::time::Instant::now() - std::time::Duration::from_secs(10);

        handle_clear(&mut app);

        assert!(
            app.editor.borrow().editor.get_text().is_empty(),
            "Editor should be cleared"
        );
    }

    #[test]
    fn test_handle_clear_double_press_exits() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);
        app.is_streaming = false;
        // Set last_clear_time to just a few ms ago to trigger double-press detection
        app.last_clear_time = std::time::Instant::now();

        handle_clear(&mut app);

        assert!(app.should_quit, "Double Ctrl+C should exit");
    }

    #[test]
    fn test_handle_thinking_cycle() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = AppConfig {
            available_models: vec!["model".into()],
            model: "model".into(),
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
            ..make_config(cwd.clone())
        };
        let mut app = App::new(config, session);

        // Start from off
        app.thinking_level = Some("off".into());

        handle_thinking_cycle(&mut app);
        assert_eq!(app.thinking_level.as_deref(), Some("xhigh"));

        handle_thinking_cycle(&mut app);
        assert_eq!(app.thinking_level.as_deref(), Some("high"));

        handle_thinking_cycle(&mut app);
        assert_eq!(app.thinking_level.as_deref(), Some("medium"));

        handle_thinking_cycle(&mut app);
        assert_eq!(app.thinking_level.as_deref(), Some("low"));

        handle_thinking_cycle(&mut app);
        assert_eq!(app.thinking_level.as_deref(), Some("off"));
    }

    #[test]
    fn test_handle_model_cycle_forward() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = AppConfig {
            available_models: vec!["A".into(), "B".into(), "C".into()],
            model: "A".into(),
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
            ..make_config(cwd.clone())
        };
        let mut app = App::new(config, session);

        handle_model_cycle(&mut app, 1);
        assert_eq!(app.model, "B");

        handle_model_cycle(&mut app, 1);
        assert_eq!(app.model, "C");

        handle_model_cycle(&mut app, 1);
        assert_eq!(app.model, "A"); // wraps around
    }

    #[test]
    fn test_handle_model_cycle_backward() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = AppConfig {
            available_models: vec!["A".into(), "B".into(), "C".into()],
            model: "A".into(),
            model_supports_reasoning: true,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
            ..make_config(cwd.clone())
        };
        let mut app = App::new(config, session);

        handle_model_cycle(&mut app, -1);
        assert_eq!(app.model, "C"); // wraps around backwards

        handle_model_cycle(&mut app, -1);
        assert_eq!(app.model, "B");

        handle_model_cycle(&mut app, -1);
        assert_eq!(app.model, "A");
    }

    #[test]
    fn test_handle_tools_expand_toggles() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);

        app.tools_expanded = false;
        app.collapse_tool_output = true;

        handle_tools_expand(&mut app);

        assert!(app.tools_expanded, "tools_expanded should be true");
        assert!(!app.collapse_tool_output, "collapse should be false");

        handle_tools_expand(&mut app);

        assert!(!app.tools_expanded, "tools_expanded should be false");
        assert!(app.collapse_tool_output, "collapse should be true");
    }

    #[test]
    fn test_handle_follow_up_queues_when_streaming() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);
        app.is_streaming = true;

        handle_follow_up(&mut app, "follow-up text".into());

        assert_eq!(app.follow_up_queue.lock().unwrap().len(), 1);
        assert_eq!(
            app.follow_up_queue.lock().unwrap().drain()[0]
                .content
                .clone(),
            "follow-up text"
        );
    }

    #[test]
    fn test_handle_dequeue_restores_messages() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("msg1"));
        app.follow_up_queue
            .lock()
            .unwrap()
            .enqueue(AgentMessage::user("msg2"));

        handle_dequeue(&mut app);

        assert!(
            app.follow_up_queue.lock().unwrap().is_empty(),
            "Queues should be empty"
        );
        assert!(
            app.editor.borrow().editor.get_text().contains("msg1"),
            "Editor should contain msg1"
        );
        assert!(
            app.editor.borrow().editor.get_text().contains("msg2"),
            "Editor should contain msg2"
        );
    }

    #[test]
    fn test_handle_compact_toggle_toggles_flag() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);

        app.auto_compact = true;
        handle_compact_toggle(&mut app);
        assert!(!app.auto_compact, "Should toggle off");

        handle_compact_toggle(&mut app);
        assert!(app.auto_compact, "Should toggle back on");
    }

    #[test]
    fn test_submit_resets_scroll_offset() {
        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);

        app.scroll_offset = 20;
        app.is_streaming = true;
        submit_message(&mut app, "test".into());

        assert_eq!(app.scroll_offset, 0, "submit should reset scroll_offset");
    }

    #[test]
    fn test_scroll_indicator_shown_when_scrolled() {
        use crate::agent::ui::theme;
        theme::init_theme(Some("dark"), false);

        let tmp = tempdir().unwrap();
        let cwd = tmp.path().to_path_buf();
        let session = SessionManager::in_memory(&cwd);
        let config = make_config(cwd.clone());
        let mut app = App::new(config, session);

        // Add enough messages that we can test scrolling
        // Use Info messages which render simply as a single line
        app.messages.push(DisplayMsg::Info("msg 1".into()));
        app.messages.push(DisplayMsg::Info("msg 2".into()));
        app.messages.push(DisplayMsg::Info("msg 3".into()));
        app.messages.push(DisplayMsg::Info("msg 4".into()));

        app.scroll_offset = 0;
        let lines_scrolled_0 = compose_ui_test(&mut app, 80);
        let text_0 = lines_scrolled_0.join("\n");
        // Should have all info messages visible
        assert!(text_0.contains("msg 1"), "Should show msg 1 at offset 0");
        assert!(text_0.contains("msg 4"), "Should show msg 4 at offset 0");
        // Should NOT have scroll indicator
        assert!(!text_0.contains("↑"), "No scroll indicator at offset 0");

        app.scroll_offset = 2;
        let lines_scrolled = compose_ui_test(&mut app, 80);
        let text = lines_scrolled.join("\n");
        // msg 1 and 2 exceed the scroll offset (there are 4 messages)
        // We scroll past 2 lines, so msg 1 should still be visible
        // Actually, Info lines: " msg 1", " msg 2", " msg 3", " msg 4" = 4 lines
        // Scroll 2 → skip first 2 → show msg 3, msg 4
        assert!(
            !text.contains("msg 2"),
            "msg 2 should be hidden when scrolled"
        );
        assert!(text.contains("msg 3"), "msg 3 should still show");
        assert!(text.contains("msg 4"), "msg 4 should still show");
        // Should show scroll indicator
        assert!(text.contains("↑"), "Should show scroll indicator");
    }

    /// Helper to create a minimal AppConfig for testing.
    fn make_config(cwd: std::path::PathBuf) -> AppConfig {
        AppConfig {
            model: "test-model".into(),
            system_prompt: String::new(),
            tools: vec![],
            agent_tools: vec![],
            extensions: vec![],
            provider: Box::new(MockProvider),
            cwd,
            thinking_level: None,
            git_branch: None,
            available_models: vec![],
            hide_thinking: true,
            collapse_tool_output: true,
            interactive: true,
            settings: crate::agent::settings::Settings::default(),
            context_files: vec![],
            skills: vec![],
            model_supports_reasoning: false,
            tool_execution: ToolExecutionMode::Parallel,
            session_info: None,
        }
    }
}
