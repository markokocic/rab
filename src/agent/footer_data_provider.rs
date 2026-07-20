use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Matches pi's `FooterDataProvider` — provides git branch, extension
/// statuses, and provider count to the Footer on a **pull** basis.
///
/// Owned by the App behind `Rc<RefCell<>>`. The Footer holds a shared
/// `Rc` clone and reads data each render cycle instead of receiving
/// push updates from the App.
///
/// Git branch resolution:
/// 1. Walk up from `cwd` looking for `.git`
/// 2. If `.git` is a file → worktree: parse `gitdir:` path, find HEAD
/// 3. If `.git` is a directory → regular repo: find HEAD
/// 4. Read HEAD file; if `ref: refs/heads/.invalid` → fall back to git
/// 5. Otherwise treat as detached HEAD
pub struct FooterDataProvider {
    cwd: PathBuf,
    git_branch: Option<String>,
    extension_statuses: BTreeMap<String, String>,
    available_provider_count: usize,
    /// Latest model provider pulled from the session.
    model_provider: Option<String>,
    /// Latest model ID pulled from the session.
    model_id: Option<String>,
}

impl FooterDataProvider {
    pub fn new(cwd: PathBuf) -> Self {
        let mut provider = Self {
            cwd,
            git_branch: None,
            extension_statuses: BTreeMap::new(),
            available_provider_count: 1,
            model_provider: None,
            model_id: None,
        };
        provider.refresh_git_branch();
        provider
    }

    // ── Git branch ──

    pub fn get_git_branch(&self) -> Option<&str> {
        self.git_branch.as_deref()
    }

    /// Re-resolve git branch from disk (e.g. after a known branch switch).
    pub fn refresh_git_branch(&mut self) {
        self.git_branch = resolve_git_branch(&self.cwd);
    }

    pub fn set_cwd(&mut self, cwd: PathBuf) {
        self.cwd = cwd;
        self.refresh_git_branch();
    }

    // ── Extension statuses (sorted by key, pi-style) ──

    pub fn get_extension_statuses(&self) -> &BTreeMap<String, String> {
        &self.extension_statuses
    }

    pub fn set_extension_status(&mut self, key: &str, text: Option<&str>) {
        if let Some(text) = text {
            self.extension_statuses
                .insert(key.to_string(), text.to_string());
        } else {
            self.extension_statuses.remove(key);
        }
    }

    pub fn clear_extension_statuses(&mut self) {
        self.extension_statuses.clear();
    }

    // ── Model / provider (pulled from session via refresh_from_session) ──

    pub fn get_model_provider(&self) -> Option<&str> {
        self.model_provider.as_deref()
    }

    pub fn get_model_id(&self) -> Option<&str> {
        self.model_id.as_deref()
    }

    /// Scan session entries for the latest `ModelChangeEntry` and cache
    /// the provider + model_id. Called from Footer::refresh_from_session.
    pub fn refresh_from_session(&mut self, session: &crate::agent::session::Session) {
        let mut latest_provider: Option<String> = None;
        let mut latest_model_id: Option<String> = None;

        for entry in session.get_entries() {
            if let yoagent::types::AgentMessage::Extension(ext) = &entry.message
                && ext.kind == "session/model_change"
            {
                latest_provider = ext
                    .data
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                latest_model_id = ext
                    .data
                    .get("modelId")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }

        self.model_provider = latest_provider;
        self.model_id = latest_model_id;
    }

    /// Test-only: set model provider directly.
    #[cfg(test)]
    pub fn set_test_model_provider(&mut self, provider: Option<&str>) {
        self.model_provider = provider.map(|s| s.to_string());
    }

    /// Test-only: set model ID directly.
    #[cfg(test)]
    pub fn set_test_model_id(&mut self, model_id: Option<&str>) {
        self.model_id = model_id.map(|s| s.to_string());
    }

    pub fn get_available_provider_count(&self) -> usize {
        self.available_provider_count
    }

    pub fn set_available_provider_count(&mut self, count: usize) {
        self.available_provider_count = count;
    }

    /// Test-only: set git branch directly (avoids filesystem resolution).
    #[cfg(test)]
    pub fn set_test_git_branch(&mut self, branch: Option<&str>) {
        self.git_branch = branch.map(|s| s.to_string());
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_provider_refreshes_git_branch() {
        let provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        // In a temp dir without git, git_branch should be None
        assert!(provider.get_git_branch().is_none());
    }

    #[test]
    fn test_set_test_git_branch() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_test_git_branch(Some("main"));
        assert_eq!(provider.get_git_branch(), Some("main"));
    }

    #[test]
    fn test_set_test_git_branch_none() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_test_git_branch(Some("feature"));
        provider.set_test_git_branch(None);
        assert!(provider.get_git_branch().is_none());
    }

