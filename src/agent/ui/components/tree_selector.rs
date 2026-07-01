//! TreeSelector component — matching pi's TreeSelectorComponent.
//!
//! Full-screen overlay for navigating the session tree: switching branches,
//! filtering, searching, folding, editing labels, and showing label timestamps.
//!
//! Uses callbacks (`on_select`, `on_cancel`) for signalling back to the app.
//! The component does NOT close itself — the app polls the result and acts.

use std::collections::HashMap;

use crate::agent::session::model::SessionTreeNode;
use crate::agent::types;
use crate::agent::ui::theme::current_theme;
use crate::tui::Component;
use crate::tui::focusable::{CURSOR_MARKER, Focusable};
use crate::tui::keybindings::{
    ACTION_EDITOR_CURSOR_LEFT, ACTION_EDITOR_CURSOR_RIGHT, ACTION_EDITOR_DELETE_CHAR_BACKWARD,
    ACTION_SELECT_CANCEL, ACTION_SELECT_CONFIRM, ACTION_SELECT_DOWN, ACTION_SELECT_UP,
    get_keybindings,
};
use crate::tui::util::{slice_by_column, truncate_to_width, visible_width, wrap_text_with_ansi};
use chrono::Datelike;
use crossterm::event::KeyEvent;
use yoagent::types::{AgentMessage, Content, Message};

// ── Filter mode ────────────────────────────────────────────────────

/// Filter mode for tree display (matching pi's FilterMode).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterMode {
    Default,
    NoTools,
    UserOnly,
    LabeledOnly,
    All,
}

impl FilterMode {
    fn cycle_forward(self) -> Self {
        match self {
            FilterMode::Default => FilterMode::NoTools,
            FilterMode::NoTools => FilterMode::UserOnly,
            FilterMode::UserOnly => FilterMode::LabeledOnly,
            FilterMode::LabeledOnly => FilterMode::All,
            FilterMode::All => FilterMode::Default,
        }
    }

    fn cycle_backward(self) -> Self {
        match self {
            FilterMode::Default => FilterMode::All,
            FilterMode::NoTools => FilterMode::Default,
            FilterMode::UserOnly => FilterMode::NoTools,
            FilterMode::LabeledOnly => FilterMode::UserOnly,
            FilterMode::All => FilterMode::LabeledOnly,
        }
    }

    fn label(self) -> &'static str {
        match self {
            FilterMode::Default => "",
            FilterMode::NoTools => " [no-tools]",
            FilterMode::UserOnly => " [user]",
            FilterMode::LabeledOnly => " [labeled]",
            FilterMode::All => " [all]",
        }
    }
}

// ── Tool call info for lookup ──────────────────────────────────────

/// Stored during flattening so tool results can show rich display.
struct ToolCallInfo {
    name: String,
    arguments: serde_json::Value,
}

// ── Internal types ──────────────────────────────────────────────────

/// Gutter info: position (displayIndent where connector was) and whether to show │.
#[derive(Debug, Clone, Copy)]
struct GutterInfo {
    position: usize,
    show: bool,
}

/// Flattened tree node for navigation.
#[derive(Clone)]
struct FlatNode {
    node: SessionTreeNode,
    indent: usize,
    show_connector: bool,
    is_last: bool,
    gutters: Vec<GutterInfo>,
    is_virtual_root_child: bool,
}

// ── TreeSelector component ──────────────────────────────────────────

pub struct TreeSelector {
    flat_nodes: Vec<FlatNode>,
    filtered_nodes: Vec<FlatNode>,
    selected_index: usize,
    current_leaf_id: Option<String>,
    max_visible_lines: usize,
    filter_mode: FilterMode,
    search_query: String,
    multiple_roots: bool,
    active_path_ids: std::collections::HashSet<String>,
    visible_parent_map: std::collections::HashMap<String, Option<String>>,
    visible_children_map: std::collections::HashMap<Option<String>, Vec<String>>,
    last_selected_id: Option<String>,
    folded_nodes: std::collections::HashSet<String>,
    label_input_active: bool,
    label_input_text: String,
    label_editing_entry_id: Option<String>,
    show_label_timestamps: bool,
    tool_call_map: HashMap<String, ToolCallInfo>,
    /// Focused state for IME cursor positioning.
    focused: bool,

    /// Called when user selects an entry.
    pub on_select: Option<Box<dyn FnMut(String)>>,
    /// Called when user cancels (presses Esc when search is empty).
    pub on_cancel: Option<Box<dyn FnMut()>>,
    /// Called when user changes a label.
    pub on_label_change: Option<BoxLabelChange>,
}

/// Type alias for label change callback.
pub type BoxLabelChange = Box<dyn FnMut(String, Option<String>)>;

impl TreeSelector {
    pub fn new(
        tree: Vec<SessionTreeNode>,
        current_leaf_id: Option<String>,
        terminal_height: usize,
        initial_filter_mode: Option<FilterMode>,
    ) -> Self {
        let max_visible_lines = (terminal_height.saturating_sub(8)).max(5);
        let multiple_roots = tree.len() > 1;
        let mut s = Self {
            flat_nodes: Vec::new(),
            filtered_nodes: Vec::new(),
            selected_index: 0,
            current_leaf_id: current_leaf_id.clone(),
            max_visible_lines,
            filter_mode: initial_filter_mode.unwrap_or(FilterMode::Default),
            search_query: String::new(),
            multiple_roots,
            active_path_ids: std::collections::HashSet::new(),
            visible_parent_map: std::collections::HashMap::new(),
            visible_children_map: std::collections::HashMap::new(),
            last_selected_id: None,
            folded_nodes: std::collections::HashSet::new(),
            label_input_active: false,
            label_input_text: String::new(),
            label_editing_entry_id: None,
            show_label_timestamps: false,
            tool_call_map: HashMap::new(),
            focused: false,
            on_select: None,
            on_cancel: None,
            on_label_change: None,
        };
        s.flat_nodes = s.flatten_tree(&tree);
        s.build_active_path();
        s.apply_filter();

        let target_id = current_leaf_id
            .clone()
            .or_else(|| tree.first().map(|n| n.entry.id().to_string()));
        s.selected_index = s.find_nearest_visible_index(target_id.as_deref());
        s.last_selected_id = s
            .filtered_nodes
            .get(s.selected_index)
            .map(|n| n.node.entry.id().to_string());

        s
    }

