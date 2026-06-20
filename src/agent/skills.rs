/// Skills — SKILL.md discovery, parsing, and formatting.
///
/// Mirrors pi's skills.ts:
/// - Loads SKILL.md files from standard directories
/// - Parses YAML-like frontmatter for name/description
/// - Formats as XML for injection into the system prompt
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// A loaded skill with metadata.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Display name (from frontmatter or filename).
    pub name: String,
    /// Short description (from frontmatter).
    pub description: String,
    /// Absolute path to the SKILL.md file.
    pub file_path: PathBuf,
    /// Base directory for resolving relative paths in the skill.
    pub base_dir: PathBuf,
    /// Whether the model should not auto-invoke this skill.
    pub disable_model_invocation: bool,
}

/// Options for loading skills.
#[derive(Debug)]
pub struct LoadSkillsOptions<'a> {
    /// Working directory for project-local skills.
    pub cwd: &'a Path,
    /// Agent config directory for global skills.
    pub agent_dir: &'a Path,
    /// Additional explicit skill paths to load.
    pub extra_skill_paths: &'a [PathBuf],
    /// Whether to include default directories.
    pub include_defaults: bool,
}

/// Parse frontmatter from a SKILL.md file.
///
/// Frontmatter is a YAML-like block between `---` delimiters at the start of the file.
/// We parse a minimal subset: `name:`, `description:`, `disable-model-invocation:`.
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>, bool) {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return (None, None, false);
    }

    let end = match content[3..].find("---") {
        Some(pos) => pos + 3,
        None => return (None, None, false),
    };
    let front = &content[3..3 + end];

    // Store all key-value pairs we find, but only use the ones we care about
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut disable = false;

    for line in front.lines() {
        let line = line.trim();
        if let Some(stripped) = line.strip_prefix("name:") {
            let val = stripped.trim().trim_matches('"').to_string();
            if !val.is_empty() {
                name = Some(val);
            }
        } else if let Some(stripped) = line.strip_prefix("description:") {
            let val = stripped.trim().trim_matches('"').to_string();
            if !val.is_empty() {
                description = Some(val);
            }
        } else if let Some(stripped) = line.strip_prefix("disable-model-invocation:") {
            let val = stripped.trim();
            disable = val == "true" || val == "yes" || val == "1";
        }
    }

    (name, description, disable)
}

/// Try to load a skill from a single SKILL.md file.
fn load_skill_from_file(file_path: &Path) -> Option<Skill> {
    let content = fs::read_to_string(file_path).ok()?;
    let (name, description, disable) = parse_frontmatter(&content);

    let name = name.unwrap_or_else(|| {
        // Use the parent directory name as fallback (e.g., skills/my-skill/SKILL.md → "my-skill")
        file_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("unnamed")
            .to_string()
    });

    let description = description.unwrap_or_default();
    let canonical_path = fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf());
    let base_dir = canonical_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/"));

    Some(Skill {
        name,
        description,
        file_path: canonical_path,
        base_dir,
        disable_model_invocation: disable,
    })
}

/// Discover skill directories.
///
/// Default locations (mirroring pi):
/// - ~/.rab/agent/skills/ (global)
/// - ~/.agents/skills/ (global alternative)
/// - .rab/skills/ (project)
/// - .agents/skills/ (project alternative, walking up from cwd)
fn discover_skill_dirs(cwd: &Path, agent_dir: &Path, include_defaults: bool) -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if include_defaults {
        // Global skill directories
        dirs.push(agent_dir.join("skills"));
        if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
            dirs.push(home.join(".agents").join("skills"));
        }

        // Project skill directories (walk up from cwd)
        let mut current = Some(cwd.to_path_buf());
        while let Some(dir) = current {
            dirs.push(dir.join(".rab").join("skills"));
            dirs.push(dir.join(".agents").join("skills"));
            let parent = match dir.parent() {
                Some(p) if p != dir => p.to_path_buf(),
                _ => break,
            };
            current = Some(parent);
        }
    }

    dirs
}

