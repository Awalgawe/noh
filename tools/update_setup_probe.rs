//! Synthetic Setup qualification: reuse NOH authentication and lifetime primitives.
//! It never extracts, maps or publishes application files. Not a production adapter.
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
struct Options {
    #[arg(long)]
    case: PathBuf,
    #[arg(long)]
    setup: PathBuf,
    #[arg(long)]
    envelope: PathBuf,
    #[arg(long)]
    keys: PathBuf,
    #[arg(long)]
    package_id: String,
    #[arg(long)]
    attempt: String,
    /// Automated fixture consent; never authorizes an ordinary NOH installation.
    #[arg(long)]
    fixture_consent: bool,
    /// Sign this experiment's explicit Setup contract, never production metadata.
    #[arg(long)]
    sign_fixture_key: Option<PathBuf>,
    /// Test-only pause: an external owner must kill this guardian, not the Setup.
    #[arg(long)]
    wait_for_guardian_loss: bool,
    /// Test-only rendezvous after authentication and protection, before launching Setup.
    #[arg(long)]
    pause_before_engine: bool,
    #[arg(long, default_value = "1.0.0", value_parser = ["1.0.0", "1.0.1"])]
    version: String,
}

#[cfg(windows)]
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SetupFixture {
    format: String,
    release: noh::update::Release,
}

