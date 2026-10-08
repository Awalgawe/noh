use super::*;
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::io::Cursor;

fn artifact(os: Os, bytes: &[u8]) -> Artifact {
    Artifact {
        target: Target {
            os,
            arch: Architecture::X64,
        },
        kind: PackageKind::Full,
        setup: None,
        base_version: None,
        file_name: format!("noh-{os:?}.nupkg"),
        url: "https://github.com/example/noh/releases/download/v2/noh.nupkg".into(),
        size: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(bytes)),
    }
}
pub(super) fn release() -> Release {
    Release {
        schema_version: 1,
        profile: Profile::Complete,
        package_id: "NOH".into(),
        version: Version::new(2, 0, 0),
        channel: Channel::Stable,
        notes: "Fixture".into(),
        artifacts: [Os::Windows, Os::Macos, Os::Linux]
            .into_iter()
            .map(|os| artifact(os, b"package"))
            .collect(),
    }
}
pub(super) fn signed(release: &Release) -> (Vec<u8>, Vec<TrustKey>) {
    let key = Ed25519KeyPair::from_seed_unchecked(&[7; 32]).unwrap();
    let payload = serde_json::to_vec(release).unwrap();
    let envelope = Envelope {
        key_id: "fixture".into(),
        payload: STANDARD.encode(&payload),
        signature: STANDARD.encode(key.sign(&payload).as_ref()),
    };
    (
        serde_json::to_vec(&envelope).unwrap(),
        vec![TrustKey {
            id: "fixture".into(),
            public_key: key.public_key().as_ref().try_into().unwrap(),
        }],
    )
}
fn verify(release: &Release) -> Result<VerifiedRelease> {
    let (bytes, keys) = signed(release);
    VerifiedRelease::verify(&bytes, &keys, "NOH")
}

#[test]
fn signed_selection_is_bound_to_target_channel_and_newer_version() {
    let verified = verify(&release()).unwrap();
    assert!(!verified.is_setup());
    for os in [Os::Windows, Os::Macos, Os::Linux] {
        let target = Target {
            os,
            arch: Architecture::X64,
        };
        assert!(
            verified
                .select(target, Channel::Stable, &Version::new(1, 0, 0))
                .unwrap()
                .is_some()
        );
        assert!(
            verified
                .select(target, Channel::Stable, &Version::new(2, 0, 0))
                .unwrap()
                .is_none()
        );
        assert!(
            verified
                .select(target, Channel::Stable, &Version::new(3, 0, 0))
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            verified.select(target, Channel::Beta, &Version::new(1, 0, 0)),
            Err(Error::Target)
        ));
        assert!(matches!(
            verified.select(
                Target {
                    arch: Architecture::Arm64,
                    ..target
                },
                Channel::Stable,
                &Version::new(1, 0, 0)
            ),
            Err(Error::Target)
        ));
    }
}

#[test]
fn altered_payload_and_unknown_key_are_rejected() {
    let (bytes, mut keys) = signed(&release());
    let mut envelope: Envelope = serde_json::from_slice(&bytes).unwrap();
    envelope.payload = STANDARD.encode(b"forged");
    assert!(matches!(
        VerifiedRelease::verify(&serde_json::to_vec(&envelope).unwrap(), &keys, "NOH"),
        Err(Error::Signature)
    ));
    keys[0].id = "other".into();
    assert!(matches!(
        VerifiedRelease::verify(&bytes, &keys, "NOH"),
        Err(Error::Signature)
    ));
    assert!(matches!(
        VerifiedRelease::verify(&bytes, &signed(&release()).1, "other"),
        Err(Error::Metadata)
    ));
}

#[test]
fn signed_but_unsafe_metadata_is_rejected() {
    for mutation in 0..7 {
        let mut value = release();
        match mutation {
            0 => value.artifacts.push(value.artifacts[0].clone()),
            1 => value.artifacts[0].file_name = "../escape.nupkg".into(),
            2 => value.artifacts[0].size = MAX_PACKAGE + 1,
            3 => value.artifacts[0].sha256 = "0".repeat(63),
            4 => value.artifacts[0].url = "https://attacker.example/a".into(),
            5 => value.version = Version::parse("2.0.0-beta.1").unwrap(),
            _ => value.schema_version = 2,
        }
        assert!(verify(&value).is_err(), "mutation {mutation}");
    }
}

