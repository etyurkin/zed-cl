/// Symbol extraction from Lisp source files using TreeSitter + regex fallback

use anyhow::Result;
use regex::Regex;
use tree_sitter::{Node, Parser};
use tree_sitter_commonlisp::LANGUAGE_COMMONLISP;

pub struct SymbolExtractor {
    parser: Parser,
}

#[derive(Debug, Clone)]
pub struct SymbolDef {
    pub name: String,
    pub kind: String,  // "function", "macro", "variable", etc.
    pub line: u32,     // 0-based
    pub character: u32, // 0-based
}

impl SymbolExtractor {
    pub fn new() -> Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&LANGUAGE_COMMONLISP.into())?;
        Ok(Self { parser })
    }

    /// Extract all symbol definitions from source code
    pub fn extract_definitions(&mut self, source: &str) -> Vec<SymbolDef> {
        let tree = match self.parser.parse(source, None) {
            Some(t) => t,
            None => {
                // Parser failed completely, fall back to regex
                return self.extract_with_regex(source);
            }
        };

        let mut definitions = vec![];
        let root = tree.root_node();

        // Always use hybrid approach: TreeSitter + regex
        // TreeSitter might miss SBCL-specific macros like define-list-map

        // Get definitions from tree
        let mut cursor = root.walk();
        'outer: loop {
            let node = cursor.node();

            if let Some(def) = self.extract_definition(source, node) {
                definitions.push(def);
            }

            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    break 'outer;
                }
            }
        }

        // Also extract with regex to catch definitions tree missed (like define-list-map)
        let regex_defs = self.extract_with_regex(source);

        // Merge, avoiding duplicates (same name + line)
        for regex_def in regex_defs {
            let is_duplicate = definitions.iter().any(|d| {
                d.name == regex_def.name && d.line == regex_def.line
            });
            if !is_duplicate {
                definitions.push(regex_def);
            }
        }

        definitions
    }

    /// Fallback regex-based extraction for files with parse errors
    fn extract_with_regex(&self, source: &str) -> Vec<SymbolDef> {
        let mut definitions = vec![];

        // Pattern: (optional-package-prefix:def... name ...) or (define-... name ...)
        // Matches: (defun foo, (sb-xc:defmacro bar, (define-list-map mapcar, etc.
        let def_pattern = Regex::new(
            r"(?m)^\s*\((?:[\w-]+::?)?(?:sb-xc:)?(def(?:un|macro|method|generic|var|parameter|constant|class|struct|package)|define-(?:list-map|macro)|def-?special(?:-operator)?)\s+([^\s()]+)"
        ).unwrap();

        for (line_num, line) in source.lines().enumerate() {
            for cap in def_pattern.captures_iter(line) {
                if let (Some(keyword_match), Some(name_match)) = (cap.get(1), cap.get(2)) {
                    let keyword = keyword_match.as_str();
                    let name = name_match.as_str();

                    let kind = match keyword.to_lowercase().as_str() {
                        "defun" | "defmethod" | "defgeneric" => "function",
                        "defmacro" | "define-macro" => "macro",
                        "defvar" | "defparameter" | "defconstant" => "variable",
                        "defclass" | "defstruct" => "class",
                        "defpackage" => "package",
                        "define-list-map" => "function",
                        "def-special-operator" | "defspecial" => "special-operator",
                        _ => continue,
                    };

                    // Column is position of the name within the line
                    let col = name_match.start();

                    definitions.push(SymbolDef {
                        name: name.to_uppercase(),
                        kind: kind.to_string(),
                        line: line_num as u32,
                        character: col as u32,
                    });
                }
            }
        }

        definitions
    }

    /// Extract package name from (in-package ...) form
    pub fn extract_package(&mut self, source: &str) -> Option<String> {
        let tree = self.parser.parse(source, None)?;
        let root = tree.root_node();
        let mut cursor = root.walk();

        loop {
            let node = cursor.node();

            // Look for (in-package ...)
            if node.kind() == "list_lit" {
                let mut child_cursor = node.walk();
                if child_cursor.goto_first_child() {
                    // Skip opening paren
                    if child_cursor.node().kind() == "(" {
                        if !child_cursor.goto_next_sibling() {
                            continue;
                        }
                    }

                    let first_symbol = child_cursor.node();

                    // Check if it's a symbol "in-package"
                    if let Ok(text) = first_symbol.utf8_text(source.as_bytes()) {
                        if text.eq_ignore_ascii_case("in-package") {
                            // Get the next symbol (package name)
                            if child_cursor.goto_next_sibling() {
                                let pkg_node = child_cursor.node();
                                // Handle both :keyword and "string" package names
                                if let Ok(pkg_text) = pkg_node.utf8_text(source.as_bytes()) {
                                    let pkg_name = pkg_text.trim_start_matches(':')
                                        .trim_matches('"')
                                        .to_uppercase();
                                    return Some(pkg_name);
                                }
                            }
                        }
                    }
                }
            }

            // Traverse
            if cursor.goto_first_child() {
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return None;
                }
            }
        }
    }

    fn extract_definition(&self, source: &str, node: Node) -> Option<SymbolDef> {
        // Pattern 1: (defun name ...) - structured with defun_header
        if node.kind() == "defun" {
            return self.extract_defun_style(source, node);
        }

        // Pattern 2: (defvar name ...) - list_lit with defvar as first symbol
        if node.kind() == "list_lit" {
            return self.extract_list_style(source, node);
        }

        None
    }

    fn extract_defun_style(&self, source: &str, node: Node) -> Option<SymbolDef> {
        // defun structure: defun -> defun_header -> (defun_keyword, sym_lit)
        let mut cursor = node.walk();
        if !cursor.goto_first_child() {
            return None;
        }

        // Find defun_header
        while cursor.node().kind() != "defun_header" {
            if !cursor.goto_next_sibling() {
                return None;
            }
        }

        let header = cursor.node();
        let mut header_cursor = header.walk();

        if !header_cursor.goto_first_child() {
            return None;
        }

        // First child is defun_keyword (defun, defmacro, etc.)
        let keyword_node = header_cursor.node();
        let keyword = keyword_node.utf8_text(source.as_bytes()).ok()?;

        let kind = match keyword.to_lowercase().as_str() {
            "defun" => "function",
            "defmacro" => "macro",
            "defmethod" => "function",
            "defgeneric" => "function",
            _ => "function",
        };

        // Second child is the symbol name
        if !header_cursor.goto_next_sibling() {
            return None;
        }

        let sym_node = header_cursor.node();
        let name = sym_node.utf8_text(source.as_bytes()).ok()?.to_uppercase();

        let pos = sym_node.start_position();

        Some(SymbolDef {
            name,
            kind: kind.to_string(),
            line: pos.row as u32,
            character: pos.column as u32,
        })
    }

    fn extract_list_style(&self, source: &str, node: Node) -> Option<SymbolDef> {
        // (defvar name ...) or (defclass name ...) or (sb-xc:defmacro name ...)
        let mut cursor = node.walk();
        if !cursor.goto_first_child() {
            return None;
        }

        // Skip opening paren - it's the first child of list_lit
        if cursor.node().kind() == "(" {
            if !cursor.goto_next_sibling() {
                return None;
            }
        }

        // Now we should be at the definition keyword (possibly package-qualified)
        let first_symbol = cursor.node();

        // Handle package-qualified symbols like sb-xc:defmacro
        let keyword = if first_symbol.kind() == "package_lit" {
            // For package_lit, navigate to the last sym_lit child (the actual keyword)
            let mut pkg_cursor = first_symbol.walk();
            let mut last_sym = None;
            if pkg_cursor.goto_first_child() {
                loop {
                    if pkg_cursor.node().kind() == "sym_lit" {
                        last_sym = Some(pkg_cursor.node());
                    }
                    if !pkg_cursor.goto_next_sibling() {
                        break;
                    }
                }
            }
            last_sym?.utf8_text(source.as_bytes()).ok()?
        } else {
            first_symbol.utf8_text(source.as_bytes()).ok()?
        };

        let kind = match keyword.to_lowercase().as_str() {
            "defmacro" => "macro",
            "defun" => "function",
            "defmethod" => "function",
            "defgeneric" => "function",
            "defvar" | "defparameter" | "defconstant" => "variable",
            "defclass" => "class",
            "defpackage" => "package",
            "defstruct" => "class",
            // SBCL-specific definition macros
            "define-list-map" => "function",  // mapc, mapcar, mapcan, etc.
            "def-special-operator" => "special-operator",
            "defspecial" => "special-operator",
            "define-macro" => "macro",
            _ => return None, // Not a definition form we care about
        };

        // Second child is the symbol name
        if !cursor.goto_next_sibling() {
            return None;
        }

        let sym_node = cursor.node();
        let name = sym_node.utf8_text(source.as_bytes()).ok()?.to_uppercase();

        let pos = sym_node.start_position();

        Some(SymbolDef {
            name,
            kind: kind.to_string(),
            line: pos.row as u32,
            character: pos.column as u32,
        })
    }
}
