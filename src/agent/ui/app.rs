use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;

use crate::agent::AgentSession;
use crate::agent::extension::{CommandResult, Extension};
use crate::agent::session::SessionManager;

use crate::agent::ui::chat_editor::{ChatEditor, InputAction};
use crate::agent::ui::components::EditorComponent;
use crate::agent::ui::components::FooterComponent;
use crate::agent::ui::components::InfoMessageComponent;
use crate::agent::ui::footer::Footer;
use crate::agent::ui::model_selector::ModelSelector;
use crate::agent::ui::theme::RabTheme;
use crate::agent::ui::working::WorkingIndicator;
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
    pub extensions: Vec<Box<dyn Extension>>,
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
    /// Tool execution strategy (parallel by default).
    #[allow(dead_code)]
    pub tool_execution: yoagent::types::ToolExecutionStrategy,
    /// Skills loaded for the session (used for /skill:name expansion).
    pub skills: Vec<yoagent::skills::Skill>,
    /// Whether the current model supports reasoning (for showing thinking level in footer).
    pub model_supports_reasoning: bool,
    /// Session info Arc for /session command (shared with CommandsExtension).
    pub session_info: Option<std::sync::Arc<std::sync::Mutex<Option<SessionInfoInternal>>>>,
    /// API key for yoagent provider.
    pub api_key: String,
}

/// Main application state.
pub struct App {
    cwd: PathBuf,
    model: String,
    thinking_level: Option<String>,
    system_prompt: String,
    theme: RabTheme,

    /// Slash commands from all extensions.
    commands: Vec<(String, String)>,

    /// Available models for the model selector.
    available_models: Vec<String>,

    /// Conversation history (AgentMessage).
    conversation: Vec<yoagent::types::AgentMessage>,

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
    event_tx: mpsc::UnboundedSender<yoagent::types::AgentEvent>,
    event_rx: mpsc::UnboundedReceiver<yoagent::types::AgentEvent>,

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

    /// Agent cancellation sender for Ctrl+C / ESC.
    cancel_tx: Option<tokio::sync::watch::Sender<bool>>,

    /// Bash abort handle for bang (!) commands.
    bash_abort_handle: Option<tokio::task::AbortHandle>,

    /// Session persistence via AgentSession lifecycle layer.
    session: Option<AgentSession>,

    /// Footer (shared ownership - App mutates, TUI.root renders).
    footer: Rc<RefCell<Footer>>,

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
    /// Extensions.
    extensions: Arc<Vec<Box<dyn Extension>>>,

    /// Steering queue: messages delivered after current turn's tool calls finish,
    /// before the next LLM call. Shared with the agent loop.
    steering_queue: Arc<std::sync::Mutex<Vec<yoagent::types::AgentMessage>>>,
    /// Follow-up queue: messages delivered only after the agent has no more
    /// tool calls (fully idle). Shared with the agent loop.
    follow_up_queue: Arc<std::sync::Mutex<Vec<yoagent::types::AgentMessage>>>,
    /// Tool execution mode (parallel by default).
    #[allow(dead_code)]
    tool_execution: yoagent::types::ToolExecutionStrategy,

    /// Skills loaded for the session (/skill:name expansion).
    skills: Vec<yoagent::skills::Skill>,
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

    /// Session picker state (Some = picker is active).
    session_picker: Option<crate::agent::ui::components::SessionPicker>,

    /// Tracks the number of children in `chat_container` after the last
    /// status message was added (pi-style `lastStatusSpacer`/`lastStatusText`).
    /// Used by `show_status()` to replace consecutive status messages in-place
    /// instead of appending indefinitely.
    last_status_len: Option<usize>,
    // ── Message rendering cache (avoids re-rendering messages every frame) ──
    // Cache fields removed - messages now rendered via Components in chat_container.
}

