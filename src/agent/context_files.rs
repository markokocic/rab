/// Context file discovery — AGENTS.md / CLAUDE.md
///
/// Mirrors pi's `loadProjectContextFiles()`:
/// 1. Global AGENTS.md from agent config dir (~/.rab/agent/AGENTS.md)
/// 2. Walk up from cwd → /, collecting AGENTS.md / CLAUDE.md from each directory
/// 3. Current directory (included in the ancestor walk above)
///
/// All files are deduplicated by resolved absolute path.
use std::fs;
use std::path::{Path, PathBuf};

use crate::paths;

/// A discovered AGENTS.md or CLAUDE.md file with its content.
#[derive(Debug, Clone)]
pub struct ContextFile {
    pub path: PathBuf,
    pub content: String,
}

/// Candidate filenames checked in each directory.
const CANDIDATES: &[&str] = &["AGENTS.md", "AGENTS.MD", "CLAUDE.md", "CLAUDE.MD"];

/// Try to load a context file from a directory. Returns `None` if no candidate exists.
fn load_context_file_from_dir(dir: &Path) -> Option<ContextFile> {
    for filename in CANDIDATES {
        let file_path = dir.join(filename);
        if file_path.exists() {
            match fs::read_to_string(&file_path) {
                Ok(content) => {
                    return Some(ContextFile {
                        path: paths::canonicalize(&file_path),
                        content,
                    });
                }
                Err(_) => {
                    // Skip unreadable files, try next candidate
                    continue;
                }
            }
        }
    }
    None
}

/// Discover context files in the standard locations.
///
/// Order: global → ancestors (rootward) → cwd
/// The returned vec has global first, then ancestors in root-to-leaf order,
/// so later entries take precedence when concatenated.
pub fn load_context_files(cwd: &Path, agent_dir: &Path) -> Vec<ContextFile> {
    let resolved_cwd = paths::canonicalize(cwd);
    let resolved_agent = paths::canonicalize(agent_dir);

    let mut context_files: Vec<ContextFile> = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();

    // 1. Global context file from agent config dir
    if let Some(cf) = load_context_file_from_dir(&resolved_agent) {
        let canon = cf.path.clone();
        if seen_paths.insert(canon) {
            context_files.push(cf);
        }
    }

    // 2. Walk ancestors from cwd up to root
    let root = Path::new("/");
    let mut current = Some(resolved_cwd.as_path());

    // Collect ancestors in a vec first (cwd first, root last)
    let mut ancestors: Vec<&Path> = Vec::new();
    while let Some(dir) = current {
        ancestors.push(dir);
        if dir == root {
            break;
        }
        let parent = dir.parent().unwrap_or(root);
        if parent == dir {
            break;
        }
        current = Some(parent);
    }

    // Iterate root-to-leaf so global comes first, then closest-to-root files,
    // then cwd file last (pi does this so later entries are more specific)
    for dir in ancestors.into_iter().rev() {
        if let Some(cf) = load_context_file_from_dir(dir) {
            let canon = cf.path.clone();
            if seen_paths.insert(canon) {
                context_files.push(cf);
            }
        }
    }

    context_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_load_from_agent_dir() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        create_file(&agent_dir, "AGENTS.md", "# Agent rules\n- be careful");

        let cwd = tmp.path().join("project");
        fs::create_dir_all(&cwd).unwrap();

        let files = load_context_files(&cwd, &agent_dir);
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Agent rules"));
    }

    #[test]
    fn test_load_from_cwd_preferred() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        create_file(&project, "AGENTS.md", "# Project rules");

        let files = load_context_files(&project, &agent_dir);
        // No global file, just project one
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Project rules"));
    }

    #[test]
    fn test_both_global_and_project() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        create_file(&agent_dir, "AGENTS.md", "# Global rules");

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        create_file(&project, "AGENTS.md", "# Project rules");

        let files = load_context_files(&project, &agent_dir);
        assert_eq!(files.len(), 2);
        assert!(files[0].content.contains("Global rules"));
        assert!(files[1].content.contains("Project rules"));
    }

    #[test]
    fn test_claude_md_alternative() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        create_file(&project, "CLAUDE.md", "# Claude instructions");

        let files = load_context_files(&project, &agent_dir);
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Claude instructions"));
    }

    #[test]
    fn test_agents_md_preferred_over_claude_md() {
        let tmp = TempDir::new().unwrap();
        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        create_file(&project, "AGENTS.md", "# Agents first");
        create_file(&project, "CLAUDE.md", "# Claude second");

        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let files = load_context_files(&project, &agent_dir);
        // Only AGENTS.md should be loaded (candidates checked in order)
        assert_eq!(files.len(), 1);
        assert!(files[0].content.contains("Agents first"));
    }

    #[test]
    fn test_deduplicate_by_path() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        // Same file path appears in both global and cwd if cwd == agent dir
        create_file(&agent_dir, "AGENTS.md", "# Shared file");

        let files = load_context_files(&agent_dir, &agent_dir);
        // Should only appear once
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn test_no_context_files_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();

        let files = load_context_files(&project, &agent_dir);
        assert!(files.is_empty());
    }

    #[test]
    fn test_ancestor_directories() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        // Create a nested project structure
        let parent = tmp.path().join("parent");
        fs::create_dir_all(&parent).unwrap();
        create_file(&parent, "AGENTS.md", "# Parent rules");

        let child = parent.join("child");
        fs::create_dir_all(&child).unwrap();
        create_file(&child, "AGENTS.md", "# Child rules");

        let files = load_context_files(&child, &agent_dir);
        assert_eq!(files.len(), 2);
        // Parent first (closer to root), child second (cwd)
        assert!(files[0].content.contains("Parent rules"));
        assert!(files[1].content.contains("Child rules"));
    }

    #[test]
    fn test_ignores_non_context_files() {
        let tmp = TempDir::new().unwrap();
        let agent_dir = tmp.path().join("agent");
        fs::create_dir_all(&agent_dir).unwrap();

        let project = tmp.path().join("project");
        fs::create_dir_all(&project).unwrap();
        create_file(&project, "README.md", "# Not a context file");

        let files = load_context_files(&project, &agent_dir);
        assert!(files.is_empty());
    }
}
