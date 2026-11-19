//! Build script: Tauri's own build step and the build tag.
//!
//! Bug reports no longer need anything at build time: the app posts them to
//! the Glint server, which holds the Discord webhook (see src/bugreport.rs).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // A private build's tag, e.g. GLINT_BUILD_TAG=-dev.share: shown after the version, and such a
    // build never offers or installs updates (update.rs). Public releases leave it unset.
    println!("cargo:rerun-if-env-changed=GLINT_BUILD_TAG");
    println!("cargo:rustc-env=GLINT_BUILD_TAG={}", std::env::var("GLINT_BUILD_TAG").unwrap_or_default());
    tauri_build::build()
}
