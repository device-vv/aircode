use std::{ops::Range, path::Path};

use tree_sitter::{
    InputEdit, Parser, Point, Query, QueryCursor, StreamingIterator, Tree, TreeCursor,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HighlightKind {
    Keyword,
    String,
    Comment,
    Number,
    Function,
    Type,
    Macro,
    Attribute,
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
    Dart,
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
            "dart" => Some(Self::Dart),
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
            Self::Dart => "Dart",
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
            Self::Dart => tree_sitter_dart::LANGUAGE.into(),
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
    query: Option<Query>,
    spans: Vec<HighlightSpan>,
    max_end_prefix: Vec<usize>,
}

impl SyntaxHighlighter {
    pub fn new(language: Language) -> Result<Self, String> {
        let mut parser = Parser::new();
        parser
            .set_language(&language.grammar())
            .map_err(|error| error.to_string())?;
        let query = query_source(language)
            .map(|source| {
                let extra = match language {
                    Language::Rust => "\n(attribute (identifier) @attribute)\n(integer_literal) @number\n(float_literal) @number",
                    Language::Python => "\n(class_definition name: (identifier) @type)\n(decorator (identifier) @attribute)\n(decorator (call function: (identifier) @attribute))\n(decorator (call function: (attribute attribute: (identifier) @attribute)))\n(type) @type\n(call function: (identifier) @function)\n(call function: (attribute attribute: (identifier) @function))",
                    Language::Dart => "\n(annotation) @attribute",
                    _ => "",
                };
                Query::new(&language.grammar(), &format!("{source}{extra}"))
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        Ok(Self {
            language,
            parser,
            tree: None,
            query,
            spans: Vec::new(),
            max_end_prefix: Vec::new(),
        })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn set_text(&mut self, text: &str) {
        self.tree = self.parser.parse(text, None);
        self.rebuild_spans(text);
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
        self.tree = self.parser.parse(
            new_text,
            if language_requires_full_parse(self.language) {
                None
            } else {
                self.tree.as_ref()
            },
        );
        self.rebuild_spans(new_text);
    }

    pub fn spans(&self) -> &[HighlightSpan] {
        &self.spans
    }

    pub fn spans_in_range(&self, range: Range<usize>) -> impl Iterator<Item = &HighlightSpan> {
        let first = self
            .max_end_prefix
            .partition_point(|end| *end <= range.start);
        self.spans[first..]
            .iter()
            .take_while(move |span| span.range.start < range.end)
            .filter(move |span| span.range.end > range.start)
    }

    fn rebuild_spans(&mut self, text: &str) {
        self.spans.clear();
        self.max_end_prefix.clear();
        let Some(tree) = &self.tree else {
            return;
        };
        let mut spans = Vec::new();
        if let Some(query) = &self.query {
            let mut cursor = QueryCursor::new();
            let mut captures = cursor.captures(query, tree.root_node(), text.as_bytes());
            while let Some((matched, capture_index)) = captures.next() {
                let capture = matched.captures()[*capture_index];
                let Some(kind) = query
                    .capture_names()
                    .get(capture.index as usize)
                    .and_then(|name| capture_kind(name))
                else {
                    continue;
                };
                let node = capture.node;
                if node.start_byte() < node.end_byte() {
                    spans.push(HighlightSpan {
                        range: node.start_byte()..node.end_byte(),
                        kind,
                    });
                }
            }
        } else {
            let mut cursor = tree.walk();
            visit(&mut cursor, &mut spans);
        }
        self.spans = spans;
        self.spans.sort_by_key(|span| {
            (
                span.range.start,
                span.range.end,
                capture_priority(span.kind),
            )
        });
        self.spans.dedup();
        let mut maximum_end = 0;
        self.max_end_prefix.extend(self.spans.iter().map(|span| {
            maximum_end = maximum_end.max(span.range.end);
            maximum_end
        }));
    }
}

fn language_requires_full_parse(language: Language) -> bool {
    language == Language::Dart
}

fn capture_kind(name: &str) -> Option<HighlightKind> {
    if name == "function.macro" {
        return Some(HighlightKind::Macro);
    }
    match name.split('.').next().unwrap_or(name) {
        "keyword" => Some(HighlightKind::Keyword),
        "string" => Some(HighlightKind::String),
        "comment" => Some(HighlightKind::Comment),
        "number" => Some(HighlightKind::Number),
        "function" => Some(HighlightKind::Function),
        "type" => Some(HighlightKind::Type),
        "macro" => Some(HighlightKind::Macro),
        "attribute" => Some(HighlightKind::Attribute),
        "boolean" | "constant" => Some(HighlightKind::Keyword),
        _ => None,
    }
}

fn capture_priority(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Attribute | HighlightKind::Macro => 0,
        HighlightKind::Function | HighlightKind::Type => 1,
        HighlightKind::Keyword | HighlightKind::Number => 2,
        HighlightKind::String | HighlightKind::Comment => 3,
    }
}

// These captures describe syntax only. In particular, an uppercase name or a
// call is not promoted to a type through heuristic name matching.
fn query_source(language: Language) -> Option<&'static str> {
    match language {
        Language::Rust => Some(tree_sitter_rust::HIGHLIGHTS_QUERY),
        Language::Python => Some(tree_sitter_python::HIGHLIGHTS_QUERY),
        Language::Dart => Some(tree_sitter_dart::HIGHLIGHTS_QUERY),
        _ => None,
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
    if kind.contains("attribute") {
        return Some(HighlightKind::Attribute);
    }
    if kind.contains("macro") {
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
        assert_eq!(
            Language::from_path(Path::new("lib/main.dart")),
            Some(Language::Dart)
        );
        assert_eq!(Language::from_path(Path::new("unknown.txt")), None);
    }

    fn assert_span(highlighter: &SyntaxHighlighter, text: &str, needle: &str, kind: HighlightKind) {
        let start = text
            .find(needle)
            .unwrap_or_else(|| panic!("missing sample {needle}"));
        let expected = start..start + needle.len();
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.range == expected && span.kind == kind),
            "missing {kind:?} span for {needle:?}; spans: {:#?}",
            highlighter.spans()
        );
    }

    fn assert_incremental_reparse(language: Language, old: &str, needle: &str, replacement: &str) {
        let start = old.find(needle).unwrap();
        let range = start..start + needle.len();
        let new = format!("{}{replacement}{}", &old[..start], &old[range.end..]);
        let mut incremental = SyntaxHighlighter::new(language).unwrap();
        incremental.set_text(old);
        incremental.replace_range(old, &new, range, replacement);
        let mut reparsed = SyntaxHighlighter::new(language).unwrap();
        reparsed.set_text(new.as_str());
        assert_eq!(incremental.spans(), reparsed.spans());
        assert!(incremental.spans().iter().all(|span| {
            span.range.start < span.range.end
                && span.range.end <= new.len()
                && new.is_char_boundary(span.range.start)
                && new.is_char_boundary(span.range.end)
        }));

        incremental.replace_range(&new, old, start..start + replacement.len(), "");
        let mut deleted = SyntaxHighlighter::new(language).unwrap();
        deleted.set_text(old);
        assert_eq!(incremental.spans(), deleted.spans());
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
    fn rust_queries_capture_precise_nested_constructs() {
        let text = r##"#![doc = "inner docs"]
#[derive(Debug, Clone)]
pub struct Container<T>
where
    T: Clone,
{
    value: T,
}

impl<T: Clone> Container<T> {
    pub async fn convert(&self) -> T {
        // Nested calls and raw strings remain distinct nodes.
        let text = r#"raw " text"#;
        let glyph = '🦀';
        println!("{text} {}", 0x2A);
        self.value.clone()
    }
}
"##;
        let mut highlighter = SyntaxHighlighter::new(Language::Rust).unwrap();
        highlighter.set_text(text);
        for (needle, kind) in [
            ("pub", HighlightKind::Keyword),
            ("struct", HighlightKind::Keyword),
            ("Container", HighlightKind::Type),
            ("convert", HighlightKind::Function),
            ("clone", HighlightKind::Function),
            ("println", HighlightKind::Macro),
            ("derive", HighlightKind::Attribute),
            ("\"inner docs\"", HighlightKind::String),
            ("r#\"raw \" text\"#", HighlightKind::String),
            ("'🦀'", HighlightKind::String),
            ("0x2A", HighlightKind::Number),
            (
                "// Nested calls and raw strings remain distinct nodes.",
                HighlightKind::Comment,
            ),
        ] {
            assert_span(&highlighter, text, needle, kind);
        }
        assert!(
            highlighter
                .spans()
                .iter()
                .filter(|span| span.kind == HighlightKind::Function)
                .all(|span| !text[span.range.clone()].contains(['{', '\n']))
        );
    }

    #[test]
    fn python_queries_capture_definitions_calls_types_and_decorators() {
        let text = r#"@decorator
class Box:
    """A nested container."""
    async def convert(self, value: list[str]) -> dict[str, int]:
        # Calls inside nested expressions keep exact identifier ranges.
        result = self.helper(f"{value!r}")
        return {"size": len(result), "hex": 0x2A, "float": 1.5e2}

@register(name="demo")
def outer():
    def inner():
        return True
    return inner
"#;
        let mut highlighter = SyntaxHighlighter::new(Language::Python).unwrap();
        highlighter.set_text(text);
        for (needle, kind) in [
            ("class", HighlightKind::Keyword),
            ("Box", HighlightKind::Type),
            ("list[str]", HighlightKind::Type),
            ("convert", HighlightKind::Function),
            ("helper", HighlightKind::Function),
            ("len", HighlightKind::Function),
            ("inner", HighlightKind::Function),
            ("decorator", HighlightKind::Attribute),
            ("register", HighlightKind::Attribute),
            ("\"\"\"A nested container.\"\"\"", HighlightKind::String),
            ("f\"{value!r}\"", HighlightKind::String),
            ("\"demo\"", HighlightKind::String),
            ("0x2A", HighlightKind::Number),
            ("1.5e2", HighlightKind::Number),
            (
                "# Calls inside nested expressions keep exact identifier ranges.",
                HighlightKind::Comment,
            ),
        ] {
            assert_span(&highlighter, text, needle, kind);
        }
        assert!(
            highlighter
                .spans()
                .iter()
                .filter(|span| span.kind == HighlightKind::Function)
                .all(|span| !text[span.range.clone()].contains([':', '\n']))
        );
    }

    #[test]
    fn dart_and_flutter_queries_use_dart_syntax() {
        let text = r#"import 'package:flutter/material.dart';

@immutable
class Counter extends StatelessWidget {
  const Counter({super.key});

  @override
  Widget build(BuildContext context) {
    // Flutter widgets and interpolation are parsed by the Dart grammar.
    return Text('Count: ${counter.toString()}');
  }
}

int increment(int value) => value + 42;
"#;
        let mut highlighter = SyntaxHighlighter::new(Language::Dart).unwrap();
        highlighter.set_text(text);
        for (needle, kind) in [
            ("import", HighlightKind::Keyword),
            ("Counter", HighlightKind::Type),
            ("StatelessWidget", HighlightKind::Type),
            ("build", HighlightKind::Function),
            ("increment", HighlightKind::Function),
            ("@immutable", HighlightKind::Attribute),
            ("'package:flutter/material.dart'", HighlightKind::String),
            ("42", HighlightKind::Number),
            (
                "// Flutter widgets and interpolation are parsed by the Dart grammar.",
                HighlightKind::Comment,
            ),
        ] {
            assert_span(&highlighter, text, needle, kind);
        }
    }

    #[test]
    fn query_highlighters_match_full_reparse_after_unicode_edits() {
        assert_incremental_reparse(
            Language::Rust,
            "fn main() {\n    let value = 42;\n    println!(\"{value}\");\n}\n",
            "42",
            "\"🦀\"",
        );
        assert_incremental_reparse(
            Language::Python,
            "def convert(value: int) -> str:\n    return str(value)\n",
            "int",
            "list[str]",
        );
        assert_incremental_reparse(
            Language::Dart,
            "String label() => 'old';\n",
            "old",
            "updated 🦀",
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

    #[test]
    fn every_language_handles_open_and_incremental_edit() {
        let samples = [
            (Language::Rust, "fn main() { let value = 1; }"),
            (Language::Python, "def main():\n    return 1\n"),
            (Language::Dart, "int main() => 1;"),
            (Language::JavaScript, "const main = () => 1;"),
            (Language::TypeScript, "const main = (): number => 1;"),
            (Language::Tsx, "const Main = () => <div />;"),
            (Language::Go, "package main\nfunc main() {}\n"),
            (Language::C, "int main(void) { return 0; }"),
            (Language::Cpp, "int main() { return 0; }"),
            (Language::Java, "class Main {}"),
            (Language::CSharp, "class Main {}"),
            (Language::Html, "<main>text</main>"),
            (Language::Css, "main { color: red; }"),
            (Language::Json, "{\"value\": 1}"),
            (Language::Toml, "value = 1"),
            (Language::Yaml, "value: 1"),
            (Language::Markdown, "# Heading"),
        ];

        for (language, sample) in samples {
            let mut highlighter = SyntaxHighlighter::new(language).unwrap();
            highlighter.set_text(sample);
            let updated = format!(" {sample}");
            highlighter.replace_range(sample, &updated, 0..0, " ");
            assert!(highlighter.spans().iter().all(|span| {
                span.range.start <= span.range.end
                    && span.range.end <= updated.len()
                    && updated.is_char_boundary(span.range.start)
                    && updated.is_char_boundary(span.range.end)
            }));
        }
    }

    #[test]
    fn range_lookup_matches_naive_filter_before_and_after_edits() {
        let old =
            "fn main() {\n    let text = \"first\";\n    // comment\n    let number = 42;\n}\n";
        let new = old.replace("first", "a much longer string");
        let replaced = old.find("first").unwrap();
        let mut highlighter = SyntaxHighlighter::new(Language::Rust).unwrap();

        for (text, replacement) in [
            (old, None),
            (
                new.as_str(),
                Some((old, replaced..replaced + 5, "a much longer string")),
            ),
        ] {
            if let Some((old, range, replacement)) = replacement {
                highlighter.replace_range(old, text, range, replacement);
            } else {
                highlighter.set_text(text);
            }
            let mut ranges = vec![0..0, 0..text.len(), text.len()..text.len()];
            ranges.extend(
                text.match_indices('\n')
                    .map(|(index, _)| index.saturating_sub(3)..(index + 4).min(text.len())),
            );
            for range in ranges {
                let expected = highlighter
                    .spans()
                    .iter()
                    .filter(|span| span.range.end > range.start && span.range.start < range.end)
                    .collect::<Vec<_>>();
                let actual = highlighter.spans_in_range(range).collect::<Vec<_>>();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn query_range_lookup_matches_naive_filter_across_viewports() {
        let samples = [
            (
                Language::Rust,
                "fn value() { let number = 42; }\n".repeat(300),
            ),
            (
                Language::Python,
                "def value():\n    return 42\n".repeat(300),
            ),
            (Language::Dart, "int value() => 42;\n".repeat(300)),
        ];
        for (language, text) in samples {
            let mut highlighter = SyntaxHighlighter::new(language).unwrap();
            highlighter.set_text(&text);
            let mut ranges = vec![0..text.len(), text.len()..text.len()];
            ranges.extend(
                text.match_indices('\n')
                    .skip(200)
                    .take(30)
                    .map(|(index, _)| index.saturating_sub(7)..(index + 11).min(text.len())),
            );
            for range in ranges {
                let expected = highlighter
                    .spans()
                    .iter()
                    .filter(|span| span.range.end > range.start && span.range.start < range.end)
                    .collect::<Vec<_>>();
                let actual = highlighter
                    .spans_in_range(range.clone())
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected, "{language:?}, {range:?}");
            }
        }
    }
}
