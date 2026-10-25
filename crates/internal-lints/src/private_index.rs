//! The index of the crate under test: the modules it declares, the items they hold and the
//! visibility every item is declared under.
//!
//! The index is what lets the lint answer the question a test asks when it calls a name: which
//! item does the name stand for, which module declares it and who may reach it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use syn::{Expr, Fields, ImplItem, Item as SynItem, Type, Visibility};

/// The visibility of an item, as the lint sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ItemVisibility {
    /// `pub`: the users of the crate can reach the item.
    Public,
    /// `pub(crate)`: every module of the crate can reach the item.
    Crate,
    /// A private item: the module that declares it, and that module's descendants, reach it.
    Private {
        /// The module that declares the item.
        module: Vec<String>,
    },
    /// `pub(super)` or `pub(in path)`: the module the path names, and that module's descendants.
    Restricted {
        /// The module the item is visible in.
        scope: Vec<String>,
    },
}

impl ItemVisibility {
    /// The visibility an item declared in `module` under `visibility` has.
    pub(crate) fn of(visibility: &Visibility, module: &[String]) -> Self {
        match visibility {
            Visibility::Public(_) => Self::Public,
            Visibility::Inherited => Self::Private { module: module.to_vec() },
            Visibility::Restricted(restricted) => {
                let path: &syn::Path = &restricted.path;
                if restricted.in_token.is_some() {
                    return Self::Restricted { scope: normalize_path(path, module) };
                }
                match path.segments.iter().map(|segment| segment.ident.to_string()).collect::<Vec<_>>().as_slice() {
                    [only] if only == "crate" => Self::Crate,
                    [only] if only == "super" => {
                        let mut scope = module.to_vec();
                        scope.pop();
                        Self::Restricted { scope }
                    }
                    [only] if only == "self" => Self::Restricted { scope: module.to_vec() },
                    _ => Self::Restricted { scope: normalize_path(path, module) },
                }
            }
        }
    }
}

/// An item the crate declares.
#[derive(Debug, Clone)]
pub(crate) struct Item {
    /// The name of the item.
    pub(crate) name: String,
    /// The visibility the item is declared under.
    pub(crate) visibility: ItemVisibility,
}

/// A type the crate declares, with its fields and its inherent methods.
#[derive(Debug, Clone)]
pub(crate) struct TypeItem {
    /// The name of the type.
    pub(crate) name: String,
    /// The visibility the type is declared under.
    pub(crate) visibility: ItemVisibility,
    /// The module that declares the type.
    pub(crate) module: Vec<String>,
    /// The fields of the type, with the visibility each one is declared under.
    pub(crate) fields: Vec<Item>,
    /// The inherent methods of the type, with the visibility each one is declared under.
    pub(crate) methods: Vec<Item>,
}

/// A `use` declaration: the name it binds and the path it names.
#[derive(Debug, Clone)]
pub(crate) struct Import {
    /// The name the import binds, empty for a glob import.
    pub(crate) alias: String,
    /// The path the import names, as its segments.
    pub(crate) path: Vec<String>,
    /// Whether the import binds every item of the path, as `use path::*` does.
    pub(crate) glob: bool,
}

/// One test function the index read.
pub(crate) struct TestFunction {
    /// The name of the test function.
    pub(crate) name: String,
    /// The body of the test function.
    pub(crate) body: syn::Block,
}

/// A module of the crate under test.
pub(crate) struct Module {
    /// The path of the module, from the root of the crate.
    pub(crate) path: Vec<String>,
    /// The free functions the module declares.
    pub(crate) functions: Vec<Item>,
    /// The constants and statics the module declares.
    pub(crate) constants: Vec<Item>,
    /// The `use` declarations of the module.
    pub(crate) imports: Vec<Import>,
    /// The test functions the module declares.
    pub(crate) tests: Vec<TestFunction>,
    /// The file the module is written in.
    pub(crate) file: Option<PathBuf>,
}

impl Module {
    fn new(path: Vec<String>) -> Self {
        Self { path, functions: Vec::new(), constants: Vec::new(), imports: Vec::new(), tests: Vec::new(), file: None }
    }

