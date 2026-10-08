#[path = "tools/build_identity.rs"]
mod identity;

fn main() {
    if let Ok(feed) = std::env::var("NOH_UPDATE_FEED_URL") {
        assert!(
            identity::credential_free_update_feed(&feed),
            "NOH_UPDATE_FEED_URL must be a public HTTPS URL without query, fragment or userinfo credentials"
        );
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && std::env::var_os("CARGO_FEATURE_GUI").is_some()
    {
        // Generate a native resource object directly, preserving the GNU/LLD
        // toolchain without requiring a separate Windows resource compiler.
        embedinator::ResourceBuilder::from_env()
            .add_string("ProductName", "NOH")
            .add_string("FileDescription", "NOH audio/video tools")
            .add_icon(
                1,
                embedinator::Icon::from_png_bytes(include_bytes!("assets/noh-icon.png").to_vec()),
            )
            .finish();
    }
    let root = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let inputs = identity::inputs(&root).expect("Cannot enumerate build inputs");
    for path in identity::input_watches(&root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let git = identity::git_info(&root);
    for path in &git.watches {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    // Dirty state also includes documentation and other tracked/untracked files.
    // Watch the tree through its existing directories, never target/dist/caches.
    for path in identity::vcs_watches(&root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let mut options = Vec::new();
    let mut features = Vec::new();
    for (key, value) in std::env::vars() {
        if let Some(feature) = key.strip_prefix("CARGO_FEATURE_") {
            features.push(feature.to_ascii_lowercase().replace('_', "-"));
        }
        if identity::is_option(&key) {
            println!("cargo:rerun-if-env-changed={key}");
            options.push((key, identity::normalize_option(&value, &root)));
        }
    }
    // Watch unset flags as well, so changing a plain Cargo invocation invalidates provenance.
    for key in identity::OPTION_ENV {
        println!("cargo:rerun-if-env-changed={key}");
    }
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=CARGO");
    let rustc = identity::compiler_info(std::env::var_os("RUSTC").as_deref());
    let info = identity::Identity {
        package_version: std::env::var("CARGO_PKG_VERSION").unwrap(),
        git_revision: git.revision,
        git_dirty: git.dirty,
        source_sha256: identity::source_hash(&root, &inputs)
            .expect("Cannot fingerprint build inputs"),
        rustc_version: rustc,
        cargo_version: identity::cargo_info(std::env::var_os("CARGO").as_deref()),
        target: std::env::var("TARGET").unwrap(),
        profile: std::env::var("PROFILE").unwrap(),
        features,
        options,
    };
    let generated = info.render();
    let output =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("build_info.rs");
    if std::fs::read_to_string(&output).ok().as_deref() != Some(&generated) {
        std::fs::write(output, generated).expect("Cannot embed build identity");
    }
}
