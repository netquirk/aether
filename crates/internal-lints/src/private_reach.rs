//! The reach of a private item: the names a test uses and the items they stand for.
//!
//! The reach is what the lint reports. A test that calls a name reaches the item the name stands
//! for, and the lint asks the index of the crate under test which item that is and who may reach
//! it.

use std::collections::BTreeMap;
use std::path::Path;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Expr, ExprCall, ExprField, ExprMethodCall, ExprStruct, Macro, Pat, Path as SynPath, Stmt, Type};

use crate::private_index::{CrateIndex, Import, Item, ItemVisibility};
use crate::private_macro;
use crate::private_scan::{PrivateFinding, PrivateItemKind};
/// Where an item a path names comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// The crate under test.
    Crate,
    /// The file the test lives in.
    Own,
}

/// What a path resolved to.
pub(crate) struct Resolved {
    /// The item the path names.
    pub(crate) item: Item,
    /// Which kind of item it is.
    pub(crate) kind: PrivateItemKind,
    /// The module that declares it.
    pub(crate) module: Vec<String>,
    /// Where the item comes from.
    pub(crate) origin: Origin,
}

/// The names a test may use: the module it lives in and the imports of that module.
#[derive(Clone)]
pub(crate) struct Scope {
    /// The path of the module the test lives in.
    pub(crate) module: Vec<String>,
    /// The imports of that module.
    pub(crate) imports: Vec<Import>,
}

/// Walks the body of one test function and collects the reaches of private items.
pub(crate) struct ReachVisitor<'a> {
    /// The index of the crate under test.
    pub(crate) index: &'a CrateIndex,
    /// The index of the file the test lives in.
    pub(crate) scope_index: &'a CrateIndex,
    /// The names the test may use.
    pub(crate) scope: Scope,
    /// Whether the test is compiled outside the crate, as an integration test is.
    pub(crate) outside: bool,
    /// The file the test is written in.
    pub(crate) file: &'a Path,
    /// The source of the file, to quote the offending expression.
    pub(crate) source: &'a str,
    /// The name of the test function.
    pub(crate) test: String,
    /// The reaches found so far.
    pub(crate) findings: Vec<PrivateFinding>,
    /// The types of the values the test binds to a name.
    pub(crate) locals: BTreeMap<String, String>,
}

