#![cfg(all(feature = "gui", feature = "mcp"))]
//! The real server opening real NOH windows (`noh_app_open`): a window opens,
//! closes when the agent asks, and one left open outlives the session while
//! the client still sees the server end. The client here is a plain child
//! process with piped input and output and no job, as an MCP client starts
//! its server; a window that inherited the server's output would keep the
//! client's pipe open after the server ends.
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
    fn start() -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_noh-mcp"));
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
    let mut server = Server::start();
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
