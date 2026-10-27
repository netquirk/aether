//! The language server must resolve its toolchain from the workspace it serves.
//!
//! The daemon spawns one server per workspace, with that workspace as the working
//! directory. A `RUSTUP_TOOLCHAIN` inherited from the process that started the
//! daemon would pin every server to that toolchain and override the one the
//! workspace selects, so the daemon drops it before starting a server.

use aether_lspd::testing::{CargoProject, TestDaemon, TestProject, configure_fake_server};
use aether_lspd::{LanguageId, LspClient};
use lsp_types::{Hover, HoverContents, MarkedString};
use std::time::Duration;

#[tokio::test]
async fn language_server_does_not_inherit_the_launchers_rustup_toolchain() {
    unsafe { configure_fake_server(LanguageId::Rust, &[]) };

    unsafe {
        std::env::set_var("RUSTUP_TOOLCHAIN", "sentinel-toolchain");
        std::env::set_var("AETHER_LSPD_ENV_MARKER", "marker-value");
    }

    let project = CargoProject::new("toolchain_env").expect("Failed to create project");
    project.add_file("src/main.rs", "fn main() { let value = 1; }\n").expect("Failed to add file");

    // The daemon runs inside this process, so the check exercises the daemon's own
    // code and not a prebuilt daemon binary.
    let daemon = TestDaemon::spawn(project.root(), LanguageId::Rust, Duration::from_secs(10))
        .await
        .expect("Failed to spawn daemon");
    let client = LspClient::connect(project.root(), LanguageId::Rust).await.expect("Failed to connect");
    let uri = project.file_uri("src/main.rs");

    let hover = hover_text(client.hover(uri, 0, 0).await.expect("Hover request failed"));

    assert!(
        hover.contains("env_marker=marker-value"),
        "the server must inherit the environment it was started with: {hover}"
    );
    assert!(hover.contains("rustup_toolchain="), "the server must report the toolchain it resolved: {hover}");
    assert!(
        !hover.contains("sentinel-toolchain"),
        "the server must not inherit RUSTUP_TOOLCHAIN from the daemon's launcher: {hover}"
    );

    daemon.shutdown().expect("Failed to shut down daemon");
}

fn hover_text(hover: Option<Hover>) -> String {
    let hover = hover.expect("Expected hover result");
    match hover.contents {
        HoverContents::Scalar(scalar) => match scalar {
            MarkedString::String(text) => text,
            MarkedString::LanguageString(value) => value.value,
        },
        HoverContents::Array(values) => values
            .into_iter()
            .map(|value| match value {
                MarkedString::String(text) => text,
                MarkedString::LanguageString(value) => value.value,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        HoverContents::Markup(markup) => markup.value,
    }
}
