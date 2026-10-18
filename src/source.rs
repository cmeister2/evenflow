//! Shared, source-preserving Rust syntax and executable-body queries.

use std::collections::BTreeMap;

use tree_sitter::{Node, Parser, Tree};

use crate::model::{AnalysisIssue, ByteSpan};

/// The structural kind of an executable body.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyKind {
    /// A function, method, or provided trait method body.
    Function,
    /// The primary or alternative branch of an `if` expression.
    Branch,
    /// A `for`, `while`, or infinite loop body.
    Loop,
    /// The diverging body of a `let ... else` declaration.
    LetElse,
    /// A braced match-arm expression.
    MatchArm,
    /// A closure with a braced body.
    Closure,
    /// An asynchronous block.
    Async,
    /// An unsafe block.
    Unsafe,
    /// An inline constant block, retained for structural consumers.
    Const,
    /// An explicit scope or another runtime block expression.
    Scope,
}

impl BodyKind {
    /// Return a short description suitable for a diagnostic.
    pub fn description(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Branch => "branch",
            Self::Loop => "loop",
            Self::LetElse => "let-else",
            Self::MatchArm => "match-arm",
            Self::Closure => "closure",
            Self::Async => "async",
            Self::Unsafe => "unsafe",
            Self::Const => "const",
            Self::Scope => "scope",
        }
    }
}

/// Lexical execution context retained even when a rule excludes the body.
#[derive(Clone, Debug, Default)]
pub struct BodyContext {
    /// The nearest enclosing function's original source span.
    pub function: Option<ByteSpan>,
    /// Whether execution is enclosed by an async function or block.
    pub is_async: bool,
    /// Whether execution is enclosed by an unsafe function or block.
    pub is_unsafe: bool,
    /// Whether the subtree belongs to a const-evaluation context.
    pub is_const: bool,
    /// Whether the subtree is definitely test-only.
    pub is_test: bool,
}

/// An indexed Rust attribute and its original source spelling.
#[derive(Clone, Debug)]
pub struct Attribute {
    /// Original source range, including `#[` and `]`.
    pub span: ByteSpan,
    /// Complete original source text.
    pub text: String,
}

/// An indexed comment, independent of any rule's interpretation.
#[derive(Clone, Debug)]
pub struct Comment {
    /// Original source range, including its delimiters.
    pub span: ByteSpan,
    /// Original source text, including its delimiters.
    pub text: String,
    /// Whether the comment occupies its own source line or lines.
    pub standalone: bool,
    /// Whether Rust interprets the comment as API documentation.
    pub is_doc: bool,
    /// Whether local lint directives are applicable at this location.
    pub in_scope: bool,
}

/// A direct executable statement or the single trailing expression of a body.
#[derive(Clone, Debug)]
pub struct Statement {
    /// Original range of the statement, including a terminating semicolon.
    pub span: ByteSpan,
    /// Tree-sitter's original statement node kind.
    pub kind: String,
    /// The expression node kind after removing an expression-statement wrapper.
    pub expression_kind: String,
    /// Range of the unwrapped expression or declaration.
    pub expression: ByteSpan,
    /// Whether this is the body's trailing expression rather than a statement.
    pub is_tail: bool,
    /// Attributes immediately attached to this statement.
    pub attributes: Vec<Attribute>,
    /// Normalized invocation path when the direct expression is a macro.
    pub macro_path: Option<String>,
}

/// An executable block and its direct source-ordered sequence.
#[derive(Clone, Debug)]
pub struct Body {
    /// Range of the braces and their contents.
    pub span: ByteSpan,
    /// Range of the construct owning this body.
    pub owner: ByteSpan,
    /// Original syntax kind of the owning construct.
    pub owner_kind: String,
    /// Structural kind of the body.
    pub kind: BodyKind,
    /// Index of the nearest lexically enclosing executable body.
    pub parent: Option<usize>,
    /// Direct executable statements, with the tail represented exactly once.
    pub statements: Vec<Statement>,
    /// Attributes attached to the owning construct.
    pub attributes: Vec<Attribute>,
    /// Lexical context, including contexts omitted by FC001.
    pub context: BodyContext,
    /// Whether production/test and const scope policies include this body.
    pub in_scope: bool,
    /// Exact leading comment ranges associated only with this body.
    pub annotations: Vec<ByteSpan>,
}

impl Body {
    /// Return the first direct executable statement, ignoring decorations/items.
    pub fn first_statement(&self) -> Option<&Statement> {
        self.statements.first()
    }
}

/// An immutable parsed file shared by all bundled rules.
pub struct SourceContext {
    /// Normalized, slash-separated file path supplied by the caller.
    pub path: String,
    /// Complete validated UTF-8 source.
    pub text: String,
    /// Complete source-preserving syntax tree, including excluded subtrees.
    pub tree: Tree,
    /// Executable bodies in source traversal order.
    pub bodies: Vec<Body>,
    /// All original comments, including documentation and excluded scopes.
    pub comments: Vec<Comment>,
    /// All original attributes.
    pub attributes: Vec<Attribute>,
    /// Possible unexamined executable token groups in opaque macro invocations.
    gaps: Vec<ByteSpan>,
    /// Whether test-only scopes participate in linting.
    include_tests: bool,
    /// Candidate body associations before ownership is made unambiguous.
    anchors: Vec<Vec<usize>>,
    /// Indexed beginning byte offset of each original source line.
    line_starts: Vec<usize>,
}

