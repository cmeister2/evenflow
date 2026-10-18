//! Conservative source-level resolution for direct entry macro and function calls.

use tree_sitter::Node;

use crate::source::SourceContext;

/// The Rust namespace occupied by a supported entry invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EntryKind {
    /// A direct macro invocation.
    Macro,
    /// A direct function call, without a receiver or dynamic callee.
    Function,
}

/// An invocation kind and its complete normalized source-resolved path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedCall {
    /// Whether the entry invokes a macro or a function.
    pub(crate) kind: EntryKind,
    /// Qualified path after explicit imports and local module aliases are followed.
    pub(crate) path: String,
}

/// A Rust path retaining whether it explicitly starts in the extern prelude.
#[derive(Clone, Debug)]
struct RustPath {
    /// Normalized identifier components without raw-identifier prefixes.
    parts: Vec<String>,
    /// A leading `::` bypasses lexical module and import bindings.
    absolute: bool,
}

/// A source import, including its alias and unexpanded target path.
struct Import {
    /// Name made visible by this explicit import.
    name: String,
    /// Target as written relative to the importing scope.
    target: RustPath,
    /// Grouped self imports contribute only a type-namespace binding.
    type_only: bool,
}

/// A lexical binding that can establish or obstruct an invocation's identity.
enum Binding<'tree> {
    /// A named import requiring resolution in its declaration's scope.
    Import(Node<'tree>, RustPath),
    /// An inline or externally defined module.
    Module(Node<'tree>),
    /// An explicit extern-crate binding and its original crate name.
    External(String),
    /// A known shadow or a declaration requiring unavailable semantic analysis.
    Unresolved(&'static str),
}

/// Candidates and uncertainty collected for one lexical scope.
struct Lookup<'tree> {
    /// Explicit declarations matching the requested namespace and name.
    bindings: Vec<Binding<'tree>>,
    /// A glob could provide a name that otherwise looks like an extern crate.
    glob: bool,
}

/// Resolve a direct call using explicit imports and source-visible module scopes.
///
/// # Errors
/// Returns an explanation for unsupported expressions, unknown bare names,
/// ambiguous imports, conditional declarations, shadows, or import cycles.
pub(crate) fn resolve_call(
    context: &SourceContext,
    mut node: Node<'_>,
) -> Result<ResolvedCall, String> {
    while matches!(
        node.kind(),
        "expression_statement" | "parenthesized_expression" | "try_expression" | "await_expression"
    ) {
        // Transparent wrappers retain the identity of the direct entry call.
        let mut cursor = node.walk();
        node = node
            .named_children(&mut cursor)
            .find(|child| !is_comment(*child))
            .ok_or("the entry expression has no call")?;
    }
    let (kind, target) = match node.kind() {
        "macro_invocation" => (
            EntryKind::Macro,
            node.child_by_field_name("macro")
                .ok_or("the macro path is missing")?,
        ),
        "call_expression" => (
            EntryKind::Function,
            node.child_by_field_name("function")
                .ok_or("the function path is missing")?,
        ),
        _ => return Err("the first item is not a direct macro or function call".into()),
    };
    let target = if target.kind() == "generic_function" {
        target
            .child_by_field_name("function")
            .ok_or("the generic function path is missing")?
    } else {
        target
    };
    let path = rust_path(context, target)?;
    let path = resolve_path(
        context,
        node,
        &path,
        kind,
        node.start_byte(),
        &mut Vec::new(),
        None,
    )?;
    Ok(ResolvedCall {
        kind,
        path: path.join("::"),
    })
}

/// Normalize only path syntax, never a field access or dynamically computed callee.
fn rust_path(context: &SourceContext, node: Node<'_>) -> Result<RustPath, String> {
    let mut parts = Vec::new();
    path_parts(context, node, &mut parts)?;
    if parts.is_empty() {
        return Err("the call path is empty".into());
    }
    Ok(RustPath {
        parts,
        absolute: context.text[node.byte_range()].starts_with("::"),
    })
}

/// Read the path grammar while omitting comments and raw-identifier prefixes.
fn path_parts(
    context: &SourceContext,
    node: Node<'_>,
    parts: &mut Vec<String>,
) -> Result<(), String> {
    match node.kind() {
        "identifier" | "type_identifier" | "self" | "super" | "crate" => {
            parts.push(identifier(context, node));
        }
        "scoped_identifier" | "scoped_type_identifier" => {
            // Path fields preserve qualification without accepting unrelated named children.
            if let Some(parent) = node.child_by_field_name("path") {
                path_parts(context, parent, parts)?;
            }
            let name = node
                .child_by_field_name("name")
                .ok_or("the path name is missing")?;
            path_parts(context, name, parts)?;
        }
        _ => {
            return Err(
                "method calls, dynamic callees, and type-dependent paths need compiler resolution"
                    .into(),
            );
        }
    }
    Ok(())
}

/// Borrow an identifier spelling and normalize Rust's raw-identifier escape.
fn identifier(context: &SourceContext, node: Node<'_>) -> String {
    let spelling = &context.text[node.byte_range()];
    spelling.strip_prefix("r#").unwrap_or(spelling).to_owned()
}

/// Resolve qualification and lexical aliases without consulting compiler state.
fn resolve_path<'tree>(
    context: &SourceContext,
    origin: Node<'tree>,
    path: &RustPath,
    kind: EntryKind,
    position: usize,
    imports: &mut Vec<usize>,
    excluded: Option<usize>,
) -> Result<Vec<String>, String> {
    let head = path.parts.first().ok_or("the target path is empty")?;
    if path.absolute {
        // An absolute extern path is unaffected by local variables or modules.
        if path.parts.len() < 2 || matches!(head.as_str(), "crate" | "self" | "super" | "Self") {
            return Err("an absolute entry needs an extern-crate-qualified path".into());
        }
        return absolute_path(context, path);
    }
    if head == "Self" {
        return Err("Self paths need compiler type resolution".into());
    }
    if head == "crate" {
        return if path.parts.len() > 1 {
            Ok(path.parts.clone())
        } else {
            Err("a crate is not an entry call".into())
        };
    }
    if matches!(head.as_str(), "self" | "super") {
        return Err(
            "self and super need a module graph to establish a crate-qualified identity".into(),
        );
    }
    let qualified = path.parts.len() > 1;
    if lexical_shadow(context, origin, head, kind, qualified, position) {
        return Err(format!(
            "{head} is shadowed by a local binding or lexical macro"
        ));
    }
    let mut current = Some(origin);
    while let Some(node) = current {
        // Imports and local items stop inheriting at the nearest module boundary.
        if is_scope(node) {
            // Explicit bindings take precedence over any unresolved glob contribution.
            let lookup = lookup_scope(context, node, head, kind, qualified, position, excluded)?;
            if !lookup.bindings.is_empty() {
                return resolve_binding(
                    context,
                    node,
                    lookup.bindings,
                    &path.parts[1..],
                    kind,
                    position,
                    imports,
                );
            }
            if lookup.glob {
                return Err(format!(
                    "a glob import may supply {head}; its identity is unresolved"
                ));
            }
        }
        if is_module_scope(node) {
            break;
        }
        current = node.parent();
    }
    if qualified {
        Ok(path.parts.clone())
    } else {
        Err(format!(
            "bare name {head} has no explicit source-visible import"
        ))
    }
}

