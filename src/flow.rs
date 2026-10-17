//! FC001: leading explanations for substantial nested executable bodies.

use crate::model::Level;
use crate::registry::{Finding, Rule, RuleDefinition};
use crate::source::{Body, BodyKind, Comment, SourceContext, is_logging_macro};

/// The first bundled rule, implemented through the shared rule interface.
pub struct FlowCommentRule;

impl Rule for FlowCommentRule {
    fn definition(&self) -> RuleDefinition {
        RuleDefinition {
            id: "FC001",
            name: "flow-comments",
            description: "A substantial nested executable body needs a leading flow comment",
            default_level: Level::Warning,
            options: Vec::new(),
        }
    }

    fn check(
        &self,
        context: &SourceContext,
        _options: &toml::Table,
    ) -> Result<Vec<Finding>, String> {
        let mut findings = Vec::new();
        // Qualifying nested bodies each receive their own exact suppression target.
        for body in &context.bodies {
            if !qualifies(context, body) || has_flow_comment(context, body) {
                continue;
            }
            findings.push(Finding {
                message: format!(
                    "{} body has {} direct statements; add a leading flow comment",
                    body.kind.description(),
                    body.statements.len()
                ),
                span: body.span,
                target: body.span,
                annotations: body.annotations.clone(),
            });
        }
        Ok(findings)
    }
}

/// Select FC001 bodies independently of the parser's complete structural model.
fn qualifies(context: &SourceContext, body: &Body) -> bool {
    body.in_scope
        && body.kind != BodyKind::Function
        && body.statements.len() >= 3
        && !body.statements.iter().all(|statement| {
            statement
                .macro_path
                .as_deref()
                .is_some_and(is_logging_macro)
                || context.is_plain_field_copy(statement)
        })
}

/// Accept an ordinary explanation associated with this exact body.
fn has_flow_comment(context: &SourceContext, body: &Body) -> bool {
    let mut group = Vec::<&Comment>::new();
    // Adjacent lines form one explanation, so continuation lines cannot disguise a license header.
    for comment in body
        .annotations
        .iter()
        .filter_map(|span| context.comment(*span))
    {
        if !comment.in_scope || !comment.standalone || comment.is_doc {
            continue;
        }
        if let Some(previous) = group.last() {
            let gap = &context.text[previous.span.end..comment.span.start];
            if !gap.trim().is_empty() || gap.bytes().filter(|byte| *byte == b'\n').count() > 1 {
                if explanatory_group(&group) {
                    return true;
                }
                group.clear();
            }
        }
        group.push(comment);
    }
    explanatory_group(&group)
}

/// Combine adjacent comment lines while rejecting an explicitly identified license header.
fn explanatory_group(comments: &[&Comment]) -> bool {
    let contents: Vec<_> = comments
        .iter()
        .map(|comment| comment_content(comment))
        .collect();
    !contents.iter().any(|content| license_header(content))
        && contents.iter().any(|content| explanation(content))
}