    /// The function the module declares under `name`, when it declares one.
    pub(crate) fn function(&self, name: &str) -> Option<&Item> {
        self.functions.iter().find(|item| item.name == name)
    }

    /// The constant the module declares under `name`, when it declares one.
    pub(crate) fn constant(&self, name: &str) -> Option<&Item> {
        self.constants.iter().find(|item| item.name == name)
    }
}

/// The index of one crate under test.
pub(crate) struct CrateIndex {
    /// The name the users of the crate write to reach it.
    pub(crate) name: String,
    /// The modules of the crate, in the order they were read.
    pub(crate) modules: Vec<Module>,
    /// The types of the crate, in the order they were read.
    pub(crate) types: Vec<TypeItem>,
    /// The source of every file the index read, to quote the offending expression.
    pub(crate) sources: BTreeMap<PathBuf, String>,
    /// The files the index read, with the module path it read them under.
    indexed: BTreeSet<(PathBuf, Vec<String>)>,
}

impl CrateIndex {
    /// An index of the crate the users of it reach under `name`.
    pub(crate) fn new(name: String) -> Self {
        Self { name, modules: Vec::new(), types: Vec::new(), sources: BTreeMap::new(), indexed: BTreeSet::new() }
    }

    /// The module the crate declares at `path`.
    pub(crate) fn module_at(&self, path: &[String]) -> Option<&Module> {
        self.modules.iter().find(|module| module.path == path)
    }

    /// The type the crate declares under `name` in the module at `module`.
    pub(crate) fn type_at(&self, module: &[String], name: &str) -> Option<&TypeItem> {
        self.types.iter().find(|item| item.module == module && item.name == name)
    }

    /// The type the crate declares under `name`, whichever module declares it.
    pub(crate) fn type_by_name(&self, name: &str) -> Option<&TypeItem> {
        self.types.iter().find(|item| item.name == name)
    }

    /// Read the crate rooted at `file` as the crate under test.
    pub(crate) fn read_root(&mut self, file: &Path) -> Result<(), String> {
        self.read_file(file, &[])
    }

