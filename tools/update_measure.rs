//! Bounded Windows job accounting for local packaging qualification.
use clap::Parser;
use std::{
    ffi::OsString,
    path::PathBuf,
    time::{Duration, Instant},
};
#[derive(Parser)]
struct Options {
    #[arg(long)]
    executable: PathBuf,
    #[arg(long)]
    directory: PathBuf,
    #[arg(long)]
    report: PathBuf,
    #[arg(long, default_value_t = 1800)]
    timeout_seconds: u64,
    #[arg(last = true)]
    arguments: Vec<OsString>,
}
#[cfg(windows)]
fn run(options: Options) -> std::io::Result<u32> {
    use std::io::Write;
    let mut report = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(options.report)?;
    let start = Instant::now();
    let mut data = serde_json::json!({
        "exit":null,"elapsed_seconds":0,"job_peak_commit_bytes":null,
        "memory_scope":"supervised process and descendants",
        "memory_kind":"Windows job committed virtual memory (not RSS)",
        "normal_completion":false,"forced_cleanup":null,"error":null
    });
    let args: Vec<_> = options
        .arguments
        .iter()
        .map(|arg| arg.as_os_str())
        .collect();
    let outcome = (|| {
        let process = noh::update::windows::SupervisedProcess::spawn(
            &options.executable,
            &args,
            &options.directory,
        )?;
        data["pid"] = serde_json::json!(process.id());
        let exit = process.wait(Duration::from_secs(options.timeout_seconds.min(1800)))?;
        data["exit"] = serde_json::json!(exit);
        let observed_active = process.active_processes()?;
        // The leader handle can signal just before job accounting removes it.
        // Allow a bounded natural drain; never silently kill surviving children.
        let drain_deadline = Instant::now() + Duration::from_secs(2);
        let active = loop {
            let active = process.active_processes()?;
            if exit.is_none() || active == 0 || Instant::now() >= drain_deadline {
                break active;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        data["active_at_first_observation"] = serde_json::json!(observed_active);
        data["active_after_leader_exit"] =
            serde_json::json!(if exit.is_some() { Some(active) } else { None });
        data["forced_cleanup"] = serde_json::json!(exit.is_none() || active > 0);
        match process.peak_committed_bytes() {
            Ok(peak) => data["job_peak_commit_bytes"] = serde_json::json!(peak),
            Err(error) => {
                data["metrics_error"] = serde_json::json!({"kind":format!("{:?}",error.kind()),"message":error.to_string()})
            }
        }
        process.terminate_tree(Duration::from_secs(5))?;
        if exit.is_none() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Measured process exceeded deadline",
            ));
        }
        if active > 0 {
            return Err(std::io::Error::other(
                "Leader exited while descendants remained; forced cleanup is not normal completion",
            ));
        }
        data["normal_completion"] = serde_json::json!(exit == Some(0));
        Ok(exit.unwrap())
    })();
    data["elapsed_seconds"] = serde_json::json!(start.elapsed().as_secs_f64());
    if let Err(error) = &outcome {
        data["error"] =
            serde_json::json!({"kind":format!("{:?}",error.kind()),"message":error.to_string()});
    }
    report.write_all(&serde_json::to_vec_pretty(&data)?)?;
    report.sync_all()?;
    outcome
}
fn main() {
    let options = Options::parse();
    #[cfg(windows)]
    match run(options) {
        Ok(code) => std::process::exit(code as i32),
        Err(error) => {
            eprintln!("update-measure: {error}");
            std::process::exit(1);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        eprintln!("Windows job measurement is unavailable on this platform");
        std::process::exit(1);
    }
}
