/// Tree-sitter based symbol extraction for Common Lisp
///
/// This provides more robust symbol extraction using the tree-sitter grammar

use tower_lsp::lsp_types::Position;
use tree_sitter::{Node, Parser, Point};
use tree_sitter_commonlisp::LANGUAGE_COMMONLISP;

pub struct TreeSitterExtractor {
    parser: Parser,
}

impl TreeSitterExtractor {
    pub fn new() -> anyhow::Result<Self> {
        let mut parser = Parser::new();
        parser.set_language(&LANGUAGE_COMMONLISP.into())?;
        Ok(Self { parser })
    }

    /// Parse source code into a tree
    pub fn parse(&mut self, source: &str) -> Option<tree_sitter::Tree> {
        self.parser.parse(source, None)
    }

    /// Find the symbol node at a specific LSP position
    #[allow(dead_code)]
    pub fn symbol_at_position(&mut self, source: &str, position: Position) -> Option<String> {
        let tree = self.parse(source)?;
        let root = tree.root_node();

        // Convert LSP position to tree-sitter point
        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };

        // Find the smallest node at this position
        let node = root.descendant_for_point_range(point, point)?;

        // Extract symbol text based on node kind
        self.extract_symbol_from_node(source, node)
    }

    /// Extract symbol name from a tree-sitter node
    #[allow(dead_code)]
    fn extract_symbol_from_node(&self, source: &str, node: Node) -> Option<String> {
        match node.kind() {
            "sym_name" | "symbol" => {
                // Direct symbol node
                Some(node.utf8_text(source.as_bytes()).ok()?.to_uppercase())
            }
            "list_lit" => {
                // We're inside a list, find the symbol child
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "sym_name" || child.kind() == "symbol" {
                        return Some(child.utf8_text(source.as_bytes()).ok()?.to_uppercase());
                    }
                }
                None
            }
            _ => {
                // Try parent node
                if let Some(parent) = node.parent() {
                    self.extract_symbol_from_node(source, parent)
                } else {
                    None
                }
            }
        }
    }

    /// Get the prefix being typed at cursor (for completion)
    /// Includes package qualifiers (e.g., "my-pkg:some-func" or "utils::helper")
    pub fn prefix_at_position(&mut self, source: &str, position: Position) -> Option<String> {
        // First, check if we're inside a string literal
        let tree = self.parse(source)?;
        let root = tree.root_node();

        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };

        // Find the node at the cursor position
        let node = root.descendant_for_point_range(point, point)?;

        // Check if we're inside a string literal
        if node.kind() == "str_lit" || node.kind() == "string" {
            tracing::debug!("Cursor inside string literal, skipping completion");
            return None;
        }

        // Also check parent nodes for string context
        let mut check_node = node;
        while let Some(parent) = check_node.parent() {
            if parent.kind() == "str_lit" || parent.kind() == "string" {
                tracing::debug!("Cursor inside string literal (parent), skipping completion");
                return None;
            }
            check_node = parent;
        }

        // Get the current line
        let lines: Vec<&str> = source.lines().collect();
        if position.line as usize >= lines.len() {
            return None;
        }

        let line = lines[position.line as usize];
        let col = position.character as usize;

        if col > line.len() {
            return None;
        }

        // Extract word before cursor
        // Look backwards from cursor to find start of word
        let before_cursor = &line[..col];

        // Find the start of the current symbol (alphanumeric, dash, underscore, *, +, /, <, >, =, :)
        // Include : to support package:symbol syntax
        let start = before_cursor
            .rfind(|c: char| {
                !c.is_alphanumeric()
                && c != '-'
                && c != '_'
                && c != '*'
                && c != '+'
                && c != '/'
                && c != '<'
                && c != '>'
                && c != '='
                && c != ':' // Include colon for package qualification
            })
            .map(|i| i + 1)
            .unwrap_or(0);

        let prefix = &before_cursor[start..];

        // Only return non-empty prefixes
        if prefix.is_empty() {
            None
        } else {
            Some(prefix.to_uppercase())
        }
    }

    /// Extract package qualifier if present
    /// Returns (symbol, package) tuple
    pub fn symbol_with_package(&mut self, source: &str, position: Position) -> Option<(String, Option<String>, Vec<String>)> {
        let tree = self.parse(source)?;
        let root = tree.root_node();

        let point = Point {
            row: position.line as usize,
            column: position.character as usize,
        };

        let mut node = root.descendant_for_point_range(point, point)?;
        let initial_kind = node.kind().to_string();
        let initial_text = node.utf8_text(source.as_bytes()).ok().map(|s| s.to_string());

        // Debug: collect parent chain
        let mut parent_chain = vec![];
        let mut temp_node = node;
        for _ in 0..5 {
            if let Some(parent) = temp_node.parent() {
                parent_chain.push(parent.kind().to_string());
                temp_node = parent;
            } else {
                break;
            }
        }

        tracing::debug!("Tree-sitter node at position: kind={}, text={:?}, parents={:?}",
                       &initial_kind,
                       &initial_text,
                       &parent_chain);

        // Check if we're on a package_lit node OR inside one - if so, we need to find the qualified symbol
        // by looking at siblings or reconstructing from the parent
        let is_in_package = node.kind() == "package_lit" || parent_chain.first().map(|s| s.as_str()) == Some("package_lit");

        tracing::info!("is_in_package check: node.kind()={}, parent_chain.first()={:?}, is_in_package={}",
                       node.kind(),
                       parent_chain.first(),
                       is_in_package);

        if is_in_package {
            tracing::info!("ENTERED is_in_package block");
            // We're hovering somewhere in a qualified symbol like "my-utils::multiply-numbers"
            // The parent node should be package_lit which contains the FULL qualified symbol text
            if let Some(parent) = node.parent() {
                tracing::info!("Got parent node: kind={}", parent.kind());

                // If parent is package_lit, it contains the full qualified symbol
                if parent.kind() == "package_lit" {
                    let package_lit_text = parent.utf8_text(source.as_bytes()).ok()?;
                    tracing::info!("package_lit text: {:?}", package_lit_text);

                    // Parse the qualified symbol from package_lit text
                    if package_lit_text.contains("::") || package_lit_text.contains(':') {
                        let parts: Vec<&str> = if package_lit_text.contains("::") {
                            package_lit_text.split("::").collect()
                        } else {
                            package_lit_text.split(':').collect()
                        };
                        tracing::info!("Parsed parts: {:?}", parts);

                        if parts.len() == 2 {
                            // Determine if we're hovering over the package part or the symbol part
                            // by checking which sym_lit child we're on
                            let mut cursor = parent.walk();
                            let children: Vec<_> = parent.children(&mut cursor)
                                .filter(|n| n.kind() == "sym_lit")
                                .collect();

                            if children.len() >= 2 {
                                let first_sym = children[0];
                                let second_sym = children[1];

                                // Check if current node is the first sym_lit (package) or second (symbol)
                                if node.id() == first_sym.id() {
                                    // Hovering over package name - return just the package as the symbol
                                    tracing::info!("Hovering over PACKAGE part: {}", parts[0]);
                                    return Some((
                                        parts[0].to_uppercase(), // Return package name as "symbol"
                                        None,                     // No package qualifier
                                        parent_chain.clone()
                                    ));
                                } else if node.id() == second_sym.id() {
                                    // Hovering over symbol name - return symbol with package
                                    tracing::info!("Hovering over SYMBOL part: {} in package {}", parts[1], parts[0]);
                                    return Some((
                                        parts[1].to_uppercase(),
                                        Some(parts[0].to_uppercase()),
                                        parent_chain.clone()
                                    ));
                                }
                            }

                            // Fallback: return the symbol with package
                            tracing::info!("Fallback: Returning symbol={}, package={}", parts[1], parts[0]);
                            return Some((
                                parts[1].to_uppercase(),
                                Some(parts[0].to_uppercase()),
                                parent_chain.clone()
                            ));
                        } else {
                            tracing::info!("Parts length != 2, got {}", parts.len());
                        }
                    }
                } else {
                    tracing::info!("Parent is not package_lit, it's {}", parent.kind());
                }
            } else {
                tracing::info!("No parent node found");
            }
            tracing::info!("Falling through is_in_package block");
        }

        // Walk up to find a qualified symbol
        loop {
            tracing::debug!("Processing node: kind={}, parent={:?}",
                           node.kind(),
                           node.parent().map(|p| p.kind()));
            match node.kind() {
                "qualified_symbol" | "qual_symbol" => {
                    // Found a package:symbol or package::symbol
                    let mut package = None;
                    let mut symbol = None;

                    for child in node.children(&mut node.walk()) {
                        match child.kind() {
                            "package" => {
                                package = Some(child.utf8_text(source.as_bytes()).ok()?.to_uppercase());
                            }
                            "sym_name" | "symbol" => {
                                symbol = Some(child.utf8_text(source.as_bytes()).ok()?.to_uppercase());
                            }
                            _ => {}
                        }
                    }

                    return Some((symbol?, package, parent_chain.clone()));
                }
                "sym_name" | "symbol" | "sym_lit" | "defun_keyword" => {
                    // Unqualified symbol
                    let symbol_text = node.utf8_text(source.as_bytes()).ok()?.to_uppercase();
                    return Some((symbol_text, None, parent_chain.clone()));
                }
                "package" | "pkg_name" | "package_name" => {
                    // Hovering over package name in a qualified symbol
                    // Walk up to parent qualified_symbol to get the full context
                    tracing::debug!("Found package-like node: {}, checking parent...", node.kind());
                    if let Some(parent) = node.parent() {
                        tracing::debug!("Parent kind: {}", parent.kind());
                        if parent.kind() == "qualified_symbol" || parent.kind() == "qual_symbol" {
                            tracing::debug!("Parent is qualified_symbol, re-processing");
                            node = parent;
                            continue; // Re-process as qualified_symbol
                        }
                    }
                    // Standalone package reference (shouldn't happen, but handle it)
                    tracing::debug!("Treating as standalone package");
                    let package_text = node.utf8_text(source.as_bytes()).ok()?.to_uppercase();
                    return Some((package_text, None, parent_chain.clone()));
                }
                _ => {
                    // Move to parent
                    if let Some(parent) = node.parent() {
                        node = parent;
                    } else {
                        break;
                    }
                }
            }
        }

        None
    }

    /// Find all function definitions in the source
    #[allow(dead_code)]
    pub fn find_definitions(&mut self, source: &str) -> Vec<(String, Position)> {
        let tree = match self.parse(source) {
            Some(t) => t,
            None => {
                eprintln!("Parse returned None");
                return vec![];
            }
        };

        let mut definitions = vec![];

        // Use iterative approach instead of recursion to avoid cursor lifetime issues
        let root = tree.root_node();
        eprintln!("Root node kind: {}", root.kind());
        eprintln!("Root node child count: {}", root.child_count());

        let mut cursor = root.walk();
        let mut node_count = 0;

        loop {
            let node = cursor.node();
            node_count += 1;

            if node_count <= 20 {  // Log first 20 nodes
                eprintln!("Node {}: kind={}, children={}", node_count, node.kind(), node.child_count());
            }

            // Check if this is a definition form
            // Two structures exist in tree-sitter-commonlisp:
            // 1. list_lit -> defun -> defun_header -> defun_keyword + sym_lit (for defun/defmacro)
            // 2. list_lit -> ( + sym_lit(defvar/defclass) + sym_lit(name) (for other defs)

            if node.kind() == "defun" || node.kind() == "defmacro" {
                // Structure 1: defun node with defun_header
                for child in node.children(&mut node.walk()) {
                    if child.kind() == "defun_header" {
                        for header_child in child.children(&mut child.walk()) {
                            if header_child.kind() == "sym_lit" {
                                if let Ok(name) = header_child.utf8_text(source.as_bytes()) {
                                    let pos = Position {
                                        line: header_child.start_position().row as u32,
                                        character: header_child.start_position().column as u32,
                                    };
                                    definitions.push((name.to_uppercase(), pos));
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
            } else if node.kind() == "list_lit" && node.child_count() >= 3 {
                // Structure 2: list_lit with ( + sym_lit + sym_lit
                // Check if child(1) is a definition keyword
                if let Some(keyword_node) = node.child(1) {
                    if keyword_node.kind() == "sym_lit" {
                        if let Ok(keyword) = keyword_node.utf8_text(source.as_bytes()) {
                            if matches!(
                                keyword.to_uppercase().as_str(),
                                "DEFVAR" | "DEFPARAMETER" | "DEFCLASS" | "DEFSTRUCT" | "DEFCONSTANT"
                            ) {
                                // child(2) should be the name
                                if let Some(name_node) = node.child(2) {
                                    if name_node.kind() == "sym_lit" {
                                        if let Ok(name) = name_node.utf8_text(source.as_bytes()) {
                                            let pos = Position {
                                                line: name_node.start_position().row as u32,
                                                character: name_node.start_position().column as u32,
                                            };
                                            definitions.push((name.to_uppercase(), pos));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Move to next node in preorder traversal
            if cursor.goto_first_child() {
                continue;
            }
            if cursor.goto_next_sibling() {
                continue;
            }
            loop {
                if !cursor.goto_parent() {
                    return definitions;
                }
                if cursor.goto_next_sibling() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_form() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = "(defun my-function (x y)\n  (+ x y))";
        let tree = extractor.parse(source).unwrap();
        assert!(tree.root_node().child_count() > 0);
    }

    #[test]
    fn test_find_symbol_at_position() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = "(defun my-function (x y)\n  (+ x y))";

        // Position on "defun"
        let symbol = extractor.symbol_at_position(source, Position { line: 0, character: 2 });
        assert!(symbol.is_some());
        assert_eq!(symbol.unwrap(), "DEFUN");
    }

    #[test]
    fn test_find_definitions() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = r#"
(defun foo (x) x)
(defvar *bar* 42)
(defclass baz () ())
"#;
        let defs = extractor.find_definitions(source);
        assert_eq!(defs.len(), 3);
        assert_eq!(defs[0].0, "FOO");
        assert_eq!(defs[1].0, "*BAR*");
        assert_eq!(defs[2].0, "BAZ");
    }

    #[test]
    fn test_no_completion_inside_string() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = r#"(defun greet (name) (format nil "Hello ~a" name))"#;

        // Position inside the string "Hello ~a" after "Hello "
        let position = Position { line: 0, character: 40 };
        let prefix = extractor.prefix_at_position(source, position);

        // Should return None because we're inside a string literal
        assert!(prefix.is_none(), "Should not provide completion inside string literal");
    }

    #[test]
    fn test_completion_outside_string() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = r#"(defun greet (name) (format nil "Hello ~a" nam))"#;

        // Position at the end of "nam" which is outside the string
        // The string is: (defun greet (name) (format nil "Hello ~a" nam))
        //                 n is at 43, a is at 44, m is at 45
        // Character 46 is the position right after 'm'
        let position = Position { line: 0, character: 46 };
        let prefix = extractor.prefix_at_position(source, position);

        // Should return the prefix because we're outside the string
        assert_eq!(prefix, Some("NAM".to_string()));
    }

    #[test]
    fn test_package_qualified_symbol_hover_on_package() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = "(my-utils::multiply-numbers 10 20)";

        // Position at character 5 (in "my-utils" - the package part)
        let position = Position { line: 0, character: 5 };
        let result = extractor.symbol_with_package(source, position);
        println!("\nHovering over package part: {:?}", result);

        // We expect to get just the package name without qualification
        assert!(result.is_some());
        let (symbol, package, _) = result.unwrap();
        assert_eq!(symbol, "MY-UTILS", "Should return package name as symbol");
        assert_eq!(package, None, "Should not have package qualifier when hovering on package");
    }

    #[test]
    fn test_package_qualified_symbol_hover_on_symbol() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = "(my-utils::multiply-numbers 10 20)";

        // Position at character 15 (in "multiply-numbers" - the symbol part)
        let position = Position { line: 0, character: 15 };
        let result = extractor.symbol_with_package(source, position);
        println!("\nHovering over symbol part: {:?}", result);

        // We expect to get the symbol with package qualification
        assert!(result.is_some());
        let (symbol, package, _) = result.unwrap();
        assert_eq!(symbol, "MULTIPLY-NUMBERS", "Should return symbol name");
        assert_eq!(package, Some("MY-UTILS".to_string()), "Should have package qualifier");
    }

    #[test]
    fn test_debug_tree_structure() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = "(defun my-add (a b)\n  (+ a b))";

        let tree = extractor.parse(source).unwrap();
        let root = tree.root_node();

        println!("\n=== Tree Structure ===");
        print_tree_helper(root, source, 0);

        println!("\n=== Looking for definitions ===");
        let defs = extractor.find_definitions(source);
        println!("Found {} definitions: {:?}", defs.len(), defs);
    }

    #[test]
    fn test_debug_defvar_defclass() {
        let mut extractor = TreeSitterExtractor::new().unwrap();
        let source = r#"(defvar *bar* 42)
(defclass baz () ())"#;

        let tree = extractor.parse(source).unwrap();
        let root = tree.root_node();

        println!("\n=== defvar and defclass Tree Structure ===");
        print_tree_helper(root, source, 0);

        println!("\n=== Looking for definitions ===");
        let defs = extractor.find_definitions(source);
        println!("Found {} definitions: {:?}", defs.len(), defs);
    }
}