impl App {
    fn new(config: AppConfig, session: SessionManager) -> Self {
        let agent_session = AgentSession::new(session);
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
        let context = agent_session.session().build_session_context();
        let history_messages = context.messages.clone();

        // Startup info: context files, skills, tools (pi-style loaded resources listing)
        let mut resource_parts: Vec<String> = Vec::new();
        if !config.context_files.is_empty() {
            let ctx = config.context_files.join(", ");
            resource_parts.push(format!("Context: {}", ctx));
        }
        if !config.skills.is_empty() {
            let skill_names: Vec<&str> = config.skills.iter().map(|s| s.name.as_str()).collect();
            resource_parts.push(format!("Skills: {}", skill_names.join(", ")));
        }

        // Build chat_container from AgentMessages directly (matching pi's renderSessionContext).
        // Adjacent toolCall content + toolResult messages are paired into single
        // ToolExecComponent so reloaded sessions look identical to live execution.
        let cwd_string = config.cwd.to_string_lossy().to_string();
        let chat_container =
            std::rc::Rc::new(std::cell::RefCell::new(crate::tui::Container::new()));
        {
            let mut chat = chat_container.borrow_mut();

            // Startup info component
            if !resource_parts.is_empty() {
                chat.add_child(std::boxed::Box::new(
                    crate::agent::ui::components::InfoMessageComponent::new(
                        resource_parts.join("  ·  "),
                    ),
                ));
            }

            rebuild_chat_from_messages(
                &mut chat,
                &history_messages,
                &cwd_string,
                config.hide_thinking,
                config.collapse_tool_output,
            );
        }

        let result = Self {
            cwd: config.cwd,
            model: config.model,
            thinking_level: config.thinking_level,
            system_prompt: config.system_prompt,
            theme,
            commands,
            available_models: config.available_models,
            conversation: history_messages,
            chat_container,
            pending_tools: HashMap::new(),
            tool_call_start_times: HashMap::new(),
            invalidate_rxs: Vec::new(),
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
            cancel_tx: None,
            bash_abort_handle: None,
            session: Some(agent_session),
            footer,
            working: WorkingIndicator::new(),
            extensions: Arc::new(config.extensions),
            steering_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            follow_up_queue: Arc::new(std::sync::Mutex::new(Vec::new())),
            tool_execution: config.tool_execution,
            skills: config.skills,
            session_info: config.session_info,
            api_key: config.api_key,
            settings: config.settings,
            auto_compact: true,
            status_text: None,
            header: Rc::new(RefCell::new(
                crate::agent::ui::components::HeaderComponent::new(),
            )),
            session_picker: None,
            last_status_len: None,
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
            let si = crate::builtin::commands::compute_session_info(session.session());
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
                        handle_input(&mut app, &mut tui, &mut term, &key);
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
                    // Open session picker
                    let mut picker = crate::agent::ui::components::SessionPicker::new();
                    let repo = crate::agent::DefaultSessionRepo::new();
                    picker.load_sessions(&repo);
                    app.session_picker = Some(picker);
                    app.status_text = None;
                }
                CommandResult::OpenSettings => {
                    chat_add(
                        &mut app,
                        std::boxed::Box::new(InfoMessageComponent::new(
                            "Settings menu - not yet implemented.",
                        )),
                    );
                }
                CommandResult::ScopedModels => {
                    chat_add(
                        &mut app,
                        std::boxed::Box::new(InfoMessageComponent::new(
                            "Scoped models - not yet implemented.",
                        )),
                    );
                }
                CommandResult::Login { .. } => {
                    chat_add(
                        &mut app,
                        std::boxed::Box::new(InfoMessageComponent::new(
                            "Login dialog - not yet implemented.",
                        )),
                    );
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

        // Safety timeout: force stop streaming if no agent events for 60s.
        // Prevents the event loop from spinning at 16ms with `is_streaming`
        // stuck true after the agent task completes or panics without
        // delivering AgentEnd through the channel.
        if app.is_streaming
            && app.last_streaming_event.elapsed() > std::time::Duration::from_secs(15)
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
    // ── Session picker ──
    if let Some(ref picker) = app.session_picker {
        let (lines, _cursor_y) = picker.render(width, &app.theme as &dyn crate::tui::Theme);
        app.pending_section.borrow_mut().set_lines(lines);
        // Clear chat container when picker is active
        app.chat_container.borrow_mut().clear();
        app.status_section.borrow_mut().set_lines(vec![]);
        app.queued_section.borrow_mut().set_lines(vec![]);
        app.working_section.borrow_mut().set_lines(vec![]);
        return;
    }

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
                    pending_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
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
            let padded = crate::agent::ui::render_utils::pad_to_width(&content, width);
            pending_lines.push(app.theme.bg("thinking_bg", &padded));
        } else {
            let level_color = app
                .thinking_level
                .as_deref()
                .and_then(crate::agent::ui::render_utils::thinking_level_color)
                .unwrap_or("thinking_text");
            for line in text.lines() {
                let content = format!(" {}", app.theme.italic(&app.theme.fg(level_color, line)));
                let padded = crate::agent::ui::render_utils::pad_to_width(&content, width);
                pending_lines.push(app.theme.bg("thinking_bg", &padded));
            }
        }
    }
    app.pending_section.borrow_mut().set_lines(pending_lines);

