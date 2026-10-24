//! Scanning of Rust sources: the banned crates everywhere, boxed public errors on the public API.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{ImplItem, Item, ItemImpl, ReturnType, TraitItem, Type, Visibility};

use crate::{Finding, ForbiddenKind};

/// The crate names the lint bans, with the kind each one is reported as.
const BANNED_CRATES: [(&str, ForbiddenKind); 2] =
    [("anyhow", ForbiddenKind::AnyhowCrate), ("color_eyre", ForbiddenKind::ColorEyreCrate)];

/// Scan one Rust file for the banned crates.
pub fn scan_rust_file(path: &Path) -> (Vec<Finding>, Vec<String>) {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return (Vec::new(), vec![format!("{}: could not be read: {error}", path.display())]);
        }
    };
    let parsed = match syn::parse_file(&source) {
        Ok(parsed) => parsed,
        Err(error) => {
            return (Vec::new(), vec![format!("{}: could not be parsed as Rust: {error}", path.display())]);
        }
    };
    let mut visitor = BannedCrateVisitor { path, findings: Vec::new() };
    visitor.visit_file(&parsed);
    (visitor.findings, Vec::new())
}

/// Scan the public API of a crate for boxed errors a caller cannot match on.
pub fn scan_public_surface(crate_root: &Path) -> Vec<Finding> {
    let src = crate_root.join("src");
    let mut findings = Vec::new();
    let lib = src.join("lib.rs");
    if lib.is_file() {
        walk_module(&lib, true, true, &mut findings);
    }
    let main = src.join("main.rs");
    if main.is_file() {
        walk_module(&main, false, true, &mut findings);
    }
    let bin = src.join("bin");
    if bin.is_dir() {
        let Ok(entries) = fs::read_dir(&bin) else {
            return findings;
        };
        let mut paths: Vec<PathBuf> =
            entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_file()).collect();
        paths.sort();
        for path in paths {
            walk_module(&path, false, true, &mut findings);
        }
    }
    findings
}

struct BannedCrateVisitor<'a> {
    path: &'a Path,
    findings: Vec<Finding>,
}

impl BannedCrateVisitor<'_> {
    fn finding(&self, kind: ForbiddenKind, detail: String, span: Span) -> Finding {
        let start = span.start();
        Finding { path: self.path.to_path_buf(), line: start.line, column: start.column + 1, kind, detail }
    }
}

impl<'ast> Visit<'ast> for BannedCrateVisitor<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        if let Some(segment) = path.segments.first()
            && let Some(kind) = banned_kind(&segment.ident.to_string())
        {
            let detail = path.segments.iter().map(|segment| segment.ident.to_string()).collect::<Vec<_>>().join("::");
            self.findings.push(self.finding(kind, detail, segment.ident.span()));
        }
        syn::visit::visit_path(self, path);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        check_use_tree(self, &node.tree);
    }

    fn visit_item_extern_crate(&mut self, node: &'ast syn::ItemExternCrate) {
        if let Some(kind) = banned_kind(&node.ident.to_string()) {
            self.findings.push(self.finding(kind, node.ident.to_string(), node.ident.span()));
        }
        syn::visit::visit_item_extern_crate(self, node);
    }
}

fn check_use_tree(visitor: &mut BannedCrateVisitor<'_>, tree: &syn::UseTree) {
    match tree {
        syn::UseTree::Path(use_path) => {
            if let Some(kind) = banned_kind(&use_path.ident.to_string()) {
                visitor.findings.push(visitor.finding(kind, use_path.ident.to_string(), use_path.ident.span()));
            }
        }
        syn::UseTree::Group(group) => {
            for item in &group.items {
                check_use_tree(visitor, item);
            }
        }
        syn::UseTree::Name(_) | syn::UseTree::Rename(_) | syn::UseTree::Glob(_) => {}
    }
}

fn banned_kind(name: &str) -> Option<ForbiddenKind> {
    BANNED_CRATES.iter().find(|(crate_name, _)| *crate_name == name).map(|(_, kind)| *kind)
}

/// What one module knows about its own scope.
struct ModuleContext {
    public_types: HashSet<String>,
    bare_error_is_std: bool,
}

impl ModuleContext {
    fn new(items: &[Item], parent_bare_error_is_std: bool) -> Self {
        let mut public_types = HashSet::new();
        let mut defines_error = false;
        let mut imports_other_error = false;
        for item in items {
            if let Item::Trait(item) = item {
                defines_error |= item.ident == "Error";
            }
            if let Some(visibility) = item_visibility(item)
                && is_public(visibility)
                && let Some(name) = item_type_name(item)
            {
                public_types.insert(name);
            }
            if let Item::Use(item) = item {
                let mut imports = Vec::new();
                collect_use_imports(&item.tree, "", &mut imports);
                for (source, name) in imports {
                    imports_other_error |= name == "Error" && !is_std_error_source(&source);
                }
            }
        }
        Self { public_types, bare_error_is_std: parent_bare_error_is_std && !defines_error && !imports_other_error }
    }
}