impl ReachVisitor<'_> {
    /// Resolve a path to the item it names.
    fn resolve(&self, segments: &[String]) -> Option<Resolved> {
        let (first, rest) = segments.split_first()?;
        match first.as_str() {
            "crate" => self.resolve_scope(&[], rest),
            "self" => self.resolve_scope(&self.scope.module.clone(), rest),
            "super" => {
                let mut base = self.scope.module.clone();
                base.pop();
                self.resolve_scope(&base, rest)
            }
            _ => {
                if self.outside && first == &self.index.name {
                    return self.resolve_crate(&[], rest);
                }
                self.resolve_name(first, rest)
            }
        }
    }

    /// Resolve a path that names a module of the crate the test lives in, or the crate itself.
    fn resolve_direct(&self, segments: &[String]) -> Option<Resolved> {
        let (first, rest) = segments.split_first()?;
        match first.as_str() {
            "crate" => self.resolve_scope(&[], rest),
            "self" => self.resolve_scope(&self.scope.module.clone(), rest),
            "super" => {
                let mut base = self.scope.module.clone();
                base.pop();
                self.resolve_scope(&base, rest)
            }
            _ => {
                if self.outside && first == &self.index.name {
                    return self.resolve_crate(&[], rest);
                }
                let mut child = self.scope.module.clone();
                child.push(first.clone());
                if self.scope_index.module_at(&child).is_some() {
                    return self.resolve_scope(&child, rest);
                }
                let root = vec![first.clone()];
                if self.scope_index.module_at(&root).is_some() {
                    return self.resolve_scope(&root, rest);
                }
                None
            }
        }
    }

    /// Resolve a path that starts with a name in scope: an item, an import or a module.
    fn resolve_name(&self, first: &str, rest: &[String]) -> Option<Resolved> {
        if rest.is_empty()
            && let Some(resolved) = self.item_in_scope(first)
        {
            return Some(resolved);
        }
        if let Some(import) = self.scope.imports.iter().find(|import| !import.glob && import.alias == first) {
            let mut path = import.path.clone();
            path.extend(rest.iter().cloned());
            return self.resolve_direct(&path);
        }
        for import in self.scope.imports.iter().filter(|import| import.glob) {
            let mut path = import.path.clone();
            path.push(first.to_string());
            path.extend(rest.iter().cloned());
            if let Some(resolved) = self.resolve_direct(&path) {
                return Some(resolved);
            }
        }
        None
    }

    /// The item the module of the test declares under `name`, when it declares one.
    fn item_in_scope(&self, name: &str) -> Option<Resolved> {
        let origin = if self.outside { Origin::Own } else { Origin::Crate };
        let module = self.scope_index.module_at(&self.scope.module)?;
        if let Some(item) = module.function(name) {
            return Some(Resolved {
                item: item.clone(),
                kind: PrivateItemKind::Function,
                module: module.path.clone(),
                origin,
            });
        }
        if let Some(item) = module.constant(name) {
            return Some(Resolved {
                item: item.clone(),
                kind: PrivateItemKind::Constant,
                module: module.path.clone(),
                origin,
            });
        }
        if let Some(type_item) = self.scope_index.type_at(&module.path, name) {
            return Some(Resolved {
                item: Item { name: type_item.name.clone(), visibility: type_item.visibility.clone() },
                kind: PrivateItemKind::Type,
                module: module.path.clone(),
                origin,
            });
        }
        None
    }

    /// Resolve the last segment of a path, written relative to the module `base`.
    fn resolve_in(index: &CrateIndex, base: &[String], rest: &[String], origin: Origin) -> Option<Resolved> {
        let (name, prefix) = rest.split_last()?;
        let module_path: Vec<String> = base.iter().chain(prefix.iter()).cloned().collect();
        if let Some(module) = index.module_at(&module_path) {
            if let Some(item) = module.function(name) {
                return Some(Resolved {
                    item: item.clone(),
                    kind: PrivateItemKind::Function,
                    module: module_path.clone(),
                    origin,
                });
            }
            if let Some(item) = module.constant(name) {
                return Some(Resolved {
                    item: item.clone(),
                    kind: PrivateItemKind::Constant,
                    module: module_path.clone(),
                    origin,
                });
            }
            if let Some(type_item) = index.type_at(&module_path, name) {
                return Some(Resolved {
                    item: Item { name: type_item.name.clone(), visibility: type_item.visibility.clone() },
                    kind: PrivateItemKind::Type,
                    module: module_path.clone(),
                    origin,
                });
            }
        }
        let (type_name, type_path) = prefix.split_last()?;
        let type_module: Vec<String> = base.iter().chain(type_path.iter()).cloned().collect();
        let type_item = index.type_at(&type_module, type_name)?;
        let method = type_item.methods.iter().find(|method| method.name == *name)?;
        Some(Resolved { item: method.clone(), kind: PrivateItemKind::Method, module: type_module, origin })
    }

    /// Resolve a path that names the module the test lives in.
    fn resolve_scope(&self, base: &[String], rest: &[String]) -> Option<Resolved> {
        let origin = if self.outside { Origin::Own } else { Origin::Crate };
        Self::resolve_in(self.scope_index, base, rest, origin)
    }

    /// Resolve a path that names an item of the crate under test.
    fn resolve_crate(&self, base: &[String], rest: &[String]) -> Option<Resolved> {
        Self::resolve_in(self.index, base, rest, Origin::Crate)
    }

    /// Whether the test may reach the item.
    fn visible(&self, resolved: &Resolved) -> bool {
        if resolved.origin == Origin::Own {
            return true;
        }
        match &resolved.item.visibility {
            ItemVisibility::Public => true,
            ItemVisibility::Crate => !self.outside,
            ItemVisibility::Private { module } => {
                if self.outside || module.is_empty() {
                    return false;
                }
                self.scope.module.starts_with(module.as_slice())
            }
            ItemVisibility::Restricted { scope } => !self.outside && self.scope.module.starts_with(scope.as_slice()),
        }
    }

    /// The type of the value an expression names, when the test declared one.
    fn receiver_type(&self, expression: &Expr) -> Option<String> {
        let Expr::Path(path) = expression else {
            return None;
        };
        if path.qself.is_some() {
            return None;
        }
        let name = path.path.segments.last()?.ident.to_string();
        self.locals.get(&name).cloned()
    }

    /// The reach a method call stands for, when it is one.
    fn method_reach(&self, node: &ExprMethodCall) -> Option<Resolved> {
        let type_name = self.receiver_type(&node.receiver)?;
        let type_item = self.index.type_by_name(&type_name)?;
        let method = type_item.methods.iter().find(|method| node.method == method.name)?;
        Some(Resolved {
            item: method.clone(),
            kind: PrivateItemKind::Method,
            module: type_item.module.clone(),
            origin: Origin::Crate,
        })
    }

    /// The reach a field access stands for, when it is one.
    fn field_reach(&self, node: &ExprField) -> Option<Resolved> {
        let syn::Member::Named(member) = &node.member else {
            return None;
        };
        let type_name = self.receiver_type(&node.base)?;
        let type_item = self.index.type_by_name(&type_name)?;
        let field = type_item.fields.iter().find(|field| *member == field.name)?;
        Some(Resolved {
            item: field.clone(),
            kind: PrivateItemKind::Field,
            module: type_item.module.clone(),
            origin: Origin::Crate,
        })
    }

    /// The reach a struct literal stands for, when it is one.
    fn struct_reach(&self, node: &ExprStruct) -> Option<Resolved> {
        let type_name = node.path.segments.last()?.ident.to_string();
        let type_item = self.index.type_by_name(&type_name)?;
        for field in &node.fields {
            let syn::Member::Named(member) = &field.member else {
                continue;
            };
            let item = type_item.fields.iter().find(|item| *member == item.name)?;
            return Some(Resolved {
                item: item.clone(),
                kind: PrivateItemKind::Field,
                module: type_item.module.clone(),
                origin: Origin::Crate,
            });
        }
        None
    }

    /// The reach a call stands for, when it is one.
    fn call_reach(&self, node: &ExprCall) -> Option<Resolved> {
        let Expr::Path(path) = &*node.func else {
            return None;
        };
        if path.qself.is_some() {
            return None;
        }
        self.resolve(&path_segments(&path.path))
    }

    /// Record the reach of a call, when the call reaches an item the test may not reach.
    fn check_call(&mut self, node: &ExprCall) {
        if let Some(resolved) = self.call_reach(node)
            && !self.visible(&resolved)
        {
            let finding = self.finding(&resolved, node.span());
            self.push_finding(finding);
        }
    }

    /// Record the reach of a method call, when it reaches an item the test may not reach.
    fn check_method(&mut self, node: &ExprMethodCall) {
        if let Some(resolved) = self.method_reach(node)
            && !self.visible(&resolved)
        {
            let finding = self.finding(&resolved, node.span());
            self.push_finding(finding);
        }
    }

    /// Record the reach of a field access, when it reaches an item the test may not reach.
    fn check_field(&mut self, node: &ExprField) {
        if let Some(resolved) = self.field_reach(node)
            && !self.visible(&resolved)
        {
            let finding = self.finding(&resolved, node.span());
            self.push_finding(finding);
        }
    }

    /// Record the reach of a struct literal, when it reaches an item the test may not reach.
    fn check_struct(&mut self, node: &ExprStruct) {
        if let Some(resolved) = self.resolve(&path_segments(&node.path))
            && !self.visible(&resolved)
        {
            let finding = self.finding(&resolved, node.span());
            self.push_finding(finding);
        }
        if let Some(resolved) = self.struct_reach(node)
            && !self.visible(&resolved)
        {
            let finding = self.finding(&resolved, node.span());
            self.push_finding(finding);
        }
    }

    /// Record the reaches a macro invocation holds: the calls it writes inside `assert_eq!`.
    fn check_macro(&mut self, node: &Macro) {
        for expression in private_macro::expressions(&node.tokens) {
            let mut checker = ReachVisitor {
                index: self.index,
                scope_index: self.scope_index,
                scope: self.scope.clone(),
                outside: self.outside,
                file: self.file,
                source: self.source,
                test: self.test.clone(),
                findings: Vec::new(),
                locals: self.locals.clone(),
            };
            checker.visit_expr(&expression);
            for finding in checker.findings {
                self.push_finding(finding);
            }
        }
        for call in private_macro::calls(&node.tokens) {
            if let Some(resolved) = self.resolve(&call.path)
                && !self.visible(&resolved)
            {
                let finding = self.finding(&resolved, call.span);
                self.push_finding(finding);
            }
        }
    }

    /// The finding a reach stands for.
    fn finding(&self, resolved: &Resolved, span: Span) -> PrivateFinding {
        let start = span.start();
        PrivateFinding {
            path: self.file.to_path_buf(),
            line: start.line,
            column: start.column + 1,
            test: self.test.clone(),
            kind: resolved.kind,
            item: resolved.item.name.clone(),
            module: module_label(&resolved.module),
            detail: expression_text(self.source, span),
        }
    }

    /// Record a finding, keeping one finding for each reach.
    fn push_finding(&mut self, finding: PrivateFinding) {
        let known = self.findings.iter().any(|existing| {
            existing.line == finding.line && existing.column == finding.column && existing.item == finding.item
        });
        if !known {
            self.findings.push(finding);
        }
    }
}

