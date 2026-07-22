//! Utility and helper functions extracted from app.rs to reduce file size.

/// Parse a bang (!) command from user input.
/// Returns Some((command, is_abort)) where is_abort is true for !! commands.
pub fn parse_bang_command(input: &str) -> Option<(String, bool)> {
    if let Some(rest) = input.strip_prefix("!!") {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), true))
        }
    } else if let Some(rest) = input.strip_prefix('!') {
        let cmd = rest.trim();
        if cmd.is_empty() {
            None
        } else {
            Some((cmd.to_string(), false))
        }
    } else {
        None
    }
}

// ── XML / text utilities ──────────────────────────────────────────

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

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

pub fn read_skill_body(file_path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(file_path).ok()?;
    Some(strip_frontmatter(&content))
}

pub fn format_skill_invocation(
    skill: &yoagent::skills::Skill,
    extra: Option<&str>,
) -> Option<String> {
    let body = read_skill_body(&skill.file_path)?;
    let block = format!(
        r#"<skill name="{}" location="{}">
References are relative to {}.

{}
</skill>"#,
        xml_escape(&skill.name),
        xml_escape(&skill.file_path.to_string_lossy()),
        xml_escape(&skill.base_dir.to_string_lossy()),
        body
    );
    Some(match extra {
        Some(instr) if !instr.is_empty() => format!("{}\n\n{}", block, instr),
        _ => block,
    })
}

pub fn expand_skill_command(text: &str, skills: &[yoagent::skills::Skill]) -> String {
    if !text.starts_with("/skill:") {
        return text.to_string();
    }
    let rest = &text[7..];
    let (skill_name, args) = match rest.find(' ') {
        Some(pos) => (&rest[..pos], rest[pos + 1..].trim()),
        None => (rest, ""),
    };
    match skills.iter().find(|s| s.name == skill_name) {
        Some(s) => format_skill_invocation(s, if args.is_empty() { None } else { Some(args) })
            .unwrap_or_else(|| text.to_string()),
        None => text.to_string(),
    }
}

/// Parse a skill block from text (pi-compatible).
/// Returns Some((name, body, user_message)) if the text is a skill block.
pub fn parse_skill_block(text: &str) -> Option<(&str, &str, Option<&str>)> {
    let text = text.trim();
    let after_open = text.strip_prefix("<skill name=\"")?;
    let (name, rest) = after_open.split_once("\" location=\"")?;
    let (_location, rest) = rest.split_once("\">\n")?;
    // Find closing tag to extract body
    let close_tag = "\n</skill>";
    let content_end = rest.rfind(close_tag)?;
    let body = rest[..content_end].trim();
    let after_close = rest[content_end + close_tag.len()..].trim();
    let user_message = if after_close.is_empty() {
        None
    } else {
        Some(after_close)
    };
    Some((name, body, user_message))
}

/// Format a skill block for display (prettify XML into a readable form).
/// Returns None if the text is not a skill block.
pub fn format_skill_block_for_display(text: &str) -> Option<String> {
    let (name, body, user_message) = parse_skill_block(text)?;
    let mut result = String::new();
    // Markdown bold label: **[skill] name**
    result.push_str("**[");
    result.push_str("skill] ");
    result.push_str(name);
    result.push_str("**\n\n");
    // Body content
    result.push_str(body);
    result.push('\n');
    // Append user message if present
    if let Some(msg) = user_message {
        result.push_str("\n---\n");
        result.push_str(msg);
        result.push('\n');
    }
    Some(result)
}
