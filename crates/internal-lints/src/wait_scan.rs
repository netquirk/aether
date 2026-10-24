//! The fixed-wait lint: the wall-clock waits `AGENTS.md` forbids in tests.
//!
//! `AGENTS.md` asks for tests without timeouts, because a fixed wait makes a test flaky on
//! machines of different speeds. This module finds every `#[test]` and `#[tokio::test]` function
//! of the workspace and reports the wall-clock wait in its body: a sleep, an explicitly bounded
//! wait or a duration bound.
//!
//! The scan is syntactic. It reads files, runs nothing, and needs no network, no credential, no
//! model provider and no deployed service.
//!
//! Two kinds of test are left alone, because their wait is not a wall-clock wait:
//!
//! - a test that runs on a fake clock (`#[tokio::test(start_paused = true)]`): its sleeps run on
//!   tokio's virtual clock,
//! - a test that drives a fake provider (its body names a `Fake`): it answers without a clock.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Attribute, Expr, ExprCall, ExprMethodCall, ImplItem, Item};

use crate::{crate_files, workspace_members};

/// The functions that put a test to sleep for a fixed duration.
const SLEEP_FUNCTIONS: [&str; 3] = ["sleep", "sleep_until", "sleep_after"];

/// The functions that bound a wait with an explicit deadline.
const BOUNDED_WAIT_FUNCTIONS: [&str; 4] = ["timeout", "timeout_at", "recv_timeout", "recv_deadline"];

/// The name prefixes of a call that waits for something to happen.
const WAIT_PREFIXES: [&str; 2] = ["wait", "poll"];

/// The kind of wall-clock wait a [`WaitFinding`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitKind {
    /// A fixed sleep: `sleep`, `sleep_until` or `sleep_after`.
    Sleep,
    /// An explicitly bounded wait: `timeout`, `timeout_at`, `recv_timeout` or `recv_deadline`.
    ExplicitWait,
    /// A call bounded by a `Duration` argument.
    DurationBound,
}

impl WaitKind {
    /// Every kind the lint reports, in the order it prints the counts.
    pub fn all() -> [Self; 3] {
        [Self::Sleep, Self::ExplicitWait, Self::DurationBound]
    }

    /// The name the lint prints for this kind.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sleep => "sleep",
            Self::ExplicitWait => "explicit wait",
            Self::DurationBound => "duration bound",
        }
    }
}

/// Why the lint did not report a test function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExemptionReason {
    /// The test runs on tokio's virtual clock (`start_paused = true`).
    FakeClock,
    /// The test drives a fake provider.
    FakeProvider,
    /// The test waits on something the test cannot fake, and is named in the allowlist.
    Allowlisted,
}

impl ExemptionReason {
    /// Every reason the lint prints, in the order it prints the counts.
    pub fn all() -> [Self; 3] {
        [Self::FakeClock, Self::FakeProvider, Self::Allowlisted]
    }

    /// The name the lint prints for this reason.
    pub fn label(&self) -> &'static str {
        match self {
            Self::FakeClock => "fake clock",
            Self::FakeProvider => "fake provider",
            Self::Allowlisted => "allowlisted",
        }
    }
}

/// One wall-clock wait found in a test function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitFinding {
    /// The file the wait was found in.
    pub path: PathBuf,
    /// The 1-based line the wait is on.
    pub line: usize,
    /// The 1-based column the wait starts at.
    pub column: usize,
    /// The name of the test function the wait is in.
    pub test: String,
    /// Which kind of wall-clock wait this is.
    pub kind: WaitKind,
    /// The wait as it was written, for example `tokio::time::sleep(Duration::from_millis(100))`.
    pub detail: String,
}

/// A test function the lint did not report, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitExemption {
    /// The file the test function is in.
    pub path: PathBuf,
    /// The name of the test function.
    pub test: String,
    /// Why the lint left the test function alone.
    pub reason: ExemptionReason,
}

/// What the scan covered and what it found.
#[derive(Debug, Clone, Default)]
pub struct WaitReport {
    /// Every wall-clock wait found, in scan order.
    pub findings: Vec<WaitFinding>,
    /// Every test function the scan read.
    pub tests_scanned: usize,
    /// How many of the test functions the scan read carry a wall-clock wait.
    pub tests_flagged: usize,
    /// Every path the scan read.
    pub files: Vec<PathBuf>,
    /// The test functions the scan left alone, with the reason.
    pub exemptions: Vec<WaitExemption>,
    /// Paths that could not be read or parsed, so their contents went unchecked.
    pub problems: Vec<String>,
}

impl WaitReport {
    /// Whether the scan found nothing and checked everything it read.
    pub fn is_clean(&self) -> bool {
        self.findings.is_empty() && self.problems.is_empty()
    }

    /// How many findings of `kind` the scan made.
    pub fn count(&self, kind: WaitKind) -> usize {
        self.findings.iter().filter(|finding| finding.kind == kind).count()
    }

