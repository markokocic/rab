pub mod agent_session;
pub mod branch_summary;
pub mod compaction;
pub mod context_files;
pub mod extension;
pub mod footer_data_provider;
pub mod prompt_templates;
pub mod session;
pub mod settings;
pub mod system_prompt;
pub mod types;
pub mod ui;

pub use agent_session::{AgentSession, CompactionEvent, CompactionEventCallback};
pub use context_files::{ContextFile, load_context_files};
pub use extension::{CommandHandler, CommandResult, Extension, SlashCommand, ToolDefinition};
pub use session::{
    DefaultSessionRepo, InMemorySessionStorage, JsonlSessionStorage, Session, SessionContext,
    SessionManager, SessionMetadata, SessionRepo, SessionStorage,
};
pub use settings::Settings;

pub use system_prompt::{SystemPromptBuilder, ToolSnippet};
pub use types::base_model_config;