/// Follow explicit reexports inside a source-visible module without guessing its crate path.
fn resolve_member<'tree>(
    context: &SourceContext,
    module: Node<'tree>,
    parts: &[String],
    kind: EntryKind,
    position: usize,
    imports: &mut Vec<usize>,
    excluded: Option<usize>,
) -> Result<Vec<String>, String> {
    let head = parts.first().ok_or("a module is not an entry call")?;
    let lookup = lookup_scope(
        context,
        module,
        head,
        kind,
        parts.len() > 1,
        position,
        excluded,
    )?;
    if !lookup.bindings.is_empty() {
        return resolve_binding(
            context,
            module,
            lookup.bindings,
            &parts[1..],
            kind,
            position,
            imports,
        );
    }
    if lookup.glob {
        return Err(format!("a glob reexport prevents resolving {head}"));
    }
    Err(format!(
        "the crate-qualified identity of local module member {head} is unavailable"
    ))
}

/// Follow one unambiguous lexical declaration, preserving invocation namespaces.
fn resolve_binding<'tree>(
    context: &SourceContext,
    _scope: Node<'tree>,
    mut bindings: Vec<Binding<'tree>>,
    suffix: &[String],
    kind: EntryKind,
    position: usize,
    imports: &mut Vec<usize>,
) -> Result<Vec<String>, String> {
    if bindings.len() != 1 {
        return Err("multiple source declarations make the entry path ambiguous".into());
    }
    match bindings.pop().ok_or("the binding is unavailable")? {
        Binding::Import(declaration, mut target) => {
            // Cycle detection also protects mutually recursive reexports.
            if imports.len() >= 64 || imports.contains(&declaration.id()) {
                return Err("explicit imports form a resolution cycle".into());
            }
            imports.push(declaration.id());
            target.parts.extend_from_slice(suffix);
            let result = resolve_path(
                context,
                declaration,
                &target,
                kind,
                position,
                imports,
                Some(declaration.id()),
            );
            imports.pop();
            result
        }
        Binding::Module(declaration) => {
            // Inline module reexports may identify a target without knowing the module's crate base.
            if suffix.is_empty() {
                return Err("a module is not an entry call".into());
            }
            if let Some(body) = declaration.child_by_field_name("body") {
                resolve_member(context, body, suffix, kind, position, imports, None)
            } else {
                Err("an external module needs an explicit crate-qualified entry path".into())
            }
        }
        Binding::External(name) => {
            // A crate alias is useful only as a namespace, never as a direct callee.
            if suffix.is_empty() {
                return Err("an extern crate is not an entry call".into());
            }
            let mut path = vec![name];
            path.extend_from_slice(suffix);
            Ok(path)
        }
        Binding::Unresolved(reason) => Err(reason.into()),
    }
}

