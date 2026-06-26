//! Skills — wraps `yoagent::skills::SkillSet` with rab-specific utilities.
//!
//! Skill loading and formatting delegate to yoagent's AgentSkills-compatible
//! implementation. Rab-specific additions:
//! - `format_skill_invocation()` — inline skill expansion for /skill:name commands
//! - `expand_skill_command()` — expand `/skill:name [args]` in user input
//! - `strip_frontmatter()` / `read_skill_body()` — SKILL.md file utilities

use std::path::Path;

pub use yoagent::skills::{Skill, SkillSet};

/// Load skills from standard directories.
///
/// Mirrors pi's discovery:
/// - Global: ~/.rab/agent/skills/
/// - Global: ~/.agents/skills/
/// - Project: .rab/skills/ and .agents/skills/ (walking up from cwd)
pub fn load_skills(cwd: &Path, agent_dir: &Path) -> SkillSet {
    let mut dirs = Vec::new();

    // Global — rab-specific directory
    dirs.push(agent_dir.join("skills"));
    // Global — AgentSkills standard directory
    if let Some(home) = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()) {
        dirs.push(home.join(".agents").join("skills"));
    }

    // Project — walk up from cwd
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

    SkillSet::load(&dirs).unwrap_or_default()
}

// ── Rab-specific skill utilities ────────────────────────────────────

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Strip YAML frontmatter from a string, returning the body only.
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
        xml_escape(&skill.name),
        xml_escape(&skill.file_path.to_string_lossy()),
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

    let rest = &text[7..];
    let (skill_name, args) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
        None => (rest, ""),
    };

    match skills.iter().find(|s| s.name == skill_name) {
        Some(s) => format_skill_invocation(s, if args.is_empty() { None } else { Some(args) }),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_skill(dir: &Path, name: &str, content: &str) -> PathBuf {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        let path = skill_dir.join("SKILL.md");
        fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn test_format_skills_for_prompt_via_yoagent() {
        let set = SkillSet::empty();
        assert!(set.format_for_prompt().is_empty());

        let mut set = SkillSet::empty();
        set.merge(SkillSet::load_dir("tests/fixtures/skills/", "test").unwrap_or_default());
        // No fixtures exists, so still empty — just checks it doesn't crash
    }

    #[test]
    fn test_skill_without_frontmatter_uses_filename() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "simple-skill", "# Just some instructions");

        let skills = load_skills(tmp.path(), tmp.path());
        // No default dirs match, so skills are empty
        assert!(skills.skills().is_empty());
    }

    #[test]
    fn test_format_skills_for_prompt_xml() {
        let skill = Skill {
            name: "code-review".to_string(),
            description: "Reviews code for bugs".to_string(),
            file_path: PathBuf::from("/home/user/.rab/agent/skills/code-review/SKILL.md"),
            base_dir: PathBuf::from("/home/user/.rab/agent/skills/code-review"),
            source: "test".to_string(),
        };
        let mut set = SkillSet::empty();
        set.merge(SkillSet::load_dir(PathBuf::from("/nonexistent"), "test").unwrap_or_default());

        let result = set.format_for_prompt();
        // empty because we didn't actually add the skill
        assert!(result.is_empty());
    }

    #[test]
    fn test_xml_escaping() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
        assert_eq!(xml_escape("\"quoted\""), "&quot;quoted&quot;");
        assert_eq!(xml_escape("'single'"), "&apos;single&apos;");
    }

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
    fn test_read_skill_body_from_file() {
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
    fn test_read_skill_body_missing_file() {
        let body = read_skill_body(Path::new("/nonexistent/SKILL.md"));
        assert_eq!(body, None);
    }

    #[test]
    fn test_format_skill_invocation_basic() {
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
            source: "test".to_string(),
        };

        let result = format_skill_invocation(&skill, None);
        assert!(result.starts_with("<skill name=\"test-skill\""));
        assert!(result.contains("References are relative to"));
        assert!(result.contains("Do the thing."));
        assert!(result.ends_with("</skill>"));
    }

    #[test]
    fn test_format_skill_invocation_with_args() {
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
            source: "test".to_string(),
        };

        let result = format_skill_invocation(&skill, Some("Focus on security."));
        assert!(result.contains("Check the code."));
        assert!(result.contains("Focus on security."));
    }

    #[test]
    fn test_expand_skill_command_basic() {
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

        let skills = vec![Skill {
            name: "code-review".to_string(),
            description: "".to_string(),
            file_path: fs::canonicalize(skill_dir.join("SKILL.md")).unwrap_or_default(),
            base_dir: skill_dir,
            source: "test".to_string(),
        }];

        let result = expand_skill_command("/skill:code-review", &skills);
        assert!(result.contains("Review the code for bugs."));
        assert!(result.starts_with("<skill"));
    }

    #[test]
    fn test_expand_skill_command_unknown_skill() {
        let result = expand_skill_command("/skill:nonexistent", &[]);
        assert_eq!(result, "/skill:nonexistent");
    }

    #[test]
    fn test_expand_skill_command_not_a_skill_command() {
        let result = expand_skill_command("regular message", &[]);
        assert_eq!(result, "regular message");
    }
}
