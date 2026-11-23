use super::*;
use std::path::Path;

const BASE: &str = DOWNLOADS;
const NEVER: &dyn Fn() -> bool = &|| false;

fn digest(bytes: &[u8]) -> String {
    sha256_reader(bytes, NEVER).unwrap()
}

fn release_json(tag: &str, assets: serde_json::Value) -> String {
    serde_json::json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": false,
        // Never used: the link is built from the version.
        "html_url": "https://example.com/elsewhere",
        "assets": assets,
    })
    .to_string()
}

fn asset(version: &str, name: &str, size: u64) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "size": size,
        "digest": format!("sha256:{}", "a".repeat(64)),
        "browser_download_url": format!("{BASE}v{version}/{name}"),
    })
}

fn good_assets(version: &str) -> serde_json::Value {
    serde_json::json!([
        asset(
            version,
            &format!("NOH-{version}-windows-x64-Minimal-Setup.exe"),
            51_238_435
        ),
        asset(version, "COMPONENTS.json", 204_812),
        asset(
            version,
            &format!("NOH-{version}-windows-x64.zip"),
            1_327_192_559
        ),
    ])
}

#[test]
fn versions_are_three_plain_numbers() {
    assert_eq!(Version::parse("0.2.0"), Some(Version(0, 2, 0)));
    assert_eq!(Version::from_tag("v10.20.30"), Some(Version(10, 20, 30)));
    for bad in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "1.2.3-pre.1",
        "1.2.3+build",
        " 1.2.3",
        "1.02.3",
        "1..3",
        "v1.2.3",
        "1.2.x",
        "1234567890.0.0",
    ] {
        assert_eq!(Version::parse(bad), None, "{bad:?}");
    }
    assert_eq!(Version::from_tag("1.2.3"), None);
    assert_eq!(Version::from_tag("v0.1.6-pre.1"), None);
    assert!(Version(0, 10, 0) > Version(0, 9, 9));
    assert!(Version(1, 0, 0) > Version(0, 99, 99));
    assert_eq!(Version(0, 2, 0).to_string(), "0.2.0");
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_newer_promoted_release_names_its_installer_and_catalog() {
    let json = release_json("v0.3.0", good_assets("0.3.0"));
    let release = parse_release(&json, Version(0, 2, 0), BASE)
        .unwrap()
        .unwrap();
    assert_eq!(release.version, Version(0, 3, 0));
    assert_eq!(
        release.setup.name,
        "NOH-0.3.0-windows-x64-Minimal-Setup.exe"
    );
    assert_eq!(
        release.setup.url,
        "https://github.com/Awalgawe/noh/releases/download/v0.3.0/NOH-0.3.0-windows-x64-Minimal-Setup.exe"
    );
    assert_eq!(release.setup.size, 51_238_435);
    assert_eq!(release.setup.sha256, "a".repeat(64));
    assert_eq!(release.catalog.name, "COMPONENTS.json");
    assert_eq!(
        release.page,
        "https://github.com/Awalgawe/noh/releases/tag/v0.3.0"
    );
}

#[test]
fn the_same_older_or_unofficial_releases_offer_nothing() {
    let current = Version(0, 2, 0);
    for tag in ["v0.2.0", "v0.1.9", "v0.3.0-pre.1", "0.3.0", "nightly"] {
        let json = release_json(tag, good_assets("0.3.0"));
        assert_eq!(parse_release(&json, current, BASE).unwrap(), None, "{tag}");
    }
    for flag in ["draft", "prerelease"] {
        let mut value: serde_json::Value =
            serde_json::from_str(&release_json("v0.3.0", good_assets("0.3.0"))).unwrap();
        value[flag] = true.into();
        assert_eq!(
            parse_release(&value.to_string(), current, BASE).unwrap(),
            None,
            "{flag}"
        );
    }
}