/// Collect declarations in one scope without traversing sibling bodies.
fn lookup_scope<'tree>(
    context: &SourceContext,
    scope: Node<'tree>,
    name: &str,
    kind: EntryKind,
    qualified: bool,
    _position: usize,
    excluded: Option<usize>,
) -> Result<Lookup<'tree>, String> {
    let mut lookup = Lookup {
        bindings: Vec::new(),
        glob: false,
    };
    let mut cursor = scope.walk();
    for child in scope.named_children(&mut cursor) {
        // Imports are visible throughout their containing scope, including before their text.
        if child.kind() == "use_declaration" && Some(child.id()) != excluded {
            // Flatten one declaration and retain only aliases relevant to this lookup.
            let argument = child
                .child_by_field_name("argument")
                .ok_or("the import target is missing")?;
            let mut entries = Vec::new();
            collect_imports(context, argument, None, &mut entries, &mut lookup.glob)?;
            for import in entries {
                if import.name == name && (qualified || !import.type_only) {
                    lookup.bindings.push(if uncertain_attributes(context, child) {
                        Binding::Unresolved("conditional or attributed imports cannot establish an entry identity")
                    } else {
                        Binding::Import(child, import.target)
                    });
                }
            }
            continue;
        }
        let Some(declared) = child.child_by_field_name("name") else {
            continue;
        };
        let visible = if child.kind() == "extern_crate_declaration" {
            child.child_by_field_name("alias").unwrap_or(declared)
        } else {
            declared
        };
        if identifier(context, visible) != name {
            continue;
        }
        let candidate = match child.kind() {
            "mod_item" if qualified || kind == EntryKind::Function => Some(Binding::Module(child)),
            "function_item" if !qualified && kind == EntryKind::Function => {
                Some(Binding::Unresolved(
                    "a local function needs an explicit crate-qualified path or import",
                ))
            }
            "extern_crate_declaration" if qualified => {
                let alias = child.child_by_field_name("alias").unwrap_or(declared);
                (identifier(context, alias) == name)
                    .then(|| Binding::External(identifier(context, declared)))
            }
            "struct_item" | "enum_item" | "type_item" | "trait_item" | "union_item"
                if qualified || kind == EntryKind::Function =>
            {
                Some(Binding::Unresolved(
                    "type or constructor names require compiler resolution",
                ))
            }
            "const_item" | "static_item" if !qualified && kind == EntryKind::Function => Some(
                Binding::Unresolved("a constant or static may contain a dynamic function value"),
            ),
            _ => None,
        };
        if let Some(candidate) = candidate {
            lookup
                .bindings
                .push(if uncertain_attributes(context, child) {
                    Binding::Unresolved(
                        "a conditional or attributed declaration has uncertain identity",
                    )
                } else {
                    candidate
                });
        }
    }
    Ok(lookup)
}

