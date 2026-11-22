#![cfg(all(feature = "gui", feature = "mcp"))]
//! The real server opening real NOH windows (`noh_app_open`): a window opens,
//! closes when the agent asks, and one left open outlives the session while
//! the client still sees the server end. The client here is a plain child
//! process with piped input and output and no job, as an MCP client starts
//! its server; a window that inherited the server's output would keep the
//! client's pipe open after the server ends.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

struct Server {
    child: Child,
    input: Option<ChildStdin>,
    /// Each stdout line, then `None` at the end of the output.
    lines: mpsc::Receiver<Option<Value>>,
    next_id: u64,
}

impl Server {
    fn start(ffmpeg: Option<&std::path::Path>) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_noh-mcp"));
        if let Some(ffmpeg) = ffmpeg {
            command.env("NOH_FFMPEG", ffmpeg);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        std::os::windows::process::CommandExt::creation_flags(&mut command, 0x0800_0000);
        let mut child = command.spawn().expect("start noh-mcp");
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str(&line) {
                    let _ = tx.send(Some(value));
                }
            }
            let _ = tx.send(None);
        });
        let mut server = Self {
            child,
            input,
            lines,
            next_id: 0,
        };
        server.call(
            "initialize",
            json!({
                "protocolVersion":"2025-11-25","capabilities":{},
                "clientInfo":{"name":"noh-driven-test","version":"1"}
            }),
        );
        server.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}));
        server
    }

    fn send(&mut self, value: &Value) {
        let input = self.input.as_mut().unwrap();
        serde_json::to_writer(&mut *input, value).unwrap();
        input.write_all(b"\n").unwrap();
        input.flush().unwrap();
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("a reply")
                .expect("the server still running");
            if line["id"] == id {
                return line;
            }
        }
    }

    fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let reply = self.call("tools/call", json!({"name":name,"arguments":arguments}));
        reply["result"]["structuredContent"].clone()
    }

    /// The window's state once `ready` says so, waiting up to 30 s.
    fn state_until(&mut self, app_id: &str, ready: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut revision = 0;
        loop {
            let reply = self.tool(
                "noh_app_state",
                json!({"app_id":app_id,"after_revision":revision,"wait_ms":2000}),
            );
            revision = reply["revision"].as_u64().unwrap_or(revision);
            if ready(&reply["view"]) {
                return reply["view"].clone();
            }
            assert!(Instant::now() < deadline, "state never ready: {reply}");
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn running(pid: u32) -> bool {
    if cfg!(windows) {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }
}

fn kill(pid: u32) {
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output();
    } else {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

#[test]
fn a_window_closes_when_asked_and_one_left_open_outlives_the_session() {
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        eprintln!("SKIPPED: no display for a NOH window");
        return;
    }
    let mut server = Server::start(None);
    let opened = server.tool("noh_app_open", json!({"title":"Integration test"}));
    assert_eq!(opened["state"], "open", "{opened}");
    let first = opened["pid"].as_u64().unwrap() as u32;
    let app_id = opened["app_id"].as_str().unwrap().to_owned();
    let closed = server.tool("noh_app_close", json!({ "app_id": app_id }));
    assert_eq!(closed["state"], "closed", "{closed}");
    assert_eq!(closed["exited"], true, "{closed}");
    assert!(!running(first));

    let opened = server.tool("noh_app_open", json!({"title":"Left open"}));
    assert_eq!(opened["state"], "open", "{opened}");
    let left = opened["pid"].as_u64().unwrap() as u32;
    // The client ends the session: it must see the server's output end and
    // the server exit, with the window still open.
    server.input.take();
    let deadline = Instant::now() + Duration::from_secs(15);
    let ended = loop {
        match server
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(None) => break true,
            Ok(Some(_)) => {}
            Err(_) => break false,
        }
    };
    let exited = loop {
        if server.child.try_wait().unwrap().is_some() {
            break true;
        }
        if Instant::now() > deadline {
            break false;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let still_open = running(left);
    kill(left);
    assert!(ended, "the client never saw the server's output end");
    assert!(exited, "the server did not exit");
    assert!(still_open, "the window did not outlive the session");
}

fn no_display() -> bool {
    cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
}

/// A short prepared by gestures, as an agent would: media added, the range,
/// the safe area, the playhead, a look at T; the times each took.
#[test]
fn a_short_is_prepared_by_gestures_and_looked_at() {
    if no_display() {
        eprintln!("SKIPPED: no display for a NOH window");
        return;
    }
    let f = support::Fixture::new();
    let video = f.path("clip.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=25:d=4",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        &video
    ]);
    let song = f.wav("song.wav", 8.0);
    let lines = f.path("lines.srt");
    std::fs::write(
        &lines,
        "1\n00:00:01,000 --> 00:00:03,000\nFirst test line\n\n2\n00:00:04,000 --> 00:00:06,000\nSecond test line\n\n",
    )
    .unwrap();
    let mut server = Server::start(Some(&f.ffmpeg));
    let opened = server.tool("noh_app_open", json!({"title":"Scenario"}));
    let app_id = opened["app_id"].as_str().expect("opened").to_owned();
    let timed = |server: &mut Server, action: Value| {
        let start = Instant::now();
        let reply = server.tool("noh_app_do", json!({"app_id":app_id,"action":action}));
        assert!(reply.get("error").is_none(), "{action}: {reply}");
        start.elapsed()
    };
    timed(
        &mut server,
        json!({"type":"add","paths":[video, song, lines]}),
    );
    let view = server.state_until(&app_id, |v| {
        v["clips"]
            .as_array()
            .is_some_and(|c| c.len() == 1 && c[0]["seconds"].is_number())
            && v["song"]["seconds"].is_number()
            && v["lyrics"]["count"] == 2
            && v["duration_ms"].as_u64().unwrap_or(0) > 0
    });
    assert_eq!(view["lyrics"]["lines"][1]["text"], "Second test line");
    let range = timed(
        &mut server,
        json!({"type":"set_range","start_ms":1000,"end_ms":5000}),
    );
    timed(&mut server, json!({"type":"set_safe_area","area":"tiktok"}));
    let seek = timed(&mut server, json!({"type":"seek","ms":4500}));
    let view = server.state_until(&app_id, |v| v["user_position"]["ms"] == 4500);
    assert_eq!(view["range"]["start_ms"], 1000);
    assert_eq!(view["safe_area"], "tiktok");
    assert_eq!(view["user_position"]["line"]["index"], 1);
    let output = f.path("look.png");
    let start = Instant::now();
    let look = server.tool(
        "noh_app_look",
        json!({"app_id":app_id,"scope":"window","output":output}),
    );
    let look_here = start.elapsed();
    assert!(look["width"].as_u64().unwrap_or(0) > 0, "{look}");
    let at = f.path("look-at.png");
    let start = Instant::now();
    let look_at = server.tool(
        "noh_app_look",
        json!({"app_id":app_id,"scope":"monitor","at_ms":2000,"output":at}),
    );
    let look_at_time = start.elapsed();
    assert_eq!(look_at["at_ms"], 2000, "{look_at}");
    let picture = image::open(&at).unwrap();
    assert_eq!(
        u64::from(picture.width()),
        look_at["width"].as_u64().unwrap()
    );
    eprintln!(
        "MEASURE set_range {range:?}, seek {seek:?}, look here {look_here:?}, look at T {look_at_time:?}"
    );
    let closed = server.tool("noh_app_close", json!({"app_id":app_id}));
    assert_eq!(closed["state"], "closed", "{closed}");
}
