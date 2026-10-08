//! Loopback responses exercise the production controller/request/body pipeline.
//! Only the connection destination is substituted; URL policy remains enabled.
use super::*;
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

struct Server {
    address: std::net::SocketAddr,
    worker: Option<JoinHandle<()>>,
}
impl Server {
    fn new(responses: Vec<Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let worker = std::thread::spawn(move || {
            for response in responses {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "request deadline");
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                read_headers(&mut stream);
                // A cancellation may close the connection while this fixture writes.
                let _ = stream.write_all(&response);
            }
        });
        Self {
            address,
            worker: Some(worker),
        }
    }
    fn transport(&self) -> GithubTransport {
        local_transport(self.address)
    }
    fn stalled_download(envelope: Vec<u8>) -> (Self, std::sync::mpsc::SyncSender<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (release, wait_for_release) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            for download in [false, true] {
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "stalled request deadline"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                read_headers(&mut stream);
                if download {
                    stream.write_all(&response(200, "", b"pac", 7)).unwrap();
                    // Actual HTTP body remains pending; a bounded event ends the fixture.
                    let _ = wait_for_release.recv_timeout(Duration::from_secs(3));
                } else {
                    stream
                        .write_all(&response(200, "", &envelope, envelope.len()))
                        .unwrap();
                }
            }
        });
        (
            Self {
                address,
                worker: Some(worker),
            },
            release,
        )
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let result = self.worker.take().unwrap().join();
        if !std::thread::panicking() {
            assert!(result.is_ok(), "fixture server failed");
        }
    }
}
fn read_headers(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    // A cancelled async client may stop draining the response before its runtime
    // polls connection cleanup. Server::drop joins this writer synchronously.
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).unwrap(), 1);
        bytes.push(byte[0]);
        assert!(bytes.len() <= 16 * 1024);
    }
}

#[test]
fn fixture_server_drop_is_bounded_when_client_stops_reading() {
    let body = vec![0; 16 * 1024 * 1024];
    let server = Server::new(vec![response(200, "", &body, body.len())]);
    let mut client = TcpStream::connect(server.address).unwrap();
    client
        .write_all(b"GET /fixture HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .unwrap();
    let (done, finished) = std::sync::mpsc::sync_channel(1);
    let cleanup = std::thread::spawn(move || {
        drop(server);
        let _ = done.send(());
    });
    // Keep the client connected without reading. A missing server write deadline
    // must fail this test promptly, not hang the whole test process in Drop.
    let result = finished.recv_timeout(Duration::from_secs(10));
    drop(client);
    cleanup.join().unwrap();
    assert!(
        result.is_ok(),
        "fixture writer did not stop without a reader"
    );
}

fn local_transport(address: std::net::SocketAddr) -> GithubTransport {
    GithubTransport {
        client: reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_millis(250))
            .build()
            .unwrap(),
        token: Some("synthetic-expired-token".into()),
        test_endpoint: Some(format!("http://{address}/fixture").parse().unwrap()),
        test_storage_full: false,
    }
}
fn response(status: u16, headers: &str, body: &[u8], declared: usize) -> Vec<u8> {
    let mut result = format!("HTTP/1.1 {status} Fixture\r\nConnection: close\r\nContent-Length: {declared}\r\n{headers}\r\n").into_bytes();
    result.extend_from_slice(body);
    result
}
fn updater(transport: GithubTransport, keys: Vec<TrustKey>) -> Controller {
    Controller::new(
        transport,
        keys,
        "NOH".into(),
        Target {
            os: Os::Windows,
            arch: Architecture::X64,
        },
        Channel::Stable,
        Version::new(1, 0, 0),
    )
}
const SOURCE: &str = "https://api.github.com/repos/example/noh/releases/assets/1";

#[tokio::test]
async fn controller_refuses_http_authentication_redirects_and_throttling() {
    for (status, headers, rate_limited) in [
        (401, "", false),
        (403, "", false),
        (429, "Retry-After: 120\r\n", true),
        (302, "Location: https://evil.example/steal\r\n", false),
    ] {
        let server = Server::new(vec![response(status, headers, b"", 0)]);
        let mut controller = updater(server.transport(), Vec::new());
        let error = controller
            .check(SOURCE, &AtomicBool::new(false))
            .await
            .unwrap_err();
        match status {
            302 => assert!(matches!(error, Error::Url)),
            429 => assert!(matches!(error, Error::RateLimited(120))),
            _ => assert!(matches!(error, Error::Http(value) if value == status)),
        }
        assert!(matches!(controller.state(), State::Failed { .. }));
        assert!(matches!(controller.take_ready(), Err(Error::Target)));
        assert_eq!(!controller.retry_after().is_zero(), rate_limited);
        if rate_limited {
            // There is no second response: this must fail before network access.
            assert!(matches!(
                controller.check(SOURCE, &AtomicBool::new(false)).await,
                Err(Error::RateLimited(_))
            ));
        }
    }
}