/// Flatten explicit use groups, preserving module aliases and flagging glob uncertainty.
fn collect_imports(
    context: &SourceContext,
    node: Node<'_>,
    prefix: Option<&RustPath>,
    imports: &mut Vec<Import>,
    glob: &mut bool,
) -> Result<(), String> {
    match node.kind() {
        "use_list" => {
            // Each group entry inherits the path outside its braces.
            let mut cursor = node.walk();
            for child in node
                .named_children(&mut cursor)
                .filter(|child| !is_comment(*child))
            {
                collect_imports(context, child, prefix, imports, glob)?;
            }
        }
        "scoped_use_list" => {
            // Nested groups compose their prefix without resolving aliases prematurely.
            let path = node
                .child_by_field_name("path")
                .map(|path| rust_path(context, path))
                .transpose()?;
            let target = join_path(prefix, path.as_ref())?;
            let list = node
                .child_by_field_name("list")
                .ok_or("the import group is missing")?;
            collect_imports(context, list, Some(&target), imports, glob)?;
        }
        "use_wildcard" => *glob = true,
        "use_as_clause" => {
            // An aliased self entry names the group's prefix itself.
            let path = node
                .child_by_field_name("path")
                .ok_or("the aliased import path is missing")?;
            let path = rust_path(context, path)?;
            let type_only = path.parts.last().is_some_and(|part| part == "self");
            let target = import_target(prefix, path)?;
            let alias = node
                .child_by_field_name("alias")
                .ok_or("the import alias is missing")?;
            let name = identifier(context, alias);
            if name != "_" {
                imports.push(Import {
                    name,
                    target,
                    type_only,
                });
            }
        }
        _ => {
            // Ordinary imports bind their final component, including grouped self imports.
            let path = rust_path(context, node)?;
            let type_only = path.parts.last().is_some_and(|part| part == "self");
            let target = import_target(prefix, path)?;
            let name = target
                .parts
                .last()
                .ok_or("the import path is empty")?
                .clone();
            imports.push(Import {
                name,
                target,
                type_only,
            });
        }
    }
    Ok(())
}

/// Compose the prefix and child path from a use group.
fn join_path(prefix: Option<&RustPath>, child: Option<&RustPath>) -> Result<RustPath, String> {
    let Some(child) = child else {
        return prefix
            .cloned()
            .ok_or_else(|| "an absolute root-only use group is unsupported".into());
    };
    if child.absolute && prefix.is_some() {
        return Err("an absolute path inside a relative import group is unsupported".into());
    }
    let mut target = prefix.cloned().unwrap_or(RustPath {
        parts: Vec::new(),
        absolute: child.absolute,
    });
    target.parts.extend_from_slice(&child.parts);
    Ok(target)
}

