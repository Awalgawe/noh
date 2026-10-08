//! Read-only fast lifecycle acknowledgements. Never run startup cache/apply code.
use std::ffi::OsString;

/// Called before acquiring the normal managed runtime lease: the installer owns
/// its exclusive lease while invoking these short-lived, side-effect-free hooks.
pub fn hook_exit(arguments: impl IntoIterator<Item = OsString>) -> Option<i32> {
    let mut arguments = arguments.into_iter();
    arguments.next();
    let hook = arguments.next()?;
    let hook = hook.to_str()?;
    if !hook.starts_with("--veloapp-") {
        return None;
    }
    if !matches!(
        hook,
        "--veloapp-install" | "--veloapp-updated" | "--veloapp-obsolete" | "--veloapp-uninstall"
    ) {
        return Some(2);
    }
    let version = arguments.next();
    let valid = version
        .as_ref()
        .and_then(|v| v.to_str())
        .and_then(|v| semver::Version::parse(v).ok())
        .is_some_and(|v| v.to_string() == crate::build_info::current().package_version);
    Some(if valid && arguments.next().is_none() {
        0
    } else {
        2
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(args: &[&str]) -> Option<i32> {
        hook_exit(args.iter().map(OsString::from))
    }
    #[test]
    fn only_exact_read_only_hooks_for_this_binary_version_are_acknowledged() {
        let version = crate::build_info::current().package_version;
        for hook in [
            "--veloapp-install",
            "--veloapp-updated",
            "--veloapp-obsolete",
            "--veloapp-uninstall",
        ] {
            assert_eq!(check(&["noh", hook, version]), Some(0));
            assert_eq!(check(&["noh", hook, "999.0.0"]), Some(2));
            assert_eq!(check(&["noh", hook, version, "--worker"]), Some(2));
            assert_eq!(check(&["noh", hook]), Some(2));
        }
        assert_eq!(check(&["noh", "--veloapp-unknown", version]), Some(2));
        assert_eq!(check(&["noh", "--worker"]), None);
        assert_eq!(check(&["noh", "--build-info"]), None);
        assert_eq!(check(&["noh"]), None);
    }
}