    /// How many test functions the scan left alone for `reason`.
    pub fn exempted(&self, reason: ExemptionReason) -> usize {
        self.exemptions.iter().filter(|exemption| exemption.reason == reason).count()
    }
}

/// The path of the allowlist the lint reads when the caller names none.
pub fn default_allowlist_path(root: &Path) -> PathBuf {
    root.join("crates/internal-lints/wait-allowlist.txt")
}

/// Scan every test function of the workspace for a wall-clock wait.
pub fn scan_workspace_waits(root: &Path) -> WaitReport {
    scan_workspace_waits_allowing(root, &default_allowlist_path(root))
}

/// Scan every test function of the workspace, reading the allowlist at `allowlist`.
///
/// A root that holds a crate but names no workspace member is scanned as that
/// one crate, so the lint also runs on a single crate.
pub fn scan_workspace_waits_allowing(root: &Path, allowlist: &Path) -> WaitReport {
    let mut report = WaitReport::default();
    let allowed = Allowlist::load(allowlist);
    let members = workspace_members(root);
    let crates =
        if members.is_empty() && root.join("Cargo.toml").is_file() { vec![root.to_path_buf()] } else { members };
    for member in crates {
        for file in crate_files(&member) {
            report.files.push(file.clone());
            scan_file(&mut report, &file, root, &allowed);
        }
    }
    report
}

fn scan_file(report: &mut WaitReport, path: &Path, root: &Path, allowed: &Allowlist) {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            report.problems.push(format!("{}: could not be read: {error}", path.display()));
            return;
        }
    };
    let parsed = match syn::parse_file(&source) {
        Ok(parsed) => parsed,
        Err(error) => {
            report.problems.push(format!("{}: could not be parsed as Rust: {error}", path.display()));
            return;
        }
    };
    let mut tests = Vec::new();
    collect_tests(&parsed.items, path, &source, &mut tests);
    for test in tests {
        report.tests_scanned += 1;
        if let Some(reason) = test.exemption {
            report.exemptions.push(WaitExemption { path: path.to_path_buf(), test: test.name, reason });
            continue;
        }
        if allowed.allows(path, root, &test.name) {
            report.exemptions.push(WaitExemption {
                path: path.to_path_buf(),
                test: test.name,
                reason: ExemptionReason::Allowlisted,
            });
            continue;
        }
        if !test.findings.is_empty() {
            report.tests_flagged += 1;
        }
        report.findings.extend(test.findings);
    }
}

/// One test function the scan read.
struct TestFunction {
    /// The name of the test function.
    name: String,
    /// The wall-clock waits in its body.
    findings: Vec<WaitFinding>,
    /// Why the lint leaves the function alone, when it does.
    exemption: Option<ExemptionReason>,
}