#[test]
fn a_release_without_trustworthy_assets_is_refused() {
    let current = Version(0, 2, 0);
    let setup = "NOH-0.3.0-windows-x64-Minimal-Setup.exe";
    type Expected = fn(&Error) -> bool;
    let cases: Vec<(serde_json::Value, Expected)> = vec![
        // The installer is missing.
        (
            serde_json::json!([asset("0.3.0", "COMPONENTS.json", 10)]),
            |e| matches!(e, Error::Unsupported(_)),
        ),
        // Served from another release.
        (
            serde_json::json!([
                {"name": setup, "size": 10, "digest": format!("sha256:{}", "a".repeat(64)),
                 "browser_download_url": format!("{BASE}v0.2.0/{setup}")},
                asset("0.3.0", "COMPONENTS.json", 10),
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
        // Served from elsewhere.
        (
            serde_json::json!([
                {"name": setup, "size": 10, "digest": format!("sha256:{}", "a".repeat(64)),
                 "browser_download_url": format!("https://example.com/v0.3.0/{setup}")},
                asset("0.3.0", "COMPONENTS.json", 10),
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
        // No digest.
        (
            serde_json::json!([
                {"name": setup, "size": 10, "browser_download_url": format!("{BASE}v0.3.0/{setup}")},
                asset("0.3.0", "COMPONENTS.json", 10),
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
        // Uppercase or short digest.
        (
            serde_json::json!([
                {"name": setup, "size": 10, "digest": format!("sha256:{}", "A".repeat(64)),
                 "browser_download_url": format!("{BASE}v0.3.0/{setup}")},
                asset("0.3.0", "COMPONENTS.json", 10),
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
        // Empty.
        (
            serde_json::json!([
                asset("0.3.0", setup, 0),
                asset("0.3.0", "COMPONENTS.json", 10)
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
        // Twice.
        (
            serde_json::json!([
                asset("0.3.0", setup, 10),
                asset("0.3.0", setup, 10),
                asset("0.3.0", "COMPONENTS.json", 10),
            ]),
            |e| matches!(e, Error::Integrity(_)),
        ),
    ];
    for (index, (assets, expected)) in cases.into_iter().enumerate() {
        let error = parse_release(&release_json("v0.3.0", assets), current, BASE).unwrap_err();
        assert!(expected(&error), "case {index}: {error}");
    }
    assert!(matches!(
        parse_release("not json", current, BASE),
        Err(Error::Network(_))
    ));
}

#[test]
fn profiles_follow_the_installer() {
    assert_eq!(Profile::from_installer(None), Some(Profile::Complete));
    assert_eq!(
        Profile::from_installer(Some("minimal")),
        Some(Profile::Minimal)
    );
    assert_eq!(
        Profile::from_installer(Some("standard")),
        Some(Profile::Standard)
    );
    assert_eq!(
        Profile::from_installer(Some("complete")),
        Some(Profile::Complete)
    );
    assert_eq!(Profile::from_installer(Some("Complete")), None);
    assert_eq!(Profile::from_installer(Some("")), None);
    assert!(Profile::Minimal.groups().is_empty());
    assert_eq!(Profile::Standard.groups(), ["media"]);
    assert_eq!(Profile::Complete.groups(), ["media", "speech"]);
}

/// A catalog describing one file per group, and an installed folder holding
/// `media` exactly and `speech` with the given content.
fn installed_copy(folder: &Path, speech: &[u8]) -> String {
    let media = b"media tool";
    let new_speech = b"new speech model";
    std::fs::create_dir_all(folder.join("bin/speech")).unwrap();
    std::fs::write(folder.join("bin/ffmpeg.exe"), media).unwrap();
    std::fs::write(folder.join("bin/speech/model.bin"), speech).unwrap();
    let notice = serde_json::json!({"sha256": "3".repeat(64), "size": 500});
    let media_file = serde_json::json!({"sha256": digest(media), "size": media.len()});
    let speech_file = serde_json::json!({"sha256": digest(new_speech), "size": new_speech.len()});
    serde_json::json!({
        "schema_version": 1,
        "version": "0.3.0",
        "target": "windows-x64",
        "groups": {
            "media": {
                "name": "NOH-windows-x64-media-1111111111111111.zip",
                "size": 100,
                "sha256": "1".repeat(64),
                // Not what the installer counts: `archive_files` is.
                "installed_bytes": 1,
                "files": {"bin/ffmpeg.exe": media_file},
                "archive_files": {"bin/ffmpeg.exe": media_file, "licenses/FFMPEG.txt": notice},
            },
            "speech": {
                "name": "NOH-windows-x64-speech-2222222222222222.zip",
                "size": 200,
                "sha256": "2".repeat(64),
                "installed_bytes": 1,
                "files": {"bin/speech/model.bin": speech_file},
                "archive_files": {"bin/speech/model.bin": speech_file, "licenses/WHISPER.txt": notice},
            },
        },
    })
    .to_string()
}

#[test]
fn only_the_changed_components_of_the_installed_profile_are_downloaded() {
    let folder = tempfile::tempdir().unwrap();
    let catalog = installed_copy(folder.path(), b"old speech model");
    let version = Version(0, 3, 0);
    let plan = |profile| {
        components_to_download(&catalog, version, folder.path(), profile, BASE, NEVER).unwrap()
    };
    assert!(plan(Profile::Minimal).is_empty());
    assert!(plan(Profile::Standard).is_empty());
    let complete = plan(Profile::Complete);
    assert_eq!(complete.len(), 1);
    let speech = &complete[0].archive;
    assert_eq!(speech.name, "NOH-windows-x64-speech-2222222222222222.zip");
    assert_eq!(
        speech.url,
        "https://github.com/Awalgawe/noh/releases/download/v0.3.0/NOH-windows-x64-speech-2222222222222222.zip"
    );
    assert_eq!(
        (speech.size, speech.sha256.as_str()),
        (200, "2".repeat(64).as_str())
    );
    // The model (16 bytes) and its notice (500), as `ComponentInstalledBytes`.
    assert_eq!(complete[0].installed_bytes, 516);
    // The installer's own free-space rule: archives + installed bytes + 64 MiB.
    assert_eq!(installer_space(&complete), 200 + 516 + 64 * 1024 * 1024);
    assert_eq!(installer_space(&[]), 64 * 1024 * 1024);

    // Same size, different bytes: the digest decides.
    std::fs::write(
        folder.path().join("bin/speech/model.bin"),
        b"new speech modeL",
    )
    .unwrap();
    assert_eq!(plan(Profile::Complete).len(), 1);
    std::fs::write(
        folder.path().join("bin/speech/model.bin"),
        b"new speech model",
    )
    .unwrap();
    assert!(plan(Profile::Complete).is_empty());

    // A missing file.
    std::fs::remove_file(folder.path().join("bin/ffmpeg.exe")).unwrap();
    assert_eq!(
        plan(Profile::Standard)[0].archive.name,
        "NOH-windows-x64-media-1111111111111111.zip"
    );
}

#[test]
fn a_catalog_for_another_release_or_with_odd_entries_is_refused() {
    let folder = tempfile::tempdir().unwrap();
    let catalog = installed_copy(folder.path(), b"x");
    let run = |catalog: &str, version| {
        components_to_download(
            catalog,
            version,
            folder.path(),
            Profile::Complete,
            BASE,
            NEVER,
        )
    };
    assert!(matches!(
        run(&catalog, Version(0, 3, 1)),
        Err(Error::Integrity(_))
    ));
    for (field, bad) in [
        ("target", serde_json::json!("linux-x64")),
        ("schema_version", serde_json::json!(2)),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
        value[field] = bad;
        assert!(matches!(
            run(&value.to_string(), Version(0, 3, 0)),
            Err(Error::Integrity(_))
        ));
    }
    // A payload file the archive does not hold.
    let mut value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    value["groups"]["speech"]["archive_files"]
        .as_object_mut()
        .unwrap()
        .remove("bin/speech/model.bin");
    assert!(matches!(
        run(&value.to_string(), Version(0, 3, 0)),
        Err(Error::Integrity(_))
    ));
    for (field, bad) in [
        ("name", serde_json::json!("../NOH-windows-x64-speech-2.zip")),
        ("name", serde_json::json!("NOH-windows-x64-media-2.zip")),
        ("sha256", serde_json::json!("xyz")),
        ("size", serde_json::json!(0)),
        (
            "files",
            serde_json::json!({"../outside.dll": {"sha256": "0".repeat(64), "size": 1}}),
        ),
        (
            "files",
            serde_json::json!({"C:/outside.dll": {"sha256": "0".repeat(64), "size": 1}}),
        ),
        ("files", serde_json::json!({})),
    ] {
        let mut value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
        value["groups"]["speech"][field] = bad.clone();
        assert!(
            matches!(
                run(&value.to_string(), Version(0, 3, 0)),
                Err(Error::Integrity(_))
            ),
            "{field} = {bad}"
        );
    }
    let mut value: serde_json::Value = serde_json::from_str(&catalog).unwrap();
    value["groups"].as_object_mut().unwrap().remove("speech");
    assert!(matches!(
        run(&value.to_string(), Version(0, 3, 0)),
        Err(Error::Integrity(_))
    ));
}

#[test]
fn hashing_stops_when_asked() {
    assert!(matches!(
        sha256_reader(&b"abc"[..], &|| true),
        Err(Error::Storage(_))
    ));
    assert_eq!(
        digest(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn preferences_default_to_checking_and_installing_on_exit() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("noh/updates.json");
    assert_eq!(Preferences::read(&path), Preferences::default());
    assert!(Preferences::default().check && Preferences::default().install_on_exit);
    let choice = Preferences {
        check: false,
        install_on_exit: true,
    };
    choice.save(&path).unwrap();
    assert_eq!(Preferences::read(&path), choice);
    std::fs::write(&path, "{\"check\": false}").unwrap();
    assert!(Preferences::read(&path).install_on_exit);
    std::fs::write(&path, "garbage").unwrap();
    assert_eq!(Preferences::read(&path), Preferences::default());
}

#[test]
fn a_check_is_due_every_four_hours() {
    let state = State {
        last_check: 1_000_000,
        ..State::default()
    };
    assert!(!state.check_due(1_000_000));
    assert!(!state.check_due(1_000_000 + 4 * 3600 - 1));
    assert!(state.check_due(1_000_000 + 4 * 3600));
    // A clock set back does not silence checks for ever.
    assert!(state.check_due(999_999));
    assert!(State::default().check_due(1));
}

#[test]
fn the_next_start_tells_whether_the_installation_happened() {
    let folder = tempfile::tempdir().unwrap();
    let state = |version: &str| State {
        last_check: 0,
        attempt: Some(Attempt {
            version: version.into(),
            servers_stopped: true,
        }),
        not_started: Some("noh-cli.exe".into()),
    };
    let current = Version(0, 3, 0);
    assert_eq!(
        state("0.3.0").outcome(current),
        Some(Outcome::Updated(current))
    );
    assert_eq!(
        state("0.3.0").outcome(Version(0, 2, 0)),
        Some(Outcome::Failed(current))
    );
    assert_eq!(
        state("0.2.0").outcome(current),
        Some(Outcome::Superseded(Version(0, 2, 0)))
    );
    // Folders are derived, never read from the state file.
    assert_eq!(
        version_folder(folder.path(), current),
        folder.path().join("0.3.0")
    );
    assert_eq!(state("../../x").outcome(current), None);
    assert_eq!(state("garbage").outcome(current), None);
    assert_eq!(State::default().outcome(current), None);

    let saved = state("0.3.0");
    saved.save(folder.path()).unwrap();
    assert_eq!(State::read(folder.path()), saved);
    assert!(!folder.path().join("state.json.partial").exists());
    std::fs::write(folder.path().join("state.json"), "garbage").unwrap();
    assert_eq!(State::read(folder.path()), State::default());
}

#[test]
fn the_installer_runs_silently_without_closing_other_noh_processes() {
    let installed = Path::new(r"C:\Users\someone\AppData\Local\Programs\NOH");
    let log = Path::new(r"C:\Users\some one\AppData\Local\noh\updates\0.3.0\install.log");
    let arguments = setup_arguments(installed, log, 4242, true);
    assert_eq!(
        arguments,
        [
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
            "/CURRENTUSER",
            "/NOCLOSEAPPLICATIONS",
            "/PROFILE=minimal",
            r"/DIR=C:\Users\someone\AppData\Local\Programs\NOH",
            r"/LOG=C:\Users\some one\AppData\Local\noh\updates\0.3.0\install.log",
            "/WAITPID=4242",
            "/RELAUNCH",
        ]
    );
    // Closing NOH installs without reopening it.
    let at_exit = setup_arguments(installed, log, 4242, false);
    assert_eq!(at_exit[..], arguments[..arguments.len() - 1]);
    // InstallLocation ends with a separator; the argument never does.
    let registry = Path::new(r"C:\Users\someone\AppData\Local\Programs\NOH\");
    assert_eq!(setup_arguments(registry, log, 4242, true), arguments);
}

#[test]
fn what_holds_the_folder_is_grouped_by_the_program_that_started_it() {
    let p = |pid, parent, name: &str| Process {
        pid,
        parent,
        name: name.into(),
    };
    let own = 1;
    let processes = [
        // Two MCP servers started by an assistant (pid 900, outside the
        // folder); one runs a job: its engine worker and that worker's FFmpeg.
        p(10, 900, "noh-mcp.exe"),
        p(11, 900, "noh-mcp.exe"),
        p(12, 11, "noh-mcp.exe"),
        p(13, 12, "ffmpeg.exe"),
        // A CLI export and its FFmpeg.
        p(20, 901, "noh-cli.exe"),
        p(21, 20, "ffmpeg.exe"),
        // Another NOH window, this NOH's own preview FFmpeg, a lone whisper-cli.
        p(30, 902, "noh.exe"),
        p(40, own, "ffmpeg.exe"),
        p(50, 903, "whisper-cli.exe"),
        p(51, 903, "whisper-cli.exe"),
        // Reused process numbers pointing at each other: each counts.
        p(60, 61, "ffprobe.exe"),
        p(61, 60, "ffprobe.exe"),
    ];
    assert_eq!(
        holders(&processes, own),
        [
            Holder::Servers {
                count: 2,
                working: 1,
            },
            Holder::Commands(1),
            Holder::Windows(1),
            Holder::Own(1),
            Holder::Program {
                name: "ffprobe.exe".into(),
                count: 2,
            },
            Holder::Program {
                name: "whisper-cli.exe".into(),
                count: 2,
            },
        ]
    );
    assert!(holders(&[], own).is_empty());
    // An ordinary window exporting: its worker is noh.exe too, and one window.
    assert_eq!(
        holders(&[p(80, 900, "noh.exe"), p(81, 80, "noh.exe")], own),
        [Holder::Windows(1)]
    );
    // A server with only the windows it opened for its agent (noh_app_open)
    // is idle: the windows count as windows, with their own FFmpeg.
    let driven = [
        p(70, 900, "noh-mcp.exe"),
        p(71, 70, "noh.exe"),
        p(72, 70, "noh.exe"),
        p(73, 72, "ffmpeg.exe"),
        // A driven window's own export worker belongs to it.
        p(74, 72, "noh.exe"),
    ];
    assert_eq!(
        holders(&driven, own),
        [
            Holder::Servers {
                count: 1,
                working: 0,
            },
            Holder::Windows(2),
        ]
    );
}

#[test]
fn a_state_from_before_servers_were_stopped_still_reads() {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("state.json"),
        r#"{"last_check":5,"attempt":{"version":"0.3.1"},"not_started":null}"#,
    )
    .unwrap();
    let attempt = State::read(folder.path()).attempt.unwrap();
    assert_eq!(attempt.version, "0.3.1");
    assert!(!attempt.servers_stopped);
}

#[test]
fn servers_are_asked_to_stop_only_when_they_alone_hold_the_folder() {
    let servers = Holder::Servers {
        count: 2,
        working: 0,
    };
    assert!(only_servers(std::slice::from_ref(&servers)));
    assert!(!only_servers(&[]));
    assert!(!only_servers(&[servers, Holder::Commands(1)]));
}
