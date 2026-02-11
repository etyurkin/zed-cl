use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

enum LineType {
    Input,
    Output,
}

struct HistoryLine {
    content: String,
    line_type: LineType,
}

struct App {
    lines: Vec<HistoryLine>,
    current_input: String,
    input_lines: Vec<String>,  // For multiline input
    current_package: String,
    cursor_position: usize,  // Cursor position within current line
    cursor_line: usize,  // Which line the cursor is on (0 = current input, 1+ = history from bottom)
    scroll_offset: usize,  // Manual scroll offset
    visible_height: usize,  // Visible screen height (updated during render)
    request_counter: u32,
    socket: UnixStream,
}

impl App {
    fn is_complete_sexp(&self) -> bool {
        // Count parentheses to determine if S-expression is complete
        let full_input = if self.input_lines.is_empty() {
            self.current_input.clone()
        } else {
            let mut all_lines = self.input_lines.clone();
            all_lines.push(self.current_input.clone());
            all_lines.join("\n")
        };

        let mut depth = 0;
        let mut in_string = false;
        let mut in_comment = false;
        let mut escape_next = false;

        for ch in full_input.chars() {
            if escape_next {
                escape_next = false;
                continue;
            }

            match ch {
                '\\' if in_string => escape_next = true,
                '"' if !in_comment => in_string = !in_string,
                ';' if !in_string => in_comment = true,
                '\n' => in_comment = false,
                '(' if !in_string && !in_comment => depth += 1,
                ')' if !in_string && !in_comment => depth -= 1,
                _ => {}
            }
        }

        // Complete if depth is 0 and not in string
        depth == 0 && !in_string && !full_input.trim().is_empty()
    }

    fn new() -> Result<Self> {
        // Get config (reads from ~/.zed-cl/config or ZED_CL_LISP_IMPL env var, defaults to SBCL)
        let config = common_rust::Config::get();
        let lisp_impl = config.lisp_impl.clone();

        // Use MasterReplClient to connect (with auto-start)
        let mut repl_client = common_rust::MasterReplClient::new("unused");
        repl_client.connect()
            .context("Failed to connect to master REPL")?;

        // Get the socket from the client
        let socket_path = common_rust::get_socket_path();
        let socket = UnixStream::connect(&socket_path)
            .context(format!("Failed to connect to master REPL at {}", socket_path.to_string_lossy()))?;

        let mut lines = Vec::new();
        lines.push(HistoryLine {
            content: format!("; Common Lisp REPL ({})", lisp_impl),
            line_type: LineType::Output,
        });
        lines.push(HistoryLine {
            content: format!("; Connected to: {}", socket_path.to_string_lossy()),
            line_type: LineType::Output,
        });
        lines.push(HistoryLine {
            content: "; Press Ctrl+D to exit".to_string(),
            line_type: LineType::Output,
        });
        lines.push(HistoryLine {
            content: "".to_string(),
            line_type: LineType::Output,
        });

        Ok(Self {
            lines,
            current_input: String::new(),
            input_lines: Vec::new(),
            current_package: "CL-USER".to_string(),
            cursor_position: 0,
            cursor_line: 0,
            scroll_offset: 0,
            visible_height: 24,  // Default, will be updated during render
            request_counter: 0,
            socket,
        })
    }