impl SourceContext {
    /// Parse and index one complete Rust source file without filesystem access.
    ///
    /// # Errors
    /// Returns an encoding, parser-compatibility, or parsing issue. Recovered
    /// trees containing any error or missing node are rejected before filtering.
    pub fn parse(path: &str, bytes: &[u8], include_tests: bool) -> Result<Self, AnalysisIssue> {
        let text = std::str::from_utf8(bytes).map_err(|error| AnalysisIssue {
            category: "encoding".into(),
            message: format!("source is not valid UTF-8: {error}"),
            span: None,
        })?;
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .map_err(|error| AnalysisIssue {
                category: "parsing".into(),
                message: format!("cannot load the Rust grammar: {error}"),
                span: None,
            })?;
        let tree = parser.parse(bytes, None).ok_or_else(|| AnalysisIssue {
            category: "parsing".into(),
            message: "Rust parsing did not complete".into(),
            span: None,
        })?;
        // A recovered tree must never turn invalid or unsupported syntax into success.
        if let Some(node) = first_parse_error(tree.root_node()) {
            let message = if node.is_missing() {
                format!("missing Rust syntax: {}", node.kind())
            } else {
                "invalid or unsupported Rust syntax".into()
            };
            return Err(AnalysisIssue {
                category: "parsing".into(),
                message,
                span: Some(node_span(node)),
            });
        }
        let mut context = Self {
            path: path.replace('\\', "/"),
            text: text.to_owned(),
            tree,
            bodies: Vec::new(),
            comments: Vec::new(),
            attributes: Vec::new(),
            gaps: Vec::new(),
            include_tests,
            anchors: Vec::new(),
            line_starts: std::iter::once(0)
                .chain(
                    bytes
                        .iter()
                        .enumerate()
                        .filter_map(|(index, byte)| (*byte == b'\n').then_some(index + 1)),
                )
                .collect(),
        };
        // Index against a cheap tree handle so mutable facts never borrow the owned tree.
        let indexing_tree = context.tree.clone();
        context.index_node(
            indexing_tree.root_node(),
            &BodyContext::default(),
            None,
            false,
        );
        context.associate_comments();
        Ok(context)
    }

    /// Borrow original source for a valid byte range.
    pub fn slice(&self, span: ByteSpan) -> &str {
        self.text.get(span.start..span.end).unwrap_or("")
    }