impl<'ast> Visit<'ast> for ReachVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        self.check_call(node);
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        self.check_method(node);
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_field(&mut self, node: &'ast ExprField) {
        self.check_field(node);
        syn::visit::visit_expr_field(self, node);
    }

    fn visit_expr_struct(&mut self, node: &'ast ExprStruct) {
        self.check_struct(node);
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        self.check_macro(node);
        syn::visit::visit_macro(self, node);
    }
}

/// The name the lint prints for the module an item lives in.
fn module_label(path: &[String]) -> String {
    if path.is_empty() { "crate root".to_string() } else { path.join("::") }
}

/// The segments of a path.
fn path_segments(path: &SynPath) -> Vec<String> {
    path.segments.iter().map(|segment| segment.ident.to_string()).collect()
}

/// The types of the values a test binds to a name.
pub(crate) fn collect_locals(body: &syn::Block) -> BTreeMap<String, String> {
    let mut locals = BTreeMap::new();
    for statement in &body.stmts {
        let Stmt::Local(local) = statement else {
            continue;
        };
        let Some(name) = local_name(&local.pat) else {
            continue;
        };
        if let Some(type_name) = annotated_type(&local.pat) {
            locals.insert(name, type_name);
            continue;
        }
        if let Some(init) = &local.init
            && let Some(type_name) = expression_type(&init.expr)
        {
            locals.insert(name, type_name);
        }
    }
    locals
}

/// The name a local binding binds.
fn local_name(pattern: &Pat) -> Option<String> {
    match pattern {
        Pat::Ident(ident) => Some(ident.ident.to_string()),
        Pat::Type(typed) => local_name(&typed.pat),
        _ => None,
    }
}

/// The type a local binding declares, when it declares one.
fn annotated_type(pattern: &Pat) -> Option<String> {
    match pattern {
        Pat::Type(typed) => match &*typed.ty {
            Type::Path(type_path) => type_path.path.segments.last().map(|segment| segment.ident.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The type of the value an expression produces, when the test names one.
fn expression_type(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Struct(structure) => structure.path.segments.last().map(|segment| segment.ident.to_string()),
        Expr::Reference(reference) => expression_type(&reference.expr),
        Expr::Call(call) => match &*call.func {
            Expr::Path(path) => {
                let segments = path_segments(&path.path);
                if segments.len() >= 2 { Some(segments[segments.len() - 2].clone()) } else { None }
            }
            _ => None,
        },
        _ => None,
    }
}

/// The source text the span covers, on one line.
fn expression_text(source: &str, span: Span) -> String {
    let range = span.byte_range();
    source.get(range).map(|text| text.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}
