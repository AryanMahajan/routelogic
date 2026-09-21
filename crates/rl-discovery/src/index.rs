//! Parsing source into syntax trees.
//!
//! tree-sitter rather than a language-specific parser, for three reasons: one API across
//! Python, JavaScript, TypeScript and Go; error tolerance, so a file that does not currently
//! compile still yields routes; and patterns that read as structure rather than as
//! hand-rolled visitor code.
//!
//! The trade-off is no name resolution. That is hand-written in [`crate::graph`] — and it
//! would have been hand-written under any of the alternatives, since none of them are type
//! checkers.

use crate::error::{DiscoveryError, Result};
use crate::project::Language;
use std::path::{Path, PathBuf};
use tree_sitter::{Node, Parser, Tree};

/// A parsed source file.
pub struct ParsedFile {
    /// Relative to the project root, so spans are stable across machines.
    pub path: PathBuf,
    pub language: Language,
    pub source: String,
    pub tree: Tree,
}

impl ParsedFile {
    pub fn root(&self) -> Node<'_> {
        self.tree.root_node()
    }

    /// The source text a node covers.
    pub fn text(&self, node: Node<'_>) -> &str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    /// 1-indexed position of a node.
    pub fn span(&self, node: Node<'_>) -> crate::facts::Span {
        let start = node.start_position();
        crate::facts::Span::new(start.row as u32 + 1, start.column as u32 + 1)
    }

    /// The path as text, forward slashes on every platform.
    pub fn display_path(&self) -> String {
        crate::project::display(&self.path)
    }

    /// Whether the parse hit a syntax error anywhere.
    ///
    /// Not a reason to discard the file — tree-sitter recovers, and the routes around the
    /// broken region are still worth having — but worth surfacing.
    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }
}

impl std::fmt::Debug for ParsedFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParsedFile")
            .field("path", &self.path)
            .field("language", &self.language)
            .field("bytes", &self.source.len())
            .finish()
    }
}

/// Parses files, reusing one parser per grammar.
pub struct SourceIndex {
    python: Parser,
    javascript: Parser,
    typescript: Parser,
    /// TSX is a separate grammar: `<T>` is a type assertion in `.ts` and a tag in `.tsx`.
    tsx: Parser,
    go: Parser,
}

fn parser(language: &tree_sitter::Language, name: &'static str) -> Result<Parser> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|source| DiscoveryError::Grammar {
            language: name,
            source,
        })?;
    Ok(parser)
}

impl SourceIndex {
    pub fn new() -> Result<SourceIndex> {
        Ok(SourceIndex {
            python: parser(&tree_sitter_python::LANGUAGE.into(), "python")?,
            javascript: parser(&tree_sitter_javascript::LANGUAGE.into(), "javascript")?,
            typescript: parser(
                &tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
            )?,
            tsx: parser(&tree_sitter_typescript::LANGUAGE_TSX.into(), "tsx")?,
            go: parser(&tree_sitter_go::LANGUAGE.into(), "go")?,
        })
    }

    pub fn parse(
        &mut self,
        path: impl Into<PathBuf>,
        language: Language,
        source: String,
    ) -> Option<ParsedFile> {
        let path = path.into();
        let is_tsx = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("tsx"));

        let parser = match language {
            Language::Python => &mut self.python,
            Language::JavaScript => &mut self.javascript,
            Language::TypeScript if is_tsx => &mut self.tsx,
            Language::TypeScript => &mut self.typescript,
            Language::Go => &mut self.go,
        };

        let tree = parser.parse(&source, None)?;
        Some(ParsedFile {
            path,
            language,
            source,
            tree,
        })
    }

    /// Parse a file from disk, relative to a project root.
    pub fn parse_file(&mut self, root: &Path, relative: &Path) -> Option<ParsedFile> {
        let language = Language::of(relative)?;
        let source = std::fs::read_to_string(root.join(relative)).ok()?;
        self.parse(relative.to_path_buf(), language, source)
    }
}

/// Walk every node in a tree, depth first.
///
/// Adapters recognise a handful of node kinds scattered anywhere in a file, so a plain walk
/// is both simpler and easier to follow than a set of queries that would each need their own
/// argument inspection afterwards.
pub fn walk<'a>(node: Node<'a>, visit: &mut dyn FnMut(Node<'a>)) {
    visit(node);
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, visit);
    }
}

/// Find the first child of a given kind.
pub fn child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    (0..node.child_count() as u32)
        .filter_map(|i| node.child(i))
        .find(|c| c.kind() == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> ParsedFile {
        SourceIndex::new()
            .unwrap()
            .parse("main.py", Language::Python, source.to_string())
            .unwrap()
    }

    #[test]
    fn parses_python() {
        let file = parse("def hello():\n    return 1\n");
        assert!(!file.has_errors());
        assert_eq!(file.root().kind(), "module");
    }

    #[test]
    fn recovers_from_a_syntax_error_and_still_yields_a_tree() {
        // The point of tree-sitter here: a half-typed file still gives usable structure.
        let file = parse("def hello(:\n    return 1\n\ndef world():\n    return 2\n");
        assert!(file.has_errors());

        let mut functions = 0;
        walk(file.root(), &mut |node| {
            if node.kind() == "function_definition" {
                functions += 1;
            }
        });
        assert!(functions >= 1, "the intact function should still be found");
    }

    #[test]
    fn node_text_and_spans_are_reported() {
        let file = parse("x = 1\ny = 2\n");
        let mut spans = Vec::new();
        walk(file.root(), &mut |node| {
            if node.kind() == "assignment" {
                spans.push((file.span(node).line, file.text(node).to_string()));
            }
        });
        assert_eq!(
            spans,
            vec![(1, "x = 1".to_string()), (2, "y = 2".to_string())]
        );
    }

    #[test]
    fn parses_javascript_and_typescript() {
        let mut index = SourceIndex::new().unwrap();

        let js = index
            .parse("a.js", Language::JavaScript, "const x = 1;".into())
            .unwrap();
        assert!(!js.has_errors());
        assert_eq!(js.root().kind(), "program");

        let ts = index
            .parse("a.ts", Language::TypeScript, "const x: number = 1;".into())
            .unwrap();
        assert!(!ts.has_errors());
    }

    /// The reason TSX gets its own grammar: this is a type assertion in `.ts` and a JSX
    /// element in `.tsx`, and a parser that only knows one will error on the other.
    #[test]
    fn tsx_files_use_the_tsx_grammar() {
        let mut index = SourceIndex::new().unwrap();
        let source = "export default function Page() { return <div>hi</div>; }".to_string();

        let tsx = index
            .parse("page.tsx", Language::TypeScript, source.clone())
            .unwrap();
        assert!(!tsx.has_errors());

        let ts = index
            .parse("page.ts", Language::TypeScript, source)
            .unwrap();
        assert!(ts.has_errors(), "JSX is not valid in a plain .ts file");
    }

    #[test]
    fn handles_multibyte_source() {
        let file = parse("x = \"héllo → world\"\n");
        assert!(!file.has_errors());
        let mut found = false;
        walk(file.root(), &mut |node| {
            if node.kind() == "string" {
                assert!(file.text(node).contains('→'));
                found = true;
            }
        });
        assert!(found);
    }
}