    /// Set the initial selection to a specific entry (used when re-opening after summarization prompt).
    /// Must be called after construction, before rendering.
    pub fn set_initial_selection(&mut self, entry_id: &str) {
        self.selected_index = self.find_nearest_visible_index(Some(entry_id));
        self.last_selected_id = self
            .filtered_nodes
            .get(self.selected_index)
            .map(|n| n.node.entry.id().to_string());
    }

    // ── Flatten tree ────────────────────────────────────────────

    fn flatten_tree(&self, roots: &[SessionTreeNode]) -> Vec<FlatNode> {
        let mut result: Vec<FlatNode> = Vec::new();
        let multiple_roots = roots.len() > 1;

        // Stack items
        struct StackEntry<'a> {
            node: &'a SessionTreeNode,
            indent: usize,
            just_branched: bool,
            show_connector: bool,
            is_last: bool,
            gutters: Vec<GutterInfo>,
            is_virtual_root_child: bool,
        }

        // Order roots: active branch first
        let ordered_roots = self.order_roots(roots);

        let mut stack: Vec<StackEntry> = Vec::new();
        for (i, node) in ordered_roots.iter().enumerate().rev() {
            let is_last = i == ordered_roots.len() - 1;
            stack.push(StackEntry {
                node,
                indent: if multiple_roots { 1 } else { 0 },
                just_branched: multiple_roots,
                show_connector: multiple_roots,
                is_last,
                gutters: Vec::new(),
                is_virtual_root_child: multiple_roots,
            });
        }

        while let Some(entry) = stack.pop() {
            // Extract tool calls from assistant messages for later lookup
            // (matching pi's flattenTree which builds toolCallMap)
            if let crate::agent::session::model::SessionEntry::Message(m) = &entry.node.entry
                && types::message_is_assistant(&m.message)
                && let AgentMessage::Llm(Message::Assistant { content, .. }) = &m.message
            {
                for c in content {
                    if let Content::ToolCall { .. } = c {
                        // Tool calls are extracted later in build_tool_call_map
                    }
                }
            }

            result.push(FlatNode {
                node: entry.node.clone(),
                indent: entry.indent,
                show_connector: entry.show_connector,
                is_last: entry.is_last,
                gutters: entry.gutters.clone(),
                is_virtual_root_child: entry.is_virtual_root_child,
            });

            let children = &entry.node.children;
            let multiple_children = children.len() > 1;

            // Order children: active branch first
            let ordered_children = self.order_child_nodes(children);

            let child_indent = if multiple_children || (entry.just_branched && entry.indent > 0) {
                entry.indent + 1
            } else {
                entry.indent
            };

            let connector_displayed = entry.show_connector && !entry.is_virtual_root_child;
            let display_indent = if multiple_roots {
                entry.indent.saturating_sub(1)
            } else {
                entry.indent
            };
            let connector_position = display_indent.saturating_sub(1);
            let mut child_gutters = entry.gutters.clone();
            if connector_displayed {
                child_gutters.push(GutterInfo {
                    position: connector_position,
                    show: !entry.is_last,
                });
            }

            for (i, child) in ordered_children.iter().enumerate().rev() {
                let child_is_last = i == ordered_children.len() - 1;
                stack.push(StackEntry {
                    node: child,
                    indent: child_indent,
                    just_branched: multiple_children,
                    show_connector: multiple_children,
                    is_last: child_is_last,
                    gutters: child_gutters.clone(),
                    is_virtual_root_child: false,
                });
            }
        }