    fn send_eval(&mut self, code: &str) -> Result<()> {
        self.request_counter += 1;
        let id = format!("tui-{}", self.request_counter);

        // Send request as s-expression
        let request = format!(
            "(:type \"eval\" :id \"{}\" :code \"{}\")\n",
            id,
            code.replace("\\", "\\\\").replace("\"", "\\\"")
        );

        self.socket
            .write_all(request.as_bytes())
            .context("Failed to send request")?;
        self.socket
            .flush()
            .context("Failed to flush socket")?;

        // Read complete s-expression response (can span multiple lines)
        // We need to read character by character and track parentheses depth
        let mut response_str = String::new();
        let mut byte_buf = [0u8; 1];
        let mut paren_depth = 0;
        let mut in_string = false;
        let mut escape_next = false;
        let mut started = false;

        loop {
            match Read::read(&mut self.socket, &mut byte_buf) {
                Ok(0) => {
                    return Err(anyhow::anyhow!("Connection closed by master REPL"));
                }
                Ok(_) => {
                    let ch = byte_buf[0] as char;
                    response_str.push(ch);

                    if escape_next {
                        escape_next = false;
                        continue;
                    }

                    match ch {
                        '\\' if in_string => {
                            escape_next = true;
                        }
                        '"' => {
                            in_string = !in_string;
                        }
                        '(' if !in_string => {
                            paren_depth += 1;
                            started = true;
                        }
                        ')' if !in_string => {
                            paren_depth -= 1;
                            // Complete s-expression when parens are balanced
                            if started && paren_depth == 0 {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Err(e) => {
                    return Err(e).context("Failed to read response");
                }
            }
        }

        // Simple S-expression parsing
        // Extract :OUTPUT "..."
        if let Some(start) = response_str.find(":OUTPUT \"") {
            let start = start + 9;
            if let Some(end) = response_str[start..].find('"') {
                let output = &response_str[start..start + end];
                if !output.is_empty() {
                    // Unescape and split by lines
                    let output = output.replace("\\n", "\n");
                    for line in output.lines() {
                        self.lines.push(HistoryLine {
                            content: line.to_string(),
                            line_type: LineType::Output,
                        });
                    }
                }
            }
        }

        // Extract :ERROR
        if response_str.contains(":ERROR \"") {
            if let Some(start) = response_str.find(":ERROR \"") {
                let start = start + 8;
                if let Some(end) = response_str[start..].find('"') {
                    let error = &response_str[start..start + end];
                    self.lines.push(HistoryLine {
                        content: format!("; Evaluation aborted: {}", error),
                        line_type: LineType::Output,
                    });
                }
            }
        } else if response_str.contains(":VALUES NIL") {
            // VALUES is NIL
            self.lines.push(HistoryLine {
                content: "NIL".to_string(),
                line_type: LineType::Output,
            });
        } else if let Some(start) = response_str.find(":VALUES (") {
            // VALUES is a list - parse quoted strings with proper escaping
            let start = start + 9;
            if let Some(end) = response_str[start..].find(") :") {
                let values_section = &response_str[start..start + end];

                // Parse quoted strings manually to handle escaped quotes
                let mut values = Vec::new();
                let mut chars = values_section.chars().peekable();

                while let Some(ch) = chars.next() {
                    if ch == '"' {
                        // Start of a quoted string
                        let mut value = String::new();
                        let mut escaped = false;

                        while let Some(ch) = chars.next() {
                            if escaped {
                                // Handle escape sequences
                                match ch {
                                    'n' => value.push('\n'),
                                    't' => value.push('\t'),
                                    'r' => value.push('\r'),
                                    '\\' => value.push('\\'),
                                    '"' => value.push('"'),
                                    _ => {
                                        value.push('\\');
                                        value.push(ch);
                                    }
                                }
                                escaped = false;
                            } else if ch == '\\' {
                                escaped = true;
                            } else if ch == '"' {
                                // End of quoted string
                                break;
                            } else {
                                value.push(ch);
                            }
                        }

                        if !value.is_empty() {
                            values.push(value);
                        }
                    }
                }

                for value in values {
                    self.lines.push(HistoryLine {
                        content: value,
                        line_type: LineType::Output,
                    });
                }
            }
        }

        // Add blank line after result
        self.lines.push(HistoryLine {
            content: "".to_string(),
            line_type: LineType::Output,
        });

        Ok(())
    }

    fn handle_key_event(&mut self, key: KeyEvent) -> Result<bool> {
        // DEBUG - log all key events to file
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/repl-debug.log")
        {
            let _ = writeln!(f, "key={:?} kind={:?} mods={:?}", key.code, key.kind, key.modifiers);
        }

        // Only process KeyPress events, ignore KeyRelease/KeyRepeat
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }

        match (key.code, key.modifiers) {
            // Exit on Ctrl+D (only if input is empty, like real SBCL)
            (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                if self.current_input.is_empty() && self.input_lines.is_empty() {
                    return Ok(true);
                }
            }
            // Cancel/abort current input on Ctrl+C
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                if !self.current_input.is_empty() || !self.input_lines.is_empty() {
                    self.current_input.clear();
                    self.input_lines.clear();
                    self.cursor_position = 0;
                    self.cursor_line = 0;
                    self.lines.push(HistoryLine {
                        content: "; Aborted".to_string(),
                        line_type: LineType::Output,
                    });
                }
            }
            // Submit on Enter (only if at current input line)
            (KeyCode::Enter, _) => {
                // Only allow submitting when at current input (cursor_line == 0)
                if self.cursor_line > 0 {
                    // In history - jump back to current input instead
                    self.cursor_line = 0;
                    self.cursor_position = self.current_input.len();
                    self.scroll_offset = 0;
                } else if self.is_complete_sexp() {
                    // Complete S-expression - submit it
                    let full_code = if self.input_lines.is_empty() {
                        self.current_input.clone()
                    } else {
                        let mut all_lines = self.input_lines.clone();
                        all_lines.push(self.current_input.clone());
                        all_lines.join("\n")
                    };

                    // Check if the code is actually non-empty (not just whitespace)
                    let has_content = !full_code.trim().is_empty();

                    // Check for quit commands before sending to master REPL
                    let trimmed_code = full_code.trim();
                    if trimmed_code == "(quit)" || trimmed_code == "(exit)" || trimmed_code == ":q" {
                        // Exit the TUI without sending to master REPL
                        return Ok(true);
                    }

                    // Add all input lines to history (only if non-empty)
                    // Don't show empty submissions or prompts in history
                    if has_content {
                        // Add input to history WITHOUT the prompt
                        // The prompt is only shown in the live input area
                        if self.input_lines.is_empty() {
                            // Single line - just add the input
                            self.lines.push(HistoryLine {
                                content: self.current_input.clone(),
                                line_type: LineType::Input,
                            });
                        } else {
                            // Multiline - collect non-empty lines for display
                            for line in &self.input_lines {
                                if !line.trim().is_empty() {
                                    self.lines.push(HistoryLine {
                                        content: line.clone(),
                                        line_type: LineType::Input,
                                    });
                                }
                            }
                            if !self.current_input.trim().is_empty() {
                                self.lines.push(HistoryLine {
                                    content: self.current_input.clone(),
                                    line_type: LineType::Input,
                                });
                            }
                        }

                        // Send for evaluation
                        self.send_eval(&full_code)?;
                    }

                    self.current_input.clear();
                    self.input_lines.clear();
                    self.cursor_position = 0;
                    self.cursor_line = 0;
                    self.scroll_offset = 0;  // Reset scroll to bottom on new eval
                } else {
                    // Incomplete S-expression - continue on next line
                    // Only add non-blank lines to input_lines
                    if !self.current_input.trim().is_empty() {
                        self.input_lines.push(self.current_input.clone());
                    }
                    self.current_input.clear();
                    self.cursor_position = 0;
                    self.cursor_line = 0;
                }
            }
            // Backspace
            (KeyCode::Backspace, _) => {
                if self.cursor_position > 0 {
                    // Find the character boundary before cursor
                    let mut idx = self.cursor_position - 1;
                    while idx > 0 && !self.current_input.is_char_boundary(idx) {
                        idx -= 1;
                    }
                    self.current_input.drain(idx..self.cursor_position);
                    self.cursor_position = idx;
                }
            }
            // Delete
            (KeyCode::Delete, _) => {
                if self.cursor_position < self.current_input.len() {
                    // Find the next character boundary
                    let mut idx = self.cursor_position + 1;
                    while idx < self.current_input.len() && !self.current_input.is_char_boundary(idx) {
                        idx += 1;
                    }
                    self.current_input.drain(self.cursor_position..idx);
                }
            }
            // Move cursor left (ignore if Shift is pressed for text selection)
            (KeyCode::Left, modifiers) if !modifiers.contains(KeyModifiers::SHIFT) => {
                if self.cursor_position > 0 {
                    // Find the previous character boundary
                    let mut idx = self.cursor_position - 1;
                    while idx > 0 && !self.current_input.is_char_boundary(idx) {
                        idx -= 1;
                    }
                    self.cursor_position = idx;
                } else {
                    // At start of line - wrap to end of line above (cursor_line + 1)
                    let total_history_lines = self.lines.len() + self.input_lines.len();
                    if self.cursor_line < total_history_lines {
                        self.cursor_line += 1;
                        // Move to end of the new line
                        let line_text = if self.cursor_line == 0 {
                            &self.current_input
                        } else if self.cursor_line <= self.input_lines.len() {
                            let idx = self.input_lines.len() - self.cursor_line;
                            &self.input_lines[idx]
                        } else {
                            let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                            &self.lines[idx].content
                        };
                        self.cursor_position = line_text.len();
                    }
                }
            }
            // Move cursor right (ignore if Shift is pressed for text selection)
            (KeyCode::Right, modifiers) if !modifiers.contains(KeyModifiers::SHIFT) => {
                // Get current line length
                let current_line_len = if self.cursor_line == 0 {
                    self.current_input.len()
                } else if self.cursor_line <= self.input_lines.len() {
                    let idx = self.input_lines.len() - self.cursor_line;
                    self.input_lines[idx].len()
                } else {
                    let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                    self.lines[idx].content.len()
                };

                if self.cursor_position < current_line_len {
                    // Find the next character boundary
                    let mut idx = self.cursor_position + 1;
                    let line_text = if self.cursor_line == 0 {
                        &self.current_input
                    } else if self.cursor_line <= self.input_lines.len() {
                        let line_idx = self.input_lines.len() - self.cursor_line;
                        &self.input_lines[line_idx]
                    } else {
                        let line_idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                        &self.lines[line_idx].content
                    };
                    while idx < line_text.len() && !line_text.is_char_boundary(idx) {
                        idx += 1;
                    }
                    self.cursor_position = idx;
                } else {
                    // At end of line - wrap to start of line below (cursor_line - 1)
                    if self.cursor_line > 0 {
                        self.cursor_line -= 1;
                        self.cursor_position = 0;
                    }
                }
            }
            // Ctrl+a - Jump to beginning of line
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.cursor_position = 0;
            }
            // Ctrl+e - Jump to end of line
            (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                let line_len = if self.cursor_line == 0 {
                    self.current_input.len()
                } else if self.cursor_line <= self.input_lines.len() {
                    let idx = self.input_lines.len() - self.cursor_line;
                    self.input_lines[idx].len()
                } else {
                    let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                    self.lines[idx].content.len()
                };
                self.cursor_position = line_len;
            }
            // Option+Shift+< (produces ¯ on Mac) - Jump to top
            (KeyCode::Char('¯'), _) => {
                let total_history_lines = self.lines.len() + self.input_lines.len();
                self.cursor_line = total_history_lines;
                self.cursor_position = 0;
                // Set scroll_offset to non-zero to indicate manual navigation mode
                self.scroll_offset = 1;
            }
            // Option+Shift+> (produces ˘ on Mac) - Jump to bottom (current input)
            (KeyCode::Char('˘'), _) => {
                self.cursor_line = 0;
                self.cursor_position = self.current_input.len();
            }
            // Up arrow - Move cursor up one line in the history (ignore if Shift is pressed for text selection)
            (KeyCode::Up, modifiers) if !modifiers.contains(KeyModifiers::SHIFT) => {
                // Total available lines = history + input_lines + current line
                let total_history_lines = self.lines.len() + self.input_lines.len();
                if self.cursor_line < total_history_lines {
                    self.cursor_line += 1;
                    // Clamp cursor position to the new line's length
                    let line_text = if self.cursor_line == 0 {
                        &self.current_input
                    } else if self.cursor_line <= self.input_lines.len() {
                        let idx = self.input_lines.len() - self.cursor_line;
                        &self.input_lines[idx]
                    } else {
                        let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                        &self.lines[idx].content
                    };
                    self.cursor_position = self.cursor_position.min(line_text.len());
                }
            }
            // Down arrow - Move cursor down one line (ignore if Shift is pressed for text selection)
            (KeyCode::Down, modifiers) if !modifiers.contains(KeyModifiers::SHIFT) => {
                if self.cursor_line > 0 {
                    self.cursor_line -= 1;
                    // Clamp cursor position to the new line's length
                    let line_text = if self.cursor_line == 0 {
                        &self.current_input
                    } else if self.cursor_line <= self.input_lines.len() {
                        let idx = self.input_lines.len() - self.cursor_line;
                        &self.input_lines[idx]
                    } else {
                        let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                        &self.lines[idx].content
                    };
                    self.cursor_position = self.cursor_position.min(line_text.len());

                    // If returning to current input, re-enable auto-scroll mode
                    if self.cursor_line == 0 {
                        self.scroll_offset = 0;
                    }
                }
            }
            // Alternative: Escape to jump to bottom (commonly available)
            (KeyCode::Esc, _) => {
                self.scroll_offset = 0;
            }
            // PageUp/PageDown if available on keyboard
            (KeyCode::PageUp, _) => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            (KeyCode::PageDown, _) => {
                if self.scroll_offset > 0 {
                    self.scroll_offset = self.scroll_offset.saturating_sub(10);
                }
            }
            // Option+v (produces √ on Mac) - Page up
            (KeyCode::Char('√'), _) => {
                let total_history_lines = self.lines.len() + self.input_lines.len();
                let page_size = self.visible_height.max(1);

                // Scroll up by page_size lines
                self.cursor_line = (self.cursor_line + page_size).min(total_history_lines);

                // Enable manual scroll mode if we moved
                if self.cursor_line > 0 {
                    self.scroll_offset = 1;
                }

                // Clamp cursor position to new line's length
                let line_text = if self.cursor_line == 0 {
                    &self.current_input
                } else if self.cursor_line <= self.input_lines.len() {
                    let idx = self.input_lines.len() - self.cursor_line;
                    &self.input_lines[idx]
                } else {
                    let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                    &self.lines[idx].content
                };
                self.cursor_position = self.cursor_position.min(line_text.len());
            }
            // Ctrl+v - Page down
            (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
                let page_size = self.visible_height.max(1);

                // Scroll down by page_size lines
                let old_cursor_line = self.cursor_line;
                if self.cursor_line >= page_size {
                    self.cursor_line -= page_size;
                } else {
                    self.cursor_line = 0;
                }

                // Adjust scroll_offset by the same amount we moved the cursor
                if self.scroll_offset > 0 {
                    let moved = old_cursor_line - self.cursor_line;
                    if self.scroll_offset >= moved {
                        self.scroll_offset -= moved;
                    } else {
                        self.scroll_offset = 0;
                    }
                }

                // Clamp cursor position to new line's length
                let line_text = if self.cursor_line == 0 {
                    &self.current_input
                } else if self.cursor_line <= self.input_lines.len() {
                    let idx = self.input_lines.len() - self.cursor_line;
                    &self.input_lines[idx]
                } else {
                    let idx = self.lines.len() - (self.cursor_line - self.input_lines.len());
                    &self.lines[idx].content
                };
                self.cursor_position = self.cursor_position.min(line_text.len());

                // If returning to current input, re-enable auto-scroll mode
                if self.cursor_line == 0 {
                    self.scroll_offset = 0;
                }
            }
            // Home - move cursor to start
            (KeyCode::Home, _) => {
                self.cursor_position = 0;
            }
            // End - move cursor to end
            (KeyCode::End, _) => {
                self.cursor_position = self.current_input.len();
            }
            // Regular character input (ignore Alt combinations to avoid special chars)
            (KeyCode::Char(c), m) if !m.contains(KeyModifiers::ALT) && !m.contains(KeyModifiers::CONTROL) => {
                // Ensure cursor position is valid for multi-byte UTF-8
                if self.cursor_position <= self.current_input.len() {
                    self.current_input.insert(self.cursor_position, c);
                    self.cursor_position += c.len_utf8();
                }
            }
            _ => {}
        }
        Ok(false)
    }
}

fn run_app() -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new()?;