    #[test]
    fn test_extension_statuses() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        assert!(provider.get_extension_statuses().is_empty());

        provider.set_extension_status("bash", Some("ready"));
        assert_eq!(
            provider.get_extension_statuses().get("bash"),
            Some(&"ready".to_string())
        );

        provider.set_extension_status("bash", None);
        assert!(provider.get_extension_statuses().is_empty());
    }

    #[test]
    fn test_extension_statuses_sorted() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_extension_status("zzz", Some("last"));
        provider.set_extension_status("aaa", Some("first"));
        provider.set_extension_status("mmm", Some("middle"));

        let keys: Vec<&String> = provider.get_extension_statuses().keys().collect();
        assert_eq!(keys, vec!["aaa", "mmm", "zzz"]);
    }

    #[test]
    fn test_clear_extension_statuses() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_extension_status("bash", Some("ready"));
        provider.clear_extension_statuses();
        assert!(provider.get_extension_statuses().is_empty());
    }

    #[test]
    fn test_provider_count() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        assert_eq!(provider.get_available_provider_count(), 1);
        provider.set_available_provider_count(3);
        assert_eq!(provider.get_available_provider_count(), 3);
    }

    #[test]
    fn test_set_cwd_refreshes_git_branch() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_test_git_branch(Some("old-branch"));
        // Changing cwd to a non-git dir should clear the branch
        provider.set_cwd(PathBuf::from("/nonexistent"));
        assert!(provider.get_git_branch().is_none());
    }

    // ── Model / provider tests ──

    #[test]
    fn test_model_provider_defaults() {
        let provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        assert!(provider.get_model_provider().is_none());
        assert!(provider.get_model_id().is_none());
    }

    #[test]
    fn test_set_test_model_provider() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_test_model_provider(Some("opencode-go"));
        assert_eq!(provider.get_model_provider(), Some("opencode-go"));
        provider.set_test_model_provider(None);
        assert!(provider.get_model_provider().is_none());
    }

    #[test]
    fn test_set_test_model_id() {
        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.set_test_model_id(Some("deepseek-v4-flash"));
        assert_eq!(provider.get_model_id(), Some("deepseek-v4-flash"));
        provider.set_test_model_id(None);
        assert!(provider.get_model_id().is_none());
    }

    #[test]
    fn test_refresh_from_session_extracts_latest_model_change() {
        use crate::agent::SessionMetadata;
        use crate::agent::session::InMemorySessionStorage;
        use crate::agent::session::*;

        let meta = SessionMetadata {
            id: "test".into(),
            created_at: String::new(),
            cwd: "/tmp".into(),
            path: None,
            parent_session_path: None,
        };
        let storage = InMemorySessionStorage::new(meta);
        let mut session = Session::new(Box::new(storage));
        session.append_model_change("provider-a", "model-a");
        session.append_model_change("provider-b", "model-b");

        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        provider.refresh_from_session(&session);

        assert_eq!(provider.get_model_provider(), Some("provider-b"));
        assert_eq!(provider.get_model_id(), Some("model-b"));
    }

    #[test]
    fn test_refresh_from_session_no_model_change() {
        use crate::agent::SessionMetadata;
        use crate::agent::session::InMemorySessionStorage;
        use crate::agent::session::*;

        let meta = SessionMetadata {
            id: "test".into(),
            created_at: String::new(),
            cwd: "/tmp".into(),
            path: None,
            parent_session_path: None,
        };
        let storage = InMemorySessionStorage::new(meta);
        let session = Session::new(Box::new(storage));

        let mut provider = FooterDataProvider::new(PathBuf::from("/tmp"));
        // Set some values first
        provider.set_test_model_provider(Some("old"));
        provider.set_test_model_id(Some("old-model"));
        // Refreshing from a session with no model changes should clear them
        provider.refresh_from_session(&session);

        assert!(provider.get_model_provider().is_none());
        assert!(provider.get_model_id().is_none());
    }

    // ── Git resolution helpers ──────────────────────────────────────

    #[test]
    fn test_find_git_paths_no_git() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let result = find_git_paths(&tmp);
        assert!(result.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_git_paths_regular_repo() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp.join(".git")).unwrap();
        std::fs::write(&tmp.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

        let result = find_git_paths(&tmp);
        assert!(result.is_some());
        let paths = result.unwrap();
        assert_eq!(paths.head_path, tmp.join(".git").join("HEAD"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_find_git_paths_walk_up() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp.join("sub").join("deep")).unwrap();
        std::fs::create_dir_all(&tmp.join(".git")).unwrap();
        std::fs::write(&tmp.join(".git").join("HEAD"), "ref: refs/heads/main\n").unwrap();

        // Should find .git by walking up from sub/deep
        let result = find_git_paths(&tmp.join("sub").join("deep"));
        assert!(result.is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_git_branch_from_head() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp.join(".git")).unwrap();
        std::fs::write(
            &tmp.join(".git").join("HEAD"),
            "ref: refs/heads/feature-branch\n",
        )
        .unwrap();

        let result = resolve_git_branch(&tmp);
        assert_eq!(result.as_deref(), Some("feature-branch"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_git_branch_detached() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp.join(".git")).unwrap();
        std::fs::write(&tmp.join(".git").join("HEAD"), "abc123def456\n").unwrap();

        let result = resolve_git_branch(&tmp);
        assert_eq!(result.as_deref(), Some("detached"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_resolve_git_branch_no_git() {
        let tmp = std::env::temp_dir().join(format!("rab-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let result = resolve_git_branch(&tmp);
        assert!(result.is_none());

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

struct GitPaths {
    _repo_dir: PathBuf,
    head_path: PathBuf,
}

/// Walk up from `cwd` looking for `.git` (directory or worktree file).
fn find_git_paths(cwd: &Path) -> Option<GitPaths> {
    let mut dir = Some(cwd.to_path_buf());
    while let Some(ref d) = dir {
        let git_path = d.join(".git");
        if git_path.exists() {
            if git_path.is_file() {
                // Worktree: .git is a file containing "gitdir: <path>"
                let content = fs::read_to_string(&git_path).ok()?;
                let content = content.trim();
                if let Some(git_dir_str) = content.strip_prefix("gitdir: ") {
                    let git_dir = d.join(git_dir_str);
                    let head_path = git_dir.join("HEAD");
                    if head_path.exists() {
                        return Some(GitPaths {
                            _repo_dir: d.clone(),
                            head_path,
                        });
                    }
                }
            } else if git_path.is_dir() {
                // Regular repo
                let head_path = git_path.join("HEAD");
                if head_path.exists() {
                    return Some(GitPaths {
                        _repo_dir: d.clone(),
                        head_path,
                    });
                }
            }
        }
        dir = d.parent().map(|p| p.to_path_buf());
    }
    None
}

/// Resolve the current git branch from HEAD, handling reftable repos.
fn resolve_git_branch(cwd: &Path) -> Option<String> {
    let paths = find_git_paths(cwd)?;
    let content = fs::read_to_string(&paths.head_path).ok()?;
    let content = content.trim();

    if let Some(branch) = content.strip_prefix("ref: refs/heads/") {
        if branch == ".invalid" {
            // Reftable repo: HEAD is a placeholder, use git symbolic-ref
            resolve_branch_with_git(&paths._repo_dir)
        } else {
            Some(branch.to_string())
        }
    } else {
        // Detached HEAD
        Some("detached".to_string())
    }
}

/// Fallback for reftable repos: ask git for the current branch.
fn resolve_branch_with_git(repo_dir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "--no-optional-locks",
            "symbolic-ref",
            "--quiet",
            "--short",
            "HEAD",
        ])
        .current_dir(repo_dir)
        .output()
        .ok()?;
    if output.status.success() {
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !branch.is_empty() {
            return Some(branch);
        }
    }
    Some("detached".to_string())
}