    /// Find the original named syntax node with exactly this range, if present.
    pub fn node(&self, span: ByteSpan) -> Option<Node<'_>> {
        let mut node = self
            .tree
            .root_node()
            .named_descendant_for_byte_range(span.start, span.end)?;
        // Wrappers can share their child's range; climb to the original outer node.
        while let Some(parent) = node.parent() {
            if node_span(parent) != span {
                break;
            }
            node = parent;
        }
        (node_span(node) == span).then_some(node)
    }

    /// Return the one-based line and Unicode code-point column for a byte offset.
    pub fn location(&self, offset: usize) -> (usize, usize) {
        let mut end = offset.min(self.text.len());
        // Keep even defensive callers on a UTF-8 character boundary.
        while !self.text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        let line = self.line_starts.partition_point(|start| *start <= end);
        let start = self.line_starts[line - 1];
        let column = self.text[start..end].chars().count() + 1;
        (line, column)
    }

    /// Return the opaque macro invocations that can hide executable bodies.
    pub fn coverage_gaps(&self) -> Vec<ByteSpan> {
        self.gaps.clone()
    }

    /// Look up an indexed comment using its exact original source span.
    pub fn comment(&self, span: ByteSpan) -> Option<&Comment> {
        self.comments
            .binary_search_by_key(&span, |comment| comment.span)
            .ok()
            .map(|index| &self.comments[index])
    }

    /// Return comments immediately preceding a syntax anchor through trivia.
    pub fn leading_comments(&self, anchor: usize) -> Vec<&Comment> {
        let mut boundary = anchor;
        let mut found = Vec::new();
        loop {
            // Walk decorations backwards; executable syntax always ends association.
            let comment_index = self
                .comments
                .partition_point(|item| item.span.end <= boundary);
            let comment = comment_index
                .checked_sub(1)
                .and_then(|index| self.comments.get(index));
            let attribute_index = self
                .attributes
                .partition_point(|item| item.span.end <= boundary);
            let attribute = attribute_index
                .checked_sub(1)
                .and_then(|index| self.attributes.get(index));
            let previous_end = comment
                .map(|item| item.span.end)
                .into_iter()
                .chain(attribute.map(|item| item.span.end))
                .max();
            let Some(end) = previous_end else { break };
            if !self.text[end..boundary].trim().is_empty() {
                break;
            }
            if let Some(item) = attribute.filter(|item| item.span.end == end) {
                boundary = item.span.start;
            } else if let Some(item) = comment.filter(|item| item.span.end == end) {
                found.push(item);
                boundary = item.span.start;
            } else {
                break;
            }
        }
        found.reverse();
        found
    }

    /// Determine whether a direct statement is a plain, noncomputed field copy.
    pub fn is_plain_field_copy(&self, statement: &Statement) -> bool {
        let Some(mut node) = self.node(statement.expression) else {
            return false;
        };
        // The unwrapped source range may coincide with an expression-statement wrapper.
        if node.kind() == "expression_statement" {
            let mut cursor = node.walk();
            if let Some(expression) = node
                .named_children(&mut cursor)
                .find(|child| !is_comment(*child))
            {
                node = expression;
            }
        }
        if node.kind() != "assignment_expression" {
            return false;
        }
        let Some(left) = node.child_by_field_name("left") else {
            return false;
        };
        let Some(right) = node.child_by_field_name("right") else {
            return false;
        };
        left.kind() == "field_expression" && plain_copy_value(left) && plain_copy_value(right)
    }

    /// Index a node while retaining inherited context and body ownership.
    fn index_node(
        &mut self,
        node: Node<'_>,
        inherited: &BodyContext,
        parent_body: Option<usize>,
        opaque: bool,
    ) {
        let mut context = inherited.clone();
        let attributes = attached_attributes(node, &self.text);
        context.is_test |= attributes
            .iter()
            .any(|attribute| test_attribute(&attribute.text));
        // Inner cfg attributes govern their complete containing crate, item, or block.
        if matches!(node.kind(), "source_file" | "declaration_list" | "block") {
            let mut cursor = node.walk();
            context.is_test |= node.named_children(&mut cursor).any(|child| {
                child.kind() == "inner_attribute_item"
                    && test_attribute(self.slice(node_span(child)))
            });
        }
        context.is_const |= const_context(node);
        // Function modifiers establish the context for the whole function subtree.
        if node.kind() == "function_item" {
            context.function = Some(node_span(node));
            if let Some(modifiers) = named_child(node, "function_modifiers") {
                // Record each modifier independently because Rust permits their combination.
                context.is_const |= has_child_token(modifiers, "const");
                context.is_async |= has_child_token(modifiers, "async");
                context.is_unsafe |= has_child_token(modifiers, "unsafe");
            }
        }
        context.is_async |= node.kind() == "async_block";
        context.is_unsafe |= node.kind() == "unsafe_block";
        let in_scope = !opaque && !context.is_const && (self.include_tests || !context.is_test);
        if is_comment(node) {
            // Comments and attributes remain indexed even inside excluded syntax.
            let span = node_span(node);
            let raw = self.slice(span).to_owned();
            self.comments.push(Comment {
                span,
                standalone: standalone_comment(&self.text, span),
                is_doc: doc_comment(&raw),
                text: raw,
                in_scope,
            });
            return;
        }
        if matches!(node.kind(), "attribute_item" | "inner_attribute_item") {
            self.attributes.push(Attribute {
                span: node_span(node),
                text: self.slice(node_span(node)).to_owned(),
            });
        }
        let macro_node = node.kind() == "macro_invocation";
        // Only real token-tree delimiters create a macro coverage boundary.
        if macro_node
            && in_scope
            && let Some(path) = macro_path(node, &self.text)
            && !is_logging_macro(&path)
            && !is_data_macro(&path)
            && macro_has_braces(node)
        {
            self.gaps.push(node_span(node));
        }
        let opaque = opaque || macro_node || node.kind() == "macro_definition";
        let mut next_parent = parent_body;
        if node.kind() == "block" && !opaque {
            // Blocks belong to the source model even when a rule omits their scope.
            let (kind, owner) = classify_body(node);
            let body_index = self.bodies.len();
            self.bodies.push(Body {
                span: node_span(node),
                owner: node_span(owner),
                owner_kind: owner.kind().to_owned(),
                kind,
                parent: parent_body,
                statements: direct_statements(node, &self.text),
                attributes: attached_attributes(owner, &self.text),
                context: context.clone(),
                in_scope,
                annotations: Vec::new(),
            });
            self.anchors.push(body_anchors(owner));
            next_parent = Some(body_index);
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            // Descend all original syntax; token trees stay opaque to executable analysis.
            let mut child_context = context.clone();
            if field_is_const(node, child) {
                child_context.is_const = true;
            }
            self.index_node(child, &child_context, next_parent, opaque);
        }
    }

    /// Resolve decoration anchors to one outermost body, then index exact comments.
    fn associate_comments(&mut self) {
        let mut owners = BTreeMap::<usize, usize>::new();
        // An enclosing construct's primary body wins over blocks in its condition.
        for (index, anchors) in self.anchors.iter().enumerate() {
            for anchor in anchors {
                let replace = owners.get(anchor).is_none_or(|old| {
                    self.bodies[index].owner.start < self.bodies[*old].owner.start
                });
                if replace {
                    owners.insert(*anchor, index);
                }
            }
        }
        let mut comment_owners = BTreeMap::<ByteSpan, usize>::new();
        // Outside and inside placements identify the exact same suppression target.
        for (anchor, index) in owners {
            for comment in self.leading_comments(anchor) {
                comment_owners.entry(comment.span).or_insert(index);
            }
        }
        // A nested body's own preamble annotates that body rather than its first child's body.
        for (index, body) in self.bodies.iter().enumerate() {
            if body.kind == BodyKind::Function {
                continue;
            }
            if let Some(first) = body.first_statement() {
                for comment in self.leading_comments(first.span.start) {
                    comment_owners.insert(comment.span, index);
                }
            }
        }
        let mut associations = vec![Vec::new(); self.bodies.len()];
        for (span, index) in comment_owners {
            associations[index].push(span);
            if !self.bodies[index].in_scope
                && let Ok(comment_index) = self
                    .comments
                    .binary_search_by_key(&span, |comment| comment.span)
            {
                self.comments[comment_index].in_scope = false;
            }
        }
        for (body, mut spans) in self.bodies.iter_mut().zip(associations) {
            // Store deterministic, duplicate-free comment targets for shared suppression handling.
            spans.sort_unstable();
            spans.dedup();
            body.annotations = spans;
        }
    }
}

