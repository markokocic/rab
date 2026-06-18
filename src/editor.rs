use ratatui::widgets::Block;

/// Minimal multi-line editor widget.
/// Supports: typing, backspace, delete, arrows, home/end, newline.
pub struct Editor {
    lines: Vec<String>,
    cursor_row: usize,
    cursor_col: usize,
    block: Block<'static>,
}

impl Editor {
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor_row: 0,
            cursor_col: 0,
            block: Block::default(),
        }
    }

    pub fn set_block(&mut self, block: Block<'static>) {
        self.block = block;
    }

    pub fn block(&self) -> &Block<'static> {
        &self.block
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = if text.is_empty() {
            vec![String::new()]
        } else {
            text.split('\n').map(|s| s.to_string()).collect()
        };
        self.cursor_row = self.lines.len() - 1;
        self.cursor_col = self.lines[self.cursor_row].len();
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.contains('\n') {
            for (i, part) in s.split('\n').enumerate() {
                if i == 0 {
                    self.insert_at_cursor(part);
                    self.newline();
                } else {
                    self.insert_at_cursor(part);
                }
            }
        } else {
            self.insert_at_cursor(s);
        }
    }

    fn insert_at_cursor(&mut self, s: &str) {
        let line = &mut self.lines[self.cursor_row];
        line.insert_str(self.cursor_col, s);
        self.cursor_col += s.len();
    }

    pub fn handle_key(&mut self, code: ratatui::crossterm::event::KeyCode, ctrl: bool) {
        match code {
            ratatui::crossterm::event::KeyCode::Char(c) if !ctrl => {
                self.insert_at_cursor(&c.to_string());
            }
            ratatui::crossterm::event::KeyCode::Backspace => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                    self.lines[self.cursor_row].remove(self.cursor_col);
                } else if self.cursor_row > 0 {
                    let rest = self.lines[self.cursor_row].clone();
                    self.lines.remove(self.cursor_row);
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                    self.lines[self.cursor_row].push_str(&rest);
                }
            }
            ratatui::crossterm::event::KeyCode::Delete => {
                let line = &mut self.lines[self.cursor_row];
                if self.cursor_col < line.len() {
                    line.remove(self.cursor_col);
                } else if self.cursor_row + 1 < self.lines.len() {
                    let next = self.lines.remove(self.cursor_row + 1);
                    self.lines[self.cursor_row].push_str(&next);
                }
            }
            ratatui::crossterm::event::KeyCode::Left => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.lines[self.cursor_row].len();
                }
            }
            ratatui::crossterm::event::KeyCode::Right => {
                if self.cursor_col < self.lines[self.cursor_row].len() {
                    self.cursor_col += 1;
                } else if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }
            }
            ratatui::crossterm::event::KeyCode::Up => {
                if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            ratatui::crossterm::event::KeyCode::Down => {
                if self.cursor_row + 1 < self.lines.len() {
                    self.cursor_row += 1;
                    self.cursor_col = self.cursor_col.min(self.lines[self.cursor_row].len());
                }
            }
            ratatui::crossterm::event::KeyCode::Home => {
                self.cursor_col = 0;
            }
            ratatui::crossterm::event::KeyCode::End => {
                self.cursor_col = self.lines[self.cursor_row].len();
            }
            _ => {}
        }
    }

    fn newline(&mut self) {
        let rest = self.lines[self.cursor_row][self.cursor_col..].to_string();
        self.lines[self.cursor_row].truncate(self.cursor_col);
        self.lines.insert(self.cursor_row + 1, rest);
        self.cursor_row += 1;
        self.cursor_col = 0;
    }
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}