#[tokio::test]
async fn controller_reports_real_loopback_connection_failure_without_ready() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let mut controller = updater(local_transport(address), Vec::new());
    assert!(matches!(
        controller.check(SOURCE, &AtomicBool::new(false)).await,
        Err(Error::Network)
    ));
    assert!(matches!(controller.state(), State::Failed { .. }));
    assert!(matches!(controller.take_ready(), Err(Error::Target)));
}

#[tokio::test]
async fn interrupted_and_cancelled_downloads_clean_attempts_and_restart_ignores_cache() {
    let bytes = vec![42; 1024 * 1024];
    let mut release = tests::release();
    for artifact in &mut release.artifacts {
        artifact.size = bytes.len() as u64;
        artifact.sha256 = format!("{:x}", Sha256::digest(&bytes));
    }
    let (envelope, _) = tests::signed(&release);
    for cancel_during_transfer in [false, true] {
        let body = if cancel_during_transfer {
            bytes.as_slice()
        } else {
            &bytes[..32768]
        };
        let server = Server::new(vec![
            response(200, "", &envelope, envelope.len()),
            response(200, "", body, bytes.len()),
        ]);
        let root = tempfile::tempdir().unwrap();
        let cache = root.path().join("old-cache");
        std::fs::create_dir(&cache).unwrap();
        let stale = cache.join(&release.artifacts[0].file_name);
        std::fs::write(&stale, b"tampered cache survives restart").unwrap();
        let mut controller = updater(server.transport(), tests::signed(&release).1);
        let cancelled = AtomicBool::new(false);
        controller.check(SOURCE, &cancelled).await.unwrap();
        let mut saw_progress = false;
        let result = controller
            .download(root.path(), &cancelled, |state| {
                if matches!(state, State::Downloading { received, .. } if *received > 0) {
                    saw_progress = true;
                    if cancel_during_transfer {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                }
            })
            .await;
        assert!(saw_progress);
        if cancel_during_transfer {
            assert!(matches!(result, Err(Error::Cancelled)));
            assert_eq!(controller.state(), &State::Idle);
        } else {
            assert!(matches!(result, Err(Error::Network)));
            assert!(matches!(controller.state(), State::Failed { .. }));
        }
        assert!(matches!(controller.take_ready(), Err(Error::Target)));
        drop(controller);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        assert_eq!(
            std::fs::read(&stale).unwrap(),
            b"tampered cache survives restart"
        );
        let mut restarted = updater(server.transport(), tests::signed(&release).1);
        assert_eq!(restarted.state(), &State::Idle);
        assert!(matches!(restarted.take_ready(), Err(Error::Target)));
        assert!(matches!(
            restarted
                .download(root.path(), &AtomicBool::new(false), |_| {})
                .await,
            Err(Error::Target)
        ));
    }
}

#[tokio::test]
async fn controller_filesystem_failure_is_reported_without_touching_existing_data() {
    let (envelope, keys) = tests::signed(&tests::release());
    let server = Server::new(vec![response(200, "", &envelope, envelope.len())]);
    let mut controller = updater(server.transport(), keys);
    let cancelled = AtomicBool::new(false);
    controller.check(SOURCE, &cancelled).await.unwrap();
    let root = tempfile::tempdir().unwrap();
    let blocked = root.path().join("not-a-directory");
    std::fs::write(&blocked, b"preserved user data").unwrap();
    let mut notified = false;
    assert!(matches!(
        controller
            .download(&blocked, &cancelled, |state| {
                notified = matches!(state, State::Failed { .. });
            })
            .await,
        Err(Error::Io(_))
    ));
    assert!(notified);
    assert!(matches!(controller.take_ready(), Err(Error::Target)));
    assert_eq!(std::fs::read(&blocked).unwrap(), b"preserved user data");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn simulated_storage_full_after_partial_write_cleans_controller_attempt() {
    let (envelope, keys) = tests::signed(&tests::release());
    let server = Server::new(vec![
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
    ]);
    let mut transport = server.transport();
    transport.test_storage_full = true;
    let mut controller = updater(transport, keys);
    let root = tempfile::tempdir().unwrap();
    let preserved = root.path().join("project.json");
    std::fs::write(&preserved, b"preserved project").unwrap();
    let cancelled = AtomicBool::new(false);
    controller.check(SOURCE, &cancelled).await.unwrap();
    let result = controller.download(root.path(), &cancelled, |_| {}).await;
    assert!(
        matches!(result, Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::StorageFull)
    );
    assert!(matches!(controller.state(), State::Failed { .. }));
    assert!(matches!(controller.take_ready(), Err(Error::Target)));
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(std::fs::read(preserved).unwrap(), b"preserved project");
}

fn desktop_options(server: &Server, cache: &Path) -> desktop::Options {
    desktop::Options {
        setup: false,
        profile: Profile::Complete,
        transport: server.transport(),
        keys: tests::signed(&tests::release()).1,
        package_id: "NOH".into(),
        target: Target {
            os: Os::Windows,
            arch: Architecture::X64,
        },
        channel: Channel::Stable,
        current: Version::new(1, 0, 0),
        feed_url: SOURCE.into(),
        cache: cache.into(),
        before_publish: None,
    }
}

fn setup_release() -> Release {
    let mut release = tests::release();
    release.schema_version = 2;
    release.artifacts.truncate(1);
    let artifact = &mut release.artifacts[0];
    artifact.kind = PackageKind::WindowsSetup;
    artifact.file_name = "NOH-Setup.exe".into();
    artifact.setup = Some(setup::SetupContract {
        engine: setup::ENGINE.into(),
        runtime_dependencies: vec![],
        build_fingerprint: "a".repeat(64),
        installed_bytes: 6,
        files: [
            "noh.exe",
            "bin/noh-cli.exe",
            "bin/noh-mcp.exe",
            "bin/noh-update-guard.exe",
            "bin/noh-update-repair.exe",
            "manifest.json",
        ]
        .into_iter()
        .map(|path| setup::InstalledFile {
            path: path.into(),
            size: 1,
            sha256: "b".repeat(64),
        })
        .collect(),
    });
    release
}

#[test]
fn controller_refuses_other_signed_profiles_before_package_download() {
    let mut release = setup_release();
    let mut responses = Vec::new();
    for profile in [Profile::Complete, Profile::Minimal, Profile::Standard] {
        release.profile = profile;
        release.schema_version = if profile.is_complete() { 2 } else { 3 };
        let (envelope, _) = tests::signed(&release);
        responses.push(response(200, "", &envelope, envelope.len()));
    }
    let server = Server::new(responses);
    let mut controller = Controller::new(
        server.transport(),
        tests::signed(&release).1,
        "NOH".into(),
        release.artifacts[0].target,
        Channel::Stable,
        Version::new(1, 0, 0),
    )
    .with_setup()
    .with_profile(Profile::Standard);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let cancel = AtomicBool::new(false);
    runtime.block_on(async {
        for _ in 0..2 {
            assert!(matches!(
                controller.check(SOURCE, &cancel).await,
                Err(Error::Target)
            ));
            assert!(controller.take_ready().is_err());
        }
        controller.check(SOURCE, &cancel).await.unwrap();
        assert!(matches!(controller.state(), State::Available { .. }));
        // A context change must invalidate the pending choice.
        controller = controller.with_profile(Profile::Minimal);
        assert_eq!(controller.state(), &State::Idle);
    });
}

#[test]
fn setup_controller_explicitly_selects_and_stages_authenticated_executable_bytes() {
    let release = setup_release();
    let (envelope, keys) = tests::signed(&release);
    let server = Server::new(vec![
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
    ]);
    let mut controller = Controller::new(
        server.transport(),
        keys,
        "NOH".into(),
        release.artifacts[0].target,
        Channel::Stable,
        Version::new(1, 0, 0),
    )
    .with_setup();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let cancel = AtomicBool::new(false);
    let folder = tempfile::tempdir().unwrap();
    runtime.block_on(async {
        controller.check(SOURCE, &cancel).await.unwrap();
        assert!(matches!(controller.state(), State::Available { .. }));
        controller
            .download(folder.path(), &cancel, |_| {})
            .await
            .unwrap();
    });
    let ready = controller.take_ready().unwrap();
    assert_eq!(ready.package.file_name().unwrap(), "NOH-Setup.exe");
    assert_eq!(std::fs::read(&ready.package).unwrap(), b"package");
    assert_eq!(ready.envelope, envelope);
}

#[cfg(windows)]
#[test]
fn bootstrap_download_authenticates_exact_profile_and_preserves_staging_on_failure() {
    let mut release = setup_release();
    release.schema_version = 3;
    release.profile = Profile::Standard;
    let (envelope, keys) = tests::signed(&release);
    let verified = VerifiedRelease::verify(&envelope, &keys, "NOH").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let folder = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let server = Server::new(vec![response(200, "", b"package", 7)]);
    let transport = server.transport();
    runtime.block_on(async {
        assert!(matches!(
            transport
                .stage_setup(
                    &verified,
                    Profile::Complete,
                    &release.version,
                    None,
                    folder.path(),
                    &cancel,
                    |_, _| {}
                )
                .await,
            Err(Error::Target)
        ));
        let path = transport
            .stage_setup(
                &verified,
                Profile::Standard,
                &release.version,
                Some("https://api.github.com/repos/example/noh/releases/assets/42"),
                folder.path(),
                &cancel,
                |_, _| {},
            )
            .await
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"package");
    });
    drop(server);
    for (body, status) in [(b"corrupt".as_slice(), 200), (b"".as_slice(), 404)] {
        let empty = tempfile::tempdir().unwrap();
        let server = Server::new(vec![response(status, "", body, body.len())]);
        assert!(
            runtime
                .block_on(server.transport().stage_setup(
                    &verified,
                    Profile::Standard,
                    &release.version,
                    None,
                    empty.path(),
                    &cancel,
                    |_, _| {}
                ))
                .is_err()
        );
        assert_eq!(std::fs::read_dir(empty.path()).unwrap().count(), 0);
    }
}
fn desktop_wait(
    worker: &desktop::DesktopUpdater,
    signal: &Arc<(Mutex<u64>, Condvar)>,
) -> desktop::Snapshot {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut count = signal.0.lock().unwrap();
    while worker.busy() {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(!remaining.is_zero(), "desktop worker completion deadline");
        count = signal.1.wait_timeout(count, remaining).unwrap().0;
    }
    worker.snapshot()
}

#[test]
fn desktop_requires_download_consent_and_owns_ready_cleanup() {
    let (envelope, _) = tests::signed(&tests::release());
    let server = Server::new(vec![
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let preserved = cache.path().join("existing-cache.nupkg");
    std::fs::write(&preserved, b"untrusted existing cache").unwrap();
    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let notify = signal.clone();
    let worker =
        desktop::DesktopUpdater::with_options(desktop_options(&server, cache.path()), move || {
            *notify.0.lock().unwrap() += 1;
            notify.1.notify_all();
        });
    assert!(!worker.download(), "no unchecked startup/cache download");
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
    assert!(worker.check());
    assert!(matches!(
        desktop_wait(&worker, &signal).state,
        State::Available { .. }
    ));
    assert_eq!(
        std::fs::read_dir(cache.path()).unwrap().count(),
        1,
        "checking never downloads"
    );
    assert!(worker.download(), "explicit download consent");
    let ready = desktop_wait(&worker, &signal);
    let State::Ready { package, .. } = ready.state else {
        panic!("expected verified download")
    };
    assert_eq!(std::fs::read(&package).unwrap(), b"package");
    assert!(
        !worker.download(),
        "Ready cannot queue a duplicate download"
    );
    assert!(worker.cancel());
    let discarded = desktop_wait(&worker, &signal);
    assert_eq!(discarded.state, State::Idle);
    assert!(
        !discarded.checked,
        "discard is not a successful latest-version check"
    );
    assert!(!package.exists());
    assert!(worker.check());
    desktop_wait(&worker, &signal);
    assert!(worker.download());
    let State::Ready { package, .. } = desktop_wait(&worker, &signal).state else {
        panic!("expected Ready again")
    };
    drop(worker);
    assert!(
        !package.exists(),
        "normal GUI shutdown discards uncommitted download"
    );
    assert_eq!(
        std::fs::read(preserved).unwrap(),
        b"untrusted existing cache"
    );
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 1);
}

#[test]
fn desktop_surfaces_rate_limit_and_retains_its_deadline_across_retry() {
    let server = Server::new(vec![response(429, "Retry-After: 120\r\n", b"", 0)]);
    let cache = tempfile::tempdir().unwrap();
    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let notify = signal.clone();
    let worker =
        desktop::DesktopUpdater::with_options(desktop_options(&server, cache.path()), move || {
            *notify.0.lock().unwrap() += 1;
            notify.1.notify_all();
        });
    assert!(worker.check());
    let failed = desktop_wait(&worker, &signal);
    assert!(matches!(failed.state, State::Failed { .. }));
    assert_eq!(failed.failure, Some(desktop::Failure::Network));
    assert!(failed.retry_after() > Duration::from_secs(119));
    assert!(worker.check());
    let again = desktop_wait(&worker, &signal);
    assert_eq!(again.retry_at, failed.retry_at);
    assert_eq!(again.failure, failed.failure);
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn desktop_refuses_installation_outside_managed_qualification_and_cleans_owned_download() {
    let (envelope, _) = tests::signed(&tests::release());
    let server = Server::new(vec![
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let notify = signal.clone();
    let worker =
        desktop::DesktopUpdater::with_options(desktop_options(&server, cache.path()), move || {
            *notify.0.lock().unwrap() += 1;
            notify.1.notify_all();
        });
    assert!(!worker.commit_for_shutdown());
    assert!(!worker.prepare_windows(cache.path().join("not-an-archive")));
    assert!(worker.check());
    desktop_wait(&worker, &signal);
    assert!(worker.download());
    let ready = desktop_wait(&worker, &signal);
    let State::Ready { package, .. } = ready.state else {
        panic!("expected authenticated Ready")
    };
    assert!(worker.prepare_windows(cache.path().join("not-an-archive")));
    let failed = desktop_wait(&worker, &signal);
    assert!(matches!(failed.state, State::Failed { .. }));
    assert_eq!(failed.failure, Some(desktop::Failure::Target));
    assert!(!failed.awaiting_shutdown && !failed.shutdown_committed);
    assert!(!worker.commit_for_shutdown());
    assert!(!package.exists());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
    assert!(worker.cancel());
    assert_eq!(desktop_wait(&worker, &signal).state, State::Idle);
}

#[test]
fn desktop_late_accepted_cancellation_discards_already_persisted_package() {
    let (envelope, _) = tests::signed(&tests::release());
    let server = Server::new(vec![
        response(200, "", &envelope, envelope.len()),
        response(200, "", b"package", 7),
    ]);
    let cache = tempfile::tempdir().unwrap();
    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let notify = signal.clone();
    let (persisted, arrived) = std::sync::mpsc::sync_channel(1);
    let (release, continue_worker) = std::sync::mpsc::sync_channel(1);
    let mut options = desktop_options(&server, cache.path());
    options.before_publish = Some(Box::new(move |state| {
        if let State::Ready { package, .. } = state {
            persisted.send(package.clone()).unwrap();
            continue_worker
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    }));
    let worker = desktop::DesktopUpdater::with_options(options, move || {
        *notify.0.lock().unwrap() += 1;
        notify.1.notify_all();
    });
    assert!(worker.check());
    desktop_wait(&worker, &signal);
    assert!(worker.download());
    let package = arrived.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        package.is_file(),
        "package already synced and persisted before cancellation"
    );
    assert!(worker.busy());
    assert!(
        matches!(worker.snapshot().state, State::Verifying),
        "progress cannot publish Ready before the terminal transition"
    );
    assert!(
        worker.cancel(),
        "late cancellation accepted before terminal publication"
    );
    release.send(()).unwrap();
    assert_eq!(desktop_wait(&worker, &signal).state, State::Idle);
    assert!(!package.exists());
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}

#[test]
fn desktop_shutdown_cancels_a_real_pending_http_body_and_cleans_partial_data() {
    let (envelope, _) = tests::signed(&tests::release());
    let (server, release) = Server::stalled_download(envelope);
    let cache = tempfile::tempdir().unwrap();
    let signal = Arc::new((Mutex::new(0u64), Condvar::new()));
    let notify = signal.clone();
    let worker =
        desktop::DesktopUpdater::with_options(desktop_options(&server, cache.path()), move || {
            *notify.0.lock().unwrap() += 1;
            notify.1.notify_all();
        });
    assert!(worker.check());
    desktop_wait(&worker, &signal);
    assert!(worker.download());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    let mut count = signal.0.lock().unwrap();
    while !matches!(
        worker.snapshot().state,
        State::Downloading {
            received: 3,
            total: 7
        }
    ) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "partial HTTP body must reach worker progress"
        );
        count = signal.1.wait_timeout(count, remaining).unwrap().0;
    }
    drop(count);
    assert!(worker.busy());
    assert_eq!(
        std::fs::read_dir(cache.path()).unwrap().count(),
        1,
        "owned partial attempt exists"
    );
    let before = std::time::Instant::now();
    drop(worker);
    let elapsed = before.elapsed();
    release.send(()).unwrap();
    assert!(
        elapsed < Duration::from_secs(2),
        "shutdown waited {elapsed:?}"
    );
    assert_eq!(std::fs::read_dir(cache.path()).unwrap().count(), 0);
}