    // Main loop
    loop {
        terminal.draw(|f| {
            // Create all lines including the current prompt
            let mut all_lines: Vec<Line> = app
                .lines
                .iter()
                .map(|hist_line| {
                    match hist_line.line_type {
                        LineType::Output => {
                            // Output in italic, default color
                            Line::from(Span::styled(
                                hist_line.content.clone(),
                                Style::default().add_modifier(Modifier::ITALIC),
                            ))
                        }
                        LineType::Input => {
                            // Input in bright white
                            Line::from(Span::styled(
                                hist_line.content.clone(),
                                Style::default().fg(Color::White),
                            ))
                        }
                    }
                })
                .collect();

            // Add previous input lines (for multiline input in progress)
            if !app.input_lines.is_empty() {
                // First line with prompt - in bright white
                let first_prompt = if app.current_package == "CL-USER" {
                    "* ".to_string()
                } else {
                    format!("{}> ", app.current_package)
                };
                all_lines.push(Line::from(Span::styled(
                    format!("{}{}", first_prompt, app.input_lines[0]),
                    Style::default().fg(Color::White),
                )));

                // Continuation lines (no prompt) - in bright white
                for line in &app.input_lines[1..] {
                    all_lines.push(Line::from(Span::styled(
                        line.clone(),
                        Style::default().fg(Color::White),
                    )));
                }
            }

            // Add scroll indicator at the top if scrolled up
            if app.scroll_offset > 0 {
                all_lines.insert(0, Line::from(format!(
                    "=== SCROLLED (offset: {}) | ↑/↓ scroll | ESC=bottom ===",
                    app.scroll_offset
                )));
            }

            // Add current prompt with input - in bright white
            let prompt_line = if app.input_lines.is_empty() {
                // First line - show full prompt
                if app.current_package == "CL-USER" {
                    if app.current_input.is_empty() {
                        "* ".to_string()
                    } else {
                        format!("* {}", app.current_input)
                    }
                } else {
                    if app.current_input.is_empty() {
                        format!("{}> ", app.current_package)
                    } else {
                        format!("{}> {}", app.current_package, app.current_input)
                    }
                }
            } else {
                // Continuation line - no prompt
                app.current_input.clone()
            };
            all_lines.push(Line::from(Span::styled(
                prompt_line,
                Style::default().fg(Color::White),
            )));

            // Render as a single scrollable paragraph (no border)
            // Calculate cursor's absolute line position from bottom
            let total_lines = all_lines.len();
            let visible_height = f.area().height as usize;
            app.visible_height = visible_height;  // Store for use in key handlers
            let cursor_absolute_line = total_lines - 1 - app.cursor_line;  // Line index where cursor is

            // Calculate maximum scroll position
            let max_scroll = if total_lines > visible_height {
                total_lines - visible_height
            } else {
                0
            };

            // Determine initial scroll position
            // If cursor is at current input (cursor_line == 0) and scroll_offset is 0, auto-scroll to bottom
            // Otherwise, use the stored scroll position
            let mut scroll_offset = if app.cursor_line == 0 && app.scroll_offset == 0 {
                // At current input and in auto-scroll mode - scroll to bottom
                max_scroll as u16
            } else if app.scroll_offset == 0 {
                // Just started navigating - keep at bottom until cursor goes off-screen
                max_scroll as u16
            } else {
                // Manual navigation - use stored scroll position
                app.scroll_offset.min(max_scroll) as u16
            };

            // Adjust scroll only if cursor is off-screen
            // Note: Only adjust for display, don't write back to app.scroll_offset
            // to avoid visible jumping when scrolling
            if cursor_absolute_line < scroll_offset as usize {
                // Cursor is above visible area - scroll up just enough to show it at top
                scroll_offset = cursor_absolute_line as u16;
            } else if cursor_absolute_line >= (scroll_offset as usize + visible_height) {
                // Cursor is below visible area - scroll down just enough to show it at bottom
                scroll_offset = (cursor_absolute_line - visible_height + 1) as u16;
            }

            let paragraph = Paragraph::new(all_lines.clone())
                .wrap(Wrap { trim: false })
                .scroll((scroll_offset, 0));

            f.render_widget(paragraph, f.area());

            // Calculate cursor screen position
            let cursor_screen_y = cursor_absolute_line.saturating_sub(scroll_offset as usize);
            let cursor_y = f.area().y + cursor_screen_y as u16;

            // Calculate prompt length for cursor positioning
            let prompt_len = if app.cursor_line == 0 {
                // Current input line
                if app.input_lines.is_empty() {
                    if app.current_package == "CL-USER" {
                        2
                    } else {
                        app.current_package.len() + 2
                    }
                } else {
                    0
                }
            } else if app.cursor_line <= app.input_lines.len() {
                // In progress input lines
                let idx = app.input_lines.len() - app.cursor_line;
                if idx == 0 {
                    if app.current_package == "CL-USER" {
                        2
                    } else {
                        app.current_package.len() + 2
                    }
                } else {
                    0
                }
            } else {
                // History lines - no prompts
                0
            };

            let cursor_x = f.area().x + prompt_len as u16 + app.cursor_position as u16;

            f.set_cursor_position((cursor_x, cursor_y));
        })?;

        // Handle events
        if event::poll(std::time::Duration::from_millis(100))? {
            let evt = event::read()?;
            // DEBUG
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open("/tmp/repl-debug.log")
            {
                let _ = writeln!(f, "Event received: {:?}", evt);
            }

            if let Event::Key(key) = evt {
                if app.handle_key_event(key)? {
                    break;
                }
            }
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn main() {
    if let Err(e) = run_app() {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}