/// Recursively find SKILL.md files in a directory.
fn find_skill_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return files,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let skill_file = path.join("SKILL.md");
            if skill_file.exists() {
                files.push(skill_file);
            }
        } else if path.is_file() && path.file_name().is_some_and(|n| n == "SKILL.md") {
            files.push(path);
        }
    }

    files
}

/// Load skills from standard directories and explicit paths.
pub fn load_skills(options: LoadSkillsOptions) -> Vec<Skill> {
    let mut seen_paths = HashSet::new();
    let mut skills = Vec::new();

    // Discover from default directories
    if options.include_defaults {
        let dirs = discover_skill_dirs(options.cwd, options.agent_dir, true);
        for dir in dirs {
            for file_path in find_skill_files(&dir) {
                let canon = fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.clone());
                if seen_paths.insert(canon)
                    && let Some(skill) = load_skill_from_file(&file_path)
                {
                    skills.push(skill);
                }
            }
        }
    }

    // Load from explicit paths
    for path in options.extra_skill_paths {
        let resolved = if path.is_absolute() {
            path.clone()
        } else {
            options.cwd.join(path)
        };

        if resolved.is_dir() {
            for file_path in find_skill_files(&resolved) {
                let canon = fs::canonicalize(&file_path).unwrap_or_else(|_| file_path.clone());
                if seen_paths.insert(canon)
                    && let Some(skill) = load_skill_from_file(&file_path)
                {
                    skills.push(skill);
                }
            }
        } else if resolved.is_file() {
            let canon = fs::canonicalize(&resolved).unwrap_or(resolved);
            if seen_paths.insert(canon.clone())
                && let Some(skill) = load_skill_from_file(&canon)
            {
                skills.push(skill);
            }
        }
    }

    skills
}