/// Convert a Tree-sitter range to an owned byte span.
pub fn node_span(node: Node<'_>) -> ByteSpan {
    ByteSpan {
        start: node.start_byte(),
        end: node.end_byte(),
    }
}

/// Normalize a Rust macro path without resolving aliases or imports.
pub fn macro_path(node: Node<'_>, source: &str) -> Option<String> {
    if node.kind() != "macro_invocation" {
        return None;
    }
    let path = node.child_by_field_name("macro")?;
    let mut parts = Vec::new();
    path_components(path, source, &mut parts);
    (!parts.is_empty()).then(|| parts.join("::"))
}

/// Gather path identifier tokens while ignoring original whitespace and comment trivia.
fn path_components<'source>(node: Node<'_>, source: &'source str, parts: &mut Vec<&'source str>) {
    if matches!(node.kind(), "identifier" | "self" | "super" | "crate") {
        parts.push(&source[node.start_byte()..node.end_byte()]);
        return;
    }
    let mut cursor = node.walk();
    for child in node
        .named_children(&mut cursor)
        .filter(|child| !is_comment(*child))
    {
        path_components(child, source, parts);
    }
}

/// Recognize the exact logging macro paths covered by the first-release policy.
pub fn is_logging_macro(path: &str) -> bool {
    let normalized = path.trim_start_matches("::");
    let (prefix, name) = normalized.rsplit_once("::").unwrap_or(("", normalized));
    match prefix {
        "" => matches!(name, "trace" | "debug" | "info" | "warn" | "error" | "crit"),
        "log" | "tracing" => matches!(name, "trace" | "debug" | "info" | "warn" | "error"),
        "slog" => matches!(name, "trace" | "debug" | "info" | "warn" | "error" | "crit"),
        _ => false,
    }
}

/// Recognize exact opaque data macros whose arguments intentionally stay unexamined.
fn is_data_macro(path: &str) -> bool {
    matches!(
        path,
        "matches"
            | "std::matches"
            | "core::matches"
            | "vec"
            | "std::vec"
            | "alloc::vec"
            | "json"
            | "serde_json::json"
    )
}

/// Return the first error or missing node in source order, including anonymous nodes.
fn first_parse_error(node: Node<'_>) -> Option<Node<'_>> {
    if node.is_error() || node.is_missing() {
        return Some(node);
    }
    if !node.has_error() {
        return None;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor).find_map(first_parse_error)
}

/// Identify ordinary or documentation comments in the grammar.
fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

/// Check Rust's documentation delimiters, preserving ordinary four-slash comments.
fn doc_comment(text: &str) -> bool {
    text.starts_with("//!")
        || (text.starts_with("///") && !text.starts_with("////"))
        || text.starts_with("/*!")
        || (text.starts_with("/**") && !text.starts_with("/***"))
}

/// Require whitespace on both sides of a comment's occupied source lines.
fn standalone_comment(source: &str, span: ByteSpan) -> bool {
    let start = source[..span.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let end = source[span.end..]
        .find('\n')
        .map_or(source.len(), |index| span.end + index);
    source[start..span.start].trim().is_empty() && source[span.end..end].trim().is_empty()
}

/// Find one direct named child of a given kind.
fn named_child<'tree>(node: Node<'tree>, kind: &str) -> Option<Node<'tree>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|child| child.kind() == kind)
}

/// Inspect actual modifier tokens so intervening comments do not change context.
fn has_child_token(node: Node<'_>, token: &str) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|child| child.kind() == token)
}

/// Retrieve the outer attributes preceding a node through adjacent comments.
fn attached_attributes(node: Node<'_>, source: &str) -> Vec<Attribute> {
    let mut attributes = Vec::new();
    let mut previous = node.prev_named_sibling();
    // Attributes are sibling decorations in the Rust grammar rather than node fields.
    while let Some(sibling) = previous {
        if sibling.kind() == "attribute_item" {
            attributes.push(Attribute {
                span: node_span(sibling),
                text: source[sibling.start_byte()..sibling.end_byte()].to_owned(),
            });
        } else if !is_comment(sibling) {
            break;
        }
        previous = sibling.prev_named_sibling();
    }
    attributes.reverse();
    attributes
}

/// Identify a block's structural owner without inventing bodies for expressions.
fn classify_body(block: Node<'_>) -> (BodyKind, Node<'_>) {
    let Some(parent) = block.parent() else {
        return (BodyKind::Scope, block);
    };
    let kind = match parent.kind() {
        "function_item" => BodyKind::Function,
        "if_expression" if parent.child_by_field_name("consequence") == Some(block) => {
            BodyKind::Branch
        }
        "else_clause" => BodyKind::Branch,
        "for_expression" | "while_expression" | "loop_expression"
            if parent.child_by_field_name("body") == Some(block) =>
        {
            BodyKind::Loop
        }
        "let_declaration" if parent.child_by_field_name("alternative") == Some(block) => {
            BodyKind::LetElse
        }
        "match_arm" if parent.child_by_field_name("value") == Some(block) => BodyKind::MatchArm,
        "closure_expression" if parent.child_by_field_name("body") == Some(block) => {
            BodyKind::Closure
        }
        "async_block" => BodyKind::Async,
        "unsafe_block" => BodyKind::Unsafe,
        "const_block" => BodyKind::Const,
        _ => return (BodyKind::Scope, block),
    };
    (kind, parent)
}

