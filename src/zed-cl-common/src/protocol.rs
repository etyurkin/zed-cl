/// Protocol definitions for communicating with the master REPL.
///
/// The master REPL accepts s-expression requests over TCP and returns
/// s-expression responses.

use anyhow::{Context, Result};
use lexpr::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::debug;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ReplRequest {
    SymbolInfo {
        id: String,
        symbol: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
    },
    ListSymbols {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        prefix: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
    },
    Eval {
        id: String,
        code: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file_path: Option<String>,
    },
    LoadFile {
        id: String,
        path: String,
    },
    SetCurrentFile {
        id: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        contents: Option<String>,
    },
    Ping {
        id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplResponse {
    pub id: String,
    #[serde(flatten)]
    pub data: ResponseData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseData {
    SymbolInfo(SymbolInfo),
    SymbolList {
        symbols: Vec<SymbolInfo>,
    },
    EvalResult {
        output: String,
        values: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        traceback: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        displays: Option<Vec<DisplayData>>,
    },
    LoadResult {
        output: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Error {
        error: String,
    },
    Pong,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayData {
    pub data: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    pub package: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "param-types")]
    pub param_types: Option<Vec<(String, Option<String>)>>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "source-file")]
    pub source_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "source-line")]
    pub source_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "source-character")]
    pub source_character: Option<u32>,
}

impl ReplRequest {
    pub fn id(&self) -> &str {
        match self {
            ReplRequest::SymbolInfo { id, .. }
            | ReplRequest::ListSymbols { id, .. }
            | ReplRequest::Eval { id, .. }
            | ReplRequest::LoadFile { id, .. }
            | ReplRequest::SetCurrentFile { id, .. }
            | ReplRequest::Ping { id } => id,
        }
    }

    pub fn id_mut(&mut self) -> &mut String {
        match self {
            ReplRequest::SymbolInfo { id, .. }
            | ReplRequest::ListSymbols { id, .. }
            | ReplRequest::Eval { id, .. }
            | ReplRequest::LoadFile { id, .. }
            | ReplRequest::SetCurrentFile { id, .. }
            | ReplRequest::Ping { id } => id,
        }
    }

    pub fn to_sexp(&self) -> String {
        match self {
            ReplRequest::SymbolInfo { id, symbol, package } => {
                let mut parts = vec![
                    ":type \"symbol-info\"".to_string(),
                    format!(":id {}", lisp_string(id)),
                    format!(":symbol {}", lisp_string(symbol)),
                ];
                if let Some(pkg) = package {
                    parts.push(format!(":package {}", lisp_string(pkg)));
                }
                format!("({})", parts.join(" "))
            }
            ReplRequest::ListSymbols { id, prefix, package } => {
                let mut parts = vec![
                    ":type \"list-symbols\"".to_string(),
                    format!(":id {}", lisp_string(id)),
                ];
                if let Some(pfx) = prefix {
                    parts.push(format!(":prefix {}", lisp_string(pfx)));
                }
                if let Some(pkg) = package {
                    parts.push(format!(":package {}", lisp_string(pkg)));
                }
                format!("({})", parts.join(" "))
            }
            ReplRequest::Eval {
                id,
                code,
                package,
                file_path,
            } => {
                let mut parts = vec![
                    ":type \"eval\"".to_string(),
                    format!(":id {}", lisp_string(id)),
                    format!(":code {}", lisp_string(code)),
                ];
                if let Some(pkg) = package {
                    parts.push(format!(":package {}", lisp_string(pkg)));
                }
                if let Some(path) = file_path {
                    parts.push(format!(":file-path {}", lisp_string(path)));
                }
                format!("({})", parts.join(" "))
            }
            ReplRequest::LoadFile { id, path } => {
                format!(
                    "(:type \"load-file\" :id {} :path {})",
                    lisp_string(id),
                    lisp_string(path)
                )
            }
            ReplRequest::SetCurrentFile { id, path, contents } => {
                let mut parts = vec![
                    ":type \"set-current-file\"".to_string(),
                    format!(":id {}", lisp_string(id)),
                    format!(":path {}", lisp_string(path)),
                ];
                if let Some(text) = contents {
                    parts.push(format!(":contents {}", lisp_string(text)));
                }
                format!("({})", parts.join(" "))
            }
            ReplRequest::Ping { id } => {
                format!("(:type \"ping\" :id {})", lisp_string(id))
            }
        }
    }
}

fn lisp_string(s: &str) -> String {
    format!("\"{}\"", escape_lisp_string(s))
}

/// Split a typed completion prefix into (package, symbol-prefix).
/// `pkg:foo` / `pkg::foo` → (Some(pkg), "foo"); `:foo` → (Some("KEYWORD"), "foo").
pub fn parse_lisp_completion_prefix(prefix: &str) -> (Option<String>, String) {
    if let Some(pos) = prefix.find("::") {
        let pkg = &prefix[..pos];
        let rest = prefix[pos + 2..].to_string();
        if pkg.is_empty() {
            (Some("KEYWORD".to_string()), rest)
        } else {
            (Some(pkg.to_string()), rest)
        }
    } else if let Some(pos) = prefix.find(':') {
        if pos == 0 {
            (Some("KEYWORD".to_string()), prefix[1..].to_string())
        } else {
            (Some(prefix[..pos].to_string()), prefix[pos + 1..].to_string())
        }
    } else {
        (None, prefix.to_string())
    }
}