        result
    }

    /// Build the tool call map by scanning all flat nodes.
    /// Called once during construction.
    fn build_tool_call_map(&mut self) {
        self.tool_call_map.clear();
        for flat in &self.flat_nodes {
            if let crate::agent::session::model::SessionEntry::Message(m) = &flat.node.entry
                && types::message_is_assistant(&m.message)
                && let AgentMessage::Llm(Message::Assistant { content, .. }) = &m.message
            {
                for c in content {
                    if let Content::ToolCall {
                        id,
                        name,
                        arguments,
                        ..
                    } = c
                    {
                        self.tool_call_map.insert(
                            id.clone(),
                            ToolCallInfo {
                                name: name.clone(),
                                arguments: arguments.clone(),
                            },
                        );
                    }
                }
            }
        }
    }

    fn node_contains_leaf(&self, node: &SessionTreeNode) -> bool {
        let Some(ref leaf) = self.current_leaf_id else {
            return false;
        };
        if node.entry.id() == leaf {
            return true;
        }
        for child in &node.children {
            if self.node_contains_leaf(child) {
                return true;
            }
        }
        false
    }

    fn order_roots<'a>(&self, roots: &'a [SessionTreeNode]) -> Vec<&'a SessionTreeNode> {
        let mut items: Vec<&SessionTreeNode> = roots.iter().collect();
        items.sort_by(|a, b| {
            let a_active = self.node_contains_leaf(a);
            let b_active = self.node_contains_leaf(b);
            b_active.cmp(&a_active)
        });
        items
    }

    fn order_child_nodes<'a>(&self, children: &'a [SessionTreeNode]) -> Vec<&'a SessionTreeNode> {
        let mut items: Vec<&SessionTreeNode> = children.iter().collect();
        items.sort_by(|a, b| {
            let a_active = self.node_contains_leaf(a);
            let b_active = self.node_contains_leaf(b);
            b_active.cmp(&a_active)
        });
        items
    }

    // ── Active path ─────────────────────────────────────────────

    fn build_active_path(&mut self) {
        let Some(ref leaf) = self.current_leaf_id else {
            return;
        };
        let parent_map: std::collections::HashMap<&str, &str> = self
            .flat_nodes
            .iter()
            .filter_map(|f| f.node.entry.parent_id().map(|p| (f.node.entry.id(), p)))
            .collect();
        let mut current: Option<&str> = Some(leaf);
        while let Some(id) = current {
            self.active_path_ids.insert(id.to_string());
            current = parent_map.get(id).copied();
        }
    }

    fn is_on_active_path(&self, id: &str) -> bool {
        self.active_path_ids.contains(id)
    }

    // ── Filter ─────────────────────────────────────────────────

    fn apply_filter(&mut self) {
        if !self.filtered_nodes.is_empty() {
            self.last_selected_id = self
                .filtered_nodes
                .get(self.selected_index)
                .map(|n| n.node.entry.id().to_string())
                .or_else(|| self.last_selected_id.take());
        }

        let search_query_lower = self.search_query.to_lowercase();
        let search_tokens: Vec<&str> = search_query_lower
            .split_whitespace()
            .filter(|s| !s.is_empty())
            .collect();

        // Build the tool call map once before filtering (so it's available for search)
        self.build_tool_call_map();

        self.filtered_nodes = self
            .flat_nodes
            .iter()
            .filter(|flat| {
                let entry = &flat.node.entry;
                let is_current_leaf = self
                    .current_leaf_id
                    .as_ref()
                    .is_some_and(|id| id == entry.id());

                // Skip assistant messages with only tool calls (no text), unless error/aborted
                if !is_current_leaf
                    && let crate::agent::session::model::SessionEntry::Message(m) = entry
                    && types::message_is_assistant(&m.message)
                {
                    let has_text = Self::message_has_text(&m.message);
                    let is_error_or_aborted = matches!(
                        &m.message,
                        AgentMessage::Llm(Message::Assistant {
                            stop_reason,
                            ..
                        }) if *stop_reason != yoagent::types::StopReason::Stop
                            && *stop_reason != yoagent::types::StopReason::ToolUse
                    );
                    if !has_text && !is_error_or_aborted {
                        return false;
                    }
                }

                // Apply filter mode
                let is_settings = matches!(
                    entry,
                    crate::agent::session::model::SessionEntry::Label(_)
                        | crate::agent::session::model::SessionEntry::Custom(_)
                        | crate::agent::session::model::SessionEntry::ModelChange(_)
                        | crate::agent::session::model::SessionEntry::ThinkingLevelChange(_)
                        | crate::agent::session::model::SessionEntry::SessionInfo(_)
                );

                let passes_filter = match self.filter_mode {
                    FilterMode::UserOnly => {
                        matches!(entry, crate::agent::session::model::SessionEntry::Message(m) if types::message_is_user(&m.message))
                    }
                    FilterMode::NoTools => {
                        !is_settings
                            && !matches!(
                                entry,
                                crate::agent::session::model::SessionEntry::Message(m) if types::message_is_tool_result(&m.message)
                            )
                    }
                    FilterMode::LabeledOnly => flat.node.label.is_some(),
                    FilterMode::All => true,
                    FilterMode::Default => !is_settings,
                };

                if !passes_filter {
                    return false;
                }

                // Search filter
                if !search_tokens.is_empty() {
                    let text = self.get_searchable_text(flat).to_lowercase();
                    return search_tokens.iter().all(|t| text.contains(t));
                }

                true
            })
            .cloned()
            .collect();

        // Filter out descendants of folded nodes
        if !self.folded_nodes.is_empty() {
            let mut skip = std::collections::HashSet::new();
            for flat in &self.flat_nodes {
                let id = flat.node.entry.id().to_string();
                let pid = flat.node.entry.parent_id().map(|s| s.to_string());
                if let Some(ref parent) = pid
                    && (self.folded_nodes.contains(parent) || skip.contains(parent))
                {
                    skip.insert(id);
                }
            }
            self.filtered_nodes
                .retain(|f| !skip.contains(f.node.entry.id()));
        }

        // Recalculate visual structure
        self.recalculate_visual_structure();

        if let Some(ref last) = self.last_selected_id {
            self.selected_index = self.find_nearest_visible_index(Some(last));
        } else if self.selected_index >= self.filtered_nodes.len() {
            self.selected_index = self.filtered_nodes.len().saturating_sub(1);
        }

        if !self.filtered_nodes.is_empty() {
            self.last_selected_id = self
                .filtered_nodes
                .get(self.selected_index)
                .map(|n| n.node.entry.id().to_string());
        }
    }

    fn message_has_text(msg: &AgentMessage) -> bool {
        match msg {
            AgentMessage::Llm(Message::Assistant { content, .. }) => content
                .iter()
                .any(|c| matches!(c, Content::Text { text } if !text.trim().is_empty())),
            _ => false,
        }
    }

    fn get_searchable_text(&self, flat: &FlatNode) -> String {
        let entry = &flat.node.entry;
        let mut parts = Vec::new();

        if let Some(ref label) = flat.node.label {
            parts.push(label.clone());
        }

        match entry {
            crate::agent::session::model::SessionEntry::Message(m) => {
                parts.push(match &m.message {
                    AgentMessage::Llm(msg) => match msg {
                        Message::User { content, .. } => {
                            format!("user: {}", types::content_text(content))
                        }
                        Message::Assistant { content, .. } => {
                            // Include tool call names in searchable text
                            let text = types::content_text(content);
                            let tool_names: Vec<&str> = content
                                .iter()
                                .filter_map(|c| {
                                    if let Content::ToolCall { name, .. } = c {
                                        Some(name.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if tool_names.is_empty() {
                                format!("assistant: {}", text)
                            } else {
                                format!("assistant: {} tools: {}", text, tool_names.join(" "))
                            }
                        }
                        Message::ToolResult {
                            tool_name,
                            tool_call_id,
                            content,
                            ..
                        } => {
                            // Include tool call info from map for search
                            let call_info = self.tool_call_map.get(tool_call_id);
                            let args_text = call_info
                                .map(|info| info.arguments.to_string())
                                .unwrap_or_default();
                            format!(
                                "toolResult: {} {} {}",
                                tool_name,
                                types::content_text(content),
                                args_text
                            )
                        }
                    },
                    AgentMessage::Extension(ext) => ext.data.to_string(),
                });
            }
            crate::agent::session::model::SessionEntry::Compaction(c) => {
                parts.push(format!("compaction {}", c.tokens_before));
            }
            crate::agent::session::model::SessionEntry::BranchSummary(b) => {
                parts.push(format!("branch summary {}", b.summary));
            }
            crate::agent::session::model::SessionEntry::SessionInfo(s) => {
                parts.push("title".to_string());
                if !s.name.is_empty() {
                    parts.push(s.name.clone());
                }
            }
            crate::agent::session::model::SessionEntry::ModelChange(m) => {
                parts.push(format!("model {}", m.model_id));
            }
            crate::agent::session::model::SessionEntry::ThinkingLevelChange(t) => {
                parts.push(format!("thinking {}", t.thinking_level));
            }
            crate::agent::session::model::SessionEntry::Custom(c) => {
                parts.push(format!("custom {}", c.custom_type));
            }
            crate::agent::session::model::SessionEntry::Label(l) => {
                if let Some(ref label) = l.label {
                    parts.push(format!("label {}", label));
                }
            }
            crate::agent::session::model::SessionEntry::CustomMessage(cm) => {
                parts.push(format!("custom_message {}", cm.custom_type));
            }
            crate::agent::session::model::SessionEntry::ActiveToolsChange(a) => {
                parts.push(format!("tools {}", a.active_tool_names.join(", ")));
            }
            crate::agent::session::model::SessionEntry::Leaf(_) => {}
        }

        parts.join(" ")
    }

    // ── Visual structure recalculation ─────────────────────────

    fn recalculate_visual_structure(&mut self) {
        if self.filtered_nodes.is_empty() {
            return;
        }

        let visible_ids: std::collections::HashSet<&str> = self
            .filtered_nodes
            .iter()
            .map(|n| n.node.entry.id())
            .collect();

        let entry_map: std::collections::HashMap<&str, &FlatNode> = self
            .flat_nodes
            .iter()
            .map(|f| (f.node.entry.id(), f))
            .collect();

        let find_visible_ancestor = |node_id: &str| -> Option<String> {
            let entry = entry_map.get(node_id)?;
            let mut current = entry.node.entry.parent_id()?.to_string();
            loop {
                if visible_ids.contains(current.as_str()) {
                    return Some(current);
                }
                let node = entry_map.get(current.as_str())?;
                current = node.node.entry.parent_id()?.to_string();
            }
        };

        self.visible_parent_map.clear();
        self.visible_children_map.clear();
        self.visible_children_map.insert(None, Vec::new());

        for flat in &self.filtered_nodes {
            let id = flat.node.entry.id().to_string();
            let ancestor = find_visible_ancestor(&id);
            self.visible_parent_map.insert(id.clone(), ancestor.clone());
            let key = ancestor.or(None);
            self.visible_children_map.entry(key).or_default().push(id);
        }

        let visible_root_ids = self
            .visible_children_map
            .get(&None)
            .cloned()
            .unwrap_or_default();
        self.multiple_roots = visible_root_ids.len() > 1;

        struct VisStackEntry {
            node_id: String,
            indent: usize,
            just_branched: bool,
            show_connector: bool,
            is_last: bool,
            gutters: Vec<GutterInfo>,
            is_virtual_root_child: bool,
        }

        let mut stack: Vec<VisStackEntry> = Vec::new();

        for (i, root_id) in visible_root_ids.iter().enumerate().rev() {
            let is_last = i == visible_root_ids.len() - 1;
            stack.push(VisStackEntry {
                node_id: root_id.clone(),
                indent: if self.multiple_roots { 1 } else { 0 },
                just_branched: self.multiple_roots,
                show_connector: self.multiple_roots,
                is_last,
                gutters: Vec::new(),
                is_virtual_root_child: self.multiple_roots,
            });
        }

        while let Some(entry) = stack.pop() {
            if let Some(pos) = self
                .filtered_nodes
                .iter()
                .position(|f| f.node.entry.id() == entry.node_id)
            {
                let flat = &mut self.filtered_nodes[pos];
                flat.indent = entry.indent;
                flat.show_connector = entry.show_connector;
                flat.is_last = entry.is_last;
                flat.gutters = entry.gutters.clone();
                flat.is_virtual_root_child = entry.is_virtual_root_child;
            }

            let children = self
                .visible_children_map
                .get(&Some(entry.node_id.clone()))
                .cloned()
                .unwrap_or_default();
            let multiple_children = children.len() > 1;

            let child_indent = if multiple_children || (entry.just_branched && entry.indent > 0) {
                entry.indent + 1
            } else {
                entry.indent
            };

            let connector_displayed = entry.show_connector && !entry.is_virtual_root_child;
            let display_indent = if self.multiple_roots {
                entry.indent.saturating_sub(1)
            } else {
                entry.indent
            };
            let connector_position = display_indent.saturating_sub(1);
            let mut child_gutters = entry.gutters.clone();
            if connector_displayed {
                child_gutters.push(GutterInfo {
                    position: connector_position,
                    show: !entry.is_last,
                });
            }

            for (i, child_id) in children.iter().enumerate().rev() {
                let child_is_last = i == children.len() - 1;
                stack.push(VisStackEntry {
                    node_id: child_id.clone(),
                    indent: child_indent,
                    just_branched: multiple_children,
                    show_connector: multiple_children,
                    is_last: child_is_last,
                    gutters: child_gutters.clone(),
                    is_virtual_root_child: false,
                });
            }
        }
    }

    // ── Navigation helpers ─────────────────────────────────────

    fn find_nearest_visible_index(&self, entry_id: Option<&str>) -> usize {
        if self.filtered_nodes.is_empty() || entry_id.is_none() {
            return 0;
        }
        let id = entry_id.unwrap();

        let visible_id_to_index: std::collections::HashMap<&str, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .map(|(i, f)| (f.node.entry.id(), i))
            .collect();

        if let Some(&idx) = visible_id_to_index.get(id) {
            return idx;
        }

        let entry_map: std::collections::HashMap<&str, &FlatNode> = self
            .flat_nodes
            .iter()
            .map(|f| (f.node.entry.id(), f))
            .collect();
        let mut current: Option<&str> = entry_map.get(id).and_then(|n| n.node.entry.parent_id());
        while let Some(cid) = current {
            if let Some(&idx) = visible_id_to_index.get(cid) {
                return idx;
            }
            current = entry_map.get(cid).and_then(|n| n.node.entry.parent_id());
        }

        self.filtered_nodes.len().saturating_sub(1)
    }

    fn is_foldable(&self, entry_id: &str) -> bool {
        let children = self.visible_children_map.get(&Some(entry_id.to_string()));
        if children.is_none_or(|c| c.is_empty()) {
            return false;
        }
        let parent = self.visible_parent_map.get(entry_id);
        match parent {
            None | Some(None) => true,
            Some(Some(pid)) => self
                .visible_children_map
                .get(&Some(pid.clone()))
                .is_some_and(|s| s.len() > 1),
        }
    }

    fn find_branch_segment_start(&self, direction: &str) -> usize {
        let selected_id = self
            .filtered_nodes
            .get(self.selected_index)
            .map(|n| n.node.entry.id().to_string());
        let Some(ref sid) = selected_id else {
            return self.selected_index;
        };

        let index_by_id: std::collections::HashMap<&str, usize> = self
            .filtered_nodes
            .iter()
            .enumerate()
            .map(|(i, f)| (f.node.entry.id(), i))
            .collect();

        let mut current: String = sid.to_string();

        if direction == "down" {
            loop {
                let children = self
                    .visible_children_map
                    .get(&Some(current.clone()))
                    .cloned()
                    .unwrap_or_default();
                if children.is_empty() {
                    return *index_by_id
                        .get(current.as_str())
                        .unwrap_or(&self.selected_index);
                }
                if children.len() > 1 {
                    return *index_by_id
                        .get(children[0].as_str())
                        .unwrap_or(&self.selected_index);
                }
                current = children[0].clone();
            }
        }

        // direction == "up"
        loop {
            let parent = self.visible_parent_map.get(current.as_str());
            let parent_id: Option<&str> = match parent {
                Some(None) | None => break,
                Some(Some(pid)) => Some(pid.as_str()),
            };
            if let Some(pid) = parent_id {
                let children = self
                    .visible_children_map
                    .get(&Some(pid.to_string()))
                    .cloned()
                    .unwrap_or_default();
                if children.len() > 1
                    && let Some(&idx) = index_by_id.get(current.as_str())
                    && idx < self.selected_index
                {
                    return idx;
                }
                current = pid.to_string();
            } else {
                break;
            }
        }

        *index_by_id
            .get(current.as_str())
            .unwrap_or(&self.selected_index)
    }

    // ── Entry display text ─────────────────────────────────────

    fn format_tool_call(&self, name: &str, args: &serde_json::Value) -> String {
        let shorten_path = |p: &str| -> String {
            if let Some(home) = std::env::var_os("HOME").and_then(|h| h.into_string().ok())
                && let Some(rest) = p.strip_prefix(&home)
            {
                format!("~{}", rest)
            } else {
                p.to_string()
            }
        };

        match name {
            "read" => {
                let path = shorten_path(
                    args.get("path")
                        .or_else(|| args.get("file_path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                );
                let offset = args.get("offset").and_then(|v| v.as_u64());
                let limit = args.get("limit").and_then(|v| v.as_u64());
                let display = match (offset, limit) {
                    (Some(o), Some(l)) => format!("{}:{}-{}", path, o, o + l - 1),
                    (Some(o), None) => format!("{}:{}", path, o),
                    _ => path,
                };
                format!("[read: {}]", display)
            }
            "write" => {
                let path = shorten_path(
                    args.get("path")
                        .or_else(|| args.get("file_path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                );
                format!("[write: {}]", path)
            }
            "edit" => {
                let path = shorten_path(
                    args.get("path")
                        .or_else(|| args.get("file_path"))
                        .and_then(|v| v.as_str())
                        .unwrap_or(""),
                );
                format!("[edit: {}]", path)
            }
            "bash" => {
                let raw_cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                let cmd = raw_cmd.replace(['\n', '\t'], " ").trim().to_string();
                let truncated: String = cmd.chars().take(50).collect();
                if cmd.len() > 50 {
                    format!("[bash: {}...]", truncated)
                } else {
                    format!("[bash: {}]", truncated)
                }
            }
            "grep" => {
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                format!("[grep: /{}/ in {}]", pattern, shorten_path(path))
            }
            "find" => {
                let pattern = args.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                format!("[find: {} in {}]", pattern, shorten_path(path))
            }
            "ls" => {
                let path = args.get("path").and_then(|v| v.as_str()).unwrap_or(".");
                format!("[ls: {}]", shorten_path(path))
            }
            _ => {
                // Custom tool — show name and truncated JSON args
                let args_str = args.to_string();
                let truncated: String = args_str.chars().take(40).collect();
                if args_str.len() > 40 {
                    format!("[{}: {}...]", name, truncated)
                } else {
                    format!("[{}: {}]", name, truncated)
                }
            }
        }
    }

    fn get_entry_display_text(&self, node: &SessionTreeNode, is_selected: bool) -> String {
        let theme = current_theme();
        let entry = &node.entry;

        let result = match entry {
            crate::agent::session::model::SessionEntry::Message(m) => {
                match &m.message {
                    AgentMessage::Llm(msg) => match msg {
                        Message::User { content, .. } => {
                            let text = types::content_text(content);
                            let truncated = self.truncate_display_text(&text);
                            format!(
                                "{}{}",
                                theme.fg("accent", "user: "),
                                truncated.replace('\n', " ").trim()
                            )
                        }
                        Message::Assistant {
                            content,
                            stop_reason,
                            error_message,
                            ..
                        } => {
                            let text = types::content_text(content);
                            let text_clean = self
                                .truncate_display_text(&text)
                                .replace('\n', " ")
                                .trim()
                                .to_string();
                            if !text_clean.is_empty() {
                                format!("{}{}", theme.fg("success", "assistant: "), text_clean)
                            } else if let Some(err) = error_message {
                                let err_display: String = err.chars().take(80).collect();
                                format!(
                                    "{}{}",
                                    theme.fg("success", "assistant: "),
                                    theme.fg("error", &err_display)
                                )
                            } else if *stop_reason == yoagent::types::StopReason::Aborted {
                                format!(
                                    "{}{}",
                                    theme.fg("success", "assistant: "),
                                    theme.fg("muted", "(aborted)")
                                )
                            } else {
                                format!(
                                    "{}{}",
                                    theme.fg("success", "assistant: "),
                                    theme.fg("muted", "(no content)")
                                )
                            }
                        }
                        Message::ToolResult {
                            tool_name,
                            tool_call_id,
                            ..
                        } => {
                            // Look up the original tool call for a rich display
                            let display = self
                                .tool_call_map
                                .get(tool_call_id)
                                .map(|info| self.format_tool_call(&info.name, &info.arguments))
                                .unwrap_or_else(|| format!("[{}]", tool_name));
                            theme.fg("muted", &display)
                        }
                    },
                    AgentMessage::Extension(ext) => {
                        format!("{}[extension: {}]", theme.fg("dim", ""), ext.data)
                    }
                }
            }
            crate::agent::session::model::SessionEntry::Compaction(c) => {
                let tokens = c.tokens_before / 1000;
                format!(
                    "{}[compaction: {}k tokens]",
                    theme.fg("borderAccent", ""),
                    tokens
                )
            }
            crate::agent::session::model::SessionEntry::BranchSummary(b) => {
                let text = b.summary.replace('\n', " ").trim().to_string();
                let truncated = self.truncate_display_text(&text);
                format!("{}{}", theme.fg("warning", "[branch summary]: "), truncated)
            }
            crate::agent::session::model::SessionEntry::ModelChange(m) => {
                format!("{}[model: {}]", theme.fg("dim", ""), m.model_id)
            }
            crate::agent::session::model::SessionEntry::ThinkingLevelChange(t) => {
                format!("{}[thinking: {}]", theme.fg("dim", ""), t.thinking_level)
            }
            crate::agent::session::model::SessionEntry::Custom(c) => {
                format!("{}[custom: {}]", theme.fg("dim", ""), c.custom_type)
            }
            crate::agent::session::model::SessionEntry::Label(l) => {
                let label_text = l.label.as_deref().unwrap_or("(cleared)");
                format!("{}[label: {}]", theme.fg("dim", ""), label_text)
            }
            crate::agent::session::model::SessionEntry::SessionInfo(s) => {
                if s.name.is_empty() {
                    format!(
                        "{}[title: {}]",
                        theme.fg("dim", ""),
                        theme.italic(&theme.fg("dim", "empty"))
                    )
                } else {
                    format!("{}[title: {}]", theme.fg("dim", ""), &s.name)
                }
            }
            crate::agent::session::model::SessionEntry::CustomMessage(cm) => {
                // Extract text from content JSON (pi-style: content.text or content array)
                let text = cm
                    .content
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let truncated = self.truncate_display_text(text.trim());
                format!(
                    "{}{}: {}",
                    theme.fg("customMessageLabel", ""),
                    cm.custom_type,
                    truncated
                )
            }
            crate::agent::session::model::SessionEntry::Leaf(_) => String::new(),
            crate::agent::session::model::SessionEntry::ActiveToolsChange(a) => {
                format!(
                    "{}[tools: {}]",
                    theme.fg("dim", ""),
                    a.active_tool_names.join(", ")
                )
            }
        };

        if is_selected {
            theme.bold(&result)
        } else {
            result
        }
    }

    /// Truncate display text to max N chars (matching pi's extractContent 200-char limit).
    fn truncate_display_text(&self, text: &str) -> String {
        const MAX_LEN: usize = 200;
        text.chars().take(MAX_LEN).collect()
    }

    // ── Format label timestamp (matching pi's formatLabelTimestamp) ──

    fn format_label_timestamp(&self, timestamp: &str) -> String {
        // Try RFC 3339 first, then other common formats
        let date = if let Ok(d) = chrono::DateTime::parse_from_rfc3339(timestamp) {
            d
        } else if let Ok(d) = chrono::DateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.fZ") {
            d
        } else if let Ok(d) = chrono::DateTime::parse_from_str(timestamp, "%Y-%m-%dT%H:%M:%S%.f%z")
        {
            d
        } else {
            return String::new();
        };
        let now = chrono::Utc::now();
        let time = date.format("%H:%M").to_string();

        if date.year() == now.year() && date.month() == now.month() && date.day() == now.day() {
            return time;
        }

        let month = date.month();
        let day = date.day();
        if date.year() == now.year() {
            return format!("{}/{} {}", month, day, time);
        }

        let year = date.year() % 100;
        format!("{}/{}/{} {}", year, month, day, time)
    }

    // ── Input handling ────────────────────────────────────────

    pub fn handle_key(&mut self, key: &KeyEvent) -> bool {
        if self.label_input_active {
            return self.handle_label_input(key);
        }

        let kb = get_keybindings();

        if kb.matches(key, ACTION_SELECT_UP) {
            if self.filtered_nodes.is_empty() {
                return true;
            }
            self.selected_index = if self.selected_index == 0 {
                self.filtered_nodes.len() - 1
            } else {
                self.selected_index - 1
            };
            return true;
        }

        if kb.matches(key, ACTION_SELECT_DOWN) {
            if self.filtered_nodes.is_empty() {
                return true;
            }
            self.selected_index = if self.selected_index >= self.filtered_nodes.len() - 1 {
                0
            } else {
                self.selected_index + 1
            };
            return true;
        }

        // Fold with '[' (up direction), unfold with ']' (down direction)
        if key.code == crossterm::event::KeyCode::Char('[') && key.modifiers.is_empty() {
            let current_id = self
                .filtered_nodes
                .get(self.selected_index)
                .map(|n| n.node.entry.id());
            if let Some(id) = current_id
                && self.is_foldable(id)
                && !self.folded_nodes.contains(id)
            {
                self.folded_nodes.insert(id.to_string());
                self.apply_filter();
                return true;
            }
            self.selected_index = self.find_branch_segment_start("up");
            return true;
        }

        if key.code == crossterm::event::KeyCode::Char(']') && key.modifiers.is_empty() {
            let current_id = self
                .filtered_nodes
                .get(self.selected_index)
                .map(|n| n.node.entry.id());
            if let Some(id) = current_id
                && self.folded_nodes.contains(id)
            {
                self.folded_nodes.remove(id);
                self.apply_filter();
                return true;
            }
            self.selected_index = self.find_branch_segment_start("down");
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_LEFT) {
            self.selected_index = self.selected_index.saturating_sub(self.max_visible_lines);
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_CURSOR_RIGHT) {
            self.selected_index = self
                .filtered_nodes
                .len()
                .saturating_sub(1)
                .min(self.selected_index + self.max_visible_lines);
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            if let Some(flat) = self.filtered_nodes.get(self.selected_index) {
                let id = flat.node.entry.id().to_string();
                if let Some(ref mut cb) = self.on_select {
                    cb(id);
                }
            }
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.folded_nodes.clear();
                self.apply_filter();
            } else if let Some(ref mut cb) = self.on_cancel {
                cb();
            }
            return true;
        }

        // Filter shortcuts: 1..5 toggle between mode and default
        if key.code == crossterm::event::KeyCode::Char('1') && key.modifiers.is_empty() {
            self.filter_mode = if self.filter_mode == FilterMode::NoTools {
                FilterMode::Default
            } else {
                FilterMode::NoTools
            };
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }
        if key.code == crossterm::event::KeyCode::Char('2') && key.modifiers.is_empty() {
            self.filter_mode = if self.filter_mode == FilterMode::UserOnly {
                FilterMode::Default
            } else {
                FilterMode::UserOnly
            };
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }
        if key.code == crossterm::event::KeyCode::Char('3') && key.modifiers.is_empty() {
            self.filter_mode = if self.filter_mode == FilterMode::LabeledOnly {
                FilterMode::Default
            } else {
                FilterMode::LabeledOnly
            };
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }
        if key.code == crossterm::event::KeyCode::Char('4') && key.modifiers.is_empty() {
            self.filter_mode = if self.filter_mode == FilterMode::All {
                FilterMode::Default
            } else {
                FilterMode::All
            };
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }
        if key.code == crossterm::event::KeyCode::Char('5') && key.modifiers.is_empty() {
            self.filter_mode = FilterMode::Default;
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }

        // Cycle filters with Tab/Shift+Tab
        if key.code == crossterm::event::KeyCode::Tab {
            let old_mode = self.filter_mode;
            self.filter_mode = self.filter_mode.cycle_forward();
            if self.filter_mode != old_mode {
                self.folded_nodes.clear();
                self.apply_filter();
            }
            return true;
        }
        if key.code == crossterm::event::KeyCode::BackTab {
            let old_mode = self.filter_mode;
            self.filter_mode = self.filter_mode.cycle_backward();
            if self.filter_mode != old_mode {
                self.folded_nodes.clear();
                self.apply_filter();
            }
            return true;
        }

        // Label editing with 'l'
        if key.code == crossterm::event::KeyCode::Char('l') && key.modifiers.is_empty() {
            if let Some(flat) = self.filtered_nodes.get(self.selected_index) {
                let id = flat.node.entry.id().to_string();
                let label = flat.node.label.clone();
                self.start_label_edit(id, label);
            }
            return true;
        }

        // Toggle label timestamp display with 't' (matching pi's app.tree.toggleLabelTimestamp)
        if key.code == crossterm::event::KeyCode::Char('t') && key.modifiers.is_empty() {
            self.show_label_timestamps = !self.show_label_timestamps;
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            if !self.search_query.is_empty() {
                self.search_query.pop();
                self.folded_nodes.clear();
                self.apply_filter();
            }
            return true;
        }

        if let crossterm::event::KeyCode::Char(c) = key.code
            && !c.is_control()
            && !key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
            && !key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
            && !key.modifiers.contains(crossterm::event::KeyModifiers::META)
        {
            self.search_query.push(c);
            self.folded_nodes.clear();
            self.apply_filter();
            return true;
        }

        false
    }

    // ── Label editing ─────────────────────────────────────────

    fn start_label_edit(&mut self, entry_id: String, current_label: Option<String>) {
        self.label_input_active = true;
        self.label_input_text = current_label.unwrap_or_default();
        self.label_editing_entry_id = Some(entry_id);
    }

    fn handle_label_input(&mut self, key: &KeyEvent) -> bool {
        let kb = get_keybindings();

        if kb.matches(key, ACTION_SELECT_CONFIRM) {
            if let Some(ref id) = self.label_editing_entry_id {
                let label = if self.label_input_text.trim().is_empty() {
                    None
                } else {
                    Some(self.label_input_text.trim().to_string())
                };
                for flat in &mut self.flat_nodes {
                    if flat.node.entry.id() == id {
                        flat.node.label = label.clone();
                        break;
                    }
                }
                if let Some(ref mut cb) = self.on_label_change {
                    cb(id.clone(), label);
                }
                self.apply_filter();
            }
            self.label_input_active = false;
            self.label_input_text.clear();
            self.label_editing_entry_id = None;
            return true;
        }

        if kb.matches(key, ACTION_SELECT_CANCEL) {
            self.label_input_active = false;
            self.label_input_text.clear();
            self.label_editing_entry_id = None;
            return true;
        }

        if kb.matches(key, ACTION_EDITOR_DELETE_CHAR_BACKWARD) {
            self.label_input_text.pop();
            return true;
        }

        if let crossterm::event::KeyCode::Char(c) = key.code
            && !c.is_control()
        {
            self.label_input_text.push(c);
            return true;
        }

        false
    }
}

impl Component for TreeSelector {
    fn render(&mut self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let mut lines: Vec<String> = Vec::new();

        lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));
        lines.push(String::new());

        lines.push(format!("  {}", theme.bold("Session Tree")));
        lines.push(String::new());

        // Label input mode
        if self.label_input_active {
            lines.push(format!(
                "  {}",
                theme.fg("muted", "Label (empty to remove):")
            ));
            let label_display = if self.focused {
                format!("  {}{}", self.label_input_text, CURSOR_MARKER)
            } else {
                format!("  {}", self.label_input_text)
            };
            lines.push(label_display);
            lines.push(format!(
                "  {}",
                theme.fg("muted", "Enter: save \u{00b7} Esc: cancel")
            ));
            lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));
            return lines;
        }

        // Help
        lines.extend(self.render_help(width));

        // Search line
        let search_display = if self.search_query.is_empty() {
            theme.fg("muted", "Type to search:")
        } else {
            format!(
                "{} {}",
                theme.fg("muted", "Search:"),
                theme.fg("accent", &self.search_query)
            )
        };
        lines.push(format!("  {}", search_display));
        lines.push(String::new());

        if self.filtered_nodes.is_empty() {
            lines.push(format!(
                "  {}",
                theme.fg(
                    "muted",
                    &format!("No entries found  (0/0){}", self.filter_mode.label())
                )
            ));
            lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));
            return lines;
        }

        let count = self.filtered_nodes.len();
        let start = self
            .selected_index
            .saturating_sub(self.max_visible_lines / 2)
            .min(count.saturating_sub(self.max_visible_lines));
        let end = (start + self.max_visible_lines).min(count);

        // ── Horizontal viewport scrolling (matching pi's renderHorizontalViewport) ──
        //
        // Collect rendered rows with anchor positions, compute horizontal scroll
        // from the selected row's anchor, then clip each body with slice_by_column.

        struct Row {
            gutter: String,
            body: String,
            anchor_col: usize,
            body_width: usize,
            is_selected: bool,
        }

        let mut rows: Vec<Row> = Vec::new();

        for i in start..end {
            let flat = &self.filtered_nodes[i];
            let is_selected = i == self.selected_index;

            let cursor = if is_selected {
                theme.fg("accent", "\u{203a} ")
            } else {
                "  ".to_string()
            };

            let prefix = self.render_tree_prefix(flat);

            let is_on_path = self.is_on_active_path(flat.node.entry.id());
            let path_marker = if is_on_path {
                theme.accent("\u{2022} ")
            } else {
                String::new()
            };

            let is_folded = self.folded_nodes.contains(flat.node.entry.id());
            let shows_fold_in_connector = flat.show_connector && !flat.is_virtual_root_child;
            let fold_marker = if is_folded && !shows_fold_in_connector {
                theme.accent("\u{229e} ")
            } else {
                String::new()
            };

            // Label badge with optional timestamp (matching pi's showLabelTimestamps)
            let label_badge = match &flat.node.label {
                Some(l) => {
                    let mut badge = format!("[{}]", l);
                    if self.show_label_timestamps
                        && let Some(ref ts) = flat.node.label_timestamp
                    {
                        let formatted = self.format_label_timestamp(ts);
                        if !formatted.is_empty() {
                            badge = format!("[{}] {} ", l, formatted);
                        }
                    }
                    theme.fg("warning", &format!("{} ", badge))
                }
                None => String::new(),
            };

            let content = self.get_entry_display_text(&flat.node, is_selected);

            // Body = everything after the cursor gutter
            let body = if label_badge.is_empty() {
                format!("{}{}{}{}", prefix, fold_marker, path_marker, content)
            } else {
                format!("{}{}{}{}", prefix, label_badge, path_marker, content)
            };

            let body_width = visible_width(&body);
            // Anchor column = visible width of tree prefix (where content text starts)
            let anchor_col = visible_width(&prefix);

            rows.push(Row {
                gutter: cursor.clone(),
                body,
                anchor_col,
                body_width,
                is_selected,
            });
        }

        // Calculate horizontal scroll based on selected row's anchor (pi-style)
        const TREE_GUTTER_WIDTH: usize = 2; // cursor column
        let viewport_width = width.saturating_sub(TREE_GUTTER_WIDTH);
        let max_body_width = rows.iter().map(|r| r.body_width).max().unwrap_or(0);
        let max_horizontal_scroll = max_body_width.saturating_sub(viewport_width);

        let mut horizontal_scroll: usize = 0;
        if max_horizontal_scroll > 0
            && let Some(selected) = rows.iter().find(|r| r.is_selected)
        {
            let min_visible_anchor_content = (viewport_width / 3).clamp(4, 20);
            if selected.anchor_col > viewport_width.saturating_sub(min_visible_anchor_content) {
                let anchor_context = (viewport_width / 4).clamp(2, 12);
                horizontal_scroll =
                    max_horizontal_scroll.min(selected.anchor_col.saturating_sub(anchor_context));
            }
        }

        // Render rows with horizontal scroll applied
        for row in rows {
            let body = if horizontal_scroll > 0 {
                slice_by_column(&row.body, horizontal_scroll, viewport_width)
            } else {
                row.body
            };

            let mut line = if row.is_selected {
                format!(
                    "{}{}",
                    theme.bg("selectedBg", &row.gutter),
                    theme.bg("selectedBg", &body)
                )
            } else {
                format!("{}{}", row.gutter, body)
            };

            line = truncate_to_width(&line, width, "", false);
            lines.push(line);
        }

        // Status line (matching pi: includes filter badge and label timestamp indicator)
        let mut status = format!(
            "  ({}/{}){}",
            self.selected_index + 1,
            self.filtered_nodes.len(),
            self.filter_mode.label()
        );
        if self.show_label_timestamps {
            status.push_str(" [+label time]");
        }
        lines.push(theme.fg("muted", &status));

        lines.push(theme.fg("muted", &"─".repeat(width.saturating_sub(2))));

        lines
    }

    fn handle_input(&mut self, key: &KeyEvent) -> bool {
        self.handle_key(key)
    }

    fn invalidate(&mut self) {}
}

impl Focusable for TreeSelector {
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn focused(&self) -> bool {
        self.focused
    }
}

// ── Rendering helpers ──────────────────────────────────────────

impl TreeSelector {
    fn render_tree_prefix(&self, flat: &FlatNode) -> String {
        let theme = current_theme();
        let display_indent = if self.multiple_roots {
            flat.indent.saturating_sub(1)
        } else {
            flat.indent
        };

        let connector = if flat.show_connector && !flat.is_virtual_root_child {
            if flat.is_last {
                "\u{2514}\u{2500} "
            } else {
                "\u{251c}\u{2500} "
            }
        } else {
            ""
        };
        let connector_position = if !connector.is_empty() {
            display_indent as isize - 1
        } else {
            -1
        };

        let total_chars = display_indent * 3;
        let mut prefix_chars: Vec<char> = Vec::with_capacity(total_chars);

        for i in 0..total_chars {
            let level = i / 3;
            let pos_in_level = i % 3;

            let gutter = flat.gutters.iter().find(|g| g.position == level);
            if let Some(g) = gutter {
                if pos_in_level == 0 {
                    prefix_chars.push(if g.show { '\u{2502}' } else { ' ' });
                } else {
                    prefix_chars.push(' ');
                }
            } else if !connector.is_empty() && level == connector_position as usize {
                if pos_in_level == 0 {
                    prefix_chars.push(if flat.is_last { '\u{2514}' } else { '\u{251c}' });
                } else if pos_in_level == 1 {
                    let is_folded = self.folded_nodes.contains(flat.node.entry.id());
                    let foldable = self.is_foldable(flat.node.entry.id());
                    prefix_chars.push(if is_folded {
                        '\u{229e}'
                    } else if foldable {
                        '\u{229f}'
                    } else {
                        '\u{2500}'
                    });
                } else {
                    prefix_chars.push(' ');
                }
            } else {
                prefix_chars.push(' ');
            }
        }

        let prefix: String = prefix_chars.into_iter().collect();
        theme.fg("dim", &prefix)
    }

    fn render_help(&self, width: usize) -> Vec<String> {
        let theme = current_theme();
        let items = [
            ("\u{2191}/\u{2193}", "move"),
            ("[/]", "branch"),
            ("\u{2190}/\u{2192}", "page"),
            ("l", "label"),
            ("t", "label time"),
            ("1-5", "filter"),
            ("Tab", "cycle"),
            ("Enter", "select"),
            ("Esc", "cancel"),
        ];
        let line: String = items
            .iter()
            .map(|(key, label)| format!("{} {} ", key, label))
            .collect::<Vec<_>>()
            .join("\u{00b7} ");
        let wrapped = wrap_text_with_ansi(&line, width.saturating_sub(4));
        wrapped
            .into_iter()
            .map(|l| theme.fg("muted", &format!("  {}", l)))
            .collect()
    }
}