/// Gather equivalent leading positions up to the nearest containing body/item.
fn body_anchors(owner: Node<'_>) -> Vec<usize> {
    let mut anchors = vec![owner.start_byte()];
    let mut current = owner;
    while let Some(parent) = current.parent() {
        // A containing statement can annotate its first outermost executable body.
        if matches!(
            parent.kind(),
            "block"
                | "source_file"
                | "declaration_list"
                | "match_block"
                | "function_item"
                | "macro_definition"
                | "token_tree"
        ) {
            break;
        }
        anchors.push(parent.start_byte());
        current = parent;
    }
    anchors.sort_unstable();
    anchors.dedup();
    anchors
}

/// Index direct executable children and their attached syntax facts.
fn direct_statements(block: Node<'_>, source: &str) -> Vec<Statement> {
    let mut statements = Vec::new();
    let mut cursor = block.walk();
    for child in block.named_children(&mut cursor) {
        // Local item definitions, attributes, comments, labels, and empty semicolons do no work.
        if !executable_child(child) {
            continue;
        }
        let expression = if child.kind() == "expression_statement" {
            let mut expression_cursor = child.walk();
            child
                .named_children(&mut expression_cursor)
                .find(|node| !is_comment(*node) && node.kind() != "attribute_item")
                .unwrap_or(child)
        } else {
            child
        };
        let is_tail = child.kind() != "expression_statement"
            && child.kind() != "let_declaration"
            && has_only_trivia_after(child, block);
        statements.push(Statement {
            span: node_span(child),
            kind: child.kind().to_owned(),
            expression_kind: expression.kind().to_owned(),
            expression: node_span(expression),
            is_tail,
            attributes: attached_attributes(child, source),
            macro_path: macro_path(expression, source),
        });
    }
    statements
}

/// Determine whether a direct expression is the final syntax before the closing brace.
fn has_only_trivia_after(node: Node<'_>, block: Node<'_>) -> bool {
    let mut sibling = node.next_sibling();
    while let Some(next) = sibling {
        if next.kind() != "}" && !is_comment(next) {
            return false;
        }
        sibling = next.next_sibling();
    }
    node.parent() == Some(block)
}

/// Distinguish executable direct children from local declarations and trivia.
fn executable_child(node: Node<'_>) -> bool {
    !is_comment(node)
        && !matches!(
            node.kind(),
            "attribute_item"
                | "inner_attribute_item"
                | "empty_statement"
                | "label"
                | "function_item"
                | "function_signature_item"
                | "const_item"
                | "static_item"
                | "struct_item"
                | "enum_item"
                | "union_item"
                | "type_item"
                | "associated_type"
                | "mod_item"
                | "foreign_mod_item"
                | "impl_item"
                | "trait_item"
                | "use_declaration"
                | "extern_crate_declaration"
                | "macro_definition"
        )
}

/// Recognize values allowed on the right-hand side of a plain field copy.
fn plain_copy_value(node: Node<'_>) -> bool {
    match node.kind() {
        "identifier" | "self" | "integer_literal" | "float_literal" | "boolean_literal"
        | "char_literal" | "string_literal" | "raw_string_literal" => true,
        "field_expression" => node
            .child_by_field_name("value")
            .is_some_and(plain_copy_value),
        "reference_expression" => node
            .child_by_field_name("value")
            .is_some_and(plain_copy_value),
        _ => false,
    }
}

/// Identify whole subtrees that exist only for constant evaluation.
fn const_context(node: Node<'_>) -> bool {
    matches!(node.kind(), "const_item" | "static_item" | "const_block")
}

/// Identify const-only fields without incorrectly excluding runtime array elements.
fn field_is_const(parent: Node<'_>, child: Node<'_>) -> bool {
    match parent.kind() {
        "array_type" | "array_expression" => parent.child_by_field_name("length") == Some(child),
        "enum_variant" => parent.child_by_field_name("value") == Some(child),
        "const_parameter" => parent.child_by_field_name("value") == Some(child),
        "type_arguments" => matches!(child.kind(), "block" | "const_block"),
        _ => false,
    }
}

/// Detect actual brace-delimited groups while ignoring braces inside literals.
fn macro_has_braces(node: Node<'_>) -> bool {
    if node.kind() == "token_tree" && node.child(0).is_some_and(|child| child.kind() == "{") {
        return true;
    }
    if node.kind().ends_with("literal") {
        return false;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor).any(macro_has_braces)
}

/// Three-valued cfg evaluation under `test = false`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Truth {
    /// A definitely enabled predicate.
    True,
    /// A definitely disabled predicate.
    False,
    /// A predicate requiring target/build information.
    Unknown,
}

/// A small conservative parser for cfg predicate syntax, never cfg expansion.
struct CfgParser<'source> {
    /// Predicate source remaining to be read.
    source: &'source str,
    /// Current UTF-8 byte position.
    position: usize,
}