#[cfg(windows)]
fn run(options: Options) -> Result<(), Box<dyn std::error::Error>> {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use noh::update::{
        Channel, Envelope, PackageKind, Target, TrustKey, trust, verify_package,
        windows::{ExclusiveLease, ProtectedFile, SupervisedProcess},
    };
    use ring::signature::{self, Ed25519KeyPair};
    use std::{
        ffi::OsStr,
        fs,
        io::{Read, Write},
        os::windows::fs::MetadataExt,
        path::Path,
        time::{Duration, Instant},
    };

    fn confined(path: &Path, parent: &Path) -> std::io::Result<PathBuf> {
        // Check the supplied spelling before canonicalization can hide a junction.
        for ancestor in path.ancestors() {
            if fs::symlink_metadata(ancestor)?.file_attributes() & 0x400 != 0 {
                return Err(std::io::Error::other("Reparse case path refused"));
            }
        }
        let full = fs::canonicalize(path)?;
        if full == parent || !full.starts_with(parent) {
            return Err(std::io::Error::other("Path escaped disposable case"));
        }
        // Reject reparse ancestors using the same production primitive.
        if full.is_file() {
            let _protected = ProtectedFile::open(&full)?;
        }
        Ok(full)
    }
    fn read(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
        let mut file = ProtectedFile::open(path)?;
        let mut bytes = Vec::new();
        file.file().take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(std::io::Error::other("Metadata too large"));
        }
        Ok(bytes)
    }
    fn consumer_identity(pid: u32) -> std::io::Result<serde_json::Value> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{Security as security, System::Threading as threading};
        let raw =
            unsafe { threading::OpenProcess(threading::PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut name = [0u16; 32768];
        let mut length = name.len() as u32;
        if unsafe {
            threading::QueryFullProcessImageNameW(
                process.as_raw_handle(),
                0,
                name.as_mut_ptr(),
                &mut length,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut raw_token = std::ptr::null_mut();
        if unsafe {
            threading::OpenProcessToken(
                process.as_raw_handle(),
                security::TOKEN_QUERY,
                &mut raw_token,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let token = unsafe { OwnedHandle::from_raw_handle(raw_token) };
        let mut elevation: security::TOKEN_ELEVATION = unsafe { std::mem::zeroed() };
        let mut returned = 0;
        if unsafe {
            security::GetTokenInformation(
                token.as_raw_handle(),
                security::TokenElevation,
                &mut elevation as *mut _ as _,
                std::mem::size_of_val(&elevation) as u32,
                &mut returned,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        Ok(
            serde_json::json!({"pid":pid,"image":String::from_utf16_lossy(&name[..length as usize]),
            "elevated":elevation.TokenIsElevated != 0}),
        )
    }
    let base = fs::canonicalize(
        Path::new(env!("CARGO_MANIFEST_DIR")).join(".mcp-dev/update-setup-probe"),
    )?;
    let case = confined(&options.case, &base)?;
    if options.attempt.is_empty()
        || !options
            .attempt
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("Unsafe attempt name".into());
    }
    let report_path = case.join(format!("{}.json", options.attempt));
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(report_path)?;
    let start = Instant::now();
    let mut report = serde_json::json!({"schema_version":1,"scope":"Setup fixture only",
        "attempt":options.attempt,"status":"failed","setup_started":false,
        "source":noh::build_info::current(),"consumer_pids":[],"fixture_consent":options.fixture_consent});
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        if !options.fixture_consent {
            return Err("Fixture consent missing".into());
        }
        let setup = confined(&options.setup, &case)?;
        let keys_path = confined(&options.keys, &case)?;
        // Official Setup also creates a missing root. Never recreate it in NOH.
        let installation = match fs::symlink_metadata(case.join("installation")) {
            Ok(_) => confined(&case.join("installation"), &case)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => case.join("installation"),
            Err(error) => return Err(error.into()),
        };
        let anchor = confined(&case.join("coordination"), &case)?;
        let fixture = confined(&case.join("fixture.exe"), &case)?;
        let keys: Vec<TrustKey> = serde_json::from_slice(&read(&keys_path, 16384)?)?;
        trust::validate_keys(&keys)?;
        // Production VerifiedRelease intentionally accepts only .nupkg. This
        // explicitly separate contract explores Setup, without changing that gate.
        let envelope_bytes = if let Some(key_path) = &options.sign_fixture_key {
            let key_path = confined(key_path, &case)?;
            let pair = Ed25519KeyPair::from_pkcs8(&read(&key_path, 8192)?)
                .map_err(|_| "Invalid local fixture key")?;
            let manifest = if options.version == "1.0.0" {
                "release.json"
            } else {
                "release-B.json"
            };
            let payload = read(&confined(&case.join(manifest), &case)?, 65536)?;
            serde_json::to_vec(&Envelope {
                key_id: "local-setup-probe".into(),
                payload: STANDARD.encode(&payload),
                signature: STANDARD.encode(pair.sign(&payload).as_ref()),
            })?
        } else {
            read(&confined(&options.envelope, &case)?, 131072)?
        };
        let envelope: Envelope = serde_json::from_slice(&envelope_bytes)?;
        let key = keys
            .iter()
            .find(|key| key.id == envelope.key_id)
            .ok_or("Unknown fixture key")?;
        let payload = STANDARD.decode(envelope.payload)?;
        if payload.len() > 65536 {
            return Err("Fixture payload too large".into());
        }
        signature::UnparsedPublicKey::new(&signature::ED25519, key.public_key)
            .verify(&payload, &STANDARD.decode(envelope.signature)?)
            .map_err(|_| "Invalid fixture signature")?;
        let contract: SetupFixture = serde_json::from_slice(&payload)?;
        let release = &contract.release;
        if contract.format != "noh-setup-probe-v1"
            || release.schema_version != 1
            || release.package_id != options.package_id
            || release.version.to_string() != options.version
            || release.channel != Channel::Stable
            || release.artifacts.len() != 1
        {
            return Err("Unexpected fixture release identity".into());
        }
        let artifact = &release.artifacts[0];
        if artifact.target != Target::native()?
            || artifact.kind != PackageKind::Full
            || artifact.base_version.is_some()
            || !artifact.file_name.ends_with("Setup.exe")
            || !artifact
                .file_name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            || artifact.size == 0
            || artifact.size > 2_147_483_647
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || artifact.url
                != format!(
                    "https://github.com/example/noh/releases/download/fixture/{}",
                    artifact.file_name
                )
            || setup.file_name() != Some(OsStr::new(&artifact.file_name))
        {
            return Err("Wrong fixture artifact".into());
        }
        let mut protected_setup = ProtectedFile::open(&setup)?;
        verify_package(protected_setup.file(), artifact)?;
        report["authenticated_sha256"] = serde_json::json!(artifact.sha256);
        report["authentication_contract"] = serde_json::json!(contract.format);
        if options.sign_fixture_key.is_some() {
            // The output is fixed inside the case and never overwrites an envelope.
            fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(case.join(if options.version == "1.0.0" {
                    "envelope.json"
                } else {
                    "envelope-B.json"
                }))?
                .write_all(&envelope_bytes)?;
            return Ok(());
        }
        // Both clients and the supervisor use this anchor outside the renamed root.
        let _anchor_protection = ProtectedFile::open(&case.join("coordination/identity"))?;
        let exclusive = ExclusiveLease::acquire(&anchor)?.into_protection();
        // Establish causality before a consumer or its working directory exists.
        let prelaunch_write_denied = fs::OpenOptions::new().write(true).open(&setup).is_err();
        let prelaunch_rename_denied = fs::rename(
            setup.parent().unwrap(),
            case.join("unexpected-package-rename"),
        )
        .is_err();
        report["prelaunch_write_denied"] = serde_json::json!(prelaunch_write_denied);
        report["prelaunch_parent_rename_denied"] = serde_json::json!(prelaunch_rename_denied);
        if !prelaunch_write_denied || !prelaunch_rename_denied {
            return Err("Prelaunch protection failed".into());
        }
        let hook = case.join(format!("hook-{}", options.attempt));
        fs::create_dir(&hook)?;
        // This single-threaded probe sets the environment before spawning its consumers.
        unsafe {
            std::env::set_var("NOH_SETUP_PROBE_HOOK", &hook);
        }
        let log = case.join(format!("{}.setup.log", options.attempt));
        let args: Vec<&OsStr> = vec![
            OsStr::new("--silent"),
            OsStr::new("--installto"),
            installation.as_os_str(),
            OsStr::new("--log"),
            log.as_os_str(),
        ];
        if options.pause_before_engine {
            let temporary = hook.join("before-engine.tmp");
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?
                .write_all(&serde_json::to_vec_pretty(&report)?)?;
            fs::rename(temporary, hook.join("before-engine.json"))?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while !hook.join("start-engine").exists() {
                if Instant::now() > deadline {
                    return Err("Pre-engine rendezvous deadline".into());
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let process = SupervisedProcess::spawn_protected(
            &setup,
            &args,
            &case,
            &[&protected_setup, &exclusive, &_anchor_protection],
        )?;
        report["setup_started"] = serde_json::json!(true);
        report["setup_pid"] = serde_json::json!(process.id());
        fs::write(
            hook.join("engine-started.tmp"),
            serde_json::to_vec(&serde_json::json!({"pid":process.id()}))?,
        )?;
        fs::rename(
            hook.join("engine-started.tmp"),
            hook.join("engine-started.json"),
        )?;
        let deadline = Instant::now() + Duration::from_secs(30);
        while !hook.join("ready").is_file() {
            if let Some(exit) = process.wait(Duration::from_millis(20))? {
                report["setup_exit"] = serde_json::json!(exit);
                report["remaining_consumers_before_cleanup"] =
                    serde_json::json!(process.active_processes()?);
                return Err("Setup exited before the observable install hook".into());
            }
            if Instant::now() > deadline {
                return Err("Install-hook deadline".into());
            }
        }
        let hook_pid: u32 = fs::read_to_string(hook.join("ready"))?.parse()?;
        report["consumer_pids"] = serde_json::json!([process.id(), hook_pid]);
        report["active_processes_at_hook"] = serde_json::json!(process.active_processes()?);
        let hook_in_job = process.contains_process(hook_pid)?;
        report["hook_in_job"] = serde_json::json!(hook_in_job);
        if !hook_in_job {
            return Err("Installer hook escaped supervision".into());
        }
        let identities = process
            .process_ids()?
            .into_iter()
            .map(consumer_identity)
            .collect::<std::io::Result<Vec<_>>>()?;
        report["observed_job_processes"] = serde_json::json!(identities);
        if identities.iter().any(|item| item["elevated"] != false) {
            return Err("Elevated consumer observed".into());
        }
        // A client is started while the actual installer consumer is paused in its hook.
        let contender = SupervisedProcess::spawn(&fixture, &[], &case)?;
        let contender_exit = contender.wait(Duration::from_secs(3))?;
        report["concurrent_client_exit"] = serde_json::json!(contender_exit);
        if contender_exit != Some(32) {
            return Err("Client admitted during repair".into());
        }
        drop(contender);
        let mutation_denied = fs::OpenOptions::new().write(true).open(&setup).is_err();
        let parent = setup.parent().unwrap();
        let rename_denied = fs::rename(parent, case.join("unexpected-package-rename")).is_err();
        report["setup_write_denied"] = serde_json::json!(mutation_denied);
        report["setup_parent_rename_denied"] = serde_json::json!(rename_denied);
        if !mutation_denied || !rename_denied {
            return Err("Consumed input is not protected".into());
        }
        if options.wait_for_guardian_loss {
            report["ready_for_guardian_loss"] = serde_json::json!(true);
            let temporary = hook.join("checkpoint.tmp");
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?
                .write_all(&serde_json::to_vec_pretty(&report)?)?;
            fs::rename(temporary, hook.join("checkpoint.json"))?;
            // This is a fault-injection rendezvous, not a second installation path.
            std::thread::sleep(Duration::from_secs(12));
            return Err("External guardian loss was not injected before deadline".into());
        }
        fs::write(hook.join("release"), b"release")?;
        let exit = process.wait(Duration::from_secs(30))?;
        report["setup_exit"] = serde_json::json!(exit);
        let drain = Instant::now() + Duration::from_secs(2);
        while process.active_processes()? != 0 && Instant::now() < drain {
            std::thread::sleep(Duration::from_millis(20));
        }
        let active = process.active_processes()?;
        report["remaining_consumers"] = serde_json::json!(active);
        if exit != Some(0) || active != 0 {
            return Err("Setup did not complete and drain naturally".into());
        }
        drop(process);
        // Verify application-owned fixture content, not engine-private archive mappings.
        let payload_directory = if options.version == "1.0.0" {
            "payload"
        } else {
            "payload-B"
        };
        for name in ["fixture.exe", "models/model.bin"] {
            if fs::read(case.join(payload_directory).join(name))?
                != fs::read(installation.join("current").join(name))?
            {
                return Err(format!("Installed fixture differs: {name}").into());
            }
        }
        report["payload_verified"] = serde_json::json!(true);
        drop(exclusive);
        let client_report = case.join(format!("client-{}.json", options.attempt));
        let restarted = SupervisedProcess::spawn(
            &installation.join("current/fixture.exe"),
            &[OsStr::new("--report"), client_report.as_os_str()],
            &case,
        )?;
        let restart_exit = restarted.wait(Duration::from_secs(3))?;
        report["client_after_repair_exit"] = serde_json::json!(restart_exit);
        if restart_exit != Some(0) {
            return Err("Repaired fixture cannot acquire its activity lease".into());
        }
        let ready: serde_json::Value = serde_json::from_slice(&read(&client_report, 4096)?)?;
        if ready["version"] != options.version || ready["pid"] != restarted.id() {
            return Err("Unexpected restarted client identity".into());
        }
        report["restarted_client"] = ready;
        Ok(())
    })();
    report["seconds"] = serde_json::json!(start.elapsed().as_secs_f64());
    match &result {
        Ok(()) => report["status"] = serde_json::json!("passed"),
        Err(error) => report["error"] = serde_json::json!(error.to_string()),
    }
    output.write_all(&serde_json::to_vec_pretty(&report)?)?;
    output.sync_all()?;
    result
}

fn main() {
    let options = Options::parse();
    #[cfg(windows)]
    if let Err(error) = run(options) {
        eprintln!("update-setup-probe: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        eprintln!("Windows only");
        std::process::exit(1);
    }
}