fn walk_module(file: &Path, public: bool, parent_bare_error_is_std: bool, findings: &mut Vec<Finding>) {
    let Ok(source) = fs::read_to_string(file) else {
        return;
    };
    let Ok(parsed) = syn::parse_file(&source) else {
        return;
    };
    let children_dir = module_children_dir(file);
    walk_items(&parsed.items, file, &children_dir, public, parent_bare_error_is_std, findings);
}

fn walk_items(
    items: &[Item],
    file: &Path,
    children_dir: &Path,
    public: bool,
    parent_bare_error_is_std: bool,
    findings: &mut Vec<Finding>,
) {
    let context = ModuleContext::new(items, parent_bare_error_is_std);
    for item in items {
        check_item(item, file, public, &context, findings);
        let Item::Mod(module) = item else {
            continue;
        };
        let child_public = public && is_public(&module.vis);
        match &module.content {
            Some((_, inline_items)) => {
                let inline_dir = children_dir.join(module.ident.to_string());
                walk_items(inline_items, file, &inline_dir, child_public, context.bare_error_is_std, findings);
            }
            None => {
                if let Some(child) = resolve_module_file(file, children_dir, module) {
                    walk_module(&child, child_public, context.bare_error_is_std, findings);
                }
            }
        }
    }
}

fn check_item(item: &Item, file: &Path, public: bool, context: &ModuleContext, findings: &mut Vec<Finding>) {
    let visible = public && item_visibility(item).is_some_and(is_public);
    match item {
        Item::Fn(item) => {
            if visible {
                check_signature(&item.sig, file, context, findings);
            }
        }
        Item::Struct(item) => {
            if visible {
                check_fields(&item.fields, file, context, findings);
            }
        }
        Item::Enum(item) => {
            if visible {
                for variant in &item.variants {
                    check_fields(&variant.fields, file, context, findings);
                }
            }
        }
        Item::Union(item) => {
            if visible {
                check_named_fields(&item.fields.named, file, context, findings);
            }
        }
        Item::Type(item) => {
            if visible {
                check_type(&item.ty, file, context, findings);
            }
        }
        Item::Const(item) => {
            if visible {
                check_type(&item.ty, file, context, findings);
            }
        }
        Item::Static(item) => {
            if visible {
                check_type(&item.ty, file, context, findings);
            }
        }
        Item::Trait(item) => {
            if visible {
                for trait_item in &item.items {
                    if let TraitItem::Fn(method) = trait_item {
                        check_signature(&method.sig, file, context, findings);
                    }
                }
            }
        }
        Item::Impl(item) if public && impl_is_public(item, &context.public_types) => {
            for impl_item in &item.items {
                if let ImplItem::Fn(method) = impl_item
                    && is_public(&method.vis)
                {
                    check_signature(&method.sig, file, context, findings);
                }
            }
        }
        _ => {}
    }
}

fn check_fields(fields: &syn::Fields, file: &Path, context: &ModuleContext, findings: &mut Vec<Finding>) {
    match fields {
        syn::Fields::Named(named) => check_named_fields(&named.named, file, context, findings),
        syn::Fields::Unnamed(unnamed) => check_named_fields(&unnamed.unnamed, file, context, findings),
        syn::Fields::Unit => {}
    }
}

fn check_named_fields(
    fields: &syn::punctuated::Punctuated<syn::Field, syn::Token![,]>,
    file: &Path,
    context: &ModuleContext,
    findings: &mut Vec<Finding>,
) {
    for field in fields {
        if is_public(&field.vis) {
            check_type(&field.ty, file, context, findings);
        }
    }
}

fn check_signature(signature: &syn::Signature, file: &Path, context: &ModuleContext, findings: &mut Vec<Finding>) {
    for input in &signature.inputs {
        if let syn::FnArg::Typed(pattern) = input {
            check_type(&pattern.ty, file, context, findings);
        }
    }
    if let ReturnType::Type(_, ty) = &signature.output {
        check_type(ty, file, context, findings);
    }
}

fn check_type(ty: &Type, file: &Path, context: &ModuleContext, findings: &mut Vec<Finding>) {
    let mut finder = BoxedErrorFinder { context, found: None };
    finder.visit_type(ty);
    let Some((detail, span)) = finder.found else {
        return;
    };
    let start = span.start();
    findings.push(Finding {
        path: file.to_path_buf(),
        line: start.line,
        column: start.column + 1,
        kind: ForbiddenKind::BoxedPublicError,
        detail,
    });
}

/// Finds a boxed error anywhere inside a type, `Result<(), Box<dyn Error>>` included.
struct BoxedErrorFinder<'a> {
    context: &'a ModuleContext,
    found: Option<(String, Span)>,
}

impl<'ast> Visit<'ast> for BoxedErrorFinder<'_> {
    fn visit_type(&mut self, ty: &'ast Type) {
        if self.found.is_none()
            && let Some(detail) = boxed_error_detail(ty, self.context)
        {
            self.found = Some((detail, ty.span()));
            return;
        }
        syn::visit::visit_type(self, ty);
    }
}

