use crate::agent::extension::{AgentTool, Extension};
use anyhow::Context;
use async_trait::async_trait;
use std::borrow::Cow;

pub struct EditExtension {
    cwd: std::path::PathBuf,
}

impl EditExtension {
    pub fn new(cwd: std::path::PathBuf) -> Self {
        Self { cwd }
    }
}

impl Extension for EditExtension {
    fn name(&self) -> Cow<'static, str> {
        "edit".into()
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(EditTool {
            cwd: self.cwd.clone(),
        })]
    }
}

struct EditTool {
    cwd: std::path::PathBuf,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Edit {
    old_text: String,
    new_text: String,
}

#[async_trait]
impl AgentTool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit a single file using exact text replacement. Every edits[].oldText must match a \
         unique, non-overlapping region of the original file. If two changes affect the same \
         block or nearby lines, merge them into one edit instead of emitting overlapping edits. \
         Do not include large unchanged regions just to connect distant changes."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["path", "edits"],
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the file to edit (relative or absolute)"
                },
                "edits": {
                    "type": "array",
                    "description": "One or more targeted replacements. Each edit is matched against the original file, not incrementally. Do not include overlapping or nested edits.",
                    "items": {
                        "type": "object",
                        "required": ["oldText", "newText"],
                        "properties": {
                            "oldText": {
                                "type": "string",
                                "description": "Exact text for one targeted replacement. Must be unique in the file."
                            },
                            "newText": {
                                "type": "string",
                                "description": "Replacement text for this edit."
                            }
                        }
                    }
                }
            }
        })
    }

    fn label(&self) -> &str {
        "Make precise file edits"
    }

    async fn execute(
        &self,
        tool_call_id: String,
        args: serde_json::Value,
    ) -> anyhow::Result<String> {
        let _ = tool_call_id;
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

        let edits: Vec<Edit> = serde_json::from_value(args["edits"].clone())
            .map_err(|e| anyhow::anyhow!("Invalid edits: {}", e))?;

        if edits.is_empty() {
            return Err(anyhow::anyhow!("At least one edit is required"));
        }

        let abs_path = {
            let p = std::path::Path::new(path);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                self.cwd.join(p)
            }
        };

        let original = std::fs::read_to_string(&abs_path)
            .with_context(|| format!("Failed to read {}", abs_path.display()))?;

        // Validate each oldText appears exactly once in the original
        for (i, edit) in edits.iter().enumerate() {
            let count = original.match_indices(&edit.old_text).count();
            if count == 0 {
                return Err(anyhow::anyhow!(
                    "edits[{}].oldText not found in file. Ensure the text matches exactly including whitespace.",
                    i
                ));
            }
            if count > 1 {
                return Err(anyhow::anyhow!(
                    "edits[{}].oldText matches {} locations. Make it more specific to be unique.",
                    i,
                    count
                ));
            }
        }

        // Check for overlapping edits
        for (i, edit_i) in edits.iter().enumerate() {
            let pos_i = original.find(&edit_i.old_text).unwrap();
            let end_i = pos_i + edit_i.old_text.len();
            for (j, edit_j) in edits.iter().enumerate().skip(i + 1) {
                let pos_j = original.find(&edit_j.old_text).unwrap();
                let end_j = pos_j + edit_j.old_text.len();
                if pos_i < end_j && pos_j < end_i {
                    return Err(anyhow::anyhow!(
                        "edits[{}] and edits[{}] overlap. Merge them into one edit.",
                        i,
                        j
                    ));
                }
            }
        }

        // Apply edits (replace largest spans first to preserve positions)
        let mut edits_with_pos: Vec<(usize, usize, &Edit)> = edits
            .iter()
            .map(|e| {
                let pos = original.find(&e.old_text).unwrap();
                (pos, pos + e.old_text.len(), e)
            })
            .collect();
        edits_with_pos.sort_by_key(|(pos, _, _)| *pos);

        let mut result = String::new();
        let mut cursor = 0;
        for (start, end, edit) in &edits_with_pos {
            result.push_str(&original[cursor..*start]);
            result.push_str(&edit.new_text);
            cursor = *end;
        }
        result.push_str(&original[cursor..]);

        std::fs::write(&abs_path, &result)
            .with_context(|| format!("Failed to write {}", abs_path.display()))?;

        Ok(format!(
            "Successfully replaced {} block(s) in {}.",
            edits.len(),
            path
        ))
    }
}
