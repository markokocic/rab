/// System prompt construction.
///
/// Mirrors pi's `buildSystemPrompt()` in system-prompt.ts.
///
/// Layers (in order):
/// 1. Default prompt (tool announcements + guidelines) — replaced if custom_prompt is set
/// 2. Append prompt (always appended, whether custom or default)
/// 3. Project context (<project_context> wrapping AGENTS.md/CLAUDE.md files)
/// 4. Skills (<available_skills> XML block)
/// 5. Current date + working directory (always last)
use crate::agent::context_files::ContextFile;
use crate::agent::skills::{Skill, format_skills_for_prompt};

use std::path::Path;

/// A one-line description of a tool for the "Available tools" section.
/// Uses prompt_snippet() when available, falling back to description().
#[derive(Debug, Clone)]
pub struct ToolSnippet {
    pub name: String,
    pub description: String,
}

impl ToolSnippet {

}

/// Builder for constructing the full system prompt.
///
/// Usage:
/// ```ignore
/// let prompt = SystemPromptBuilder::new()
///     .tool_snippets(tool_snippets)
///     .guidelines(guidelines)
///     .context_files(context_files)
///     .skills(skills)
///     .custom_prompt(custom_system_md)
///     .append_prompt(append_system_md)
///     .cwd(&cwd)
///     .build();
/// ```
#[derive(Debug, Default)]
pub struct SystemPromptBuilder {
    /// Tool one-liners for "Available tools" section.
    tool_snippets: Vec<ToolSnippet>,
    /// Extra guideline bullets beyond the standard ones.
    guidelines: Vec<String>,
    /// Context files (AGENTS.md / CLAUDE.md) wrapped in `<project_context>`.
    context_files: Vec<ContextFile>,
    /// Skills formatted as `<available_skills>` XML.
    skills: Vec<Skill>,
    /// Custom system prompt (replaces default). From SYSTEM.md or `--system-prompt`.
    custom_prompt: Option<String>,
    /// Text to append to the system prompt. From APPEND_SYSTEM.md or `--append-system-prompt`.
    append_prompt: Option<String>,
    /// Working directory.
    cwd: Option<String>,
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn tool_snippets(mut self, snippets: Vec<ToolSnippet>) -> Self {
        self.tool_snippets = snippets;
        self
    }

    pub fn guidelines(mut self, guidelines: Vec<String>) -> Self {
        self.guidelines = guidelines;
        self
    }

    pub fn context_files(mut self, files: Vec<ContextFile>) -> Self {
        self.context_files = files;
        self
    }

    pub fn skills(mut self, skills: Vec<Skill>) -> Self {
        self.skills = skills;
        self
    }

    pub fn custom_prompt(mut self, prompt: Option<String>) -> Self {
        self.custom_prompt = prompt;
        self
    }

    pub fn append_prompt(mut self, prompt: Option<String>) -> Self {
        self.append_prompt = prompt;
        self
    }

    pub fn cwd(mut self, cwd: &Path) -> Self {
        self.cwd = Some(cwd.to_string_lossy().replace('\\', "/"));
        self
    }

    /// Build the final system prompt string.
    pub fn build(&self) -> String {
        let now = chrono::Utc::now();
        let date = now.format("%Y-%m-%d").to_string();
        let prompt_cwd = self.cwd.clone().unwrap_or_else(|| String::from("/unknown"));

        // ── 1. Default or custom prompt ────────────────────────────
        let mut prompt = if let Some(ref custom) = self.custom_prompt {
            // Custom prompt replaces default entirely
            custom.clone()
        } else {
            self.build_default_prompt()
        };

        // ── 2. Append prompt ──────────────────────────────────────
        if let Some(ref append) = self.append_prompt
            && !append.is_empty()
        {
            prompt.push('\n');
            prompt.push('\n');
            prompt.push_str(append);
        }

        // ── 3. Project context (AGENTS.md / CLAUDE.md) ────────────
        if !self.context_files.is_empty() {
            prompt.push_str("\n\n<project_context>\n\n");
            prompt.push_str("Project-specific instructions and guidelines:\n\n");

            for cf in &self.context_files {
                let path_str = cf.path.to_string_lossy();
                prompt.push_str(&format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                    path_str, cf.content
                ));
            }

            prompt.push_str("</project_context>\n");
        }

        // ── 4. Skills ─────────────────────────────────────────────
        let skills_section = format_skills_for_prompt(&self.skills);
        if !skills_section.is_empty() {
            prompt.push_str(&skills_section);
        }

