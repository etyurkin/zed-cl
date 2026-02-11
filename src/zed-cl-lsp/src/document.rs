/// Document tracking for LSP server

use std::collections::HashMap;
use tower_lsp::lsp_types::*;

/// Tracks open documents and their content
pub struct DocumentTracker {
    documents: HashMap<Url, String>,
}

impl DocumentTracker {
    pub fn new() -> Self {
        Self {
            documents: HashMap::new(),
        }
    }

    /// Open a new document
    pub fn open(&mut self, uri: Url, text: String) {
        self.documents.insert(uri, text);
    }

    /// Update document with changes
    pub fn change(&mut self, uri: Url, changes: Vec<TextDocumentContentChangeEvent>) {
        if let Some(text) = self.documents.get_mut(&uri) {
            for change in changes {
                if let Some(range) = change.range {
                    // Incremental change
                    apply_change(text, range, &change.text);
                } else {
                    // Full document sync
                    *text = change.text;
                }
            }
        }
    }

    /// Close a document
    pub fn close(&mut self, uri: &Url) {
        self.documents.remove(uri);
    }

    /// Get document text
    pub fn get(&self, uri: &Url) -> Option<&String> {
        self.documents.get(uri)
    }
}

/// Apply an incremental text change to a document
fn apply_change(text: &mut String, range: Range, new_text: &str) {
    let start_offset = position_to_offset(text, range.start);
    let end_offset = position_to_offset(text, range.end);

    let mut new_content = String::new();
    new_content.push_str(&text[..start_offset]);
    new_content.push_str(new_text);
    new_content.push_str(&text[end_offset..]);

    *text = new_content;
}

/// Convert LSP position to byte offset in text
fn position_to_offset(text: &str, position: Position) -> usize {
    let mut current_line = 0;
    let mut offset = 0;

    for (i, ch) in text.char_indices() {
        if current_line == position.line as usize {
            // Found target line, add character offset
            let line_start = offset;
            let line_text = &text[line_start..];
            let char_offset = line_text
                .chars()
                .take(position.character as usize)
                .map(|c| c.len_utf8())
                .sum::<usize>();
            return line_start + char_offset;
        }

        if ch == '\n' {
            current_line += 1;
        }
        offset = i + ch.len_utf8();
    }

    text.len()
}
