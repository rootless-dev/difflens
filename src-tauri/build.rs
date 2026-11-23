use std::path::Path;

/// The sidecars of `bundle.externalBin` in tauri.conf.json.
const SIDECARS: &[&str] = &["binaries/difflens-analyzer-go"];

fn main() {
    // tauri-build stops when a sidecar is missing. `pnpm analyzers:build`
    // builds them and runs before `tauri dev` and `tauri build`; a plain
    // `cargo build` or `cargo test` without them goes on without sidecars,
    // and the app then reports the analyzer as not installed.
    println!("cargo:rerun-if-changed=binaries");
    let target = std::env::var("TARGET").unwrap_or_default();
    let extension = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    let missing: Vec<String> = SIDECARS
        .iter()
        .map(|sidecar| format!("{sidecar}-{target}{extension}"))
        .filter(|path| !Path::new(path).is_file())
        .collect();
    if !missing.is_empty() && std::env::var_os("TAURI_CONFIG").is_none() {
        println!(
            "cargo:warning=Building without the analyzers ({}); run pnpm analyzers:build.",
            missing.join(", ")
        );
        std::env::set_var("TAURI_CONFIG", r#"{"bundle":{"externalBin":[]}}"#);
    }
    tauri_build::build()
}
