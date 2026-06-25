pub mod agent_session;
pub mod compaction;
pub mod context_files;
pub mod extension;
pub mod r#loop;
pub mod provider;
pub mod session;
pub mod session_repo;
pub mod session_storage;
pub mod settings;
pub mod skills;
pub mod system_prompt;
pub mod types;
pub mod ui;

pub use agent_session::AgentSession;
pub use context_files::{ContextFile, load_context_files};
pub use extension::{AgentTool, CommandHandler, CommandResult, Extension, SlashCommand};
pub use r#loop::{
    AgentEvent, LoopConfig, PrepareNextTurnFn, ShouldStopFn, TransformFn, TurnUpdate,
    collect_tool_defs, run_agent_loop,
};
pub use provider::{Provider, StreamEvent, ToolDef};
pub use session::SessionManager;
pub use session_repo::{DefaultSessionRepo, SessionRepo};
pub use session_storage::{InMemorySessionStorage, JsonlSessionStorage, SessionStorage};
pub use settings::Settings;
pub use skills::{LoadSkillsOptions, Skill, format_skills_for_prompt, load_skills};
pub use system_prompt::{SystemPromptBuilder, ToolSnippet};
pub use types::{
    AgentMessage, PendingMessageQueue, QueueMode, Role, ToolCall, ToolExecutionMode, Usage,
};