    /// Read the module the crate declares at `path` from the file it is written in.
    pub(crate) fn read_file(&mut self, file: &Path, path: &[String]) -> Result<(), String> {
        if !self.indexed.insert((file.to_path_buf(), path.to_vec())) {
            return Ok(());
        }
        let source =
            fs::read_to_string(file).map_err(|error| format!("{}: could not be read: {error}", file.display()))?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("{}: could not be parsed as Rust: {error}", file.display()))?;
        self.sources.insert(file.to_path_buf(), source);
        let directory = module_directory(file);
        self.read_items(&parsed.items, file, path, &directory);
        Ok(())
    }

    /// Read the source files of the crate that its modules do not already reach.
    ///
    /// A crate may declare its modules with a macro, as the Wisp crate does, and then the module
    /// tree of the crate root does not name every file of the crate. The compiler looks for the
    /// module of `src/foo.rs` in `src/foo/`, so the path of a file names the module it holds.
    pub(crate) fn read_sources(&mut self, root: &Path) {
        let mut files = Vec::new();
        collect_sources(root, &mut files);
        for file in files {
            let Some(path) = derived_path(root, &file) else {
                continue;
            };
            if path.is_empty() {
                continue;
            }
            let _ = self.read_file(&file, &path);
        }
    }

    /// Read the items of one module.
    fn read_items(&mut self, items: &[SynItem], file: &Path, path: &[String], directory: &Path) {
        let module = self.module_index(path);
        self.modules[module].file = Some(file.to_path_buf());
        for item in items {
            match item {
                SynItem::Fn(item) => {
                    let name = item.sig.ident.to_string();
                    if is_test_function(&item.attrs) {
                        self.modules[module].tests.push(TestFunction { name, body: (*item.block).clone() });
                    } else {
                        let visibility = ItemVisibility::of(&item.vis, path);
                        self.modules[module].functions.push(Item { name, visibility });
                    }
                }
                SynItem::Const(item) => {
                    let visibility = ItemVisibility::of(&item.vis, path);
                    self.modules[module].constants.push(Item { name: item.ident.to_string(), visibility });
                }
                SynItem::Static(item) => {
                    let visibility = ItemVisibility::of(&item.vis, path);
                    self.modules[module].constants.push(Item { name: item.ident.to_string(), visibility });
                }
                SynItem::Use(item) => {
                    let mut imports = Vec::new();
                    collect_use(&item.tree, &mut Vec::new(), &mut imports);
                    for (alias, path, glob) in imports {
                        self.modules[module].imports.push(Import { alias, path, glob });
                    }
                }
                SynItem::Struct(item) => {
                    let visibility = ItemVisibility::of(&item.vis, path);
                    let fields = fields_of(&item.fields, path);
                    self.declare_type(item.ident.to_string(), visibility, path.to_vec(), fields);
                }
                SynItem::Enum(item) => {
                    let visibility = ItemVisibility::of(&item.vis, path);
                    let mut fields = Vec::new();
                    for variant in &item.variants {
                        fields.extend(fields_of(&variant.fields, path));
                    }
                    self.declare_type(item.ident.to_string(), visibility, path.to_vec(), fields);
                }
                SynItem::Union(item) => {
                    let visibility = ItemVisibility::of(&item.vis, path);
                    let fields = named_fields(&item.fields, path);
                    self.declare_type(item.ident.to_string(), visibility, path.to_vec(), fields);
                }
                SynItem::Impl(item) => {
                    if item.trait_.is_none() {
                        let type_name = type_name(&item.self_ty);
                        for impl_item in &item.items {
                            if let ImplItem::Fn(method) = impl_item {
                                let visibility = ItemVisibility::of(&method.vis, path);
                                let method = Item { name: method.sig.ident.to_string(), visibility };
                                self.declare_method(&type_name, method);
                            }
                        }
                    }
                }
                SynItem::Mod(item) => {
                    let mut child_path = path.to_vec();
                    child_path.push(item.ident.to_string());
                    match &item.content {
                        Some((_, items)) => {
                            let child_directory = directory.join(item.ident.to_string());
                            self.read_items(items, file, &child_path, &child_directory);
                        }
                        None => {
                            if let Some(child) = resolve_module_file(file, directory, item) {
                                let _ = self.read_file(&child, &child_path);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The index of the module the crate declares at `path`, adding it when it is new.
    fn module_index(&mut self, path: &[String]) -> usize {
        if let Some(index) = self.modules.iter().position(|module| module.path == path) {
            return index;
        }
        self.modules.push(Module::new(path.to_vec()));
        self.modules.len() - 1
    }

    /// Record a type the crate declares.
    fn declare_type(&mut self, name: String, visibility: ItemVisibility, module: Vec<String>, fields: Vec<Item>) {
        self.types.push(TypeItem { name, visibility, module, fields, methods: Vec::new() });
    }

    /// Record an inherent method of a type the crate declares.
    fn declare_method(&mut self, type_name: &str, method: Item) {
        let entry = self.types.iter_mut().find(|item| item.name == type_name);
        match entry {
            Some(entry) => entry.methods.push(method),
            None => self.types.push(TypeItem {
                name: type_name.to_string(),
                visibility: ItemVisibility::Crate,
                module: Vec::new(),
                fields: Vec::new(),
                methods: vec![method],
            }),
        }
    }
}

/// The path a `pub(in path)` names, relative to the crate.
fn normalize_path(path: &syn::Path, module: &[String]) -> Vec<String> {
    let mut scope = Vec::new();
    for (index, segment) in path.segments.iter().enumerate() {
        match segment.ident.to_string().as_str() {
            "crate" if index == 0 => {}
            "self" if index == 0 => scope.extend(module.iter().cloned()),
            "super" if index == 0 => {
                let mut parent = module.to_vec();
                parent.pop();
                scope.extend(parent);
            }
            _ => scope.push(segment.ident.to_string()),
        }
    }
    scope
}

/// The named fields of a struct, an enum variant or a union.
fn named_fields(fields: &syn::FieldsNamed, module: &[String]) -> Vec<Item> {
    fields
        .named
        .iter()
        .map(|field| Item {
            name: field.ident.as_ref().map(ToString::to_string).unwrap_or_default(),
            visibility: ItemVisibility::of(&field.vis, module),
        })
        .collect()
}

/// The named fields of a set of fields, when it holds named ones.
fn fields_of(fields: &Fields, module: &[String]) -> Vec<Item> {
    match fields {
        Fields::Named(named) => named_fields(named, module),
        _ => Vec::new(),
    }
}

/// The name of the type an implementation is for.
fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(type_path) => {
            type_path.path.segments.last().map(|segment| segment.ident.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// The imports a `use` declaration binds, with the path each one names.
fn collect_use(tree: &syn::UseTree, prefix: &mut Vec<String>, imports: &mut Vec<(String, Vec<String>, bool)>) {
    match tree {
        syn::UseTree::Path(use_path) => {
            prefix.push(use_path.ident.to_string());
            collect_use(&use_path.tree, prefix, imports);
            prefix.pop();
        }
        syn::UseTree::Name(name) => imports.push((name.ident.to_string(), prefix.clone(), false)),
        syn::UseTree::Rename(rename) => {
            let mut path = prefix.clone();
            path.push(rename.ident.to_string());
            imports.push((rename.rename.to_string(), path, false));
        }
        syn::UseTree::Glob(_) => imports.push((String::new(), prefix.clone(), true)),
        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use(item, prefix, imports);
            }
        }
    }
}

/// Whether the attributes mark the function as a test.
pub(crate) fn is_test_function(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attribute| attribute.path().segments.last().is_some_and(|segment| segment.ident == "test"))
}

/// The directory the modules of the file at `file` live in.
///
/// The compiler looks for the child modules of `src/foo.rs` in `src/foo/`, and the child modules
/// of `src/foo/mod.rs`, `src/lib.rs` and `src/main.rs` in the directory of the file.
pub(crate) fn module_directory(file: &Path) -> PathBuf {
    let directory = file.parent().unwrap_or_else(|| Path::new("."));
    let stem = file.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
    if stem == "mod" || stem == "lib" || stem == "main" {
        return directory.to_path_buf();
    }
    directory.join(stem)
}

/// The file the module declaration points at, when the module lives in its own file.
fn resolve_module_file(file: &Path, directory: &Path, item: &syn::ItemMod) -> Option<PathBuf> {
    if let Some(path) = path_attribute(item) {
        let own_directory = file.parent().unwrap_or_else(|| Path::new("."));
        let module_file = own_directory.join(path);
        return module_file.is_file().then_some(module_file);
    }
    let name = item.ident.to_string();
    let module_file = directory.join(format!("{name}.rs"));
    if module_file.is_file() {
        return Some(module_file);
    }
    let module_directory = directory.join(&name).join("mod.rs");
    if module_directory.is_file() {
        return Some(module_directory);
    }
    None
}

/// The path a `#[path = "..."]` attribute names, when the module declaration holds one.
fn path_attribute(item: &syn::ItemMod) -> Option<String> {
    item.attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("path") {
            return None;
        }
        let syn::Meta::NameValue(value) = &attribute.meta else {
            return None;
        };
        let Expr::Lit(expression) = &value.value else {
            return None;
        };
        let syn::Lit::Str(text) = &expression.lit else {
            return None;
        };
        Some(text.value())
    })
}

/// The source files of a crate, the files the compiler compiles as its modules.
fn collect_sources(directory: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
        if name.starts_with('.') || name == "target" || name == "bin" {
            continue;
        }
        if path.is_dir() {
            collect_sources(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

/// The module a source file of the crate holds, as the compiler names it.
fn derived_path(root: &Path, file: &Path) -> Option<Vec<String>> {
    let relative = file.strip_prefix(root).ok()?;
    let mut components: Vec<String> =
        relative.components().map(|component| component.as_os_str().to_string_lossy().to_string()).collect();
    let file_name = components.pop()?;
    let stem = Path::new(&file_name).file_stem()?.to_string_lossy().to_string();
    if stem == "lib" || stem == "main" {
        return None;
    }
    if stem != "mod" {
        components.push(stem);
    }
    Some(components)
}