/// Strip ordinary comment delimiters and conventional block-comment margin stars.
fn comment_content(comment: &Comment) -> String {
    let raw = comment.text.trim();
    if let Some(line) = raw.strip_prefix("//") {
        line.trim().to_owned()
    } else {
        raw.strip_prefix("/*")
            .and_then(|block| block.strip_suffix("*/"))
            .unwrap_or("")
            .lines()
            .map(|line| line.trim().trim_start_matches('*').trim())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Recognize nonempty prose without inventing a required prefix or prose score.
fn explanation(content: &str) -> bool {
    if content.trim().is_empty() || content.trim_start().starts_with("evenflow:") {
        return false;
    }
    let words: Vec<_> = content
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    words.is_empty()
        || !words
            .iter()
            .all(|word| word.eq_ignore_ascii_case("TODO") || word.eq_ignore_ascii_case("FIXME"))
}

/// Reject explicit license-header markers without treating ordinary prose keywords as policy.
fn license_header(content: &str) -> bool {
    content.lines().any(|line| {
        let normalized = line.trim().to_ascii_lowercase();
        copyright_notice(&normalized)
            || [
                "spdx-license-identifier:",
                "spdx-filecopyrighttext:",
                "(c) copyright ",
                "licensed under the apache license",
                "permission is hereby granted, free of charge",
                "this program is free software:",
            ]
            .iter()
            .any(|prefix| normalized.starts_with(prefix))
    })
}

/// Identify an explicit copyright notice without rejecting prose about copyright checks.
fn copyright_notice(normalized: &str) -> bool {
    let Some(remainder) = normalized.strip_prefix("copyright") else {
        return false;
    };
    let remainder = remainder.trim_start();
    remainder.starts_with("(c)")
        || remainder.starts_with('©')
        || (remainder
            .as_bytes()
            .get(..4)
            .is_some_and(|year| year.iter().all(u8::is_ascii_digit))
            && remainder
                .as_bytes()
                .get(4)
                .is_none_or(|byte| !byte.is_ascii_alphabetic()))
}

#[cfg(test)]
mod tests {
    //! Checked examples of FC001's syntax, scope, and placement policies.

    use super::*;

    /// Analyze an isolated production fixture through the real rule implementation.
    fn lint(source: &str) -> Vec<Finding> {
        let context = SourceContext::parse("src/example.rs", source.as_bytes(), false)
            .expect("fixture must parse");
        FlowCommentRule
            .check(&context, &toml::Table::new())
            .expect("bundled rule must complete")
    }

    /// Count statements once and exempt outer function bodies.
    #[test]
    fn statement_threshold_and_tails() {
        assert!(lint("fn f() { a(); b(); c(); }").is_empty());
        assert!(lint("fn f() { if x { a(); b(); } }").is_empty());
        assert_eq!(lint("fn f() { if x { a(); b(); c() } }").len(), 1);
        assert_eq!(lint("fn f() { if x { a(\n1,\n2\n); b(); c(); } }").len(), 1);
        assert_eq!(
            lint("fn f() { if x { fn local() {} struct S; a(); b(); c(); } }").len(),
            1
        );
    }

    /// Leading comments explain only their exact structural boundary.
    #[test]
    fn comment_placement_and_siblings() {
        assert!(
            lint("fn f() {\n// Explain the primary branch.\nif x { a(); b(); c(); } }").is_empty()
        );
        assert!(
            lint("fn f() { if x {\n// Preserve the update order.\na(); b(); c(); } }").is_empty()
        );
        assert_eq!(
            lint("fn f() { if x { a(); // trailing\nb(); c(); } }").len(),
            1
        );
        assert_eq!(lint("fn f() {\n// Explain the primary branch.\nif x { a(); b(); c(); } else { a(); b(); c(); } }").len(), 1);
        assert_eq!(lint("fn f() {\n// Explain only the outer body.\nif x { if y { a(); b(); c(); } d(); e(); } }").len(), 1);
        assert_eq!(lint("fn f() { if x {\n// Explain only this parent.\nif y { a(); b(); c(); } d(); e(); } }").len(), 1);
    }

    /// Conditions retain independent bodies while the construct comment explains its primary body.
    #[test]
    fn condition_bodies_are_independent() {
        let source =
            "fn f() {\n// Explain the chosen branch.\nif { a(); b(); true } { a(); b(); c(); } }";
        assert_eq!(lint(source).len(), 1);
        let source =
            "fn f() {\n// Explain the loop.\nwhile { a(); b(); true } { a(); b(); c(); } }";
        assert_eq!(lint(source).len(), 1);
    }

    /// Ordinary comments survive attributes, while docs and bounded placeholders do not qualify.
    #[test]
    fn comments_attributes_and_placeholders() {
        assert!(
            lint(
                "fn f() {\n// Preserve order.\n#[cfg(feature = \"x\")]\nif x { a(); b(); c(); } }"
            )
            .is_empty()
        );
        assert!(
            lint("fn f() { if x {\n/* Preserve\n * this order. */\na(); b(); c(); } }").is_empty()
        );
        assert_eq!(
            lint("fn f() {\n/// API docs do not explain flow.\nif x { a(); b(); c(); } }").len(),
            1
        );
        assert_eq!(
            lint("fn f() {\n// TODO:\n// FIXME!\nif x { a(); b(); c(); } }").len(),
            1
        );
        assert!(lint("fn f() {\n// TODO: preserve updates until the cache is ready.\nif x { a(); b(); c(); } }").is_empty());
        assert_eq!(lint("fn f() {\n// evenflow: allow(FC001): Deliberate exception.\nif x { a(); b(); c(); } }").len(), 1);
        assert!(lint("fn f() {\n// ✓\nif x { a(); b(); c(); } }").is_empty());
    }

    /// Explicit license headers and their continuation lines are never flow explanations.
    #[test]
    fn license_headers_are_bounded_and_grouped() {
        assert_eq!(
            lint("fn f() {\n// SPDX-License-Identifier: MIT\nif x { a(); b(); c(); } }").len(),
            1
        );
        assert_eq!(lint("fn f() {\n// Copyright 2026 Example Authors\n// All rights reserved.\nif x { a(); b(); c(); } }").len(), 1);
        assert_eq!(lint("fn f() {\n/* License information\n * SPDX-License-Identifier: Apache-2.0\n */\nif x { a(); b(); c(); } }").len(), 1);
        assert!(lint("fn f() {\n// Copyright 2026 Example Authors\n\n// Keep the writes ordered.\nif x { a(); b(); c(); } }").is_empty());
        assert!(lint("fn f() {\n// Check the license before allowing the update.\nif x { a(); b(); c(); } }").is_empty());
        assert!(lint("fn f() {\n// Copyright checks must finish before persisting the package.\nif x { a(); b(); c(); } }").is_empty());
        assert_eq!(lint("fn f() {\n// Copyright (c) Example Authors\n// All rights reserved.\nif x { a(); b(); c(); } }").len(), 1);
        assert_eq!(
            lint("fn f() {\n// Copyright © Example Authors\nif x { a(); b(); c(); } }").len(),
            1
        );
    }

    /// Logging and simple field copies form a narrow exemption; computed work does not.
    #[test]
    fn narrow_logging_and_copy_exemption() {
        assert!(
            lint("fn f() { if x { trace!(\"a\"); log::debug!(\"b\"); slog::crit!(log, \"c\"); } }")
                .is_empty()
        );
        assert!(
            lint("fn f() { if x { self.a = other.a; self.b = &value; self.c = 1; } }").is_empty()
        );
        assert_eq!(
            lint("fn f() { if x { self.a = call(); self.b = value; self.c = 1; } }").len(),
            1
        );
        assert_eq!(
            lint("fn f() { if x { self.a += value; self.b = value; self.c = 1; } }").len(),
            1
        );
        assert_eq!(
            lint("fn f() { if x { self.a = make().field; self.b = value; self.c = 1; } }").len(),
            1
        );
        assert_eq!(
            lint("fn f() { if x { trace!(\"a\"); debug!(\"b\"); work(); } }").len(),
            1
        );
    }

    /// Block-bearing constructs are checked while expression-only closures/arms add no body.
    #[test]
    fn block_constructs_and_expression_only_forms() {
        let source = "fn f() { let a = || { a(); b(); c(); }; let b = async { a(); b(); c(); }; unsafe { a(); b(); c(); } match x { A => { a(); b(); c(); } B => work(), } let a = || work(); }";
        assert_eq!(lint(source).len(), 4);
        assert_eq!(
            lint("fn f() { let Some(x) = value else { a(); b(); panic!(); }; }").len(),
            1
        );
        assert_eq!(
            lint("trait T { fn method(&self) { if x { a(); b(); c(); } } }").len(),
            1
        );
    }

    /// Constant initializers omit deferred runtime closures as specified.
    #[test]
    fn constant_contexts_stay_out_of_scope() {
        let source = "const fn f() { if x { a(); b(); c(); } } static X: LazyLock<()> = LazyLock::new(|| { if x { a(); b(); c(); } }); fn g() { const { a(); b(); c(); }; let x: [u8; { a(); b(); 3 }] = [0; { a(); b(); 3 }]; } enum E { A = { a(); b(); 1 } }";
        assert!(lint(source).is_empty());
    }

    /// Default filtering keeps unknown production cfg alternatives and excludes definite tests.
    #[test]
    fn conservative_test_filtering() {
        assert!(lint("#[test] fn f() { if x { a(); b(); c(); } }").is_empty());
        assert!(lint("#[tokio::test(flavor = \"current_thread\")] async fn f() { if x { a(); b(); c(); } }").is_empty());
        assert!(
            lint("#[cfg(all(test, feature = \"integration\"))] fn f() { if x { a(); b(); c(); } }")
                .is_empty()
        );
        assert_eq!(
            lint("#[cfg(any(test, feature = \"production\"))] fn f() { if x { a(); b(); c(); } }")
                .len(),
            1
        );
        assert_eq!(
            lint("#[cfg_attr(test, ignore)] fn f() { if x { a(); b(); c(); } }").len(),
            1
        );
        assert_eq!(lint("#[cfg(test)] mod tests { fn t() { if x { a(); b(); c(); } } } fn production() { if x { a(); b(); c(); } }").len(), 1);
    }
}
