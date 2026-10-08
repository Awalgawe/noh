//! Read-only wizard planning. Installation still belongs to PreparedSetup.
use super::{windows::ProtectedFile, windows_anchor::InstallationAnchor, *};

#[derive(Serialize)]
pub struct Plan {
    pub initialize: bool,
    pub profile: Option<Profile>,
    pub latest_version: Option<Version>,
}

pub fn inspect(
    base: &Path,
    version: &Version,
    keys: &[TrustKey],
    package_id: &str,
) -> Result<Plan> {
    if windows_guard::is_elevated()? {
        return Err(Error::InstallationUnavailable);
    }
    trust::validate_keys(keys)?;
    if !base.try_exists()? {
        let _parent = ProtectedFile::directory(base.parent().ok_or(Error::Metadata)?)?;
        return Ok(Plan {
            initialize: true,
            profile: None,
            latest_version: None,
        });
    }
    let _base = ProtectedFile::directory(base)?;
    if !base.join(".noh-update").try_exists()? {
        if base.join("application").try_exists()? {
            return Err(Error::InstallationUnavailable);
        }
        return Ok(Plan {
            initialize: true,
            profile: None,
            latest_version: None,
        });
    }
    let anchor = InstallationAnchor::open(base, package_id, Some(Channel::Stable))?;
    let latest = require_no_downgrade(&anchor, version, keys, package_id)?;
    Ok(Plan {
        initialize: false,
        profile: Some(anchor.profile()),
        latest_version: latest,
    })
}

/// Retained signed releases survive loss of the whole application root. Conservatively
/// refuse an older wizard even if a newer authenticated installation was interrupted.
pub(crate) fn require_no_downgrade(
    anchor: &InstallationAnchor,
    version: &Version,
    keys: &[TrustKey],
    package_id: &str,
) -> Result<Option<Version>> {
    let mut latest: Option<Version> = None;
    let archive = anchor.coordination().join("retained");
    match std::fs::read_dir(&archive) {
        Ok(entries) => {
            for (index, entry) in entries.enumerate() {
                if index >= 128 {
                    return Err(Error::Metadata);
                }
                let path = entry?.path();
                let candidate = (|| -> Result<Version> {
                    let mut envelope = ProtectedFile::open(&path.join("envelope.json"))?;
                    let mut bytes = Vec::new();
                    envelope
                        .file()
                        .take(MAX_ENVELOPE as u64 + 1)
                        .read_to_end(&mut bytes)?;
                    let release = VerifiedRelease::verify(&bytes, keys, package_id)?;
                    release.require_profile(anchor.profile())?;
                    windows_setup::exact_setup(
                        &release,
                        anchor.channel(),
                        &release.release().version,
                    )?;
                    Ok(release.release().version.clone())
                })();
                if let Ok(candidate) = candidate {
                    if latest
                        .as_ref()
                        .is_none_or(|old| candidate.cmp_precedence(old).is_gt())
                    {
                        latest = Some(candidate);
                    }
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if anchor
        .installation()
        .join("current/sq.version")
        .try_exists()?
    {
        let installed = windows_guard::installed_version(&anchor.installation(), package_id)?;
        if latest
            .as_ref()
            .is_none_or(|old| installed.cmp_precedence(old).is_gt())
        {
            latest = Some(installed);
        }
    } else if anchor.installation().try_exists()? && latest.is_none() {
        return Err(Error::InstallationUnavailable);
    }
    if latest
        .as_ref()
        .is_some_and(|old| version.cmp_precedence(old).is_lt())
    {
        return Err(Error::Target);
    }
    Ok(latest)
}
