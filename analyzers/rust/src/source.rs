//! Rust source as syntax: files parsed with syn, the lines of a node (spans
//! carry positions through proc-macro2's `span-locations`), the generated
//! file convention, and the expressions inside macro invocations, which syn
//! leaves as tokens.

use proc_macro2::{Span, TokenStream, TokenTree};
use syn::parse::{ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, Stmt, Token};

/// The line a span starts on, at least 1.
pub fn line(span: Span) -> u32 {
    u32::try_from(span.start().line).unwrap_or(u32::MAX).max(1)
}

/// The line a span ends on, at least 1.
pub fn end_line(span: Span) -> u32 {
    u32::try_from(span.end().line).unwrap_or(u32::MAX).max(1)
}

/// First and last line of a node.
pub fn lines(node: &impl Spanned) -> (u32, u32) {
    let span = node.span();
    let start = line(span);
    (start, end_line(span).max(start))
}

/// Whether the file says it is generated: "@generated" (Cargo, prost,
/// bindgen and others write it) or a "generated ... do not edit" comment in
/// its first lines.
pub fn is_generated(text: &str) -> bool {
    text.lines().take(15).any(|line| {
        let trimmed = line.trim_start();
        if !(trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || trimmed.starts_with("#!"))
        {
            return false;
        }
        let lower = trimmed.to_ascii_lowercase();
        lower.contains("@generated")
            || (lower.contains("generated") && lower.contains("do not edit"))
    })
}

/// The code inside a macro invocation, as statements, for the macros whose
/// arguments are Rust expressions (`println!`, `assert_eq!`, `vec!`,
/// `format!`, `write!`, `matches!`…) or a block of statements. Whatever is
/// not Rust syntax gives nothing; a macro body that is not one list is
/// searched group by group (`tokio::select! { … => { … } }`).
pub fn macro_statements(tokens: &TokenStream) -> Vec<Stmt> {
    if tokens.is_empty() {
        return Vec::new();
    }
    if let Some(statements) = parse_statements(tokens.clone()) {
        return statements;
    }
    let mut found = Vec::new();
    for tree in tokens.clone() {
        if let TokenTree::Group(group) = tree {
            found.extend(macro_statements(&group.stream()));
        }
    }
    found
}

fn parse_statements(tokens: TokenStream) -> Option<Vec<Stmt>> {
    let expressions = |input: ParseStream| Punctuated::<Expr, Token![,]>::parse_terminated(input);
    if let Ok(list) = expressions.parse2(tokens.clone()) {
        return Some(
            list.into_iter()
                .map(|expr| Stmt::Expr(expr, None))
                .collect(),
        );
    }
    // vec![value; count]
    let repeat = |input: ParseStream| {
        let value: Expr = input.parse()?;
        let _: Token![;] = input.parse()?;
        let count: Expr = input.parse()?;
        Ok(vec![value, count])
    };
    if let Ok(list) = repeat.parse2(tokens.clone()) {
        return Some(
            list.into_iter()
                .map(|expr| Stmt::Expr(expr, None))
                .collect(),
        );
    }
    syn::Block::parse_within.parse2(tokens).ok()
}

/// The paths of a `routes![a, b::c]`-like macro: a list of paths.
pub fn macro_paths(tokens: &TokenStream) -> Vec<syn::Path> {
    let paths = |input: ParseStream| Punctuated::<syn::Path, Token![,]>::parse_terminated(input);
    paths
        .parse2(tokens.clone())
        .map(|list| list.into_iter().collect())
        .unwrap_or_default()
}

/// The identifier as written, without a raw `r#` prefix.
pub fn ident(ident: &syn::Ident) -> String {
    let text = ident.to_string();
    match text.strip_prefix("r#") {
        Some(rest) => rest.to_string(),
        None => text,
    }
}

/// The segments of a path as written, without generic arguments.
pub fn segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|segment| ident(&segment.ident))
        .collect()
}

/// The string a literal expression holds.
pub fn string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(text),
            ..
        }) => Some(text.value()),
        Expr::Paren(inner) => string_literal(&inner.expr),
        Expr::Group(inner) => string_literal(&inner.expr),
        Expr::Reference(inner) => string_literal(&inner.expr),
        _ => None,
    }
}

/// Whether a name is written like a type, a variant or a constant (starts
/// with an upper-case letter), as Rust's naming conventions have it.
pub fn is_type_like(name: &str) -> bool {
    name.chars().next().is_some_and(char::is_uppercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macro_of(code: &str) -> syn::Macro {
        match syn::parse_str::<Expr>(code).unwrap() {
            Expr::Macro(mac) => mac.mac,
            _ => panic!("not a macro: {code}"),
        }
    }

    #[test]
    fn macro_arguments_are_read_as_code() {
        assert_eq!(
            macro_statements(&macro_of("assert_eq!(add(1, 2), 3)").tokens).len(),
            2
        );
        assert_eq!(
            macro_statements(&macro_of("println!(\"{}\", value())").tokens).len(),
            2
        );
        assert_eq!(
            macro_statements(&macro_of("vec![make(); 3]").tokens).len(),
            2
        );
        assert_eq!(
            macro_statements(&macro_of("run! { let x = one(); two(x); }").tokens).len(),
            2
        );
        assert_eq!(
            macro_statements(&macro_of("select! { v = rx => { handle(v) } }").tokens).len(),
            1
        );
        assert!(macro_statements(&macro_of("json!({ \"a\": 1 })").tokens).is_empty());
        assert_eq!(
            macro_paths(&macro_of("routes![index, api::users]").tokens).len(),
            2
        );
    }

    #[test]
    fn generated_files_are_recognised_by_their_header() {
        assert!(is_generated(
            "// This file is @generated by prost-build.\npub struct A;"
        ));
        assert!(is_generated("//! Code generated by tool. DO NOT EDIT.\n"));
        assert!(!is_generated(
            "fn main() { let generated = \"do not edit\"; }"
        ));
    }

    #[test]
    fn lines_come_from_the_source() {
        let file = syn::parse_file("\n\nfn a() {\n    b();\n}\n").unwrap();
        assert_eq!(lines(&file.items[0]), (3, 5));
    }
}