fn escape_lisp_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

impl ReplResponse {
    pub fn from_sexp(sexp: &str, expected_id: &str) -> Result<Self> {
        debug!("Parsing s-expression: {}", sexp);
        let value = lexpr::from_str(sexp).context("Failed to parse s-expression")?;
        let map = plist_map(&value)?;

        let id = map
            .get("ID")
            .and_then(value_as_string)
            .unwrap_or_else(|| expected_id.to_string());

        if map.contains_key("PONG") || map.contains_key("OK") {
            return Ok(ReplResponse {
                id,
                data: ResponseData::Pong,
            });
        }

        if let Some(error) = map.get("ERROR").and_then(value_as_string) {
            if !map.contains_key("OUTPUT") {
                return Ok(ReplResponse {
                    id,
                    data: ResponseData::Error { error },
                });
            }
        }

        if map.contains_key("SYMBOL") {
            return Ok(ReplResponse {
                id,
                data: ResponseData::SymbolInfo(symbol_from_map(&map)),
            });
        }

        if map.contains_key("SYMBOLS") {
            let symbols = map
                .get("SYMBOLS")
                .map(extract_symbol_list)
                .unwrap_or_default();
            return Ok(ReplResponse {
                id,
                data: ResponseData::SymbolList { symbols },
            });
        }

        if map.contains_key("OUTPUT") || map.contains_key("VALUES") {
            let output = map
                .get("OUTPUT")
                .and_then(value_as_string)
                .unwrap_or_default();
            let values = map
                .get("VALUES")
                .map(value_as_string_list)
                .unwrap_or_default();
            let error = map
                .get("ERROR")
                .and_then(value_as_string)
                .filter(|s| !s.is_empty());
            let traceback = map
                .get("TRACEBACK")
                .and_then(value_as_string)
                .filter(|s| !s.is_empty());
            let displays = map.get("DISPLAYS").and_then(extract_displays);

            return Ok(ReplResponse {
                id,
                data: ResponseData::EvalResult {
                    output,
                    values,
                    error,
                    traceback,
                    displays,
                },
            });
        }

        anyhow::bail!("Unknown response format: {:?}", map.keys().collect::<Vec<_>>())
    }
}

fn plist_map(value: &Value) -> Result<HashMap<String, Value>> {
    let items = value
        .to_vec()
        .context("Expected a property list")?;
    let mut map = HashMap::new();
    let mut i = 0;
    while i + 1 < items.len() {
        if let Some(key) = keyword_name(&items[i]) {
            map.insert(key, items[i + 1].clone());
        }
        i += 2;
    }
    Ok(map)
}

fn keyword_name(value: &Value) -> Option<String> {
    value
        .as_keyword()
        .map(|s| s.to_uppercase())
        .or_else(|| {
            value.as_symbol().map(|s| {
                s.trim_start_matches(':').to_uppercase()
            })
        })
}

fn value_as_string(value: &Value) -> Option<String> {
    if value.is_nil() || value.is_null() {
        return None;
    }
    if let Some(sym) = value.as_symbol() {
        if sym.eq_ignore_ascii_case("nil") {
            return None;
        }
        if sym.eq_ignore_ascii_case("t") {
            return Some("T".to_string());
        }
    }
    if let Some(s) = value.as_str() {
        return Some(unescape_lisp_string(s));
    }
    if let Some(n) = value.as_i64() {
        return Some(n.to_string());
    }
    if let Some(n) = value.as_u64() {
        return Some(n.to_string());
    }
    if let Some(n) = value.as_f64() {
        return Some(n.to_string());
    }
    if value == &Value::symbol("T") || value == &Value::Bool(true) {
        return Some("T".to_string());
    }
    Some(value.to_string())
}

fn unescape_lisp_string(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                result.push(next);
            }
        } else {
            result.push(ch);
        }
    }
    result
}

fn value_as_string_list(value: &Value) -> Vec<String> {
    if value.is_nil() {
        return Vec::new();
    }
    if let Some(items) = value.to_vec() {
        return items.iter().filter_map(value_as_string).collect();
    }
    Vec::new()
}

fn symbol_from_map(map: &HashMap<String, Value>) -> SymbolInfo {
    SymbolInfo {
        symbol: map.get("SYMBOL").and_then(value_as_string).unwrap_or_default(),
        package: map.get("PACKAGE").and_then(value_as_string).unwrap_or_default(),
        kind: map.get("KIND").and_then(value_as_string).unwrap_or_default(),
        source: map.get("SOURCE").and_then(value_as_string),
        doc: map.get("DOC").and_then(value_as_string),
        param_types: map.get("PARAM-TYPES").and_then(parse_param_types),
        source_file: map.get("SOURCE-FILE").and_then(value_as_string),
        source_line: map
            .get("SOURCE-LINE")
            .and_then(value_as_string)
            .and_then(|s| s.parse().ok()),
        source_character: map
            .get("SOURCE-CHARACTER")
            .and_then(value_as_string)
            .and_then(|s| s.parse().ok()),
    }
}