/// Remove grouped self syntax while retaining its module target and alias.
fn import_target(prefix: Option<&RustPath>, mut child: RustPath) -> Result<RustPath, String> {
    if child.parts.last().is_some_and(|part| part == "self")
        && (prefix.is_some() || child.parts.len() > 1)
    {
        child.parts.pop();
    }
    join_path(prefix, Some(&child))
}

/// Recognize attributes that may change whether or what a declaration contributes.
fn uncertain_attributes(context: &SourceContext, node: Node<'_>) -> bool {
    let mut previous = node.prev_named_sibling();
    while let Some(sibling) = previous {
        // Documentation and lint attributes do not change name resolution.
        if sibling.kind() == "attribute_item" {
            let name = attribute_name(context, sibling);
            if !matches!(
                name.as_deref(),
                Some("allow" | "expect" | "warn" | "deny" | "forbid" | "doc")
            ) {
                return true;
            }
        } else if !is_comment(sibling) {
            break;
        }
        previous = sibling.prev_named_sibling();
    }
    false
}

/// Read an attribute's exact path without confusing a longer procedural name with `doc`.
fn attribute_name(context: &SourceContext, node: Node<'_>) -> Option<String> {
    let attribute = node.named_child(0)?;
    let path = attribute.named_child(0)?;
    rust_path(context, path)
        .ok()
        .map(|path| path.parts.join("::"))
}

/// Check the attributes attached to one declaration through comment trivia.
fn has_attribute(context: &SourceContext, node: Node<'_>, name: &str) -> bool {
    let mut previous = node.prev_named_sibling();
    while let Some(sibling) = previous {
        // Only contiguous attributes and comments decorate this declaration.
        if sibling.kind() == "attribute_item" {
            if attribute_name(context, sibling).as_deref() == Some(name) {
                return true;
            }
        } else if !is_comment(sibling) {
            break;
        }
        previous = sibling.prev_named_sibling();
    }
    false
}

/// Preserve absolute qualification without assuming this file establishes the extern prelude.
fn absolute_path(context: &SourceContext, source: &RustPath) -> Result<Vec<String>, String> {
    let root = context.tree.root_node();
    let mut cursor = root.walk();
    let mut seen = false;
    for declaration in root.named_children(&mut cursor) {
        // A renamed declaration might belong to a module file rather than the crate root.
        if declaration.kind() != "extern_crate_declaration" {
            continue;
        }
        let name = declaration
            .child_by_field_name("name")
            .ok_or("the extern crate name is missing")?;
        let alias = declaration.child_by_field_name("alias").unwrap_or(name);
        if identifier(context, alias) != source.parts[0] {
            continue;
        }
        if seen || uncertain_attributes(context, declaration) {
            return Err("an extern-prelude alias is conditional or ambiguous".into());
        }
        let original = identifier(context, name);
        if original != source.parts[0] {
            return Err("an absolute extern-crate alias needs a known crate root".into());
        }
        seen = true;
    }
    Ok(source.parts.clone())
}

/// Inspect parameter binding patterns without treating their type annotations as bindings.
fn parameter_binding(context: &SourceContext, parameters: Node<'_>, name: &str) -> bool {
    let mut cursor = parameters.walk();
    parameters.named_children(&mut cursor).any(|parameter| {
        if parameter.kind() == "parameter" {
            parameter
                .child_by_field_name("pattern")
                .is_some_and(|pattern| contains_identifier(context, pattern, name))
        } else {
            parameters.kind() == "closure_parameters"
                && !is_comment(parameter)
                && parameter.kind() != "attribute_item"
                && contains_identifier(context, parameter, name)
        }
    })
}

