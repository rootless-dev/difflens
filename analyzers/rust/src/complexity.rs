//! Cyclomatic complexity from Rust syntax, counted the way the Go analyzer
//! counts Go: 1 plus one per decision point.
//!
//! Decision points: `if` (each `else if` is one more `if`, `if let`
//! included), `while` and `while let`, `for`, `loop`, each `match` arm other
//! than a catch-all (`_`, or a lower-case binding like `other`), each match
//! guard, `&&` and `||`, each `?` (an early return, as Go's
//! `if err != nil { return }` is), and each `let … else`. Closures and
//! nested items are functions of their own and are not counted; the code
//! inside macro invocations is counted when it parses as expressions
//! (`assert!(a && b)` counts its `&&`, not the branch the macro hides).

use syn::visit::Visit;
use syn::{BinOp, Expr, Pat};

use crate::source;

/// Complexity of a function body.
pub fn of_block(block: &syn::Block) -> u32 {
    let mut counter = Counter { count: 1 };
    counter.visit_block(block);
    counter.count
}

/// Complexity of a closure body.
pub fn of_expr(expr: &Expr) -> u32 {
    let mut counter = Counter { count: 1 };
    counter.visit_expr(expr);
    counter.count
}

struct Counter {
    count: u32,
}

/// An arm pattern that matches anything: what Go's `default:` is.
fn catch_all(pattern: &Pat) -> bool {
    match pattern {
        Pat::Wild(_) => true,
        Pat::Ident(binding) => {
            binding.subpat.is_none() && !source::is_type_like(&source::ident(&binding.ident))
        }
        _ => false,
    }
}

impl<'ast> Visit<'ast> for Counter {
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}

    fn visit_item(&mut self, _: &'ast syn::Item) {}

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        self.count += 1;
        syn::visit::visit_expr_if(self, node);
    }

    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.count += 1;
        syn::visit::visit_expr_while(self, node);
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.count += 1;
        syn::visit::visit_expr_for_loop(self, node);
    }

    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.count += 1;
        syn::visit::visit_expr_loop(self, node);
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        if !catch_all(&node.pat) {
            self.count += 1;
        }
        if node.guard.is_some() {
            self.count += 1;
        }
        syn::visit::visit_arm(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        if matches!(node.op, BinOp::And(_) | BinOp::Or(_)) {
            self.count += 1;
        }
        syn::visit::visit_expr_binary(self, node);
    }

    fn visit_expr_try(&mut self, node: &'ast syn::ExprTry) {
        self.count += 1;
        syn::visit::visit_expr_try(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if node
            .init
            .as_ref()
            .is_some_and(|init| init.diverge.is_some())
        {
            self.count += 1;
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        for statement in source::macro_statements(&node.tokens) {
            self.visit_stmt(&statement);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complexity(body: &str) -> u32 {
        let function: syn::ItemFn = syn::parse_str(&format!("fn f() {{ {body} }}")).unwrap();
        of_block(&function.block)
    }

    #[test]
    fn counts_decisions_like_the_go_analyzer() {
        assert_eq!(complexity(""), 1);
        assert_eq!(complexity("if a { b() } else if c { d() } else { e() }"), 3);
        assert_eq!(complexity("while x {} for i in y {} loop { break }"), 4);
        assert_eq!(complexity("if a && b || c {}"), 4);
        assert_eq!(
            complexity("match v { 1 => a(), 2 if ok => b(), _ => c() }"),
            4
        );
        assert_eq!(
            complexity("match v { None => a(), Some(x) => b(), other => c() }"),
            3
        );
        assert_eq!(complexity("let x = load()?; let y = parse(x)?;"), 3);
        assert_eq!(complexity("let Some(x) = y else { return };"), 2);
        assert_eq!(complexity("let f = |x| if x { 1 } else { 2 };"), 1);
        assert_eq!(complexity("assert!(a && b);"), 2);
        assert_eq!(complexity("fn inner() { if a {} }"), 1);
    }

    #[test]
    fn a_closure_counts_its_own_body() {
        let closure: Expr = syn::parse_str("|x| if x > 1 && x < 5 { 1 } else { 2 }").unwrap();
        let Expr::Closure(closure) = closure else {
            panic!()
        };
        assert_eq!(of_expr(&closure.body), 3);
    }
}