        // ── 5. Date and working directory ─────────────────────────
        prompt.push_str(&format!("\nCurrent date: {}", date));
        prompt.push_str(&format!("\nCurrent working directory: {}", prompt_cwd));

        prompt
    }

    /// Build the default system prompt (used when no custom_prompt is set).
    fn build_default_prompt(&self) -> String {
        let mut prompt = String::new();

        // Identity
        prompt.push_str(
            "You are an expert coding assistant operating inside rab, a coding agent harness. \
             You help users by reading files, executing commands, editing code, and writing new files.\n\n",
        );

        // Available tools
        prompt.push_str("Available tools:\n");
        if self.tool_snippets.is_empty() {
            prompt.push_str("(none)\n");
        } else {
            for snippet in &self.tool_snippets {
                prompt.push_str(&format!("- {}: {}\n", snippet.name, snippet.description));
            }
        }

        // Custom tools note
        prompt.push_str(
            "\nIn addition to the tools above, you may have access to other custom tools depending on the project.\n",
        );

        // Guidelines
        prompt.push_str("\nGuidelines:\n");

        let has_bash = self.tool_snippets.iter().any(|t| t.name == "bash");
        let has_grep = self.tool_snippets.iter().any(|t| t.name == "grep");
        let has_find = self.tool_snippets.iter().any(|t| t.name == "find");
        let has_ls = self.tool_snippets.iter().any(|t| t.name == "ls");

        if has_bash && !has_grep && !has_find && !has_ls {
            prompt.push_str("- Use bash for file operations like ls, rg, find\n");
        }

        for guideline in &self.guidelines {
            let trimmed = guideline.trim();
            if !trimmed.is_empty() {
                prompt.push_str(&format!("- {}\n", trimmed));
            }
        }

        prompt.push_str("- Be concise in your responses\n");
        prompt.push_str("- Show file paths clearly when working with files\n");

        prompt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::context_files::ContextFile;

    fn make_snippet(name: &str, desc: &str) -> ToolSnippet {
        ToolSnippet {
            name: name.to_string(),
            description: desc.to_string(),
        }
    }

    #[test]
    fn test_default_prompt_has_tools_and_guidelines() {
        let prompt = SystemPromptBuilder::new()
            .tool_snippets(vec![
                make_snippet("read", "Read file contents"),
                make_snippet("bash", "Execute bash commands"),
            ])
            .guidelines(vec!["Use careful paths".to_string()])
            .build();

        assert!(prompt.contains("rab, a coding agent harness"));
        assert!(prompt.contains("read: Read file contents"));
        assert!(prompt.contains("bash: Execute bash commands"));
        assert!(prompt.contains("Use careful paths"));
        assert!(prompt.contains("Be concise in your responses"));
        assert!(prompt.contains("Current date:"));
        assert!(prompt.contains("Current working directory:"));
    }

    #[test]
    fn test_custom_prompt_replaces_default() {
        let prompt = SystemPromptBuilder::new()
            .custom_prompt(Some("You are a custom agent.".to_string()))
            .tool_snippets(vec![make_snippet("read", "Read files")])
            .build();

        // Custom prompt replaces default
        assert!(prompt.contains("You are a custom agent."));
        assert!(!prompt.contains("rab, a coding agent harness"));
        assert!(!prompt.contains("Available tools:"));
        // But context and date still appended
        assert!(prompt.contains("Current date:"));
    }

    #[test]
    fn test_append_prompt() {
        let prompt = SystemPromptBuilder::new()
            .append_prompt(Some("Additional instructions.".to_string()))
            .build();

        assert!(prompt.contains("Additional instructions."));
    }

    #[test]
    fn test_project_context() {
        let files = vec![ContextFile {
            path: "/home/user/project/AGENTS.md".into(),
            content: "# Project rules\n- be tidy".to_string(),
        }];

        let prompt = SystemPromptBuilder::new().context_files(files).build();

        assert!(prompt.contains("<project_context>"));
        assert!(prompt.contains("<project_instructions path=\"/home/user/project/AGENTS.md\">"));
        assert!(prompt.contains("# Project rules\n- be tidy"));
        assert!(prompt.contains("</project_instructions>"));
        assert!(prompt.contains("</project_context>"));
    }

    #[test]
    fn test_multiple_context_files() {
        let files = vec![
            ContextFile {
                path: "/home/user/.rab/agent/AGENTS.md".into(),
                content: "# Global".to_string(),
            },
            ContextFile {
                path: "/home/user/project/AGENTS.md".into(),
                content: "# Project".to_string(),
            },
        ];

        let prompt = SystemPromptBuilder::new().context_files(files).build();

        // Both should appear
        assert!(prompt.contains("# Global"));
        assert!(prompt.contains("# Project"));
    }

    #[test]
    fn test_skills_section() {
        let skills = vec![Skill {
            name: "code-review".to_string(),
            description: "Reviews code for bugs".to_string(),
            file_path: "/home/user/.rab/agent/skills/code-review/SKILL.md".into(),
            base_dir: "/home/user/.rab/agent/skills/code-review".into(),
            disable_model_invocation: false,
        }];

        let prompt = SystemPromptBuilder::new().skills(skills).build();

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>code-review</name>"));
        assert!(prompt.contains("</available_skills>"));
    }

    #[test]
    fn test_date_and_cwd_at_end() {
        let prompt = SystemPromptBuilder::new()
            .cwd(Path::new("/home/user/project"))
            .build();

        let lines: Vec<&str> = prompt.lines().collect();
        // Last two lines should be date and cwd
        assert!(lines[lines.len() - 2].starts_with("Current date:"));
        assert_eq!(
            lines[lines.len() - 1],
            "Current working directory: /home/user/project"
        );
    }

    #[test]
    fn test_no_tools_shows_none() {
        let prompt = SystemPromptBuilder::new().build();
        assert!(prompt.contains("Available tools:\n(none)"));
    }

    #[test]
    fn test_bash_without_grep_find_ls() {
        let prompt = SystemPromptBuilder::new()
            .tool_snippets(vec![make_snippet("bash", "Execute bash")])
            .build();

        assert!(prompt.contains("Use bash for file operations like ls, rg, find"));
    }

    #[test]
    fn test_bash_with_grep() {
        let prompt = SystemPromptBuilder::new()
            .tool_snippets(vec![
                make_snippet("bash", "Execute bash"),
                make_snippet("grep", "Search text"),
            ])
            .build();

        // Should NOT add the bash-for-files guideline since grep is available
        assert!(!prompt.contains("Use bash for file operations like ls, rg, find"));
    }

    #[test]
    fn test_custom_prompt_still_gets_context_and_skills() {
        let files = vec![ContextFile {
            path: "/project/AGENTS.md".into(),
            content: "# Rules".to_string(),
        }];

        let skills = vec![Skill {
            name: "test-skill".to_string(),
            description: "Test".to_string(),
            file_path: "/tmp/SKILL.md".into(),
            base_dir: "/tmp".into(),
            disable_model_invocation: false,
        }];

        let prompt = SystemPromptBuilder::new()
            .custom_prompt(Some("Custom base.".to_string()))
            .context_files(files)
            .skills(skills)
            .build();

        assert!(prompt.starts_with("Custom base."));
        assert!(prompt.contains("<project_instructions"));
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("Current date:"));
    }

    #[test]
    fn test_full_build_integration() {
        let files = vec![ContextFile {
            path: "/home/user/project/AGENTS.md".into(),
            content: "# Project rules".to_string(),
        }];

        let skills = vec![Skill {
            name: "code-review".to_string(),
            description: "Review code".to_string(),
            file_path: "/home/user/.rab/agent/skills/code-review/SKILL.md".into(),
            base_dir: "/home/user/.rab/agent/skills/code-review".into(),
            disable_model_invocation: false,
        }];

        let prompt = SystemPromptBuilder::new()
            .tool_snippets(vec![
                make_snippet("read", "Read file contents"),
                make_snippet("edit", "Make precise edits"),
                make_snippet("bash", "Execute bash commands"),
                make_snippet("write", "Create or overwrite files"),
            ])
            .guidelines(vec![
                "Use the edit tool for precise changes with exact text matching".to_string(),
            ])
            .context_files(files)
            .skills(skills)
            .cwd(Path::new("/home/user/project"))
            .build();

        // Verify structure
        assert!(prompt.starts_with("You are an expert coding assistant"));
        assert!(prompt.contains("Available tools:"));
        assert!(prompt.contains("- read: Read file contents"));
        assert!(prompt.contains("Guidelines:"));
        assert!(prompt.contains("Make precise edits"));
        assert!(prompt.contains("<project_context>"));
        assert!(prompt.contains("# Project rules"));
        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>code-review</name>"));
        assert!(prompt.ends_with("/home/user/project"));

        // Verify order: guidelines before context before skills before date
        let guidelines_pos = prompt.find("Guidelines:").unwrap();
        let context_pos = prompt.find("<project_context>").unwrap();
        let skills_pos = prompt.find("<available_skills>").unwrap();
        let date_pos = prompt.find("Current date:").unwrap();

        assert!(guidelines_pos < context_pos);
        assert!(context_pos < skills_pos);
        assert!(skills_pos < date_pos);
    }
}
