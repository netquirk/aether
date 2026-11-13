use std::path::Path;

const OWN_MODULE_PROVIDERS: [&str; 2] = ["codex", "amazon-bedrock"];

fn main() {
    // Read the manifest directory when the build script runs, not when it is
    // compiled. The compiled build script is cached in the target directory,
    // and a path baked in at compile time names the checkout that compiled
    // it: a target directory shared with another checkout (a second worktree,
    // a CI job that runs the base commit) would then look for `models.json`
    // in the wrong tree.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR for build scripts");
    let models_json = Path::new(&manifest_dir).join("models.json");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let output_path = Path::new(&out_dir).join("generated.rs");

    println!("cargo::rerun-if-changed={}", models_json.display());

    let output = llm_codegen::generate(&models_json).unwrap_or_else(|e| {
        panic!("Codegen failed: {e}");
    });

    std::fs::write(&output_path, &output.rust_source).unwrap();

    let docs_dir = Path::new(&out_dir).join("docs");
    std::fs::create_dir_all(&docs_dir).unwrap();
    let mut generic =
        String::from("Providers served by [`GenericProvider`], each described by a [`ProviderConfig`] constant.\n\n");
    for (provider_id, markdown) in &output.provider_docs {
        if OWN_MODULE_PROVIDERS.contains(&provider_id.as_str()) {
            std::fs::write(docs_dir.join(format!("{provider_id}.md")), markdown).unwrap();
        } else {
            generic.push_str(markdown);
            generic.push('\n');
        }
    }
    std::fs::write(docs_dir.join("generic.md"), generic).unwrap();
}