/// Format skills as XML for the system prompt.
///
/// Mirrors pi's `formatSkillsForPrompt()` in skills.ts.
pub fn format_skills_for_prompt(skills: &[Skill]) -> String {
    let visible: Vec<&Skill> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();

    if visible.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        String::new(),
        String::new(),
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Use the read tool to load a skill's file when the task matches its description.".to_string(),
        "When a skill file references a relative path, resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in &visible {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path.to_string_lossy())
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Strip YAML frontmatter from a string, returning the body only.
///
/// Matches pi's `stripFrontmatter()` in frontmatter.ts.
/// If no `---` delimiters are found, returns the original string.
pub fn strip_frontmatter(content: &str) -> String {
    let content = content.trim_start();
    if !content.starts_with("---") {
        return content.to_string();
    }

    let remaining = &content[3..];
    let end = match remaining.find("---") {
        Some(pos) => pos,
        None => return content.to_string(),
    };

    // Skip to after the closing ---
    let body_start = 3 + end + 3;
    content[body_start..].trim().to_string()
}

/// Read a SKILL.md file, strip frontmatter, and return the body.
pub fn read_skill_body(file_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
    Some(strip_frontmatter(&content))
}

/// Format a skill invocation block for the LLM.
///
/// Mirrors pi's `formatSkillInvocation()` in harness/skills.ts.
/// Produces:
/// ```xml
/// <skill name="..." location="...">
/// References are relative to <basedir>.
///
/// <body>
/// </skill>
/// ```
pub fn format_skill_invocation(skill: &Skill, additional_instructions: Option<&str>) -> String {
    let body = read_skill_body(&skill.file_path).unwrap_or_default();
    let base_dir_str = skill.base_dir.to_string_lossy();
    let skill_block = format!(
        "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {}.\n\n{}\n</skill>",
        escape_xml(&skill.name),
        escape_xml(&skill.file_path.to_string_lossy()),
        base_dir_str,
        body
    );
    match additional_instructions {
        Some(instr) if !instr.is_empty() => format!("{}\n\n{}", skill_block, instr),
        _ => skill_block,
    }
}

/// Expand a `/skill:name [args]` command into a skill invocation block.
/// If the skill is not found, returns the original text unchanged.
pub fn expand_skill_command(text: &str, skills: &[Skill]) -> String {
    if !text.starts_with("/skill:") {
        return text.to_string();
    }

    let rest = &text[7..]; // after "/skill:"
    let (skill_name, args) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
        None => (rest, ""),
    };

    let skill = skills.iter().find(|s| s.name == skill_name);
    match skill {
        Some(s) => format_skill_invocation(s, if args.is_empty() { None } else { Some(args) }),
        None => text.to_string(), // unknown skill, pass through
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_skill(dir: &Path, name: &str, content: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_load_skill_with_frontmatter() {
        let tmp = TempDir::new().unwrap();
        create_skill(
            tmp.path(),
            "my-skill",
            r#"---
name: my-skill
description: My custom skill
---

# My Skill

Do something specific.
"#,
        );

        let skills = load_skills(LoadSkillsOptions {
            cwd: tmp.path(),
            agent_dir: tmp.path(),
            extra_skill_paths: &[],
            include_defaults: false,
        });
        assert!(skills.is_empty(), "no default dirs in tmp");

        let skills = load_skills(LoadSkillsOptions {
            cwd: tmp.path(),
            agent_dir: tmp.path(),
            extra_skill_paths: &[tmp.path().join("my-skill")],
            include_defaults: false,
        });
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert_eq!(skills[0].description, "My custom skill");
        assert!(!skills[0].disable_model_invocation);
    }

    #[test]
    fn test_skill_without_frontmatter_uses_filename() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "simple-skill", "# Just some instructions");

        let skills = load_skills(LoadSkillsOptions {
            cwd: tmp.path(),
            agent_dir: tmp.path(),
            extra_skill_paths: &[tmp.path().join("simple-skill")],
            include_defaults: false,
        });
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "simple-skill");
        assert_eq!(skills[0].description, "");
    }

    #[test]
    fn test_disable_model_invocation() {
        let tmp = TempDir::new().unwrap();
        create_skill(
            tmp.path(),
            "hidden-skill",
            r#"---
name: hidden-skill
description: Should not auto-invoke
disable-model-invocation: true
---

# Hidden
"#,
        );

        let skills = load_skills(LoadSkillsOptions {
            cwd: tmp.path(),
            agent_dir: tmp.path(),
            extra_skill_paths: &[tmp.path().join("hidden-skill")],
            include_defaults: false,
        });
        assert_eq!(skills.len(), 1);
        assert!(skills[0].disable_model_invocation);
    }

    #[test]
    fn test_format_skills_for_prompt() {
        let skills = vec![
            Skill {
                name: "code-review".to_string(),
                description: "Reviews code for bugs".to_string(),
                file_path: PathBuf::from("/home/user/.rab/agent/skills/code-review/SKILL.md"),
                base_dir: PathBuf::from("/home/user/.rab/agent/skills/code-review"),
                disable_model_invocation: false,
            },
            Skill {
                name: "hidden".to_string(),
                description: "Hidden skill".to_string(),
                file_path: PathBuf::from("/home/user/.rab/agent/skills/hidden/SKILL.md"),
                base_dir: PathBuf::from("/home/user/.rab/agent/skills/hidden"),
                disable_model_invocation: true,
            },
        ];

        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("<available_skills>"));
        assert!(result.contains("<name>code-review</name>"));
        assert!(result.contains("<description>Reviews code for bugs</description>"));
        assert!(
            result
                .contains("<location>/home/user/.rab/agent/skills/code-review/SKILL.md</location>")
        );
        assert!(!result.contains("hidden"), "disabled skills are excluded");
    }

    #[test]
    fn test_format_skills_empty() {
        assert!(format_skills_for_prompt(&[]).is_empty());
    }

    #[test]
    fn test_format_skills_all_disabled() {
        let skills = vec![Skill {
            name: "hidden".to_string(),
            description: "Hidden".to_string(),
            file_path: PathBuf::from("/tmp/SKILL.md"),
            base_dir: PathBuf::from("/tmp"),
            disable_model_invocation: true,
        }];
        assert!(format_skills_for_prompt(&skills).is_empty());
    }

    #[test]
    fn test_xml_escaping() {
        let skills = vec![Skill {
            name: "escape<test>".to_string(),
            description: "description with & special chars".to_string(),
            file_path: PathBuf::from("/tmp/skill's file\"name\".md"),
            base_dir: PathBuf::from("/tmp"),
            disable_model_invocation: false,
        }];

        let result = format_skills_for_prompt(&skills);
        assert!(result.contains("&lt;test&gt;"));
        assert!(result.contains("&amp;"));
        assert!(result.contains("&apos;"));
        assert!(result.contains("&quot;"));
    }

    #[test]
    fn test_parse_frontmatter_minimal() {
        let (name, desc, disable) = parse_frontmatter(
            r#"---
name: my-skill
---"#,
        );
        assert_eq!(name.as_deref(), Some("my-skill"));
        assert_eq!(desc, None);
        assert!(!disable);
    }

    #[test]
    fn test_parse_frontmatter_no_delimiters() {
        let (name, desc, disable) = parse_frontmatter("# Just markdown");
        assert_eq!(name, None);
        assert_eq!(desc, None);
        assert!(!disable);
    }

    #[test]
    fn test_parse_frontmatter_all_fields() {
        let (name, desc, disable) = parse_frontmatter(
            r#"---
name: my-skill
description: Does things
disable-model-invocation: true
---"#,
        );
        assert_eq!(name.as_deref(), Some("my-skill"));
        assert_eq!(desc.as_deref(), Some("Does things"));
        assert!(disable);
    }

    #[test]
    fn test_duplicate_path_deduplication() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "dup-skill", "# Skill content");

        // Load the same skill twice via redundant paths
        let path = tmp.path().join("dup-skill").join("SKILL.md");
        let skills = load_skills(LoadSkillsOptions {
            cwd: tmp.path(),
            agent_dir: tmp.path(),
            extra_skill_paths: &[path.clone(), path],
            include_defaults: false,
        });
        assert_eq!(skills.len(), 1);
    }

    // ── strip_frontmatter ─────────────────────────────────────────

    #[test]
    fn test_strip_frontmatter_basic() {
        let result = strip_frontmatter(
            r"---
name: my-skill
description: A test
---

This is the body.
",
        );
        assert_eq!(result, "This is the body.");
    }

    #[test]
    fn test_strip_frontmatter_no_frontmatter() {
        let result = strip_frontmatter("Just body text");
        assert_eq!(result, "Just body text");
    }

    #[test]
    fn test_strip_frontmatter_partial_delimiters() {
        // Opening --- but no closing --- returns original
        let result = strip_frontmatter("---\nname: broken\n");
        assert_eq!(result, "---\nname: broken\n");
    }

    #[test]
    fn test_strip_frontmatter_empty_body() {
        let result = strip_frontmatter(
            r"---
name: empty
---

",
        );
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_frontmatter_leading_whitespace() {
        // Content with leading whitespace before --- should still be stripped
        let result = strip_frontmatter("  \n---\nname: test\n---\n\nBody content");
        assert_eq!(result, "Body content");
    }

    #[test]
    fn test_strip_frontmatter_crlf_newlines() {
        let result = strip_frontmatter("---\r\nname: test\r\n---\r\n\r\nBody text");
        assert_eq!(result, "Body text");
    }

    // ── read_skill_body ───────────────────────────────────────────

    #[test]
    fn test_read_skill_body_from_file() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("SKILL.md");
        fs::write(
            &path,
            r"---
name: test-skill
---

# Actual content
",
        )
        .unwrap();

        let body = read_skill_body(&path);
        assert_eq!(body, Some("# Actual content".to_string()));
    }

    #[test]
    fn test_read_skill_body_no_frontmatter() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("SKILL.md");
        fs::write(&path, "Just content\n").unwrap();

        let body = read_skill_body(&path);
        assert_eq!(body, Some("Just content\n".to_string()));
    }

    #[test]
    fn test_read_skill_body_missing_file() {
        let body = read_skill_body(Path::new("/nonexistent/SKILL.md"));
        assert_eq!(body, None);
    }

    // ── format_skill_invocation ───────────────────────────────────

    #[test]
    fn test_format_skill_invocation_basic() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("test-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            r"---
name: test-skill
description: A test skill
---

Do the thing.
",
        )
        .unwrap();

        let skill = Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            file_path: fs::canonicalize(&skill_path).unwrap_or(skill_path.clone()),
            base_dir: skill_dir.clone(),
            disable_model_invocation: false,
        };

        let result = format_skill_invocation(&skill, None);
        assert!(result.starts_with("<skill name=\"test-skill\""));
        assert!(result.contains("References are relative to"));
        assert!(result.contains("Do the thing."));
        assert!(result.ends_with("</skill>"));
    }

    #[test]
    fn test_format_skill_invocation_with_args() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("review-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        let skill_path = skill_dir.join("SKILL.md");
        fs::write(
            &skill_path,
            r"---
name: review
---

Check the code.
",
        )
        .unwrap();

        let skill = Skill {
            name: "review".to_string(),
            description: "".to_string(),
            file_path: fs::canonicalize(&skill_path).unwrap_or(skill_path.clone()),
            base_dir: skill_dir,
            disable_model_invocation: false,
        };

        let result = format_skill_invocation(&skill, Some("Focus on security."));
        assert!(result.contains("Check the code."));
        assert!(result.contains("Focus on security."));
    }

    #[test]
    fn test_format_skill_invocation_missing_file() {
        let skill = Skill {
            name: "missing".to_string(),
            description: "Missing skill".to_string(),
            file_path: PathBuf::from("/nonexistent/SKILL.md"),
            base_dir: PathBuf::from("/nonexistent"),
            disable_model_invocation: false,
        };

        let result = format_skill_invocation(&skill, None);
        // Should still produce the wrapper, body will be empty
        assert!(result.starts_with("<skill"));
        assert!(result.ends_with("</skill>"));
    }

    // ── expand_skill_command ──────────────────────────────────────

    #[test]
    fn test_expand_skill_command_basic() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("code-review");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r"---
