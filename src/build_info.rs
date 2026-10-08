//! Compile-time provenance shared by every adapter in the same Cargo build.
//! Source hashes cover declared repository inputs, not external dependencies or
//! toolchain binaries. Git state is optional and records the build-time tree.

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct BuildInfo {
    pub schema_version: u32,
    pub package_version: &'static str,
    pub git_revision: Option<&'static str>,
    pub git_dirty: Option<bool>,
    pub source_sha256: &'static str,
    pub build_fingerprint: &'static str,
    pub rustc_version: Option<&'static str>,
    pub cargo_version: Option<&'static str>,
    pub target: &'static str,
    pub profile: &'static str,
    pub features: &'static [&'static str],
    /// Digest of explicit Cargo/compiler options; flag values and paths are not exposed.
    pub options_sha256: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/build_info.rs"));

pub fn current() -> &'static BuildInfo {
    &BUILD_INFO
}

impl BuildInfo {
    pub fn short_label(&self) -> &'static str {
        LABEL
    }
    pub fn json(&self) -> String {
        serde_json::to_string(self).expect("Static build identity is serializable")
    }
}

#[cfg(test)]
#[path = "../tools/build_identity.rs"]
mod generation;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_identity_has_one_versioned_json_contract() {
        let info = current();
        let json: serde_json::Value = serde_json::from_str(&info.json()).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["package_version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(json["source_sha256"], info.source_sha256);
        assert_eq!(json["build_fingerprint"], info.build_fingerprint);
        for digest in [
            info.source_sha256,
            info.build_fingerprint,
            info.options_sha256,
        ] {
            assert_eq!(digest.len(), 64);
            assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        assert!(info.features.windows(2).all(|w| w[0] < w[1]));
        assert_eq!(VERSION, LABEL);
        assert_eq!(info.short_label(), LABEL);
        assert!(LABEL.contains(&info.build_fingerprint[..12]));
        assert!(!info.json().contains(env!("CARGO_MANIFEST_DIR")));
    }
}
