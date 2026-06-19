use crate::agent::ui::messages::fmt_tokens;
use crate::agent::ui::theme::RabTheme;
use crate::tui::Component;
use crate::tui::util::visible_width;
use crate::types::Usage;

/// Footer component showing cwd, git branch, token stats, and model.
pub struct Footer {
    cwd: String,
    git_branch: Option<String>,
    last_usage: Option<Usage>,
    model: String,
    is_streaming: bool,
    theme: RabTheme,
}

impl Footer {
    pub fn new(cwd: impl Into<String>) -> Self {
        Self {
            cwd: cwd.into(),
            git_branch: None,
            last_usage: None,
            model: String::new(),
            is_streaming: false,
            theme: RabTheme,
        }
    }

    pub fn set_cwd(&mut self, cwd: impl Into<String>) {
        self.cwd = cwd.into();
    }

    pub fn set_git_branch(&mut self, branch: Option<String>) {
        self.git_branch = branch;
    }

    pub fn set_usage(&mut self, usage: Option<Usage>) {
        self.last_usage = usage;
    }

    pub fn set_model(&mut self, model: impl Into<String>) {
        self.model = model.into();
    }

    pub fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
    }
}

impl Component for Footer {
    fn render(&self, width: usize) -> Vec<String> {
        let mut lines = Vec::new();

        // Line 1: cwd + git branch
        let mut line1 = match &self.git_branch {
            Some(branch) => format!("{} ({})", self.cwd, branch),
            None => self.cwd.clone(),
        };
        line1 = self.theme.dim(&line1);
        lines.push(pad_right(&line1, width));

        // Line 2: streaming indicator + model + token stats
        let status_dot = if self.is_streaming {
            self.theme.accent("●")
        } else {
            self.theme.dim("○")
        };

        let model_display = self
            .model
            .strip_prefix("opencode_go::")
            .unwrap_or(&self.model);

        let mut line2 = format!("{} {}", status_dot, model_display);

        if let Some(ref usage) = self.last_usage {
            let in_tok = usage.input_tokens.map(fmt_tokens).unwrap_or_default();
            let out_tok = usage.output_tokens.map(fmt_tokens).unwrap_or_default();
            let cache_tok = usage.cache_tokens.map(fmt_tokens).unwrap_or_default();

            if !in_tok.is_empty() || !out_tok.is_empty() {
                line2.push_str(&format!(" · in:{} out:{}", in_tok, out_tok));
                if !cache_tok.is_empty() {
                    line2.push_str(&format!(" cache:{}", cache_tok));
                }
            }
        }

        line2 = self.theme.dim(&line2);
        lines.push(pad_right(&line2, width));

        lines
    }
}

fn pad_right(s: &str, width: usize) -> String {
    let vw = visible_width(s);
    if vw >= width {
        s.to_string()
    } else {
        format!("{}{}", " ".repeat(width - vw), s)
    }
}