fn boxed_error_detail(ty: &Type, context: &ModuleContext) -> Option<String> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    if type_path.qself.is_some() {
        return None;
    }
    let segment = type_path.path.segments.last()?;
    if segment.ident != "Box" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    if arguments.args.len() != 1 {
        return None;
    }
    let syn::GenericArgument::Type(inner) = &arguments.args[0] else {
        return None;
    };
    let Type::TraitObject(object) = inner else {
        return None;
    };
    let Some(syn::TypeParamBound::Trait(bound)) = object.bounds.first() else {
        return None;
    };
    if !is_std_error(&bound.path, context) {
        return None;
    }
    let mut bounds = vec!["std::error::Error".to_string()];
    for extra in object.bounds.iter().skip(1) {
        let text = match extra {
            syn::TypeParamBound::Trait(trait_bound) => {
                trait_bound.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
            }
            syn::TypeParamBound::Lifetime(lifetime) => format!("'{}", lifetime.ident),
            _ => String::new(),
        };
        bounds.push(text);
    }
    Some(format!("Box<dyn {}>", bounds.join(" + ")))
}

fn is_std_error(path: &syn::Path, context: &ModuleContext) -> bool {
    let segments: Vec<String> = path.segments.iter().map(|segment| segment.ident.to_string()).collect();
    match segments.as_slice() {
        [name] => name == "Error" && context.bare_error_is_std,
        [root, module, name] => (root == "std" || root == "core") && module == "error" && name == "Error",
        _ => false,
    }
}

fn impl_is_public(item: &ItemImpl, public_types: &HashSet<String>) -> bool {
    type_name(&item.self_ty).is_some_and(|name| public_types.contains(&name))
}

fn type_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(type_path) => type_path.path.segments.last().map(|segment| segment.ident.to_string()),
        _ => None,
    }
}

fn item_visibility(item: &Item) -> Option<&Visibility> {
    match item {
        Item::Const(item) => Some(&item.vis),
        Item::Enum(item) => Some(&item.vis),
        Item::Fn(item) => Some(&item.vis),
        Item::Mod(item) => Some(&item.vis),
        Item::Static(item) => Some(&item.vis),
        Item::Struct(item) => Some(&item.vis),
        Item::Trait(item) => Some(&item.vis),
        Item::TraitAlias(item) => Some(&item.vis),
        Item::Type(item) => Some(&item.vis),
        Item::Union(item) => Some(&item.vis),
        _ => None,
    }
}

fn item_type_name(item: &Item) -> Option<String> {
    match item {
        Item::Enum(item) => Some(item.ident.to_string()),
        Item::Struct(item) => Some(item.ident.to_string()),
        Item::Trait(item) => Some(item.ident.to_string()),
        Item::Type(item) => Some(item.ident.to_string()),
        Item::Union(item) => Some(item.ident.to_string()),
        _ => None,
    }
}

fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

fn module_children_dir(file: &Path) -> PathBuf {
    let parent = file.parent().unwrap_or_else(|| Path::new(""));
    let name = file.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
    if name == "lib.rs" || name == "main.rs" || name == "mod.rs" {
        parent.to_path_buf()
    } else {
        let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
        parent.join(stem)
    }
}

fn resolve_module_file(file: &Path, children_dir: &Path, module: &syn::ItemMod) -> Option<PathBuf> {
    if let Some(relative) = path_attribute(module) {
        let candidate = file.parent()?.join(relative);
        return candidate.is_file().then_some(candidate);
    }
    let name = module.ident.to_string();
    let flat = children_dir.join(format!("{name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }
    let nested = children_dir.join(&name).join("mod.rs");
    if nested.is_file() {
        return Some(nested);
    }
    None
}

fn path_attribute(module: &syn::ItemMod) -> Option<String> {
    for attribute in &module.attrs {
        if !attribute.path().is_ident("path") {
            continue;
        }
        if let syn::Meta::NameValue(name_value) = &attribute.meta
            && let syn::Expr::Lit(expression) = &name_value.value
            && let syn::Lit::Str(value) = &expression.lit
        {
            return Some(value.value());
        }
    }
    None
}

fn collect_use_imports(tree: &syn::UseTree, prefix: &str, imports: &mut Vec<(String, String)>) {
    match tree {
        syn::UseTree::Path(use_path) => {
            let prefix =
                if prefix.is_empty() { use_path.ident.to_string() } else { format!("{prefix}::{}", use_path.ident) };
            collect_use_imports(&use_path.tree, &prefix, imports);
        }
        syn::UseTree::Name(name) => imports.push((prefix.to_string(), name.ident.to_string())),
        syn::UseTree::Rename(rename) => imports.push((prefix.to_string(), rename.rename.to_string())),
        syn::UseTree::Glob(_) => {}
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_imports(item, prefix, imports);
            }
        }
    }
}

fn is_std_error_source(source: &str) -> bool {
    source == "std::error" || source == "core::error" || source == "std" || source == "core"
}
