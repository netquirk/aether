//! The reaches a macro invocation hides: the calls a test writes inside `assert_eq!` and friends.
//!
//! The syntax tree of a macro invocation holds its tokens, not its expressions, so a test that
//! calls a private item inside `assert_eq!` writes a call the tree does not show. This module
//! reads the tokens of a macro invocation twice: once as expressions, which the tree understands,
//! and once as a list of calls, which reaches the calls a nested macro holds.

use proc_macro2::{Delimiter, Span, TokenStream, TokenTree};
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{Expr, Token};

/// A call a macro invocation holds in its token stream.
pub(crate) struct MacroCall {
    /// The segments of the path the call names.
    pub(crate) path: Vec<String>,
    /// Where the call stands.
    pub(crate) span: Span,
}

/// The expressions a macro invocation holds, when its tokens parse as expressions.
pub(crate) fn expressions(tokens: &TokenStream) -> Vec<Expr> {
    if let Ok(expression) = syn::parse2::<Expr>(tokens.clone()) {
        return vec![expression];
    }
    let parser = Punctuated::<Expr, Token![,]>::parse_terminated;
    parser.parse2(tokens.clone()).map(|expressions| expressions.into_iter().collect()).unwrap_or_default()
}

/// The calls a macro invocation holds: every name a parenthesis group follows.
pub(crate) fn calls(tokens: &TokenStream) -> Vec<MacroCall> {
    let mut calls = Vec::new();
    collect_calls(tokens, &mut calls);
    calls
}

/// The calls the tokens of one macro invocation hold, and the calls of the groups they hold.
fn collect_calls(tokens: &TokenStream, calls: &mut Vec<MacroCall>) {
    let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
    for (index, tree) in trees.iter().enumerate() {
        match tree {
            TokenTree::Group(group) => collect_calls(&group.stream(), calls),
            TokenTree::Ident(ident) => {
                let mut path = vec![ident.to_string()];
                let mut next = index + 1;
                while next + 2 < trees.len() && is_colon(&trees[next]) && is_colon(&trees[next + 1]) {
                    let Some(TokenTree::Ident(segment)) = trees.get(next + 2) else {
                        break;
                    };
                    path.push(segment.to_string());
                    next += 3;
                }
                let called = matches!(trees.get(next), Some(TokenTree::Group(group)) if group.delimiter() == Delimiter::Parenthesis);
                if called {
                    calls.push(MacroCall { path, span: ident.span() });
                }
            }
            _ => {}
        }
    }
}

/// Whether the token is a colon, the first of the two a path holds between its segments.
fn is_colon(tree: &TokenTree) -> bool {
    matches!(tree, TokenTree::Punct(punct) if punct.as_char() == ':')
}