fn extract_symbol_list(value: &Value) -> Vec<SymbolInfo> {
    if value.is_nil() {
        return Vec::new();
    }
    let Some(items) = value.to_vec() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| plist_map(item).ok().map(|m| symbol_from_map(&m)))
        .collect()
}

fn extract_displays(value: &Value) -> Option<Vec<DisplayData>> {
    if value.is_nil() {
        return None;
    }
    let items = value.to_vec()?;
    let mut displays = Vec::new();
    for item in items {
        if let Ok(map) = plist_map(&item) {
            if let Some(data_val) = map.get("DATA") {
                displays.push(DisplayData {
                    data: parse_alist(data_val),
                    metadata: None,
                });
            }
        }
    }
    if displays.is_empty() {
        None
    } else {
        Some(displays)
    }
}

fn parse_alist(value: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(items) = value.to_vec() else {
        return map;
    };
    for item in items {
        if let Value::Cons(cons) = &item {
            let car = cons.car();
            let cdr = cons.cdr();
            if let Some(key) = value_as_string(car) {
                if let Some(val) = value_as_string(cdr) {
                    if val != "NIL" {
                        map.insert(key, val);
                    }
                }
            }
        } else if let Some(pair) = item.to_vec() {
            if pair.len() >= 2 {
                if let (Some(key), Some(val)) = (value_as_string(&pair[0]), value_as_string(&pair[1])) {
                    if val != "NIL" {
                        map.insert(key, val);
                    }
                }
            }
        }
    }
    map
}

fn parse_param_types(value: &Value) -> Option<Vec<(String, Option<String>)>> {
    if value.is_nil() {
        return Some(Vec::new());
    }
    let items = value.to_vec()?;
    let mut result = Vec::new();
    for item in items {
        let (name, type_val) = if let Value::Cons(cons) = &item {
            (value_as_string(cons.car()), value_as_string(cons.cdr()))
        } else if let Some(pair) = item.to_vec() {
            if pair.len() >= 2 {
                (value_as_string(&pair[0]), value_as_string(&pair[1]))
            } else {
                continue;
            }
        } else {
            continue;
        };
        if let Some(name) = name {
            let type_val = type_val.filter(|s| s != "NIL" && !s.is_empty());
            result.push((name, type_val));
        }
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_eval_result() {
        let sexp = r#"(:ID "1" :OUTPUT "hi" :VALUES ("3") :ERROR NIL)"#;
        let response = ReplResponse::from_sexp(sexp, "1").unwrap();
        match response.data {
            ResponseData::EvalResult { output, values, error, .. } => {
                assert_eq!(output, "hi");
                assert_eq!(values, vec!["3"]);
                assert!(error.is_none());
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn parses_ping() {
        let sexp = r#"(:ID "p" :PONG T)"#;
        let response = ReplResponse::from_sexp(sexp, "p").unwrap();
        assert!(matches!(response.data, ResponseData::Pong));
    }

    #[test]
    fn escapes_windows_paths_and_quotes() {
        let request = ReplRequest::LoadFile {
            id: "1".into(),
            path: r#"C:\Users\a\"file\".lisp"#.into(),
        };
        let sexp = request.to_sexp();
        assert!(sexp.contains(r#"C:\\Users\\a\\\"file\\\".lisp"#));
    }

    #[test]
    fn list_symbols_includes_package() {
        let request = ReplRequest::ListSymbols {
            id: "1".into(),
            prefix: Some("MAP".into()),
            package: Some("MY-APP".into()),
        };
        let sexp = request.to_sexp();
        assert!(sexp.contains(":prefix \"MAP\""));
        assert!(sexp.contains(":package \"MY-APP\""));
    }

    #[test]
    fn parse_completion_prefix_splits_qualifiers() {
        assert_eq!(
            parse_lisp_completion_prefix("my-app:map"),
            (Some("my-app".into()), "map".into())
        );
        assert_eq!(
            parse_lisp_completion_prefix("utils::foo"),
            (Some("utils".into()), "foo".into())
        );
        assert_eq!(
            parse_lisp_completion_prefix(":kw"),
            (Some("KEYWORD".into()), "kw".into())
        );
        assert_eq!(parse_lisp_completion_prefix("bar"), (None, "bar".into()));
    }

    #[test]
    fn nil_source_location_fields_are_omitted() {
        let sexp = r#"(:ID "1" :SYMBOL "FOO" :KIND "function" :PACKAGE "CL-USER" :SOURCE-FILE "/tmp/a.lisp" :SOURCE-LINE NIL :SOURCE-CHARACTER NIL)"#;
        let response = ReplResponse::from_sexp(sexp, "1").unwrap();
        match response.data {
            ResponseData::SymbolInfo(info) => {
                assert_eq!(info.source_file.as_deref(), Some("/tmp/a.lisp"));
                assert_eq!(info.source_line, None);
                assert_eq!(info.source_character, None);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
