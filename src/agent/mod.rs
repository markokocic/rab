pub mod agent_session;
pub mod branch_summary;
pub mod compaction;
pub mod context_files;
pub mod extension;
pub mod footer_data_provider;
pub mod session;
pub mod session_repo;
pub mod session_storage;
pub mod settings;
pub mod system_prompt;
pub mod types;
pub mod ui;

pub use agent_session::{AgentSession, CompactionEvent, CompactionEventCallback};
pub use context_files::{ContextFile, load_context_files};
pub use extension::{CommandHandler, CommandResult, Extension, SlashCommand, ToolDefinition};
pub use session::{Session, SessionContext, SessionManager};
pub use session_repo::{DefaultSessionRepo, SessionRepo};
pub use session_storage::{
    InMemorySessionStorage, JsonlSessionStorage, SessionMetadata, SessionStorage,
};
pub use settings::Settings;

pub use system_prompt::{SystemPromptBuilder, ToolSnippet};
pub use types::base_model_config;
