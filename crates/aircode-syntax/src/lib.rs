use std::{ops::Range, path::Path};

use tree_sitter::{InputEdit, Parser, Point, Tree, TreeCursor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightKind {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Macro,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Go,
    C,
    Cpp,
    Java,
    CSharp,
    Html,
    Css,
    Json,
    Toml,
    Yaml,
    Markdown,
}

impl Language {
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "rs" => Some(Self::Rust),
            "py" | "pyi" => Some(Self::Python),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "mts" | "cts" => Some(Self::TypeScript),
            "tsx" => Some(Self::Tsx),
            "go" => Some(Self::Go),
            "c" | "h" => Some(Self::C),
            "cc" | "cpp" | "cxx" | "hpp" | "hh" => Some(Self::Cpp),
            "java" => Some(Self::Java),
            "cs" => Some(Self::CSharp),
            "html" | "htm" => Some(Self::Html),
            "css" => Some(Self::Css),
            "json" | "jsonc" => Some(Self::Json),
            "toml" => Some(Self::Toml),
            "yaml" | "yml" => Some(Self::Yaml),
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Rust => "Rust",
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Tsx => "TSX",
            Self::Go => "Go",
            Self::C => "C",
            Self::Cpp => "C++",
            Self::Java => "Java",
            Self::CSharp => "C#",
            Self::Html => "HTML",
            Self::Css => "CSS",
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
            Self::Markdown => "Markdown",
        }
    }

    fn grammar(self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::Go => tree_sitter_go::LANGUAGE.into(),
            Self::C => tree_sitter_c::LANGUAGE.into(),
            Self::Cpp => tree_sitter_cpp::LANGUAGE.into(),
            Self::Java => tree_sitter_java::LANGUAGE.into(),
            Self::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
            Self::Html => tree_sitter_html::LANGUAGE.into(),
            Self::Css => tree_sitter_css::LANGUAGE.into(),
            Self::Json => tree_sitter_json::LANGUAGE.into(),
            Self::Toml => tree_sitter_toml_ng::LANGUAGE.into(),
            Self::Yaml => tree_sitter_yaml::LANGUAGE.into(),
            Self::Markdown => tree_sitter_md::LANGUAGE.into(),
        }
    }
}

pub struct SyntaxHighlighter {
    language: Language,
    parser: Parser,
    tree: Option<Tree>,
    spans: Vec<HighlightSpan>,
}

impl SyntaxHighlighter {
    pub fn new(language: Language) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&language.grammar())
            .map_err(|error| error.to_string())?;
        Ok(Self {
            language,
            parser,
            tree: None,
            spans: Vec::new(),
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn set_text(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
        self.rebuild_spans();
    }

    pub fn replace_range(
        &mut self,
        old_text: &str,
        new_text: &str,
        range: Range<usize>,
        replacement: &str,
    ) {
        if let Some(tree) = &mut self.tree {
            let old_end = range.end;
            let new_end = range.start + replacement.len();
            tree.edit(&InputEdit {
                start_byte: range.start,
                old_end_byte: old_end,
                new_end_byte: new_end,
                start_position: point_at(old_text, range.start),
                old_end_position: point_at(old_text, old_end),
                new_end_position: point_at(new_text, new_end),
            });
        }
        self.tree = self.parser.parse(new_text, self.tree.as_ref());
        self.rebuild_spans();
    }

    pub fn spans(&self) -> &[HighlightSpan] {
        &self.spans
    }

    fn rebuild_spans(&mut self) {
        self.spans.clear();
        let Some(tree) = &self.tree else {
            return;
        };
        let mut cursor = tree.walk();
        visit(&mut cursor, &mut self.spans);
        self.spans
            .sort_by_key(|span| (span.range.start, span.range.end));
        self.spans.dedup();
    }
}

fn point_at(text: &str, offset: usize) -> Point {
    let prefix = &text[..offset];
    let row = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let column = prefix
        .rfind('\n')
        .map(|index| offset - index - 1)
        .unwrap_or(offset);
    Point { row, column }
}

fn visit(cursor: &mut TreeCursor, spans: &mut Vec<HighlightSpan>) {
    let mut visited = false;
    loop {
        if !visited {
            let node = cursor.node();
            if let Some(kind) = classify(node.kind()) {
                spans.push(HighlightSpan {
                    range: node.start_byte()..node.end_byte(),
                    kind,
                });
            }
            if cursor.goto_first_child() {
                visited = false;
                continue;
            }
        }
        if cursor.goto_next_sibling() {
            visited = false;
            continue;
        }
        if !cursor.goto_parent() {
            return;
        }
        visited = true;
    }
}

fn classify(kind: &str) -> Option<HighlightKind> {
    if kind.contains("comment") {
        return Some(HighlightKind::Comment);
    }
    if kind.contains("string") || matches!(kind, "char_literal" | "character") {
        return Some(HighlightKind::String);
    }
    if kind.contains("number") || kind.contains("integer") || kind.contains("float") {
        return Some(HighlightKind::Number);
    }
    if kind.contains("macro") || kind.contains("attribute") {
        return Some(HighlightKind::Macro);
    }
    if kind.contains("function") || kind.contains("method") || kind == "call_expression" {
        return Some(HighlightKind::Function);
    }
    if kind.contains("type") || matches!(kind, "class" | "struct" | "enum" | "interface") {
        return Some(HighlightKind::Type);
    }
    if matches!(
        kind,
        "async"
            | "await"
            | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "def"
            | "default"
            | "defer"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "final"
            | "finally"
            | "fn"
            | "for"
            | "func"
            | "function"
            | "go"
            | "goto"
            | "if"
            | "implements"
            | "import"
            | "in"
            | "interface"
            | "let"
            | "match"
            | "mod"
            | "move"
            | "mut"
            | "namespace"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "pub"
            | "public"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "use"
            | "using"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
    ) {
        return Some(HighlightKind::Keyword);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_popular_languages() {
        assert_eq!(
            Language::from_path(Path::new("src/main.rs")),
            Some(Language::Rust)
        );
        assert_eq!(
            Language::from_path(Path::new("component.tsx")),
            Some(Language::Tsx)
        );
        assert_eq!(
            Language::from_path(Path::new("readme.md")),
            Some(Language::Markdown)
        );
        assert_eq!(Language::from_path(Path::new("unknown.txt")), None);
    }

    #[test]
    fn highlights_rust_and_updates_incrementally() {
        let mut highlighter = SyntaxHighlighter::new(Language::Rust).unwrap();
        let old = "fn main() {\n    // comment\n    let value = \"text\";\n}\n";
        highlighter.set_text(old);
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.kind == HighlightKind::Keyword)
        );
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.kind == HighlightKind::Comment)
        );
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.kind == HighlightKind::String)
        );

        let new = old.replace("text", "updated");
        let start = old.find("text").unwrap();
        highlighter.replace_range(old, &new, start..start + 4, "updated");
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.kind == HighlightKind::String)
        );
    }

    #[test]
    fn highlights_json_strings_and_numbers() {
        let mut highlighter = SyntaxHighlighter::new(Language::Json).unwrap();
        highlighter.set_text("{\"answer\": 42}");
        let kinds = highlighter
            .spans()
            .iter()
            .map(|span| span.kind)
            .collect::<Vec<_>>();
        assert!(kinds.contains(&HighlightKind::String));
        assert!(kinds.contains(&HighlightKind::Number));
    }
}