    // ── Transient status text (pi-style: replaces previous status, not added to chat) ──
    let mut status_lines = Vec::new();
    if let Some(ref status) = app.status_text {
        let line = app.theme.fg_key(ThemeKey::Dim, &format!(" {}", status));
        status_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
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
        queued_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
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
        queued_lines.push(crate::agent::ui::render_utils::pad_to_width(&line, width));
    }
    if steer_count > 0 || follow_count > 0 {
        let hint = app.theme.fg_key(
            ThemeKey::Dim,
            " ↳ Esc to abort, Alt+↑ to restore follow-ups",
        );
        queued_lines.push(crate::agent::ui::render_utils::pad_to_width(&hint, width));
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
fn handle_input(app: &mut App, tui: &mut TUI, term: &mut ProcessTerminal, key: &KeyEvent) {
    // ── Session picker input handling ──
    if app.session_picker.is_some() {
        handle_session_picker_input(app, key);
        return;
    }

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
    // Record the change in the session (pi-compatible)
    if let Some(ref mut agent_session) = app.session {
        agent_session.on_thinking_level_change(next);
    }
    // yoagent hardcodes ThinkingLevel::High, no provider call needed
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

/// Queue a follow-up message (Alt+Enter). Pushes to follow-up queue during streaming
/// (delivered only after agent has no more tool calls). When idle, submits immediately.
fn handle_follow_up(app: &mut App, text: String) {
    if app.is_streaming {
        app.follow_up_queue
            .lock()
            .unwrap()
            .push(crate::agent::types::user_message(text));
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
    let all: Vec<yoagent::types::AgentMessage> = queue.drain(..).collect();
    let restored: Vec<String> = all.iter().map(crate::agent::types::message_text).collect();
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
            let all: Vec<yoagent::types::AgentMessage> = follow_up.drain(..).collect();
            let text: Vec<String> = all.iter().map(crate::agent::types::message_text).collect();
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
        let expanded = expand_skill_command(&trimmed, &app.skills);
        chat_add(
            app,
            std::boxed::Box::new(crate::agent::ui::components::UserMessageComponent::new(
                &expanded,
            )),
        );
        // Message will be persisted on message_end (pi-compatible)
        if app.is_streaming {
            app.steering_queue
                .lock()
                .unwrap()
                .push(crate::agent::types::user_message(expanded));
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

    // In-memory conversation is updated on AgentEnd (pi-compatible: messages
    // are persisted to the session file on message_end, not here).

    if app.is_streaming {
        // Safety check: if is_streaming is true but no events arrived for >5s,
        // the spawned task may have crashed without sending AgentEnd (e.g. panic
        // in provider.stream() before the Drop guard was added). Force-reset so
        // the new message actually starts a fresh agent loop instead of silently
        // queueing to the steering queue.
        if app.last_streaming_event.elapsed() > std::time::Duration::from_secs(5) {
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            app.cancel_tx = None;
            app.status_text = Some("Previous agent loop appears stuck — restarting".into());
        } else {
            // Steering: delivered after current turn's tool calls finish, before next LLM call
            app.steering_queue
                .lock()
                .unwrap()
                .push(crate::agent::types::user_message(trimmed));
            return;
        }
    }

    start_agent_loop(app, trimmed);
}

/// Actually start an agent loop (not queued).
/// Uses yoagent's Agent internally.
fn start_agent_loop(app: &mut App, message: String) {
    let model = app.model.clone();
    let system_prompt = app.system_prompt.clone();
    let tx = app.event_tx.clone();
    let api_key = app.api_key.clone();
    let extensions = Arc::clone(&app.extensions);

    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);
    app.pending_text = None;
    app.pending_thinking = None;

    // Capture conversation history and thinking level before spawning
    let history = app.conversation.clone();
    let thinking_level = app.thinking_level.clone();

    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    app.cancel_tx = Some(cancel_tx);

    tokio::spawn(async move {
        // ToolWithMeta IS an AgentTool — no unwrapping needed
        let yoagent_tools: Vec<Box<dyn yoagent::types::AgentTool>> = extensions
            .iter()
            .flat_map(|ext| ext.tools())
            .map(|twm| Box::new(twm) as Box<dyn yoagent::types::AgentTool>)
            .collect();
        // Map rab's thinking level string to yoagent enum
        let thinking = match thinking_level.as_deref() {
            Some("off") => yoagent::types::ThinkingLevel::Off,
            Some("low") => yoagent::types::ThinkingLevel::Low,
            Some("medium") => yoagent::types::ThinkingLevel::Medium,
            Some("high") | Some("xhigh") => yoagent::types::ThinkingLevel::High,
            _ => yoagent::types::ThinkingLevel::High,
        };
        let mut agent = yoagent::agent::Agent::new(yoagent::provider::OpenAiCompatProvider)
            .with_model(&model)
            .with_api_key(&api_key)
            .with_model_config(yoagent::provider::model::ModelConfig::openai_compat(
                "https://opencode.ai/zen/go/v1",
                &model,
                "opencode-go",
                yoagent::provider::model::OpenAiCompat::deepseek(),
            ))
            .with_system_prompt(&system_prompt)
            .with_thinking(thinking)
            .with_messages(history)
            .with_tools(yoagent_tools)
            .without_context_management();

        let (yo_tx, mut yo_rx) = tokio::sync::mpsc::unbounded_channel();

        // Track whether we've forwarded an AgentEnd event.
        // If the agent task finishes without sending one (e.g. panic),
        // we synthesize an error to avoid silently hanging the UI.
        let mut agent_end_seen = false;

        // Spawn agent loop in a separate task (it blocks until done)
        let inner_handle = tokio::spawn(async move {
            agent.prompt_with_sender(message, yo_tx).await;
        });

        // Forward yoagent events directly to the app event channel, cancellable
        tokio::select! {
            _ = async {
                while let Some(event) = yo_rx.recv().await {
                    if matches!(&event, yoagent::types::AgentEvent::AgentEnd { .. }) {
                        agent_end_seen = true;
                    }
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            } => {}
            _ = cancel_rx.changed() => {}
        }

        // If the agent task finished without sending AgentEnd (panic/crash),
        // notify the UI so it doesn't stay stuck in streaming state.
        if !agent_end_seen {
            let _ = tx.send(yoagent::types::AgentEvent::AgentEnd { messages: vec![] });
            // Also drop a status message for the user about the crash
            let _ = tx.send(yoagent::types::AgentEvent::ProgressMessage {
                tool_call_id: String::new(),
                tool_name: String::new(),
                text: "\n⚠ Agent loop ended unexpectedly — it may have crashed or encountered a network error.\n".to_string(),
            });
        }

        // Abort the inner task in case it's still running (e.g. cancelled but not stopped)
        inner_handle.abort();
    });
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
                // Switch to the selected session
                let new_sm = SessionManager::open(&path, None, Some(&app.cwd));
                let new_session = AgentSession::new(new_sm);
                let ctx = new_session.session().build_session_context();
                app.conversation = ctx.messages;
                app.chat_container.borrow_mut().clear();
                app.streaming_component = None;
                app.pending_text = None;
                app.pending_thinking = None;
                app.pending_tools.clear();
                app.tool_call_start_times.clear();
                rebuild_chat_from_messages(
                    &mut app.chat_container.borrow_mut(),
                    &app.conversation,
                    &app.cwd.to_string_lossy(),
                    app.hide_thinking,
                    app.collapse_tool_output,
                );
                app.session = Some(new_session);
                app.update_session_info();
                app.status_text = Some(format!("Switched to session: {}", path.display()));
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
                        chat_add(
                            app,
                            std::boxed::Box::new(InfoMessageComponent::new(format!(
                                "Error executing /{}: {}",
                                cmd_name, e
                            ))),
                        );
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
            // Actually reload settings from disk (pi-compatible)
            if let Err(e) = app.settings.reload(&app.cwd) {
                app.status_text = Some(format!("Failed to reload settings: {}", e));
            } else {
                // Apply reloaded settings to runtime state
                if let Some(level) = app.settings.default_thinking_level.clone() {
                    app.thinking_level = Some(level.clone());
                    app.footer
                        .borrow_mut()
                        .set_thinking_level(Some(level.clone()));
                    // yoagent hardcodes ThinkingLevel::High
                }
                app.hide_thinking = app.settings.hide_thinking.unwrap_or(true);
                // Propagate to all chat container components
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
                app.editor.borrow_mut().update_border_color(
                    app.thinking_level.as_deref(),
                    &app.theme as &dyn crate::tui::Theme,
                );
                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(
                        "Settings, extensions, and keybindings reloaded.".to_string(),
                    )),
                );
            }
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
            app.conversation.clear();
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
            // Open the new session file
            let new_sm = crate::agent::session::SessionManager::open(&path, None, Some(&app.cwd));
            let new_session = crate::agent::AgentSession::new(new_sm);

            // Load conversation from new session
            let ctx = new_session.session().build_session_context();
            app.conversation = ctx.messages;
            app.chat_container.borrow_mut().clear();
            app.streaming_component = None;
            app.pending_text = None;
            app.pending_thinking = None;
            app.pending_tools.clear();
            app.tool_call_start_times.clear();

            rebuild_chat_from_messages(
                &mut app.chat_container.borrow_mut(),
                &app.conversation,
                &app.cwd.to_string_lossy(),
                app.hide_thinking,
                app.collapse_tool_output,
            );

            app.session = Some(new_session);
            app.update_session_info();
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
                .or_else(|| {
                    app.session
                        .as_ref()
                        .and_then(|s| s.session().session_name())
                })
                .unwrap_or("unnamed");
            let file_display = file_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "in-memory".to_string());
            let sid = if session_id.is_empty() {
                app.session
                    .as_ref()
                    .map(|s| s.session().session_id().to_string())
                    .unwrap_or_default()
            } else {
                session_id
            };

            let user_messages = app
                .conversation
                .iter()
                .filter(|m| crate::agent::types::message_is_user(m))
                .count();
            let assistant_messages = app
                .conversation
                .iter()
                .filter(|m| crate::agent::types::message_is_assistant(m))
                .count();
            let tool_results = app
                .conversation
                .iter()
                .filter(|m| crate::agent::types::message_is_tool_result(m))
                .count();
            let tool_calls: usize = app
                .conversation
                .iter()
                .map(crate::agent::types::message_tool_call_count)
                .sum();
            let total_messages = user_messages + assistant_messages + tool_results;

            let mut input_tokens: u64 = 0;
            let mut output_tokens: u64 = 0;
            let mut cache_read_tokens: u64 = 0;
            let cost: f64 = 0.0;
            for msg in &app.conversation {
                if let Some(usage) = crate::agent::types::message_usage(msg) {
                    input_tokens += usage.input;
                    output_tokens += usage.output;
                    cache_read_tokens += usage.cache_read;
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

            // Parent session (fork chain)
            if let Some(ref session) = app.session
                && let Some(header) = session.session().get_header()
                && let Some(ref parent) = header.parent_session
            {
                info += &format!("\n\nParent: {}", parent);
            }

            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(info.clone())),
            );
        }
        CommandResult::OpenSessionSelector => {
            // Load and display available sessions
            use crate::agent::SessionRepo;
            let repo = crate::agent::DefaultSessionRepo::new();
            let sessions = repo.list_all(None);

            if sessions.is_empty() {
                let msg = "No sessions found.".to_string();
                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
                );
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

                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(info.clone())),
                );
            }
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
        }
        CommandResult::ImportSession { path } => {
            let msg = format!("Import session from {} - not yet implemented.", path);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::ShareSession => {
            let msg = "Share session - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::CopyLastMessage => {
            let msg = "Copy last agent message to clipboard - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::ShowChangelog => {
            let msg = "Changelog - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::ForkSession { message_id } => {
            // Clone the session info before modifying app.session
            let source_path = app
                .session
                .as_ref()
                .and_then(|s| s.session().session_file().map(|p| p.to_path_buf()));
            let session_dir = app
                .session
                .as_ref()
                .map(|s| s.session().session_dir().to_path_buf());
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
                                    let new_sm = crate::agent::session::SessionManager::open(
                                        path,
                                        None,
                                        Some(&cwd),
                                    );
                                    let new_session = crate::agent::AgentSession::new(new_sm);

                                    // Reload conversation
                                    let ctx = new_session.session().build_session_context();
                                    app.conversation = ctx.messages;
                                    app.chat_container.borrow_mut().clear();
                                    app.streaming_component = None;
                                    app.pending_text = None;
                                    app.pending_thinking = None;
                                    app.pending_tools.clear();
                                    app.tool_call_start_times.clear();

                                    rebuild_chat_from_messages(
                                        &mut app.chat_container.borrow_mut(),
                                        &app.conversation,
                                        &app.cwd.to_string_lossy(),
                                        app.hide_thinking,
                                        app.collapse_tool_output,
                                    );

                                    app.session = Some(new_session);

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
                                    chat_add(
                                        app,
                                        std::boxed::Box::new(InfoMessageComponent::new(msg)),
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Fork failed: {}", e);
                            chat_add(
                                app,
                                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
                            );
                        }
                    }
                }
                _ => {
                    let msg = "No active session to fork".to_string();
                    chat_add(
                        app,
                        std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
                    );
                }
            }
        }
        CommandResult::CloneSession => {
            let msg = "Clone session - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::SessionTree => {
            let msg = "Session tree - not yet implemented.".to_string();
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        CommandResult::TrustDecision { decision } => {
            let msg = format!("Trust decision '{}' saved.", decision);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
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
        }
        CommandResult::CompactSession => {
            // Run manual compaction via AgentSession
            if let Some(ref mut _agent_session) = app.session {
                // Show working indicator
                app.working.start();

                // We can't await in this sync context, so we return a status message
                // and the actual compaction runs asynchronously. For now, show a note.
                let msg = "Manual compaction requested. Use /compact in print mode or restart interactive.".to_string();
                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
                );
            } else {
                let msg = "No active session to compact".to_string();
                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
                );
            }
        }
    }
}