fn collect_tests(items: &[Item], path: &Path, source: &str, tests: &mut Vec<TestFunction>) {
    for item in items {
        match item {
            Item::Fn(item) => {
                if is_test_function(&item.attrs) {
                    tests.push(scan_test(&item.sig.ident.to_string(), &item.attrs, &item.block, path, source));
                }
            }
            Item::Mod(item) => {
                if let Some((_, items)) = &item.content {
                    collect_tests(items, path, source, tests);
                }
            }
            Item::Impl(item) => {
                for impl_item in &item.items {
                    if let ImplItem::Fn(method) = impl_item
                        && is_test_function(&method.attrs)
                    {
                        tests.push(scan_test(
                            &method.sig.ident.to_string(),
                            &method.attrs,
                            &method.block,
                            path,
                            source,
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn scan_test(name: &str, attrs: &[Attribute], block: &syn::Block, path: &Path, source: &str) -> TestFunction {
    if let Some(reason) = exemption_reason(attrs, block) {
        return TestFunction { name: name.to_string(), findings: Vec::new(), exemption: Some(reason) };
    }
    let mut visitor = WaitVisitor { path, test: name.to_string(), source, findings: Vec::new() };
    visitor.visit_block(block);
    TestFunction { name: name.to_string(), findings: visitor.findings, exemption: None }
}

/// Whether the attributes mark the function as a test.
fn is_test_function(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| is_test_attribute(attr.path()))
}

/// Whether the attribute path names a test attribute (`test` or `tokio::test`).
fn is_test_attribute(path: &syn::Path) -> bool {
    path.segments.last().is_some_and(|segment| segment.ident == "test")
}

/// Why the lint leaves the test function alone, when it does.
fn exemption_reason(attrs: &[Attribute], block: &syn::Block) -> Option<ExemptionReason> {
    if runs_on_fake_clock(attrs) {
        return Some(ExemptionReason::FakeClock);
    }
    let mut finder = FakeFinder { found: false };
    finder.visit_block(block);
    if finder.found { Some(ExemptionReason::FakeProvider) } else { None }
}

/// Whether the attributes put the test on tokio's virtual clock.
fn runs_on_fake_clock(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if !is_test_attribute(attr.path()) {
            return false;
        }
        let mut paused = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("start_paused") {
                paused = meta.value()?.parse::<syn::LitBool>()?.value();
            }
            Ok(())
        });
        paused
    })
}

/// Walks the body of one test function and collects the wall-clock waits in it.
struct WaitVisitor<'a> {
    /// The file the test function is in.
    path: &'a Path,
    /// The name of the test function.
    test: String,
    /// The source of the file, to quote the offending expression.
    source: &'a str,
    /// The waits found so far.
    findings: Vec<WaitFinding>,
}

impl WaitVisitor<'_> {
    fn finding(&self, kind: WaitKind, span: Span) -> WaitFinding {
        let start = span.start();
        WaitFinding {
            path: self.path.to_path_buf(),
            line: start.line,
            column: start.column + 1,
            test: self.test.clone(),
            kind,
            detail: expression_text(self.source, span),
        }
    }

    /// The wait a free-function call stands for, when it is one.
    fn call_finding(&self, node: &ExprCall) -> Option<WaitFinding> {
        let Expr::Path(path) = &*node.func else {
            return None;
        };
        if path.qself.is_some() {
            return None;
        }
        let segments: Vec<String> = path.path.segments.iter().map(|segment| segment.ident.to_string()).collect();
        let name = segments.last()?;
        // A path that starts with a type name reaches an associated item or an enum variant
        // (`ProviderError::timeout(..)`), never a free function.
        if is_type_path(&segments) {
            return None;
        }
        if let Some(kind) = wait_kind(name) {
            return Some(self.finding(kind, node.span()));
        }
        if is_wait_shaped(name) && node.args.iter().any(is_duration) {
            return Some(self.finding(WaitKind::DurationBound, node.span()));
        }
        None
    }

    /// The wait a method call stands for, when it is one.
    fn method_finding(&self, node: &ExprMethodCall) -> Option<WaitFinding> {
        let name = node.method.to_string();
        if let Some(kind) = wait_kind(&name) {
            return Some(self.finding(kind, node.span()));
        }
        if is_wait_shaped(&name) && node.args.iter().any(is_duration) {
            return Some(self.finding(WaitKind::DurationBound, node.span()));
        }
        None
    }
}

impl<'ast> Visit<'ast> for WaitVisitor<'_> {
    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        if let Some(finding) = self.call_finding(node) {
            self.findings.push(finding);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if let Some(finding) = self.method_finding(node) {
            self.findings.push(finding);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

/// Spots the fakes a test function drives.
struct FakeFinder {
    /// Whether the body names a fake.
    found: bool,
}

impl<'ast> Visit<'ast> for FakeFinder {
    fn visit_ident(&mut self, ident: &'ast syn::Ident) {
        if ident.to_string().starts_with("Fake") {
            self.found = true;
        }
    }
}

/// The kind of wait the callee stands for, when it is one.
fn wait_kind(name: &str) -> Option<WaitKind> {
    if SLEEP_FUNCTIONS.contains(&name) {
        return Some(WaitKind::Sleep);
    }
    if BOUNDED_WAIT_FUNCTIONS.contains(&name) {
        return Some(WaitKind::ExplicitWait);
    }
    None
}

/// Whether the callee waits for something to happen.
fn is_wait_shaped(name: &str) -> bool {
    WAIT_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Whether the path starts with a type name.
fn is_type_path(segments: &[String]) -> bool {
    segments.first().is_some_and(|first| first.starts_with(|character: char| character.is_uppercase()))
}

/// Whether the expression is a `Duration` value.
fn is_duration(expression: &Expr) -> bool {
    let path = match expression {
        Expr::Path(path) => &path.path,
        Expr::Call(call) => match &*call.func {
            Expr::Path(path) => &path.path,
            _ => return false,
        },
        _ => return false,
    };
    path.segments.iter().any(|segment| segment.ident == "Duration")
}

/// The source text the span covers, on one line.
fn expression_text(source: &str, span: Span) -> String {
    let range = span.byte_range();
    source.get(range).map(|text| text.split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default()
}

/// The test functions the allowlist names, with the file they live in.
struct Allowlist {
    /// The entries of the allowlist, each `path::test function`.
    entries: BTreeSet<String>,
}

impl Allowlist {
    /// Read the allowlist, treating a missing file as an empty allowlist.
    fn load(path: &Path) -> Self {
        let Ok(text) = fs::read_to_string(path) else {
            return Self { entries: BTreeSet::new() };
        };
        let entries = text
            .lines()
            .map(|line| line.split('#').next().unwrap_or("").trim())
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        Self { entries }
    }

    /// Whether the allowlist names the test function.
    fn allows(&self, path: &Path, root: &Path, test: &str) -> bool {
        let relative = path.strip_prefix(root).unwrap_or(path);
        self.entries.contains(&format!("{}::{}", relative.display(), test))
    }
}
