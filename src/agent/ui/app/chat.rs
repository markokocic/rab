//! Chat rendering utilities — rebuilding chat, adding messages, bang commands, clipboard.
//!
//! Extracted from `mod.rs` to reduce file size.

use std::cell::RefCell;
use std::rc::Rc;

use crate::tui::Component;
use crate::tui::components::Spacer;

use super::App;
use super::command_handlers::find_tool_renderer;
use crate::agent::ui::components::InfoMessageComponent;

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

/// Show a status message in the chat (pi-style `showStatus`).
///
/// If the last two children of `chat_container` are from a previous status
/// (spacer + InfoMessageComponent), they are replaced in-place rather than
/// appending new entries. This prevents multiple consecutive status messages
/// from accumulating at the end of the chat session.
pub fn show_status(app: &mut App, message: impl Into<String>) {
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
pub fn extract_text_content(content: &[yoagent::types::Content]) -> String {
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

/// Handle ! and !! bang commands.
/// Renders via ToolExecComponent with the bash renderer (same visual treatment
/// as LLM-invoked bash tool calls, eliminating the separate BashExecution split).
pub fn handle_bang_command(app: &mut App, command: String) {
    use crate::agent::ui::components::RcToolExec;
    use crate::agent::ui::components::ToolExecComponent;
    use std::time::Instant;
    use tokio::sync::mpsc;
    use yoagent::types::{AgentEvent as YoEvent, Content as YoContent, ToolResult as YoResult};

    let cwd = app.cwd.clone();
    let tx = app.event_tx.clone();

    let renderer = find_tool_renderer(&app.extensions, "bash");
    let mut tool = ToolExecComponent::new(
        "bash",
        renderer,
        serde_json::json!({"command": command}),
        app.cwd.to_string_lossy().to_string(),
        "__bang__".to_string(),
    );
    tool.set_started_at(Instant::now());
    let (invalidate_tx, invalidate_rx) = ToolExecComponent::make_invalidation_channel();
    app.invalidate_rxs.push(invalidate_rx);
    tool.set_invalidate_tx(invalidate_tx);
    tool.set_expanded(app.tools_expanded);
    let tool = Rc::new(RefCell::new(tool));
    app.pending_tools
        .insert("__bang__".to_string(), Rc::downgrade(&tool));
    chat_add(app, std::boxed::Box::new(RcToolExec(tool)));
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

/// Try to copy text to the system clipboard using platform-specific tools.
/// Returns true if successful, false if no tool was available.
/// Falls back to OSC 52 escape sequence for remote sessions.
/// Mirrors pi's clipboard strategy exactly.
pub(crate) fn copy_to_clipboard(text: &str) -> bool {
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

/// Try to copy text to the system clipboard. Public wrapper.
pub fn copy_text_to_clipboard(text: &str) -> bool {
    copy_to_clipboard(text)
}