#[test]
fn download_is_streamed_verified_and_never_overwrites() {
    let bytes = vec![42; 100_000];
    let artifact = artifact(Os::Windows, &bytes);
    let folder = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let mut progress = Vec::new();
    let path = stage_reader(
        &mut Cursor::new(&bytes),
        &artifact,
        folder.path(),
        &cancel,
        |done, total| progress.push((done, total)),
    )
    .unwrap();
    assert!(progress.len() >= 4);
    assert_eq!(progress.last(), Some(&(100_000, 100_000)));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    std::fs::write(&path, b"tampered existing package").unwrap();
    assert!(
        stage_reader(
            &mut Cursor::new(&bytes),
            &artifact,
            folder.path(),
            &cancel,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"tampered existing package");
    std::fs::write(&path, &bytes).unwrap();
    assert!(
        stage_reader(
            &mut Cursor::new(&bytes),
            &artifact,
            folder.path(),
            &cancel,
            |_, _| {}
        )
        .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let mut file = File::open(path).unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    verify_package(&mut file, &artifact).unwrap();
}

#[test]
fn failed_or_cancelled_download_leaves_no_package() {
    let artifact = artifact(Os::Windows, b"package");
    for input in [b"corrupt".as_slice(), b"short", b"package extra"] {
        let folder = tempfile::tempdir().unwrap();
        assert!(matches!(
            stage_reader(
                &mut Cursor::new(input),
                &artifact,
                folder.path(),
                &AtomicBool::new(false),
                |_, _| {}
            ),
            Err(Error::Integrity)
        ));
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
    }
    let folder = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(true);
    assert!(matches!(
        stage_reader(
            &mut Cursor::new(b"package"),
            &artifact,
            folder.path(),
            &cancel,
            |_, _| {}
        ),
        Err(Error::Cancelled)
    ));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[test]
fn url_policy_rejects_credentials_and_untrusted_redirect_destinations() {
    for value in [
        "http://github.com/a",
        "https://github.com.evil.example/a",
        "https://token@api.github.com/a",
        "https://github.com:444/a",
        "https://github.com/a#fragment",
        "file:///a",
    ] {
        assert!(matches!(validate_url(value), Err(Error::Url)));
    }
    assert!(
        validate_url("https://release-assets.githubusercontent.com/a?signature=opaque").is_ok()
    );
}

#[test]
fn build_metadata_does_not_authorize_lateral_updates_or_deltas() {
    let mut value = release();
    value.version = Version::parse("2.0.0+build.2").unwrap();
    let verified = verify(&value).unwrap();
    assert!(
        verified
            .select(
                value.artifacts[0].target,
                Channel::Stable,
                &Version::parse("2.0.0+build.1").unwrap()
            )
            .unwrap()
            .is_none()
    );
    value.artifacts[0].kind = PackageKind::Delta;
    value.artifacts[0].base_version = Some(Version::parse("2.0.0+build.1").unwrap());
    assert!(matches!(verify(&value), Err(Error::Metadata)));
}

#[test]
fn redirects_remove_api_authorization_and_reject_untrusted_destinations() {
    let transport = GithubTransport::new(Some("secret".into())).unwrap();
    let api = validate_url("https://api.github.com/repos/example/noh/releases/assets/1").unwrap();
    assert_eq!(token_for(&api, Some("secret")), Some("secret"));
    assert_eq!(
        transport
            .build_request(&api)
            .build()
            .unwrap()
            .headers()
            .get(reqwest::header::AUTHORIZATION)
            .unwrap(),
        "Bearer secret"
    );
    let cdn = redirect_url(
        &api,
        "https://release-assets.githubusercontent.com/asset?opaque=1",
    )
    .unwrap();
    assert_eq!(token_for(&cdn, Some("secret")), None);
    assert!(
        !transport
            .build_request(&cdn)
            .build()
            .unwrap()
            .headers()
            .contains_key(reqwest::header::AUTHORIZATION)
    );
    assert!(redirect_url(&cdn, "https://evil.example/steal").is_err());
    assert!(redirect_url(&cdn, "http://api.github.com/steal").is_err());
}

#[test]
fn cancellation_during_transfer_removes_temporary_file() {
    let bytes = vec![42; 100_000];
    let artifact = artifact(Os::Windows, &bytes);
    let folder = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let result = stage_reader(
        &mut Cursor::new(bytes),
        &artifact,
        folder.path(),
        &cancel,
        |_, _| cancel.store(true, Ordering::Relaxed),
    );
    assert!(matches!(result, Err(Error::Cancelled)));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn cancellation_drops_a_stalled_network_operation() {
    let cancelled = AtomicBool::new(false);
    let operation = cancellable(
        std::future::pending::<std::result::Result<(), reqwest::Error>>(),
        &cancelled,
    );
    let trigger = async {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancelled.store(true, Ordering::Relaxed);
    };
    let (result, _) = tokio::time::timeout(Duration::from_secs(1), async {
        tokio::join!(operation, trigger)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::Cancelled)));
}

#[test]
fn rate_limits_are_distinct_from_authentication_errors() {
    let mut headers = reqwest::header::HeaderMap::new();
    assert_eq!(retry_delay(401, &headers), None);
    assert_eq!(retry_delay(403, &headers), None);
    assert_eq!(retry_delay(429, &headers), Some(60));
    headers.insert("retry-after", "120".parse().unwrap());
    assert_eq!(retry_delay(403, &headers), Some(120));
    headers.insert("retry-after", "9999999".parse().unwrap());
    assert_eq!(retry_delay(429, &headers), Some(86400));
}

#[test]
fn parsing_platform_metadata_does_not_qualify_native_installation() {
    for os in [Os::Windows, Os::Macos, Os::Linux] {
        for arch in [Architecture::X64, Architecture::Arm64] {
            assert!(!Target { os, arch }.installation_qualified());
        }
    }
}

#[test]
fn key_rotation_accepts_only_explicitly_provisioned_keys() {
    let (bytes, mut keys) = signed(&release());
    keys.insert(
        0,
        TrustKey {
            id: "replacement".into(),
            public_key: [1; 32],
        },
    );
    assert!(VerifiedRelease::verify(&bytes, &keys, "NOH").is_ok());
    assert_eq!(
        VerifiedRelease::verify(&bytes, &keys, "NOH")
            .unwrap()
            .envelope(),
        bytes
    );
    keys.retain(|key| key.id != "fixture");
    assert!(matches!(
        VerifiedRelease::verify(&bytes, &keys, "NOH"),
        Err(Error::Signature)
    ));
}

#[test]
fn private_installer_mirrors_are_credential_free_and_repository_bound() {
    let signed = "https://github.com/example/noh/releases/download/v1/NOH-Setup.exe";
    let api = "https://api.github.com/repos/example/noh/releases/assets/42";
    assert_eq!(setup_download_source(signed, Some(api)).unwrap(), api);
    assert_eq!(setup_download_source(signed, None).unwrap(), signed);
    for invalid in [
        "https://api.github.com/repos/other/noh/releases/assets/42",
        "https://api.github.com/repos/example/noh/releases/assets/42?token=secret",
        "https://api.github.com/repos/example/noh/releases/assets/42/extra",
        "https://api.github.com/repos/example/noh/releases/assets/",
        "https://github.com/repos/example/noh/releases/assets/42",
        "http://api.github.com/repos/example/noh/releases/assets/42",
    ] {
        assert!(setup_download_source(signed, Some(invalid)).is_err());
    }
    assert!(
        setup_download_source("https://github.com/example/noh/not-a-release", Some(api)).is_err()
    );
}
