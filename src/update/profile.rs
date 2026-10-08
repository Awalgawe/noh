//! Installation content identity, independent of release channel and executable build.
use super::*;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Minimal,
    Standard,
    #[default]
    Complete,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Complete => "complete",
        }
    }

    pub fn is_complete(&self) -> bool {
        *self == Self::Complete
    }

    /// Old bundles and installation anchors always contained the complete runtime.
    pub fn from_manifest(manifest: &serde_json::Value) -> Result<Self> {
        match manifest.get("distribution_profile") {
            None => Ok(Self::Complete),
            Some(value) => serde_json::from_value(value.clone()).map_err(|_| Error::Metadata),
        }
    }

    /// Keep the existing complete feed URL. Other profiles have explicit sibling
    /// feeds; never guess an API asset ID or reinterpret a different filename.
    pub fn feed_url(self, complete_url: &str) -> Result<String> {
        let mut url = validate_url(complete_url)?;
        if url.query().is_some() {
            return Err(Error::Url);
        }
        if self != Self::Complete {
            let prefix = url
                .path()
                .strip_suffix("/envelope.json")
                .ok_or(Error::Url)?;
            url.set_path(&format!("{prefix}/envelope-{}.json", self.as_str()));
        }
        Ok(url.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_content_is_complete_and_unknown_profiles_are_refused() {
        assert_eq!(
            Profile::from_manifest(&serde_json::json!({})).unwrap(),
            Profile::Complete
        );
        for invalid in [
            serde_json::json!(null),
            serde_json::json!("lite"),
            serde_json::json!(3),
        ] {
            assert!(
                Profile::from_manifest(&serde_json::json!({"distribution_profile": invalid}))
                    .is_err()
            );
        }
    }

    #[test]
    fn feeds_preserve_repository_channel_and_explicit_profile() {
        let base = "https://github.com/example/noh/releases/download/beta/envelope.json";
        assert_eq!(Profile::Complete.feed_url(base).unwrap(), base);
        assert_eq!(
            Profile::Standard.feed_url(base).unwrap(),
            "https://github.com/example/noh/releases/download/beta/envelope-standard.json"
        );
        assert_eq!(
            Profile::Minimal.feed_url(base).unwrap(),
            "https://github.com/example/noh/releases/download/beta/envelope-minimal.json"
        );
        assert!(
            Profile::Standard
                .feed_url("https://api.github.com/repos/example/noh/releases/assets/42")
                .is_err()
        );
        assert!(
            Profile::Complete
                .feed_url(&format!("{base}?token=secret"))
                .is_err()
        );
    }
}
