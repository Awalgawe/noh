//! Build-time public trust configuration; never loaded from an update feed.
use super::*;

pub fn package_id() -> &'static str {
    option_env!("NOH_UPDATE_PACKAGE_ID").unwrap_or("NOH")
}
/// Legacy fixture automation, or explicit user consent on the command line.
/// Environment variables alone never authorize ordinary Setup installation.
pub fn qualification_enabled() -> bool {
    option_env!("NOH_UPDATE_QUALIFICATION_ROOT").is_some() || offline_install_archive().is_some()
}
pub fn offline_install_archive() -> Option<PathBuf> {
    let args: Vec<_> = std::env::args_os().collect();
    if setup_enabled() && args.len() == 3 && args[1] == "--install-retained-update" {
        Some(PathBuf::from(&args[2]))
    } else {
        None
    }
}
pub fn setup_enabled() -> bool {
    cfg!(windows) && option_env!("NOH_UPDATE_FORMAT") == Some("windows-setup")
}

#[cfg(windows)]
pub fn validate_setup_base(base: &std::path::Path) -> Result<()> {
    if let Some(root) = option_env!("NOH_UPDATE_QUALIFICATION_ROOT") {
        let allowed = std::fs::canonicalize(root)?;
        if base == allowed || !base.starts_with(&allowed) {
            return Err(Error::InstallationUnavailable);
        }
    } else if !setup_enabled() {
        return Err(Error::InstallationUnavailable);
    }
    // Trust is embedded independently from the installation and downloaded inputs.
    compiled_keys()?;
    Ok(())
}
pub(crate) fn compiled_version() -> Result<Version> {
    Version::parse(crate::build_info::current().package_version).map_err(|_| Error::Metadata)
}
pub fn compiled_keys() -> Result<Vec<TrustKey>> {
    parse_keys(option_env!("NOH_UPDATE_TRUST_JSON").unwrap_or("[]"))
}
fn parse_keys(value: &str) -> Result<Vec<TrustKey>> {
    if value.len() > 16384 {
        return Err(Error::Metadata);
    }
    let keys: Vec<TrustKey> = serde_json::from_str(value).map_err(|_| Error::Metadata)?;
    validate_keys(&keys)?;
    Ok(keys)
}
pub fn validate_keys(keys: &[TrustKey]) -> Result<()> {
    if keys.is_empty() || keys.len() > 8 {
        return Err(Error::Signature);
    }
    let mut ids = HashSet::new();
    for key in keys {
        if key.id.is_empty()
            || key.id.len() > 64
            || !key
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            || !ids.insert(&key.id)
        {
            return Err(Error::Metadata);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn update_version_uses_semver_package_field_instead_of_provenance_label() {
        assert!(Version::parse(crate::build_info::VERSION).is_err());
        assert_eq!(
            compiled_version().unwrap().to_string(),
            env!("CARGO_PKG_VERSION")
        );
    }
    #[test]
    fn empty_or_ambiguous_trust_configuration_fails_closed() {
        assert!(parse_keys("[]").is_err());
        let key = TrustKey {
            id: "fixture".into(),
            public_key: [1; 32],
        };
        let single = serde_json::to_string(&vec![&key]).unwrap();
        assert!(parse_keys(&single).is_ok());
        assert!(parse_keys(&serde_json::to_string(&vec![&key, &key]).unwrap()).is_err());
    }
}