/// Match only declared generic names in the relevant type or value namespace.
fn generic_binding(
    context: &SourceContext,
    parameters: Node<'_>,
    name: &str,
    qualified: bool,
) -> bool {
    let expected = if qualified {
        "type_parameter"
    } else {
        "const_parameter"
    };
    let mut cursor = parameters.walk();
    parameters.named_children(&mut cursor).any(|parameter| {
        parameter.kind() == expected
            && parameter
                .child_by_field_name("name")
                .is_some_and(|binding| identifier(context, binding) == name)
    })
}

/// Detect value bindings, generic type parameters, and textually scoped macro shadows.
fn lexical_shadow(
    context: &SourceContext,
    origin: Node<'_>,
    name: &str,
    kind: EntryKind,
    qualified: bool,
    position: usize,
) -> bool {
    let mut current = Some(origin);
    while let Some(node) = current {
        // Ancestor patterns cover parameters, closure captures, arms, and for-loop bindings.
        if kind == EntryKind::Function && !qualified {
            if node
                .child_by_field_name("pattern")
                .is_some_and(|pattern| contains_identifier(context, pattern, name))
                || node
                    .child_by_field_name("parameters")
                    .is_some_and(|parameters| parameter_binding(context, parameters, name))
            {
                return true;
            }
            if matches!(node.kind(), "if_expression" | "while_expression")
                && node
                    .child_by_field_name("condition")
                    .is_some_and(|condition| condition_binding(context, condition, name))
            {
                return true;
            }
        }
        if (qualified || kind == EntryKind::Function)
            && node
                .child_by_field_name("type_parameters")
                .is_some_and(|parameters| generic_binding(context, parameters, name, qualified))
        {
            return true;
        }
        if is_scope(node) {
            // Only completed let bindings and preceding lexical macros can affect this position.
            let mut cursor = node.walk();
            for child in node.named_children(&mut cursor) {
                // Textually later declarations cannot shadow this earlier expression.
                if child.start_byte() >= position {
                    continue;
                }
                if kind == EntryKind::Function
                    && !qualified
                    && child.kind() == "let_declaration"
                    && child.end_byte() <= position
                    && child
                        .child_by_field_name("pattern")
                        .is_some_and(|pattern| contains_identifier(context, pattern, name))
                {
                    return true;
                }
                if kind == EntryKind::Macro
                    && !qualified
                    && child.kind() == "macro_definition"
                    && child
                        .child_by_field_name("name")
                        .is_some_and(|declared| identifier(context, declared) == name)
                {
                    return true;
                }
                if kind == EntryKind::Macro
                    && !qualified
                    && child.kind() == "mod_item"
                    && child.end_byte() <= position
                    && has_attribute(context, child, "macro_use")
                {
                    return true;
                }
            }
        }
        current = node.parent();
    }
    false
}

/// Find identifier-shaped bindings conservatively without inferring their types.
fn contains_identifier(context: &SourceContext, node: Node<'_>, name: &str) -> bool {
    if matches!(node.kind(), "identifier" | "type_identifier") && identifier(context, node) == name
    {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| contains_identifier(context, child, name))
}

/// Inspect only pattern-bearing condition nodes rather than names used as values.
fn condition_binding(context: &SourceContext, node: Node<'_>, name: &str) -> bool {
    if node
        .child_by_field_name("pattern")
        .is_some_and(|pattern| contains_identifier(context, pattern, name))
    {
        return true;
    }
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .any(|child| condition_binding(context, child, name))
}

/// Identify lexical declaration containers usable by source-only lookup.
fn is_scope(node: Node<'_>) -> bool {
    node.kind() == "block" || is_module_scope(node)
}

/// Distinguish module declaration lists from impl, trait, and extern declaration lists.
fn is_module_scope(node: Node<'_>) -> bool {
    node.kind() == "source_file"
        || (node.kind() == "declaration_list"
            && node
                .parent()
                .is_some_and(|parent| parent.kind() == "mod_item"))
}

/// Treat Rust comments as trivia when examining invocation wrappers or use groups.
fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