/// Handle ! and !! bang commands.
/// Renders via BashExecutionComponent (borders, spinner, expand/collapse)
/// matching pi's handleBashCommand.
fn handle_bang_command(app: &mut App, command: String) {
    let cwd = app.cwd.clone();
    let tx = app.event_tx.clone();
    use yoagent::types::{AgentEvent as YoEvent, Content as YoContent, ToolResult as YoResult};

    // Add BashExecutionComponent to chat_container (track for result updates)
    let bash_comp = Rc::new(RefCell::new(
        crate::agent::ui::components::BashExecution::new(&command),
    ));
    bash_comp
        .borrow_mut()
        .set_started_at(std::time::Instant::now());
    bash_comp.borrow_mut().set_expanded(app.tools_expanded);
    app.bash_component = Some(Rc::downgrade(&bash_comp));
    chat_add(
        app,
        std::boxed::Box::new(crate::tui::components::RcRefCellComponent(bash_comp)),
    );
    app.is_streaming = true;
    app.working.start();
    app.footer.borrow_mut().set_streaming(true);

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
                let _ = tx.send(YoEvent::ProgressMessage {
                    tool_call_id: String::new(),
                    tool_name: "bash".into(),
                    text: format!("Failed to spawn: {}", e),
                });
                let _ = tx.send(YoEvent::ToolExecutionEnd {
                    tool_call_id: String::new(),
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
                                let _ = tx.send(YoEvent::ProgressMessage {
                                    tool_call_id: String::new(),
                                    tool_name: "bash".into(),
                                    text: text.to_string(),
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
                                let _ = tx.send(YoEvent::ProgressMessage {
                                    tool_call_id: String::new(),
                                    tool_name: "bash".into(),
                                    text: text.to_string(),
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

        let _ = tx.send(YoEvent::ToolExecutionEnd {
            tool_call_id: String::new(),
            tool_name: "bash".into(),
            result: YoResult {
                content: vec![YoContent::Text {
                    text: format!(
                        "$ {}\n\n{}\n\n[{}s]",
                        command,
                        result,
                        elapsed.as_secs_f64()
                    ),
                }],
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
                        if !chat.children().is_empty() {
                            chat.add_child(std::boxed::Box::new(Spacer::new(1)));
                        }
                        let mut asst =
                            crate::agent::ui::components::AssistantMessageComponent::new(&text);
                        if hide_thinking {
                            asst.set_hide_thinking(true);
                        }
                        chat.add_child(std::boxed::Box::new(asst));
                    }
                    // Create ToolExecComponent for each tool call
                    for (id, name, args) in &tcs {
                        let mut tool = crate::agent::ui::components::ToolExecComponent::new(
                            name,
                            None,
                            args.clone(),
                            cwd.to_string(),
                        );
                        tool.set_bash(name == "bash");
                        let tool = Rc::new(RefCell::new(tool));
                        chat.add_child(std::boxed::Box::new(
                            crate::agent::ui::components::RcToolExec(tool.clone()),
                        ));
                        pending_tool_components.insert(id.clone(), tool);
                    }
                } else if !text.trim().is_empty() {
                    // Plain text assistant
                    if !chat.children().is_empty() {
                        chat.add_child(std::boxed::Box::new(Spacer::new(1)));
                    }
                    let mut asst =
                        crate::agent::ui::components::AssistantMessageComponent::new(&text);
                    if hide_thinking {
                        asst.set_hide_thinking(true);
                    }
                    chat.add_child(std::boxed::Box::new(asst));
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

/// Handle agent events from the channel.
fn handle_agent_event(app: &mut App, event: yoagent::types::AgentEvent) {
    use yoagent::types::AgentEvent as E;
    match event {
        E::AgentStart => {
            app.is_streaming = true;
            app.working.start();
            app.pending_text = None;
            app.pending_thinking = None;
            app.last_streaming_event = std::time::Instant::now();
            app.refresh_git_branch();
        }
        E::TurnStart => {}
        E::MessageUpdate { delta, .. } => {
            app.last_streaming_event = std::time::Instant::now();
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
            app.last_streaming_event = std::time::Instant::now();
            flush_all(app);
            app.streaming_component = None;
            let name = tool_name;
            let renderer = app
                .extensions
                .iter()
                .find_map(|ext| ext.tool_renderer(&name));
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
        E::ToolExecutionEnd {
            tool_call_id,
            tool_name: _,
            result,
            is_error,
        } => {
            let content: String = result
                .content
                .iter()
                .filter_map(|c| {
                    if let yoagent::types::Content::Text { text } = c {
                        Some(text.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("");
            if let Some(weak) = app.pending_tools.get(&tool_call_id)
                && let Some(comp) = weak.upgrade()
            {
                comp.borrow_mut()
                    .set_result_with_details(&content, is_error, Some(result.details));
                if let Some(start) = app.tool_call_start_times.remove(&tool_call_id) {
                    comp.borrow_mut()
                        .set_final_duration(start.elapsed().as_secs_f64());
                }
            }
            // Crash-safe persistence: persist tool result immediately
            if let Some(ref mut s) = app.session {
                s.persist_tool_result(&tool_call_id, content.clone(), is_error);
            }
        }
        E::ProgressMessage {
            text, tool_name, ..
        } => {
            if let Some(weak) = app.bash_component.as_ref().and_then(|w| w.upgrade()) {
                weak.borrow_mut().append_output(&text);
            } else if tool_name.is_empty() {
                // General progress message (not tool-specific) — show as status
                app.status_text = Some(text.trim().to_string());
            }
        }
        E::TurnEnd { .. } => {
            flush_all(app);
            app.streaming_component = None;
        }
        E::AgentEnd { messages } => {
            flush_all(app);
            app.is_streaming = false;
            app.working.stop();
            app.footer.borrow_mut().set_streaming(false);
            // Update in-memory conversation with all messages from this turn.
            // (Messages are already persisted on message_end — on_agent_end is a
            //  safety net for any stragglers not covered by message_end.)
            for msg in &messages {
                app.conversation.push(msg.clone());
            }
            // Safety net: persist any remaining messages not captured on message_end
            if let Some(ref mut s) = app.session {
                s.on_agent_end(&messages);
            }
        }
        E::MessageEnd { message } => {
            // Pi-compatible: persist every message (user, assistant, toolResult) immediately
            // on message_end, not deferred to agent_end.
            if let Some(ref mut s) = app.session {
                s.persist_message_end(&message);
            }
            // Check for error messages from the provider (network errors, etc.)
            if let Some(err) = crate::agent::types::message_error(&message) {
                let error_text = if err.is_empty() {
                    "Provider error: The agent encountered an issue and stopped.".to_string()
                } else {
                    format!("Provider error: {}", err)
                };
                chat_add(
                    app,
                    std::boxed::Box::new(InfoMessageComponent::new(error_text.clone())),
                );
            }
        }
        E::InputRejected { reason } => {
            let msg = format!("Input rejected: {}", reason);
            chat_add(
                app,
                std::boxed::Box::new(InfoMessageComponent::new(msg.clone())),
            );
        }
        _ => {}
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
    }
}

fn flush_all(app: &mut App) {
    flush_text(app);
    flush_thinking(app);
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

#[cfg(test)]
mod tests {}

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

fn format_skill_invocation(skill: &yoagent::skills::Skill, extra: Option<&str>) -> String {
    let body = read_skill_body(&skill.file_path).unwrap_or_default();
    let base = skill.base_dir.to_string_lossy();
    let block = format!(
        r#"<skill name="{}" location="{}">
References are relative to {}.

{}
</skill>"#,
        xml_escape(&skill.name),
        xml_escape(&skill.file_path.to_string_lossy()),
        base,
        body
    );
    match extra {
        Some(instr) if !instr.is_empty() => format!("{}\n\n{}", block, instr),
        _ => block,
    }
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
        Some(s) => format_skill_invocation(s, if args.is_empty() { None } else { Some(args) }),
        None => text.to_string(),
    }
}