impl CfgParser<'_> {
    /// Skip Rust whitespace between cfg tokens.
    fn whitespace(&mut self) {
        loop {
            // Comments are trivia within cfg token groups just as they are in Rust syntax.
            while self
                .source
                .as_bytes()
                .get(self.position)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.position += 1;
            }
            let remaining = &self.source.as_bytes()[self.position..];
            if remaining.starts_with(b"//") {
                self.position += remaining
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .unwrap_or(remaining.len());
            } else if remaining.starts_with(b"/*") {
                // Consume the complete nested comment before looking for another cfg token.
                self.position += 2;
                let mut depth = 1;
                // Rust block comments nest, including comments in cfg token trees.
                while self.position < self.source.len() && depth > 0 {
                    let rest = &self.source.as_bytes()[self.position..];
                    if rest.starts_with(b"/*") {
                        depth += 1;
                        self.position += 2;
                    } else if rest.starts_with(b"*/") {
                        depth -= 1;
                        self.position += 2;
                    } else {
                        self.position += 1;
                    }
                }
            } else {
                break;
            }
        }
    }

    /// Consume one expected punctuation character after whitespace.
    fn consume(&mut self, expected: u8) -> bool {
        self.whitespace();
        if self.source.as_bytes().get(self.position) == Some(&expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    /// Read an identifier used as a cfg operator or predicate name.
    fn identifier(&mut self) -> &str {
        self.whitespace();
        let start = self.position;
        while let Some(character) = self.source[self.position..].chars().next() {
            if !character.is_alphanumeric() && character != '_' {
                break;
            }
            self.position += character.len_utf8();
        }
        &self.source[start..self.position]
    }

    /// Read one predicate, keeping unknown names and values conservative.
    fn predicate(&mut self) -> Option<Truth> {
        let name = self.identifier().to_owned();
        if name.is_empty() {
            return None;
        }
        if self.consume(b'=') {
            // Name-value predicates are unknown; quoted values can contain delimiters.
            self.whitespace();
            if self.raw_string_value() {
                return Some(Truth::Unknown);
            }
            if self.consume(b'"') {
                // A string value is one unknown predicate even when it contains punctuation.
                let mut escaped = false;
                while let Some(byte) = self.source.as_bytes().get(self.position).copied() {
                    // Stop only at an unescaped quote so commas cannot terminate the value.
                    self.position += 1;
                    if byte == b'"' && !escaped {
                        return Some(Truth::Unknown);
                    }
                    escaped = byte == b'\\' && !escaped;
                }
                return None;
            }
            let _ = self.identifier();
            return Some(Truth::Unknown);
        }
        if !self.consume(b'(') {
            return Some(if name == "test" {
                Truth::False
            } else {
                Truth::Unknown
            });
        }
        let mut values = Vec::new();
        while !self.consume(b')') {
            // Nested all/any/not predicates use the same three-valued evaluation.
            values.push(self.predicate()?);
            if self.consume(b')') {
                break;
            }
            if !self.consume(b',') {
                return None;
            }
        }
        Some(match name.as_str() {
            "all" if values.contains(&Truth::False) => Truth::False,
            "all" if values.iter().all(|value| *value == Truth::True) => Truth::True,
            "any" if values.contains(&Truth::True) => Truth::True,
            "any" if values.iter().all(|value| *value == Truth::False) => Truth::False,
            "not" if values.len() == 1 => match values[0] {
                Truth::True => Truth::False,
                Truth::False => Truth::True,
                Truth::Unknown => Truth::Unknown,
            },
            _ => Truth::Unknown,
        })
    }

    /// Consume a raw string cfg value, including any delimiter hashes.
    fn raw_string_value(&mut self) -> bool {
        let start = self.position;
        if !self.consume(b'r') {
            return false;
        }
        let mut hashes = 0;
        while self.source.as_bytes().get(self.position) == Some(&b'#') {
            hashes += 1;
            self.position += 1;
        }
        // A plain `r` identifier is not a raw string value.
        if self.source.as_bytes().get(self.position) != Some(&b'"') {
            self.position = start;
            return false;
        }
        self.position += 1;
        let terminator = format!("\"{}", "#".repeat(hashes));
        if let Some(end) = self.source[self.position..].find(&terminator) {
            self.position += end + terminator.len();
        } else {
            self.position = self.source.len();
        }
        true
    }
}

/// Recognize explicit test attributes and definitely disabled cfg attributes.
fn test_attribute(raw: &str) -> bool {
    let inner = raw
        .trim()
        .strip_prefix("#[")
        .or_else(|| raw.trim().strip_prefix("#!["))
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or("")
        .trim();
    let mut parser = CfgParser {
        source: inner,
        position: 0,
    };
    if parser.consume(b':') && !parser.consume(b':') {
        return false;
    }
    let mut path = parser.identifier().to_owned();
    while parser.consume(b':') {
        // Attribute paths use Rust tokens; comments may separate any path component.
        if !parser.consume(b':') {
            return false;
        }
        path.push_str("::");
        path.push_str(parser.identifier());
    }
    if matches!(path.as_str(), "test" | "tokio::test" | "async_std::test") {
        return parser.position == inner.len()
            || inner.as_bytes().get(parser.position) == Some(&b'(');
    }
    if path != "cfg" || !parser.consume(b'(') {
        return false;
    }
    parser.predicate() == Some(Truth::False)
}

#[cfg(test)]
mod tests {
    //! Direct structural-fact tests independent of FC001's decisions.

    use super::*;

    /// Parse a source fixture with normal production filtering.
    fn parse(source: &str) -> SourceContext {
        SourceContext::parse("src/example.rs", source.as_bytes(), false)
            .expect("fixture must parse")
    }

    /// Leading comments/attributes are retained but cannot become executable statements.
    #[test]
    fn statement_facts_preserve_decorations_and_tail() {
        let context = parse(
            "fn f() { if ready {\n// Keep all evidence.\n#[allow(unused)]\ntracing::info!(\"start\"); let x = compute(); x } }",
        );
        let body = &context.bodies[1];
        assert_eq!(body.parent, Some(0));
        assert_eq!(body.kind, BodyKind::Branch);
        assert_eq!(body.owner_kind, "if_expression");
        assert_eq!(body.statements.len(), 3);
        let first = body.first_statement().expect("body has a first statement");
        assert_eq!(first.macro_path.as_deref(), Some("tracing::info"));
        assert_eq!(first.attributes.len(), 1);
        assert!(!first.is_tail);
        assert!(body.statements[2].is_tail);
        assert_eq!(context.slice(body.statements[2].span), "x");
        assert_eq!(body.annotations.len(), 1);
        assert_eq!(context.comments.len(), 1);
        assert_eq!(context.attributes.len(), 1);
    }

    /// Brace-delimited macro statements are not marked as tails when more syntax follows.
    #[test]
    fn macro_statement_tail_flags_are_precise() {
        let context = parse("fn f() { custom!{} custom!{}; value } ");
        let body = &context.bodies[0];
        assert_eq!(body.statements.len(), 3);
        assert!(!body.statements[0].is_tail);
        assert!(!body.statements[1].is_tail);
        assert!(body.statements[2].is_tail);
    }

    /// Syntax nodes and context remain discoverable even when FC001 omits them.
    #[test]
    fn context_retains_test_const_async_and_unsafe() {
        let context = parse(
            "#[cfg(test)] mod tests { unsafe fn f() { async { a(); } } } const fn c() { { a(); } } fn p() {} ",
        );
        assert_eq!(context.bodies.len(), 5);
        assert!(context.bodies[0].context.is_test);
        assert!(context.bodies[1].context.is_async);
        assert!(context.bodies[1].context.is_unsafe);
        assert!(!context.bodies[1].in_scope);
        assert!(context.bodies[2].context.is_const);
        assert!(context.bodies[3].context.is_const);
        assert!(context.bodies[4].in_scope);
        assert!(context.bodies[4].context.function.is_some());
        let commented = parse("const /* qualifier trivia */ fn f() { if x { a(); b(); c(); } }");
        assert!(commented.bodies[1].context.is_const);
    }

    /// Test inclusion changes policy rather than hiding syntax from the parsed tree.
    #[test]
    fn include_tests_preserves_same_body_facts() {
        let source = "#[test] fn f() { if x { a(); b(); c(); } }";
        let production = parse(source);
        let tests =
            SourceContext::parse("tests.rs", source.as_bytes(), true).expect("fixture must parse");
        assert_eq!(production.bodies.len(), tests.bodies.len());
        assert!(!production.bodies[1].in_scope);
        assert!(tests.bodies[1].in_scope);
        assert!(tests.bodies[1].context.is_test);
    }

    /// Decorations before an excluded function are inactive together with that target.
    #[test]
    fn leading_directives_follow_excluded_target_scope() {
        let context = parse(
            "// evenflow: allow(FC001): This function is test-only.\n#[test]\nfn f() { if x { a(); b(); c(); } }",
        );
        assert!(!context.comments[0].in_scope);
        let context = parse(
            "fn f() {\n// evenflow: allow(FC001): This cfg branch is test-only.\n#[cfg(test)]\nif x { a(); b(); c(); } }",
        );
        assert!(!context.comments[0].in_scope);
    }

    /// Inner cfg attributes and formatted attribute paths keep definite test scopes excluded.
    #[test]
    fn inner_cfg_and_spaced_test_paths() {
        let context = parse("#![cfg(test)] fn f() { if x { a(); b(); c(); } }");
        assert!(context.bodies.iter().all(|body| !body.in_scope));
        let context =
            parse("mod m { #![cfg(test)] fn f() { if x { a(); b(); c(); } } } fn p() {} ");
        assert!(!context.bodies[0].in_scope);
        assert!(context.bodies[2].in_scope);
        let context = parse("#[tokio :: test]\nasync fn f() { if x { a(); b(); c(); } }");
        assert!(!context.bodies[0].in_scope);
    }

    /// Comment trivia around recognized test paths cannot change test exclusion.
    #[test]
    fn test_attribute_paths_ignore_comment_trivia() {
        for attribute in [
            "#[/* explanation */test]",
            "#[test/* explanation */]",
            "#[/* before */ test /* after */]",
            "#[tokio/* qualifier */::/* name */test]",
            "#[/* before */tokio::test/* options */(flavor = \"current_thread\")]",
            "#[async_std/* qualifier */::test/* after */]",
            "#[/* outer /* nested */ comment */async_std::/* name */test]",
        ] {
            let source = format!("{attribute} fn f() {{ if x {{ a(); b(); c(); }} }}");
            let context = parse(&source);
            assert!(
                !context.bodies[1].in_scope,
                "expected test exclusion: {attribute}"
            );
            let tests = SourceContext::parse("src/example.rs", source.as_bytes(), true)
                .expect("fixture must parse with tests enabled");
            assert!(
                tests.bodies[1].in_scope,
                "expected test inclusion: {attribute}"
            );
        }
        for attribute in [
            "#[other::/* unrelated */test]",
            "#[cfg_attr(test, /* do not expand */ test)]",
            "#[cfg(feature = \"/* test */\")]",
        ] {
            assert!(
                !test_attribute(attribute),
                "unexpected test exclusion: {attribute}"
            );
        }
        assert!(test_attribute(
            "#[/* cfg path */cfg/* predicate */(all(test, feature = \"/* literal */\"))]"
        ));
    }

    /// Const-generic arguments and defaults remain excluded while runtime arrays stay visible.
    #[test]
    fn const_generic_and_array_expression_contexts() {
        let context = parse(
            "struct S<const N: usize = { a(); b(); 3 }>; fn f() { let _x: S<{ a(); b(); 3 }>; let _a = [{ a(); b(); c() }]; }",
        );
        assert!(context.bodies[0].context.is_const);
        assert!(context.bodies[2].context.is_const);
        assert!(!context.bodies[3].context.is_const);
        assert!(context.bodies[3].in_scope);
    }

    /// Macro coverage inspects real token-tree groups and emits one gap per invocation.
    #[test]
    fn opaque_macros_report_only_unrecognized_brace_groups() {
        let context = parse(
            "fn f() { custom! { a(); { b(); } } custom!({ c(); }); custom!(\"{ string }\"); custom!(r#\"{ raw }\"#); tracing::info!({ a(); }); json!({\"a\": 1}); matches!(x, Some({ a(); })); tokio::select! { a = future => { a(); } } select! { a = future => { a(); } } }",
        );
        assert_eq!(context.coverage_gaps().len(), 4);
        assert_eq!(context.bodies.len(), 1);
        assert!(
            context
                .coverage_gaps()
                .iter()
                .all(|span| context.slice(*span).contains('!'))
        );
    }

    /// Disabled test scopes and macro definitions contribute no spurious coverage notes.
    #[test]
    fn opaque_test_and_const_scopes_are_not_covered() {
        let context = parse(
            "macro_rules! m { () => { work! { a(); } }; } #[test] fn t() { custom!{ work(); } } const X: () = custom! { work(); }; fn p() { custom! { work(); } }",
        );
        assert_eq!(context.coverage_gaps().len(), 1);
    }

    /// The documented logging paths are exact and aliases remain unknown.
    #[test]
    fn logging_paths_are_exact() {
        for path in [
            "trace",
            "debug",
            "info",
            "warn",
            "error",
            "crit",
            "::log::info",
            "tracing::error",
            "slog::crit",
            "slog::trace",
        ] {
            assert!(is_logging_macro(path), "expected logging path: {path}");
        }
        for path in [
            "log::crit",
            "tracing::crit",
            "alias::info",
            "my::log::info",
            "println",
            "critical",
        ] {
            assert!(!is_logging_macro(path), "unexpected logging path: {path}");
        }
        let context = parse("fn f() { ::tracing :: /* preserve path trivia */ info!(\"start\"); }");
        assert_eq!(
            context.bodies[0].statements[0].macro_path.as_deref(),
            Some("tracing::info")
        );
    }

    /// Columns count Unicode code points and CRLF lines are counted once.
    #[test]
    fn unicode_crlf_locations_and_original_slices() {
        let source = "fn café() {\r\n    let é = 1;\r\n}\r\n";
        let context = parse(source);
        let position = source.find("é =").expect("fixture contains identifier");
        assert_eq!(context.location(position), (2, 9));
        assert_eq!(
            context.location(source.find("() {").expect("signature exists")),
            (1, 8)
        );
        assert_eq!(
            context.slice(context.bodies[0].statements[0].span),
            "let é = 1;"
        );
    }

    /// Parser recovery and missing tokens are errors before scope filtering.
    #[test]
    fn parsing_and_encoding_failures_are_visible() {
        assert!(SourceContext::parse("bad.rs", &[0xff], false).is_err());
        assert!(
            SourceContext::parse("bad.rs", b"#[cfg(test)] fn bad() { let x = ; }", false).is_err()
        );
        assert!(SourceContext::parse("bad.rs", b"fn bad() {", false).is_err());
        assert!(SourceContext::parse("bad.rs", b"const X: () = { let x = ; };", false).is_err());
    }

    /// A declaration comment is consumed by its first outermost block-bearing closure.
    #[test]
    fn comments_and_directives_have_exact_body_ownership() {
        let context = parse(
            "fn f() {\n// evenflow: allow(FC001): Explain the first closure only.\nlet pair = (|x| x, || { a(); b(); c(); }, || { a(); b(); c(); }); }",
        );
        assert_eq!(context.bodies.len(), 3);
        assert_eq!(context.bodies[1].annotations.len(), 1);
        assert!(context.bodies[2].annotations.is_empty());
        let context = parse(
            "fn f() { if x {\n// evenflow: allow(FC001): Explain only this parent.\nif y { a(); b(); c(); } d(); e(); } }",
        );
        assert_eq!(context.bodies[1].annotations.len(), 1);
        assert!(context.bodies[2].annotations.is_empty());
    }

    /// Tri-state cfg filtering includes uncertain production alternatives.
    #[test]
    fn cfg_truth_tables() {
        for attribute in [
            "#[cfg(test)]",
            "#[cfg(all(test, feature = \"x\"))]",
            "#[cfg(not(not(test)))]",
            "#[cfg(any())]",
            "#[tokio::test(flavor = \"current_thread\")]",
            "#[cfg(all(/* explanation */ test, feature = \"x\"))]",
            "#[cfg(all(test, feature = r#\"quoted, value\"#))]",
            "#[cfg(all(test, π))]",
        ] {
            assert!(test_attribute(attribute), "expected exclusion: {attribute}");
        }
        for attribute in [
            "#[cfg(any(test, feature = \"x\"))]",
            "#[cfg(not(test))]",
            "#[cfg(all())]",
            "#[cfg(feature = \"test\")]",
            "#[cfg_attr(test, ignore)]",
            "#[cfg(any(test, not(feature = \"x\")))]",
        ] {
            assert!(
                !test_attribute(attribute),
                "unexpected exclusion: {attribute}"
            );
        }
    }
}
