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
}

impl FooterDataProvider {
    pub fn new(cwd: PathBuf) -> Self {
        let mut provider = Self {
            cwd,
            git_branch: None,
            extension_statuses: BTreeMap::new(),
            available_provider_count: 1,
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

    // ── Provider count (for multi-provider display) ──

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

// ── Git branch resolution ──────────────────────────────────────────

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
