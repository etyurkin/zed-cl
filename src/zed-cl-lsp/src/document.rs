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
            // Found target line; count UTF-16 units (the LSP wire format) and
            // stop at the end of the line so an overshoot cannot spill into
            // the next one.
            let line_start = offset;
            let mut units = 0u32;
            let mut bytes = 0usize;
            for c in text[line_start..].chars() {
                if units >= position.character || c == '\n' {
                    break;
                }
                units += c.len_utf16() as u32;
                bytes += c.len_utf8();
            }
            return line_start + bytes;
        }

        if ch == '\n' {
            current_line += 1;
        }
        offset = i + ch.len_utf8();
    }

    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_change_uses_utf16_columns() {
        // A non-BMP char is 1 char but 2 UTF-16 units; the old char-counting
        // math placed this edit past the end of the string.
        let mut text = String::from("(\u{1f600})");
        let range = Range {
            start: Position { line: 0, character: 3 },
            end: Position { line: 0, character: 4 },
        };
        apply_change(&mut text, range, " x)");
        assert_eq!(text, "(\u{1f600} x)");
    }
}