#[cfg(test)]
mod tests {
    //! Source-only resolution fixtures cover aliases, module scopes, and conservative failures.

    use super::*;

    /// Resolve the final direct executable statement in a fixture's deepest body.
    fn resolve(source: &str) -> Result<ResolvedCall, String> {
        let context = SourceContext::parse("src/lib.rs", source.as_bytes(), false)
            .expect("resolver fixture must parse");
        let statement = context
            .bodies
            .iter()
            .rev()
            .find_map(|body| body.statements.last())
            .expect("resolver fixture has a final call");
        let node = context.node(statement.span).expect("statement node exists");
        resolve_call(&context, node)
    }

    /// Assert the complete resolved path and its invocation namespace.
    fn resolves(source: &str, kind: EntryKind, path: &str) {
        assert_eq!(
            resolve(source),
            Ok(ResolvedCall {
                kind,
                path: path.into()
            })
        );
    }

    /// Extern qualification normalizes leading separators, trivia, and raw identifiers.
    #[test]
    fn qualified_paths() {
        resolves(
            "fn f() { tracing::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "fn f() { ::tracing::r#info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "fn f() { telemetry /* prefix */ :: r#enter(); }",
            EntryKind::Function,
            "telemetry::enter",
        );
        assert!(resolve("fn f() { info!(\"entry\"); }").is_err());
        assert!(resolve("fn f() { enter(); }").is_err());
    }

    /// Explicit named imports retain complete target identities through aliases and groups.
    #[test]
    fn explicit_imports_and_groups() {
        resolves(
            "use tracing::info; fn f() { info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use tracing::{self as logs, debug as detail, info}; fn f() { logs::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use tracing::{instrumentation::{enter as begin}, info}; fn f() { begin(); }",
            EntryKind::Function,
            "tracing::instrumentation::enter",
        );
        resolves(
            "use tracing as logs; fn f() { logs::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "extern crate tracing as logs; fn f() { logs::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        assert!(
            resolve("extern crate tracing as logs; fn f() { ::logs::info!(\"entry\"); }").is_err()
        );
        resolves(
            "fn f() { use tracing::info as entry; entry!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use tracing::info as r#entry; fn f() { r#entry!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
    }

    /// Explicit crate paths work without assuming that the current file is the crate root.
    #[test]
    fn inline_modules_and_reexports() {
        resolves(
            "mod logging { pub fn enter() {} } fn f() { crate::logging::enter(); }",
            EntryKind::Function,
            "crate::logging::enter",
        );
        resolves(
            "mod logging { pub use tracing::info; } fn f() { logging::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "mod logging { pub fn enter() {} } use crate::logging as logs; fn f() { logs::enter(); }",
            EntryKind::Function,
            "crate::logging::enter",
        );
        resolves(
            "use crate::logging::enter as start; fn f() { start(); }",
            EntryKind::Function,
            "crate::logging::enter",
        );
        resolves(
            "mod worker { fn f() { crate::enter(); } }",
            EntryKind::Function,
            "crate::enter",
        );
        resolves(
            "mod worker { mod outer {} fn f() { crate::outer::enter(); } }",
            EntryKind::Function,
            "crate::outer::enter",
        );
        assert!(resolve("use tracing::info; mod nested { fn f() { info!(\"entry\"); } }").is_err());
        for source in [
            "fn enter() {} fn f() { enter(); }",
            "mod logging { pub fn enter() {} } fn f() { logging::enter(); }",
            "mod logging { pub fn enter() {} fn f() { self::enter(); } }",
            "fn enter() {} mod worker { fn f() { super::enter(); } }",
            "mod logging { pub fn enter() {} } fn f() { mod logging { pub fn enter() {} } logging::enter(); }",
        ] {
            // A single parsed file cannot establish any of these local definitions' crate prefixes.
            assert!(
                resolve(source).is_err(),
                "unexpected local root assumption: {source}"
            );
        }
    }

    /// Parentheses, generic function arguments, propagation, and awaiting retain direct identity.
    #[test]
    fn transparent_wrappers_and_namespaces() {
        resolves(
            "fn f() { (telemetry::enter::<u8>())?; }",
            EntryKind::Function,
            "telemetry::enter",
        );
        resolves(
            "async fn f() { telemetry::enter().await?; }",
            EntryKind::Function,
            "telemetry::enter",
        );
        resolves(
            "fn f() { (tracing::info!(\"entry\")); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use telemetry::enter; fn f() { enter!(); }",
            EntryKind::Macro,
            "telemetry::enter",
        );
        resolves(
            "use telemetry::enter; fn f() { enter(); }",
            EntryKind::Function,
            "telemetry::enter",
        );
        assert!(resolve("fn f() { logger.enter(); }").is_err());
        assert!(resolve("fn f() { (logger)(); }").is_err());
        assert!(resolve("fn f() { return telemetry::enter(); }").is_err());
    }

    /// Unknown globs, conditional alternatives, and cycles never produce accepted foreign names.
    #[test]
    fn uncertain_imports_fail_closed() {
        for source in [
            "use tracing::*; fn f() { info!(\"entry\"); }",
            "use custom::*; fn f() { tracing::info!(\"entry\"); }",
            "#[cfg(feature=\"logging\")] use tracing::info; fn f() { info!(\"entry\"); }",
            "use tracing::info; use custom::info; fn f() { info!(\"entry\"); }",
            "use first as second; use second as first; fn f() { first::enter(); }",
            "use telemetry::enter::{self as entry}; fn f() { entry(); }",
        ] {
            // Every fixture contains a specific source-visible uncertainty, rather than an arbitrary bare name.
            assert!(resolve(source).is_err(), "unexpected resolution: {source}");
        }
        resolves(
            "use custom::*; use ::tracing::info; fn f() { info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use custom::*; fn f() { ::tracing::info!(\"entry\"); }",
            EntryKind::Macro,
            "tracing::info",
        );
    }

    /// Values, type parameters, and lexical definitions cannot masquerade as imported entries.
    #[test]
    fn lexical_shadows_fail_closed() {
        for source in [
            "use telemetry::enter; fn f(enter: fn()) { enter(); }",
            "use telemetry::enter; fn f() { let enter = || (); enter(); }",
            "use telemetry::enter; fn f() { fn enter() {} enter(); }",
            "use tracing::info; fn f() { macro_rules! info { () => {} } info!(); }",
            "use tracing::info; macro_rules! info { () => {} } mod inner { fn f() { info!(); } }",
            "#[macro_use] mod custom { macro_rules! info { () => {} } } use tracing::info; fn f() { info!(); }",
            "fn f<tracing>() { tracing::info!(); }",
            "struct tracing; fn f() { tracing::info!(); }",
            "use telemetry::enter; fn f() { for enter in source { enter(); } }",
            "use telemetry::enter; fn f() { if let Some(enter) = value { enter(); } }",
        ] {
            // Conservative failures include both dynamic value calls and unresolvable type namespaces.
            assert!(resolve(source).is_err(), "unexpected resolution: {source}");
        }
        resolves(
            "use tracing::info; fn info() {} fn f() { info!(); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "mod tracing {} fn f() { ::tracing::info!(); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "fn f<T: tracing::Instrument>() { tracing::info!(); }",
            EntryKind::Macro,
            "tracing::info",
        );
        resolves(
            "use telemetry::enter; fn f(value: enter) { enter(); }",
            EntryKind::Function,
            "telemetry::enter",
        );
        resolves(
            "use telemetry::enter; fn f() { let action = |value: enter| { enter(); }; }",
            EntryKind::Function,
            "telemetry::enter",
        );
        resolves(
            "use tracing::info; fn f<const info: usize>() { info!(); }",
            EntryKind::Macro,
            "tracing::info",
        );
    }
}
