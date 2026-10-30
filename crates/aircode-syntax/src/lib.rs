use std::{cmp::Ordering, collections::BinaryHeap, ops::Range, path::Path};

use tree_sitter::{InputEdit, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

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

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapturedSpan {
    span: HighlightSpan,
    specificity: u8,
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
    query: Query,
    markdown_inline: Option<MarkdownInlineHighlighter>,
    spans: Vec<HighlightSpan>,
    max_end_prefix: Vec<usize>,
}

impl SyntaxHighlighter {
    pub fn new(language: Language) -> Result<Self, String> {
        let grammar = language.grammar();
        let mut parser = Parser::new();
        parser
            .set_language(&grammar)
            .map_err(|error| format!("{} grammar: {error}", language.label()))?;
        let query = Query::new(&grammar, &query_source(language)).map_err(|error| {
            format!(
                "{} highlight query failed to compile: {error}",
                language.label()
            )
        })?;
        let markdown_inline = if language == Language::Markdown {
            Some(MarkdownInlineHighlighter::new()?)
        } else {
            None
        };
        Ok(Self {
            language,
            parser,
            tree: None,
            query,
            markdown_inline,
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
        collect_query_spans(&self.query, tree.root_node(), text, 0, &mut spans);
        if let Some(markdown_inline) = &mut self.markdown_inline {
            collect_markdown_inline_spans(markdown_inline, tree, text, &mut spans);
        }
        self.spans = resolve_captured_overlaps(spans);
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

struct MarkdownInlineHighlighter {
    parser: Parser,
    query: Query,
}

impl MarkdownInlineHighlighter {
    fn new() -> Result<Self, String> {
        let grammar: tree_sitter::Language = tree_sitter_md::INLINE_LANGUAGE.into();
        let mut parser = Parser::new();
        parser
            .set_language(&grammar)
            .map_err(|error| format!("Markdown inline grammar: {error}"))?;
        let query =
            Query::new(&grammar, tree_sitter_md::HIGHLIGHT_QUERY_INLINE).map_err(|error| {
                format!("Markdown inline highlight query failed to compile: {error}")
            })?;
        Ok(Self { parser, query })
    }
}

fn capture_kind(name: &str) -> Option<HighlightKind> {
    match name {
        "function.macro" => return Some(HighlightKind::Macro),
        "text.literal" | "text.uri" | "string.escape" => {
            return Some(HighlightKind::String);
        }
        "text.title" | "text.strong" => return Some(HighlightKind::Type),
        "text.emphasis" => return Some(HighlightKind::Attribute),
        "text.reference" => return Some(HighlightKind::Function),
        "markup.heading" | "markup.bold" => return Some(HighlightKind::Type),
        "markup.italic" => return Some(HighlightKind::Attribute),
        "markup.link" | "markup.link.label" => return Some(HighlightKind::Function),
        "markup.link.url" | "markup.raw" | "markup.raw.block" | "markup.raw.inline" => {
            return Some(HighlightKind::String);
        }
        "markup.quote" => return Some(HighlightKind::Comment),
        "markup.list" => return Some(HighlightKind::Keyword),
        _ => {}
    }

    match name.split('.').next().unwrap_or(name) {
        "keyword" => Some(HighlightKind::Keyword),
        "string" | "escape" | "character" => Some(HighlightKind::String),
        "comment" => Some(HighlightKind::Comment),
        "number" => Some(HighlightKind::Number),
        "function" => Some(HighlightKind::Function),
        "type" | "tag" | "module" | "namespace" | "constructor" => Some(HighlightKind::Type),
        "macro" => Some(HighlightKind::Macro),
        "attribute" | "property" | "label" => Some(HighlightKind::Attribute),
        "operator" => Some(HighlightKind::Keyword),
        "boolean" | "constant" => Some(HighlightKind::Keyword),
        "markup" => match name {
            "markup.raw" | "markup.raw.block" | "markup.raw.inline" => Some(HighlightKind::String),
            "markup.quote" => Some(HighlightKind::Comment),
            "markup.link" | "markup.link.label" => Some(HighlightKind::Function),
            "markup.heading" => Some(HighlightKind::Type),
            "markup.bold" => Some(HighlightKind::Type),
            "markup.italic" => Some(HighlightKind::Attribute),
            "markup.list" => Some(HighlightKind::Keyword),
            _ => None,
        },
        _ => None,
    }
}

fn capture_priority(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Comment => 9,
        HighlightKind::String => 8,
        HighlightKind::Attribute => 7,
        HighlightKind::Number => 6,
        HighlightKind::Function => 5,
        HighlightKind::Type => 4,
        HighlightKind::Macro => 3,
        HighlightKind::Keyword => 2,
    }
}

// The capture name carries more information than the rendered highlight kind.
// In particular, upstream queries can capture the same identifier as both a
// generic field/property and an explicit method or macro. Keep that raw
// specificity so an exact-range dedup prefers the semantic capture while
// decorators/annotations still stay attributes.
fn capture_specificity(name: &str) -> u8 {
    if name == "function.macro" {
        return 70;
    }
    match name.split('.').next().unwrap_or(name) {
        "macro" => 70,
        "attribute" | "annotation" => 60,
        "function" => 50,
        "type" | "tag" | "module" | "namespace" | "constructor" => 45,
        "number" | "boolean" | "constant" => 40,
        "keyword" | "operator" => 35,
        "label" => 30,
        "property" | "field" | "member" => 10,
        _ => 20,
    }
}

// Every supported language uses bundled grammar highlight queries. The small
// extras only restore precise captures (attributes, calls, numbers, annotations)
// that are useful to the editor and are already supported by the grammar.
fn query_source(language: Language) -> String {
    match language {
        Language::Rust => join_queries(&[
            tree_sitter_rust::HIGHLIGHTS_QUERY,
            "(attribute (identifier) @attribute)\n(integer_literal) @number\n(float_literal) @number",
        ]),
        Language::Python => join_queries(&[
            tree_sitter_python::HIGHLIGHTS_QUERY,
            "(class_definition name: (identifier) @type)\n\
             (decorator (identifier) @attribute)\n\
             (decorator (call function: (identifier) @attribute))\n\
             (decorator (call function: (attribute attribute: (identifier) @attribute)))\n\
             (type) @type\n\
             (call function: (identifier) @function)\n\
             (call function: (attribute attribute: (identifier) @function))",
        ]),
        Language::Dart => join_queries(&[
            tree_sitter_dart::HIGHLIGHTS_QUERY,
            "(annotation) @attribute",
        ]),
        Language::JavaScript => join_queries(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
        ]),
        Language::TypeScript => join_queries(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ]),
        Language::Tsx => join_queries(&[
            tree_sitter_javascript::HIGHLIGHT_QUERY,
            tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        ]),
        Language::Go => join_queries(&[tree_sitter_go::HIGHLIGHTS_QUERY]),
        Language::C => join_queries(&[tree_sitter_c::HIGHLIGHT_QUERY, "(char_literal) @string"]),
        // C++'s bundled query covers C++-specific constructs but relies on the
        // C query for common keywords, literals, comments, and declarations.
        Language::Cpp => join_queries(&[
            tree_sitter_c::HIGHLIGHT_QUERY,
            tree_sitter_cpp::HIGHLIGHT_QUERY,
            "(char_literal) @string",
        ]),
        Language::Java => join_queries(&[tree_sitter_java::HIGHLIGHTS_QUERY]),
        Language::CSharp => join_queries(&[tree_sitter_c_sharp::HIGHLIGHTS_QUERY]),
        Language::Html => join_queries(&[
            tree_sitter_html::HIGHLIGHTS_QUERY,
            "(quoted_attribute_value) @string",
        ]),
        Language::Css => join_queries(&[tree_sitter_css::HIGHLIGHTS_QUERY]),
        Language::Json => join_queries(&[tree_sitter_json::HIGHLIGHTS_QUERY]),
        Language::Toml => join_queries(&[tree_sitter_toml_ng::HIGHLIGHTS_QUERY]),
        Language::Yaml => join_queries(&[tree_sitter_yaml::HIGHLIGHTS_QUERY]),
        Language::Markdown => join_queries(&[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK]),
    }
}

fn join_queries(parts: &[&str]) -> String {
    let mut source = String::new();
    for part in parts {
        if !source.is_empty() {
            source.push('\n');
        }
        source.push_str(part);
    }
    source
}

fn collect_query_spans(
    query: &Query,
    root: Node<'_>,
    text: &str,
    offset: usize,
    spans: &mut Vec<CapturedSpan>,
) {
    let mut cursor = QueryCursor::new();
    let mut captures = cursor.captures(query, root, text.as_bytes());
    while let Some((matched, capture_index)) = captures.next() {
        let capture = matched.captures()[*capture_index];
        let Some(name) = query.capture_names().get(capture.index as usize) else {
            continue;
        };
        let Some(kind) = capture_kind(name) else {
            continue;
        };
        let node = capture.node;
        let start = offset + node.start_byte();
        let end = offset + node.end_byte();
        if start < end {
            spans.push(CapturedSpan {
                span: HighlightSpan {
                    range: start..end,
                    kind,
                },
                specificity: capture_specificity(name),
            });
        }
    }
}

fn collect_markdown_inline_spans(
    inline: &mut MarkdownInlineHighlighter,
    tree: &Tree,
    text: &str,
    spans: &mut Vec<CapturedSpan>,
) {
    let mut cursor = tree.walk();
    let mut visited = false;
    loop {
        if !visited {
            let node = cursor.node();
            if node.kind() == "inline" {
                let range = node.byte_range();
                if let Some(source) = text.get(range.clone())
                    && let Some(inline_tree) = inline.parser.parse(source, None)
                {
                    collect_query_spans(
                        &inline.query,
                        inline_tree.root_node(),
                        source,
                        range.start,
                        spans,
                    );
                }
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

fn coherence_rank(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Comment => 2,
        HighlightKind::String => 1,
        _ => 0,
    }
}

fn kind_rank(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Keyword => 0,
        HighlightKind::String => 1,
        HighlightKind::Comment => 2,
        HighlightKind::Number => 3,
        HighlightKind::Function => 4,
        HighlightKind::Type => 5,
        HighlightKind::Macro => 6,
        HighlightKind::Attribute => 7,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ActiveSpan {
    start: usize,
    end: usize,
    kind: HighlightKind,
    specificity: u8,
    id: usize,
}

impl ActiveSpan {
    fn len(self) -> usize {
        self.end - self.start
    }
}

impl Ord for ActiveSpan {
    fn cmp(&self, other: &Self) -> Ordering {
        // Comments and strings are coherent tokens: nested captures must not
        // split them. For other overlaps, the most specific capture wins and
        // the outer capture only fills the uncovered gaps.
        coherence_rank(self.kind)
            .cmp(&coherence_rank(other.kind))
            .then_with(|| other.len().cmp(&self.len()))
            .then_with(|| self.specificity.cmp(&other.specificity))
            .then_with(|| capture_priority(self.kind).cmp(&capture_priority(other.kind)))
            .then_with(|| kind_rank(self.kind).cmp(&kind_rank(other.kind)))
            .then_with(|| other.start.cmp(&self.start))
            .then_with(|| other.end.cmp(&self.end))
            .then_with(|| other.id.cmp(&self.id))
    }
}

impl PartialOrd for ActiveSpan {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Converts overlapping tree-sitter captures into the sorted, non-overlapping
/// spans expected by the renderer.
///
/// Captures are sorted once and then swept with two boundary events each. The
/// active set is a heap, so the cost is O(n log n) time and O(n) working memory;
/// rendering a viewport remains independent of the number of document bytes.
#[cfg(test)]
fn resolve_overlaps(spans: Vec<HighlightSpan>) -> Vec<HighlightSpan> {
    resolve_captured_overlaps(
        spans
            .into_iter()
            .map(|span| CapturedSpan {
                span,
                specificity: 0,
            })
            .collect(),
    )
}

fn resolve_captured_overlaps(mut spans: Vec<CapturedSpan>) -> Vec<HighlightSpan> {
    if spans.is_empty() {
        return Vec::new();
    }

    // A tree-sitter node can be captured by several queries. Coalesce identical
    // ranges by raw capture specificity before the sweep so an explicit method,
    // macro, or annotation wins over a generic property capture at that range.
    spans.sort_unstable_by(|left, right| {
        left.span
            .range
            .start
            .cmp(&right.span.range.start)
            .then_with(|| left.span.range.end.cmp(&right.span.range.end))
            .then_with(|| coherence_rank(right.span.kind).cmp(&coherence_rank(left.span.kind)))
            .then_with(|| right.specificity.cmp(&left.specificity))
            .then_with(|| capture_priority(right.span.kind).cmp(&capture_priority(left.span.kind)))
            .then_with(|| kind_rank(left.span.kind).cmp(&kind_rank(right.span.kind)))
    });
    spans.dedup_by(|left, right| left.span.range == right.span.range);

    let mut events = Vec::with_capacity(spans.len() * 2);
    for (id, span) in spans.iter().enumerate() {
        // End events sort before start events at the same byte boundary.
        events.push((span.span.range.start, 1u8, id));
        events.push((span.span.range.end, 0u8, id));
    }
    events.sort_unstable_by_key(|event| (event.0, event.1, event.2));

    let mut active = vec![false; spans.len()];
    let mut heap = BinaryHeap::new();
    let mut resolved = Vec::new();
    let mut event_index = 0;

    while event_index < events.len() {
        let position = events[event_index].0;
        while event_index < events.len() && events[event_index].0 == position {
            let (_, is_start, id) = events[event_index];
            if is_start == 1 {
                active[id] = true;
                let span = spans[id].clone();
                heap.push(ActiveSpan {
                    start: span.span.range.start,
                    end: span.span.range.end,
                    kind: span.span.kind,
                    specificity: span.specificity,
                    id,
                });
            } else {
                active[id] = false;
            }
            event_index += 1;
        }

        while heap.peek().is_some_and(|span| !active[span.id]) {
            heap.pop();
        }

        if let Some(next) = events.get(event_index).map(|event| event.0)
            && next > position
            && let Some(span) = heap.peek()
        {
            push_span(&mut resolved, position..next, span.kind);
        }
    }

    resolved
}

fn push_span(spans: &mut Vec<HighlightSpan>, range: Range<usize>, kind: HighlightKind) {
    if range.start >= range.end {
        return;
    }
    if let Some(previous) = spans.last_mut()
        && previous.kind == kind
        && previous.range.end == range.start
    {
        previous.range.end = range.end;
        return;
    }
    spans.push(HighlightSpan { range, kind });
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
        let language = highlighter.language();
        assert!(
            highlighter
                .spans()
                .iter()
                .any(|span| span.range == expected && span.kind == kind),
            "{language:?} missing {kind:?} span for {needle:?}; spans: {:#?}",
            highlighter.spans()
        );
    }

    fn assert_covered_span(
        highlighter: &SyntaxHighlighter,
        text: &str,
        needle: &str,
        kind: HighlightKind,
    ) {
        let start = text
            .find(needle)
            .unwrap_or_else(|| panic!("missing sample {needle}"));
        let end = start + needle.len();
        let language = highlighter.language();
        assert!(
            highlighter.spans().iter().any(|span| {
                span.kind == kind && span.range.start <= start && span.range.end >= end
            }),
            "{language:?} missing {kind:?} coverage for {needle:?}; spans: {:#?}",
            highlighter.spans()
        );
    }

    fn assert_sorted_non_overlapping(highlighter: &SyntaxHighlighter) {
        let mut previous_end = 0;
        let language = highlighter.language();
        for span in highlighter.spans() {
            assert!(
                span.range.start < span.range.end,
                "{language:?} has an empty span in {:#?}",
                highlighter.spans()
            );
            assert!(
                span.range.start >= previous_end,
                "{language:?} has overlapping spans in {:#?}",
                highlighter.spans()
            );
            previous_end = span.range.end;
        }
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
        assert_eq!(
            incremental.spans(),
            reparsed.spans(),
            "{language:?} incremental insert did not match a full reparse"
        );
        assert!(
            incremental.spans().iter().all(|span| {
                span.range.start < span.range.end
                    && span.range.end <= new.len()
                    && new.is_char_boundary(span.range.start)
                    && new.is_char_boundary(span.range.end)
            }),
            "{language:?} produced an invalid span after insert"
        );

        let revert_range = start..start + replacement.len();
        assert!(
            new.is_char_boundary(revert_range.start) && new.is_char_boundary(revert_range.end),
            "{language:?} produced an invalid revert range {revert_range:?}"
        );
        incremental.replace_range(&new, old, revert_range, needle);
        let mut deleted = SyntaxHighlighter::new(language).unwrap();
        deleted.set_text(old);
        assert_eq!(
            incremental.spans(),
            deleted.spans(),
            "{language:?} incremental revert did not match the original text"
        );
        assert!(
            incremental.spans().iter().all(|span| {
                span.range.start < span.range.end
                    && span.range.end <= old.len()
                    && old.is_char_boundary(span.range.start)
                    && old.is_char_boundary(span.range.end)
            }),
            "{language:?} produced an invalid span after revert"
        );
    }

    struct LanguageSample {
        language: Language,
        text: &'static str,
        highlights: &'static [(&'static str, HighlightKind)],
        unicode_edit: (&'static str, &'static str),
    }

    fn language_samples() -> [LanguageSample; 17] {
        [
            LanguageSample {
                language: Language::Rust,
                text: r##"use std::fmt;

// color
pub struct Widget { value: i32 }

impl Widget {
    pub fn render(&self) -> String {
        let label = "hello 🦀";
        format!("{label}: {}", 42)
    }
}
"##,
                highlights: &[
                    ("use", HighlightKind::Keyword),
                    ("pub", HighlightKind::Keyword),
                    ("struct", HighlightKind::Keyword),
                    ("Widget", HighlightKind::Type),
                    ("i32", HighlightKind::Type),
                    ("render", HighlightKind::Function),
                    ("String", HighlightKind::Type),
                    ("\"hello 🦀\"", HighlightKind::String),
                    ("format!", HighlightKind::Macro),
                    ("42", HighlightKind::Number),
                    ("// color", HighlightKind::Comment),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::Python,
                text: r#"@decorator
class Box:
    """container"""
    def render(self, value: int) -> str:
        # color
        total = 42
        return f"value={value!r} {total}"
"#,
                highlights: &[
                    ("decorator", HighlightKind::Attribute),
                    ("class", HighlightKind::Keyword),
                    ("Box", HighlightKind::Type),
                    ("\"\"\"container\"\"\"", HighlightKind::String),
                    ("def", HighlightKind::Keyword),
                    ("render", HighlightKind::Function),
                    ("int", HighlightKind::Type),
                    ("str", HighlightKind::Type),
                    ("42", HighlightKind::Number),
                    ("f\"value={value!r} {total}\"", HighlightKind::String),
                    ("# color", HighlightKind::Comment),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::Dart,
                text: r#"import 'dart:math';

@immutable
class Counter {
  // color
  int next(int value) => value + 42;
}
"#,
                highlights: &[
                    ("import", HighlightKind::Keyword),
                    ("'dart:math'", HighlightKind::String),
                    ("@immutable", HighlightKind::Attribute),
                    ("class", HighlightKind::Keyword),
                    ("Counter", HighlightKind::Type),
                    ("int", HighlightKind::Type),
                    ("next", HighlightKind::Function),
                    ("42", HighlightKind::Number),
                    ("// color", HighlightKind::Comment),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::JavaScript,
                text: r#"import { render } from "./ui.js";

// color
export class Widget {}
export function greet(name, count = 42) {
  return paint(`Hello ${name}`, count);
}
"#,
                highlights: &[
                    ("import", HighlightKind::Keyword),
                    ("\"./ui.js\"", HighlightKind::String),
                    ("// color", HighlightKind::Comment),
                    ("class", HighlightKind::Keyword),
                    ("Widget", HighlightKind::Type),
                    ("function", HighlightKind::Keyword),
                    ("greet", HighlightKind::Function),
                    ("42", HighlightKind::Number),
                    ("paint", HighlightKind::Function),
                    ("`Hello ${name}`", HighlightKind::String),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::TypeScript,
                text: r#"interface User { id: number; name: string; }

// color
export function format(user: User): string {
  const count = 42;
  return `${user.name}: ${count}`;
}
"#,
                highlights: &[
                    ("interface", HighlightKind::Keyword),
                    ("User", HighlightKind::Type),
                    ("number", HighlightKind::Type),
                    ("string", HighlightKind::Type),
                    ("// color", HighlightKind::Comment),
                    ("function", HighlightKind::Keyword),
                    ("format", HighlightKind::Function),
                    ("42", HighlightKind::Number),
                    ("`${user.name}: ${count}`", HighlightKind::String),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::Tsx,
                text: r#"type Props = { title: string };

// color
export function Card({ title }: Props) {
  return <section className="card">{title}</section>;
}
"#,
                highlights: &[
                    ("type", HighlightKind::Keyword),
                    ("Props", HighlightKind::Type),
                    ("string", HighlightKind::Type),
                    ("// color", HighlightKind::Comment),
                    ("function", HighlightKind::Keyword),
                    ("Card", HighlightKind::Function),
                    ("section", HighlightKind::Type),
                    ("className", HighlightKind::Attribute),
                    ("\"card\"", HighlightKind::String),
                ],
                unicode_edit: ("\"card\"", "\"card 🦀\""),
            },
            LanguageSample {
                language: Language::Go,
                text: r#"package main

import "fmt"

// color
type User struct {
    Name string
}

func greet(user User) string {
    return fmt.Sprintf("hello %s", user.Name) + fmt.Sprint(42)
}
"#,
                highlights: &[
                    ("package", HighlightKind::Keyword),
                    ("\"fmt\"", HighlightKind::String),
                    ("// color", HighlightKind::Comment),
                    ("type", HighlightKind::Keyword),
                    ("User", HighlightKind::Type),
                    ("struct", HighlightKind::Keyword),
                    ("string", HighlightKind::Type),
                    ("greet", HighlightKind::Function),
                    ("Sprintf", HighlightKind::Function),
                    ("\"hello %s\"", HighlightKind::String),
                    ("42", HighlightKind::Number),
                ],
                unicode_edit: ("42", "\"🦀\""),
            },
            LanguageSample {
                language: Language::C,
                text: r#"#include <stdio.h>

// color
static int add(int left, int right) {
    return left + right + 42;
}

int main(void) {
    printf("value %c\n", 'x');
    return add(1, 2);
}
"#,
                highlights: &[
                    ("#include", HighlightKind::Keyword),
                    ("// color", HighlightKind::Comment),
                    ("static", HighlightKind::Keyword),
                    ("int", HighlightKind::Type),
                    ("add", HighlightKind::Function),
                    ("42", HighlightKind::Number),
                    ("printf", HighlightKind::Function),
                    ("\"value %c\\n\"", HighlightKind::String),
                    ("'x'", HighlightKind::String),
                ],
                unicode_edit: ("color", "cölör 🦀"),
            },
            LanguageSample {
                language: Language::Cpp,
                text: r#"#include <string>

// color
class Widget {
public:
    int add(int value) const;
};

int Widget::add(int value) const {
    return value + 42;
}
"#,
                highlights: &[
                    ("#include", HighlightKind::Keyword),
                    ("<string>", HighlightKind::String),
                    ("// color", HighlightKind::Comment),
                    ("class", HighlightKind::Keyword),
                    ("Widget", HighlightKind::Type),
                    ("public", HighlightKind::Keyword),
                    ("int", HighlightKind::Type),
                    ("add", HighlightKind::Function),
                    ("42", HighlightKind::Number),
                ],
                unicode_edit: ("color", "cölör 🦀"),
            },
            LanguageSample {
                language: Language::Java,
                text: r#"import java.util.List;

// color
public final class Box {
    private int count = 42;

    public String label(String name) {
        return "Hi " + name;
    }
}
"#,
                highlights: &[
                    ("import", HighlightKind::Keyword),
                    ("// color", HighlightKind::Comment),
                    ("public", HighlightKind::Keyword),
                    ("class", HighlightKind::Keyword),
                    ("Box", HighlightKind::Type),
                    ("int", HighlightKind::Type),
                    ("42", HighlightKind::Number),
                    ("label", HighlightKind::Function),
                    ("String", HighlightKind::Type),
                    ("\"Hi \"", HighlightKind::String),
                ],
                unicode_edit: ("color", "cölör 🦀"),
            },
            LanguageSample {
                language: Language::CSharp,
                text: r#"using System;

// color
public sealed class Box {
    private int count = 42;

    public string Label(string name) {
        return "Hi " + name;
    }
}
"#,
                highlights: &[
                    ("using", HighlightKind::Keyword),
                    ("// color", HighlightKind::Comment),
                    ("sealed", HighlightKind::Keyword),
                    ("class", HighlightKind::Keyword),
                    ("Box", HighlightKind::Type),
                    ("int", HighlightKind::Type),
                    ("42", HighlightKind::Number),
                    ("Label", HighlightKind::Function),
                    ("string", HighlightKind::Type),
                    ("\"Hi \"", HighlightKind::String),
                ],
                unicode_edit: ("color", "cölör 🦀"),
            },
            LanguageSample {
                language: Language::Html,
                text: r#"<!DOCTYPE html>
<!-- color -->
<main class="app" data-count="42">
  <h1>Hello</h1>
</main>
"#,
                highlights: &[
                    ("<!DOCTYPE html>", HighlightKind::Keyword),
                    ("<!-- color -->", HighlightKind::Comment),
                    ("main", HighlightKind::Type),
                    ("class", HighlightKind::Attribute),
                    ("\"app\"", HighlightKind::String),
                    ("\"42\"", HighlightKind::String),
                    ("h1", HighlightKind::Type),
                ],
                unicode_edit: ("color", "cölör 🦀"),
            },
            LanguageSample {
                language: Language::Css,
                text: r#"/* theme */
:root { --accent: #ff6600; }
.card:hover {
  color: rgb(255, 102, 0);
  font-size: 14px;
}
"#,
                highlights: &[
                    ("/* theme */", HighlightKind::Comment),
                    ("--accent", HighlightKind::Attribute),
                    ("#ff6600", HighlightKind::String),
                    ("card", HighlightKind::Attribute),
                    ("color", HighlightKind::Attribute),
                    ("rgb", HighlightKind::Function),
                    ("255", HighlightKind::Number),
                    ("14", HighlightKind::Number),
                    ("px", HighlightKind::Type),
                ],
                unicode_edit: ("theme", "théme 🦀"),
            },
            LanguageSample {
                language: Language::Json,
                text: r#"{
  // color
  "name": "aircode 🦀",
  "count": 42,
  "enabled": true
}
"#,
                highlights: &[
                    ("// color", HighlightKind::Comment),
                    ("\"name\"", HighlightKind::String),
                    ("\"aircode 🦀\"", HighlightKind::String),
                    ("42", HighlightKind::Number),
                    ("true", HighlightKind::Keyword),
                ],
                unicode_edit: ("\"aircode 🦀\"", "\"🦀\""),
            },
            LanguageSample {
                language: Language::Toml,
                text: r#"# color
title = "AIRcode"
count = 42
enabled = true

[package]
name = "demo"
"#,
                highlights: &[
                    ("# color", HighlightKind::Comment),
                    ("\"AIRcode\"", HighlightKind::String),
                    ("42", HighlightKind::Number),
                    ("true", HighlightKind::Keyword),
                    ("package", HighlightKind::Type),
                    ("\"demo\"", HighlightKind::String),
                ],
                unicode_edit: ("\"AIRcode\"", "\"AIRcode 🦀\""),
            },
            LanguageSample {
                language: Language::Yaml,
                text: r#"# color
name: AIRcode
count: 42
enabled: true
nested:
  - "item"
"#,
                highlights: &[
                    ("# color", HighlightKind::Comment),
                    ("AIRcode", HighlightKind::String),
                    ("42", HighlightKind::Number),
                    ("true", HighlightKind::Keyword),
                    ("\"item\"", HighlightKind::String),
                ],
                unicode_edit: ("AIRcode", "AIRcode 🦀"),
            },
            LanguageSample {
                language: Language::Markdown,
                text: r####"# AIRcode 🦀

A **bold** and *italic* line with `code`.

[site](https://example.com)

```rust
fn main() {}
```
"####,
                highlights: &[
                    ("AIRcode 🦀", HighlightKind::Type),
                    ("**bold**", HighlightKind::Type),
                    ("*italic*", HighlightKind::Attribute),
                    ("`code`", HighlightKind::String),
                    ("https://example.com", HighlightKind::String),
                ],
                unicode_edit: ("bold", "bold 🦀"),
            },
        ]
    }

    #[test]
    fn every_language_fixture_has_precise_non_overlapping_spans() {
        for sample in language_samples() {
            let mut highlighter = SyntaxHighlighter::new(sample.language).unwrap();
            highlighter.set_text(sample.text);
            assert_sorted_non_overlapping(&highlighter);
            for &(needle, kind) in sample.highlights {
                assert_span(&highlighter, sample.text, needle, kind);
            }
            for span in highlighter.spans() {
                if matches!(span.kind, HighlightKind::Function | HighlightKind::Type) {
                    let highlighted = &sample.text[span.range.clone()];
                    assert!(
                        !highlighted.contains(['{', '}', '\n']),
                        "{:?} broad container highlighted as {:?}: {highlighted:?}",
                        sample.language,
                        span.kind
                    );
                }
            }
        }
    }

    #[test]
    fn overlap_resolution_preserves_nested_and_coherent_tokens() {
        let resolved = resolve_overlaps(vec![
            HighlightSpan {
                range: 0..20,
                kind: HighlightKind::Type,
            },
            HighlightSpan {
                range: 2..8,
                kind: HighlightKind::Keyword,
            },
            HighlightSpan {
                range: 5..15,
                kind: HighlightKind::String,
            },
            HighlightSpan {
                range: 17..20,
                kind: HighlightKind::Comment,
            },
            HighlightSpan {
                range: 6..7,
                kind: HighlightKind::Number,
            },
        ]);
        assert_eq!(
            resolved,
            vec![
                HighlightSpan {
                    range: 0..2,
                    kind: HighlightKind::Type,
                },
                HighlightSpan {
                    range: 2..5,
                    kind: HighlightKind::Keyword,
                },
                HighlightSpan {
                    range: 5..15,
                    kind: HighlightKind::String,
                },
                HighlightSpan {
                    range: 15..17,
                    kind: HighlightKind::Type,
                },
                HighlightSpan {
                    range: 17..20,
                    kind: HighlightKind::Comment,
                },
            ]
        );

        let resolved = resolve_overlaps(vec![
            HighlightSpan {
                range: 0..10,
                kind: HighlightKind::Comment,
            },
            HighlightSpan {
                range: 3..5,
                kind: HighlightKind::Number,
            },
        ]);
        assert_eq!(
            resolved,
            vec![HighlightSpan {
                range: 0..10,
                kind: HighlightKind::Comment,
            }]
        );
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
            ("println!", HighlightKind::Macro),
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
        assert_covered_span(&highlighter, text, "derive", HighlightKind::Attribute);
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
    fn every_language_matches_full_reparse_after_unicode_insert_and_remove() {
        for sample in language_samples() {
            let (needle, replacement) = sample.unicode_edit;
            assert_incremental_reparse(sample.language, sample.text, needle, replacement);
        }
    }

    fn large_fixture_text(sample: &LanguageSample) -> String {
        if sample.language != Language::Json {
            return sample.text.repeat(64);
        }

        let trimmed = sample.text.trim();
        let inner = trimmed
            .strip_prefix('{')
            .and_then(|text| text.strip_suffix('}'))
            .unwrap_or(trimmed);
        let mut text = String::from("[");
        for index in 0..64 {
            if index > 0 {
                text.push(',');
            }
            text.push('{');
            text.push_str(inner);
            text.push('}');
        }
        text.push(']');
        text
    }

    #[test]
    fn large_fixtures_remain_precise_and_non_overlapping() {
        for sample in language_samples() {
            let text = large_fixture_text(&sample);
            let mut highlighter = SyntaxHighlighter::new(sample.language).unwrap();
            highlighter.set_text(&text);
            assert_sorted_non_overlapping(&highlighter);
            assert!(!highlighter.spans().is_empty(), "{:?}", sample.language);
            for &(needle, kind) in sample.highlights {
                assert_span(&highlighter, &text, needle, kind);
            }
            for span in highlighter.spans() {
                assert!(
                    span.range.end <= text.len()
                        && text.is_char_boundary(span.range.start)
                        && text.is_char_boundary(span.range.end),
                    "{:?} returned an invalid UTF-8 byte range: {span:?}",
                    sample.language
                );
                if matches!(span.kind, HighlightKind::Function | HighlightKind::Type) {
                    let highlighted = &text[span.range.clone()];
                    assert!(
                        !highlighted.contains(['{', '}', '\n']),
                        "{:?} broad container highlighted as {:?}: {highlighted:?}",
                        sample.language,
                        span.kind
                    );
                }
            }
        }
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
        for sample in language_samples() {
            let text = large_fixture_text(&sample);
            let mut highlighter = SyntaxHighlighter::new(sample.language).unwrap();
            highlighter.set_text(&text);
            let mut ranges = vec![0..text.len(), text.len()..text.len()];
            ranges.extend(
                text.match_indices('\n')
                    .skip(20)
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
                assert_eq!(actual, expected, "{:?}, {range:?}", sample.language);
            }
        }
    }
}
