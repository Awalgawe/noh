#[path = "../tools/process.rs"]
mod process;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[test]
#[ignore = "subprocess fixture"]
fn fixture() {
    match std::env::var("NOH_RUNNER_FIXTURE").as_deref() {
        Ok("parent") => {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--ignored", "--exact", "fixture", "--nocapture"])
                .env("NOH_RUNNER_FIXTURE", "child")
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let _ = child.wait();
        }
        Ok("child") => {
            println!("descendant ready");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(60));
        }
        Ok("noisy") => {
            let block = [b'x'; 8192];
            loop {
                std::io::stdout().write_all(&block).unwrap();
            }
        }
        #[cfg(windows)]
        Ok("console") => {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn GetConsoleWindow() -> *mut std::ffi::c_void;
            }
            println!("CONSOLE_WINDOW|{}", unsafe { GetConsoleWindow() } as usize);
        }
        #[cfg(windows)]
        Ok("desktop-parent") => {
            #[link(name = "kernel32")]
            unsafe extern "system" {
                fn FreeConsole() -> i32;
            }
            unsafe { FreeConsole() };
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--ignored", "--exact", "fixture", "--nocapture"])
                .env("NOH_RUNNER_FIXTURE", "console");
            let output = process::run(command, Duration::from_secs(5)).unwrap();
            assert!(output.status.success());
            std::io::stdout().write_all(&output.stdout).unwrap();
        }
        _ => panic!("Only run as an owned test fixture"),
    }
}

#[test]
#[cfg(windows)]
fn owned_development_helpers_do_not_allocate_a_console() {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "fixture", "--nocapture"])
        .env("NOH_RUNNER_FIXTURE", "desktop-parent");
    let output = process::run(command, Duration::from_secs(5)).unwrap();
    assert!(output.status.success());
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        text.lines().any(|line| line == "CONSOLE_WINDOW|0"),
        "Development helper acquired a console and can steal focus: {text}"
    );
}

#[test]
fn timeout_kills_a_ready_descendant_holding_the_output_pipe() {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--ignored", "--exact", "fixture", "--nocapture"])
        .env("NOH_RUNNER_FIXTURE", "parent");
    let started = Instant::now();
    let err = process::run(cmd, Duration::from_secs(2))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Timed out after 2s"), "{err}");
    assert!(err.contains("descendant ready"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(15));
}

#[test]
fn noisy_process_is_stopped_when_capture_reaches_its_limit() {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--ignored", "--exact", "fixture", "--nocapture"])
        .env("NOH_RUNNER_FIXTURE", "noisy");
    let err = process::run(cmd, Duration::from_secs(10))
        .unwrap_err()
        .to_string();
    assert!(err.contains("Capture exceeded"), "{err}");
}