name: code-review
---

Review the code for bugs.
",
        )
        .unwrap();

        let skill_path = fs::canonicalize(skill_dir.join("SKILL.md")).unwrap_or_default();
        let skills = vec![Skill {
            name: "code-review".to_string(),
            description: "".to_string(),
            file_path: skill_path,
            base_dir: skill_dir,
            disable_model_invocation: false,
        }];

        let result = expand_skill_command("/skill:code-review", &skills);
        assert!(result.contains("Review the code for bugs."));
        assert!(result.starts_with("<skill"));
    }

    #[test]
    fn test_expand_skill_command_with_args() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("test");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r"---
name: test
---

Run tests.
",
        )
        .unwrap();

        let skill_path = fs::canonicalize(skill_dir.join("SKILL.md")).unwrap_or_default();
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "".to_string(),
            file_path: skill_path,
            base_dir: skill_dir,
            disable_model_invocation: false,
        }];

        let result = expand_skill_command("/skill:test focus on unit tests", &skills);
        assert!(result.contains("Run tests."));
        assert!(result.contains("focus on unit tests"));
    }

    #[test]
    fn test_expand_skill_command_unknown_skill() {
        let skills: Vec<Skill> = vec![];
        let result = expand_skill_command("/skill:nonexistent", &skills);
        // Unknown skill passes through unchanged
        assert_eq!(result, "/skill:nonexistent");
    }

    #[test]
    fn test_expand_skill_command_not_a_skill_command() {
        let skills: Vec<Skill> = vec![];
        let result = expand_skill_command("regular message", &skills);
        assert_eq!(result, "regular message");
    }

    #[test]
    fn test_expand_skill_command_no_frontmatter_file() {
        use std::fs;
        let tmp = TempDir::new().unwrap();
        // The skill has name matching the dir, but no frontmatter
        let skill_dir = tmp.path().join("simple");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# Simple\nJust instructions.\n").unwrap();

        let skill_path = fs::canonicalize(skill_dir.join("SKILL.md")).unwrap_or_default();
        let skills = vec![Skill {
            name: "simple".to_string(),
            description: "".to_string(),
            file_path: skill_path,
            base_dir: skill_dir,
            disable_model_invocation: false,
        }];

        let result = expand_skill_command("/skill:simple", &skills);
        // No frontmatter to strip, body should contain the raw content
        assert!(result.contains("Just instructions."));
    }
}
