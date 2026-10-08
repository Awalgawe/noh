//! Stable coordination outside the tree replaced by Setup. Not access control:
//! executable trust still comes from the controller and its compiled public keys.
use super::{windows::ProtectedFile, *};

const FORMAT: &str = "noh-setup-installation-v1";
const DIRECTORY: &str = ".noh-update";
const IDENTITY: &str = "identity.json";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    format: String,
    package_id: String,
    channel: Channel,
    #[serde(default, skip_serializing_if = "Profile::is_complete")]
    profile: Profile,
}

pub struct InstallationAnchor {
    base: PathBuf,
    channel: Channel,
    profile: Profile,
    protection: ProtectedFile,
}

impl InstallationAnchor {
    /// Called only by explicit initialization, never by ordinary client startup.
    /// An existing coordination directory is refused, never overwritten here.
    pub fn create(base: &Path, package_id: &str, channel: Channel) -> Result<Self> {
        Self::create_for_profile(base, package_id, channel, Profile::Complete)
    }

    pub fn create_for_profile(
        base: &Path,
        package_id: &str,
        channel: Channel,
        profile: Profile,
    ) -> Result<Self> {
        let _base = ProtectedFile::directory(base)?;
        let directory = base.join(DIRECTORY);
        std::fs::create_dir(&directory)?;
        let _directory = ProtectedFile::directory(&directory)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(directory.join(IDENTITY))?;
        let identity = Identity {
            format: FORMAT.into(),
            package_id: package_id.into(),
            channel,
            profile,
        };
        file.write_all(&serde_json::to_vec(&identity).map_err(|_| Error::Metadata)?)?;
        file.sync_all()?;
        drop(file);
        Self::open(base, package_id, Some(channel))
    }

    pub fn open(base: &Path, package_id: &str, channel: Option<Channel>) -> Result<Self> {
        // Open the supplied spelling first: canonicalization must not hide a junction.
        let mut protection = ProtectedFile::open(&base.join(DIRECTORY).join(IDENTITY))?;
        let mut bytes = Vec::new();
        protection.file().take(4097).read_to_end(&mut bytes)?;
        if bytes.len() > 4096 {
            return Err(Error::Metadata);
        }
        let identity: Identity = serde_json::from_slice(&bytes).map_err(|_| Error::Metadata)?;
        if identity.format != FORMAT
            || identity.package_id != package_id
            || channel.is_some_and(|expected| expected != identity.channel)
        {
            return Err(Error::Target);
        }
        Ok(Self {
            base: std::fs::canonicalize(base)?,
            channel: identity.channel,
            profile: identity.profile,
            protection,
        })
    }

    pub fn base(&self) -> &Path {
        &self.base
    }
    pub fn coordination(&self) -> PathBuf {
        self.base.join(DIRECTORY)
    }
    pub fn installation(&self) -> PathBuf {
        self.base.join("application")
    }
    pub fn channel(&self) -> Channel {
        self.channel
    }
    pub fn profile(&self) -> Profile {
        self.profile
    }
    pub fn protection(&self) -> &ProtectedFile {
        &self.protection
    }
    pub(crate) fn into_protection(self) -> ProtectedFile {
        self.protection
    }

    pub(crate) fn for_executable(executable: &Path, package_id: &str) -> Result<Option<Self>> {
        let directory = executable.parent().ok_or(Error::Metadata)?;
        let current = if directory.file_name() == Some(std::ffi::OsStr::new("bin")) {
            directory.parent().ok_or(Error::Metadata)?
        } else {
            directory
        };
        let Some(base) = current.parent().and_then(Path::parent) else {
            return Ok(None);
        };
        if !base.join(DIRECTORY).try_exists()? {
            return Ok(None);
        }
        let anchor = Self::open(base, package_id, None)?;
        if std::fs::canonicalize(current)? != anchor.installation().join("current") {
            return Err(Error::InstallationUnavailable);
        }
        Ok(Some(anchor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::windows::{ExclusiveLease, RuntimeLease};

    #[test]
    fn same_binary_identity_cannot_hide_a_different_content_profile() {
        for profile in [Profile::Minimal, Profile::Standard, Profile::Complete] {
            let base = tempfile::tempdir().unwrap();
            let anchor = InstallationAnchor::create_for_profile(
                base.path(),
                "NOH",
                Channel::Stable,
                profile,
            )
            .unwrap();
            assert_eq!(
                InstallationAnchor::open(base.path(), "NOH", None)
                    .unwrap()
                    .profile(),
                profile
            );
            let current = anchor.installation().join("current");
            std::fs::create_dir_all(&current).unwrap();
            std::fs::write(current.join("noh.exe"), b"same executable").unwrap();
            std::fs::write(current.join("sq.version"), b"fixture").unwrap();
            let mut manifest = serde_json::json!({"distribution_profile":profile,"build":{"build_fingerprint":"same-build"}});
            std::fs::write(
                current.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(
                RuntimeLease::for_executable(&current.join("noh.exe"), "same-build")
                    .unwrap()
                    .is_some()
            );
            manifest["distribution_profile"] = if profile == Profile::Complete {
                "standard"
            } else {
                "complete"
            }
            .into();
            std::fs::write(
                current.join("manifest.json"),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
            assert!(RuntimeLease::for_executable(&current.join("noh.exe"), "same-build").is_err());
        }
    }

    #[test]
    fn root_rename_and_absence_preserve_one_external_identity_and_lock() {
        let base = tempfile::tempdir().unwrap();
        let anchor = InstallationAnchor::create(base.path(), "NOH", Channel::Stable).unwrap();
        let root = anchor.installation();
        std::fs::create_dir_all(root.join("current/bin")).unwrap();
        let manifest = serde_json::json!({"build":{"build_fingerprint":"generation-a"}});
        std::fs::write(
            root.join("current/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::write(root.join("current/sq.version"), b"fixture").unwrap();
        for relative in ["noh.exe", "bin/noh.exe", "bin/noh-mcp.exe"] {
            let executable = root.join("current").join(relative);
            std::fs::write(&executable, b"fixture").unwrap();
            let client = RuntimeLease::for_executable(&executable, "generation-a")
                .unwrap()
                .unwrap();
            assert!(ExclusiveLease::acquire(&anchor.coordination()).is_err());
            drop(client);
        }
        let exclusive = ExclusiveLease::acquire(&anchor.coordination()).unwrap();
        assert!(
            RuntimeLease::for_executable(&root.join("current/noh.exe"), "generation-a").is_err()
        );
        std::fs::rename(&root, base.path().join("application.saved")).unwrap();
        let reopened = InstallationAnchor::open(base.path(), "NOH", Some(Channel::Stable)).unwrap();
        assert_eq!(reopened.coordination(), anchor.coordination());
        assert!(RuntimeLease::acquire(&reopened.coordination()).is_err());
        assert!(
            RuntimeLease::for_executable(
                &base.path().join("application.saved/current/noh.exe"),
                "generation-a"
            )
            .is_err()
        );
        assert!(std::fs::rename(anchor.coordination(), base.path().join("moved-anchor")).is_err());
        drop(exclusive);
        std::fs::rename(base.path().join("application.saved"), &root).unwrap();
        assert!(
            RuntimeLease::for_executable(&root.join("current/noh.exe"), "generation-b").is_err()
        );
        assert!(InstallationAnchor::open(base.path(), "Other", None).is_err());
        assert!(InstallationAnchor::open(base.path(), "NOH", Some(Channel::Beta)).is_err());
        assert!(InstallationAnchor::create(base.path(), "NOH", Channel::Stable).is_err());
    }
}
