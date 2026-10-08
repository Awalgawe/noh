#![cfg(feature = "mcp")]
//! Real stdio/worker tests: the protocol adapter must preserve engine contracts.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
#[path = "support/mcp_client.rs"]
mod client;
use client::{Client, structured};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use support::Fixture;

fn start(client: &mut Client, name: &str, arguments: Value) -> String {
    let reply = client.tool(name, arguments);
    assert_ne!(reply["isError"], true, "{reply}");
    structured(&reply)["job_id"]
        .as_str()
        .expect("Started job ID")
        .to_owned()
}

fn terminal(client: &mut Client, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut revision = 0;
    let mut percent = 0;
    loop {
        assert!(Instant::now() < deadline, "Job deadline: {id}");
        let result = client.tool(
            "noh_job",
            json!({
                "job_id":id,"after_revision":revision,"wait_ms":1000,"include_result":true
            }),
        );
        let data = structured(&result);
        let next = data["revision"].as_u64().unwrap();
        assert!(next >= revision);
        revision = next;
        if let Some(next) = data["progress"]["percent"].as_u64() {
            assert!(next >= percent, "Nonmonotonic progress: {data}");
            percent = next;
        }
        match data["state"].as_str().unwrap() {
            "succeeded" | "failed" | "cancelled" => return result,
            "running" | "cancelling" => {}
            other => panic!("Unknown state: {other}"),
        }
    }
}

#[test]
fn stdio_project_reuses_inputs_and_enforces_interval_and_publication_contracts() {
    let f = Fixture::new();
    let video = f.path("project-source.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=blue:s=160x90:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "0",
        &video
    ]);
    let wav = f.wav("project.wav", 3.0);
    let srt = f.path("project.srt");
    std::fs::write(&srt, "1\n00:00:01,100 --> 00:00:01,700\nProject clock\n\n").unwrap();
    let output = f.path("project-short.mp4");
    let mut client = Client::spawn(Some(&f.ffmpeg));
    client.initialize("2025-11-25");
    let arguments = json!({
        "request": {"videos":[video], "wav":wav, "output":output},
        "short": {"start_ms":1000, "end_ms":1800, "restart_loops":true},
        "captions":{"subtitles":srt,"size":"small"}, "preview":true
    });
    let id = start(&mut client, "noh_export_project", arguments.clone());
    let result = terminal(&mut client, &id);
    let data = structured(&result);
    assert_eq!(data["state"], "succeeded", "{result}");
    assert_eq!(data["result"]["output"], json!(output));
    let decoded = f.packets(&output, false, true, false);
    assert!(decoded.contains("#dimensions 0: 360x640"), "{decoded}");
    assert!(!support::rows(&f.packets(&output, false, true, true)).is_empty());
    let bytes = std::fs::read(&output).unwrap();
    let duplicate = start(&mut client, "noh_export_project", arguments.clone());
    let refused = terminal(&mut client, &duplicate);
    assert_eq!(
        structured(&refused)["error"]["code"],
        "error.output_exists",
        "{refused}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), bytes);
    let mut invalid = arguments.clone();
    invalid["short"]["end_ms"] = json!(1000);
    let rejected = client.tool("noh_export_project", invalid);
    assert_eq!(rejected["isError"], true, "{rejected}");
    assert_eq!(structured(&rejected)["error"]["code"], "error.short_range");
    let mut unknown = arguments;
    unknown["short"]["unrecognized"] = json!(true);
    let rejected = client.call(
        "tools/call",
        json!({"name":"noh_export_project", "arguments":unknown}),
    );
    assert!(
        rejected.get("error").is_some() || rejected["result"]["isError"] == true,
        "{rejected}"
    );
}

#[test]
fn stdio_short_maps_captions_preview_and_shared_no_overwrite_results() {
    let f = Fixture::new();
    let source = f.path("short-source.mp4");
    let subtitles = f.path("reviewed.srt");
    let output = f.path("short-preview.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        &source
    ]);
    std::fs::write(
        &subtitles,
        "1\n00:00:00,100 --> 00:00:00,900\nReviewed source times\n\n",
    )
    .unwrap();
    let mut client = Client::spawn(Some(&f.ffmpeg));
    client.initialize("2025-11-25");
    let args = json!({"source":source,"output":output,"start_ms":135,"end_ms":775,
        "framing":"crop","preview":true,
        "captions":{"subtitles":subtitles,"size":"small","placement":"top","safe_area":"tiktok"}});
    let id = start(&mut client, "noh_make_short", args.clone());
    let reply = terminal(&mut client, &id);
    let data = structured(&reply);
    assert_eq!(data["state"], "succeeded", "{reply}");
    assert_eq!(data["result"]["kind"], "export", "{reply}");
    assert_eq!(data["result"]["output"], json!(output), "{reply}");
    let frames = f.packets(&output, false, true, false);
    assert!(frames.contains("#dimensions 0: 360x640"), "{frames}");
    assert!(!support::rows(&frames).is_empty());
    let original = std::fs::read(&output).unwrap();
    let duplicate = start(&mut client, "noh_make_short", args.clone());
    let failed = terminal(&mut client, &duplicate);
    assert_eq!(
        structured(&failed)["error"]["code"],
        "error.output_exists",
        "{failed}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), original);
    for (key, value) in [
        ("framing", json!("automatic")),
        ("start_ms", json!(-1)),
        ("output", json!("relative.mp4")),
        ("arbitrary_filter", json!("drawtext")),
    ] {
        let mut bad = args.clone();
        bad[key] = value;
        let rejected = client.call(
            "tools/call",
            json!({"name":"noh_make_short","arguments":bad}),
        );
        assert!(
            rejected.get("error").is_some() || rejected["result"]["isError"] == true,
            "{rejected}"
        );
    }
    let mut bad = args.clone();
    bad["end_ms"] = 135.into();
    let rejected = client.tool("noh_make_short", bad);
    assert_eq!(
        structured(&rejected)["error"]["code"],
        "error.short_range",
        "{rejected}"
    );
    let mut bad = args;
    bad["captions"]["size"] = "giant".into();
    let rejected = client.call(
        "tools/call",
        json!({"name":"noh_make_short","arguments":bad}),
    );
    assert!(
        rejected.get("error").is_some() || rejected["result"]["isError"] == true,
        "{rejected}"
    );
    f.clean();
}

#[test]
fn stdio_caption_rendering_uses_the_shared_worker_and_never_overwrites() {
    let f = Fixture::new();
    let source = f.path("caption-source.mp4");
    let subtitles = f.path("reviewed.srt");
    let output = f.path("burned.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=1",
        "-c:v",
        "libx264",
        &source
    ]);
    std::fs::write(
        &subtitles,
        "1\n00:00:00,100 --> 00:00:00,800\nReviewed subtitles\n\n",
    )
    .unwrap();
    let mut client = Client::spawn(Some(&f.ffmpeg));
    client.initialize("2025-11-25");
    let args = json!({"source":source,"subtitles":subtitles,"output":output,"size":"medium","placement":"bottom"});
    let id = start(&mut client, "noh_burn_subtitles", args.clone());
    let reply = terminal(&mut client, &id);
    let data = structured(&reply);
    assert_eq!(data["state"], "succeeded", "{reply}");
    assert_eq!(data["result"]["kind"], "export", "{reply}");
    assert!(output.is_file());
    assert!(
        data["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "caption.reencode"),
        "{data}"
    );
    let original = std::fs::read(&output).unwrap();
    let duplicate = start(&mut client, "noh_burn_subtitles", args.clone());
    let failed = terminal(&mut client, &duplicate);
    assert_eq!(
        structured(&failed)["error"]["code"],
        "error.output_exists",
        "{failed}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), original);
    for (key, value) in [
        ("size", json!("giant")),
        ("placement", json!("anywhere")),
        ("output", json!("relative.mp4")),
        ("arbitrary_filter", json!("drawtext")),
    ] {
        let mut bad = args.clone();
        bad[key] = value;
        let rejected = client.call(
            "tools/call",
            json!({"name":"noh_burn_subtitles","arguments":bad}),
        );
        assert!(
            rejected.get("error").is_some() || rejected["result"]["isError"] == true,
            "{rejected}"
        );
    }
    f.clean();
}

#[test]
fn stdio_negotiation_schemas_strict_inputs_and_session_ownership() {
    for version in ["2024-11-05", "2025-06-18", "2025-11-25"] {
        let mut client = Client::spawn(None);
        let initialize = client.initialize(version);
        assert_eq!(initialize["protocolVersion"], version);
        let server_version = initialize["serverInfo"]["version"].as_str().unwrap();
        assert!(server_version.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(
            server_version.contains("build "),
            "Missing server build identity"
        );
        assert!(initialize["capabilities"].get("tools").is_some());
        let listed = client.call("tools/list", json!({}));
        let tools = listed["result"]["tools"].as_array().unwrap();
        let mut names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "noh_burn_subtitles",
                "noh_cancel",
                "noh_diagnose",
                "noh_export",
                "noh_export_project",
                "noh_inspect",
                "noh_job",
                "noh_make_short",
                "noh_preview",
                "noh_transcribe",
            ]
        );
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert_eq!(tool["inputSchema"]["additionalProperties"], false, "{tool}");
            assert!(
                tool["outputSchema"].is_object(),
                "Missing output schema: {tool}"
            );
        }
        for arguments in [
            json!({"job_id":"unowned","unexpected":true}),
            json!({"job_id":"unowned","wait_ms":30001}),
            json!({"job_id":"unowned","wait_ms":-1}),
        ] {
            let reply = client.call(
                "tools/call",
                json!({"name":"noh_job","arguments":arguments}),
            );
            assert!(
                reply.get("error").is_some() || reply["result"]["isError"] == true,
                "{reply}"
            );
        }
        let unknown = client.tool("noh_job", json!({"job_id":"unowned"}));
        assert_eq!(unknown["isError"], true);
        assert!(structured(&unknown)["error"]["code"].as_str().is_some());
        let invalid = client.tool("noh_inspect", json!({"path":"relative.mp4","kind":"video"}));
        assert_eq!(invalid["isError"], true);
        for arguments in [
            json!({"source":"relative.wav","output":"relative.srt","transcriber":"whisper","model":"model.bin","vad_model":"vad.bin"}),
            json!({"source":"relative.wav","output":"relative.srt","transcriber":"whisper","model":"model.bin","vad_model":"vad.bin","args":"--language fr"}),
            json!({"source":"relative.wav","output":"relative.mp4","transcriber":"whisper","model":"model.bin","vad_model":"vad.bin"}),
        ] {
            let rejected = client.tool("noh_transcribe", arguments);
            assert_eq!(rejected["isError"], true, "{rejected}");
        }
        client.close_input();
        client.wait_exit();
    }
}

#[test]
fn optional_local_transcription_returns_a_bounded_subtitle_job_result() {
    let (Some(transcriber), Some(model), Some(vad_model)) = (
        std::env::var_os("NOH_WHISPER"),
        std::env::var_os("NOH_WHISPER_MODEL"),
        std::env::var_os("NOH_WHISPER_VAD"),
    ) else {
        return;
    };
    let fixture = Fixture::new();
    let source = fixture.path("silent-source.wav");
    fixture.ff(args![
        "-f",
        "lavfi",
        "-i",
        "anullsrc=r=16000:cl=mono",
        "-t",
        "2",
        "-c:a",
        "pcm_s16le",
        &source
    ]);
    let output = fixture.path("subtitles.srt");
    let mut client = Client::spawn(Some(&fixture.ffmpeg));
    client.initialize("2025-11-25");
    let id = start(
        &mut client,
        "noh_transcribe",
        json!({
            "source":source,
            "output":output,
            "transcriber":transcriber.to_string_lossy(),
            "model":model.to_string_lossy(),
            "vad_model":vad_model.to_string_lossy(),
            "language":"auto"
        }),
    );
    let result = structured(&terminal(&mut client, &id));
    assert_eq!(result["state"], "succeeded", "{result}");
    let subtitles = &result["result"];
    assert_eq!(subtitles["kind"], "subtitles");
    assert_eq!(
        subtitles["output"],
        fixture.path("subtitles.srt").to_string_lossy().as_ref()
    );
    assert!(subtitles["track"]["duration_ms"].as_u64().unwrap() > 0);
    assert!(subtitles["track"]["cues"].is_array());
    assert!(fixture.path("subtitles.srt").exists());
    client.close_input();
    client.wait_exit();
}

#[test]
fn input_eof_and_oversized_unterminated_frame_exit_without_a_client_close() {
    let mut uninitialized = Client::spawn(None);
    uninitialized.close_input();
    uninitialized.wait_exit();
    let mut oversized = Client::spawn(None);
    oversized.initialize("2025-11-25");
    oversized.raw(&vec![b' '; 4 * 1024 * 1024 + 1]);
    // Deliberately retain stdin: the frame limit itself must end the server.
    oversized.wait_exit();
}

#[test]
fn terminal_retention_is_bounded_and_expired_ids_are_explicit() {
    let files = tempfile::tempdir().unwrap();
    let mut client = Client::spawn(None);
    client.initialize("2025-11-25");
    let mut ids = Vec::new();
    for index in 0..18 {
        let id = start(
            &mut client,
            "noh_inspect",
            json!({
                "path":files.path().join(format!("missing-{index}.mp4")),"kind":"video"
            }),
        );
        assert_eq!(structured(&terminal(&mut client, &id))["state"], "failed");
        ids.push(id);
    }
    let expired = client.tool("noh_job", json!({"job_id":ids[0],"include_result":true}));
    assert_eq!(expired["isError"], true);
    assert!(
        structured(&expired)["error"]["code"]
            .as_str()
            .unwrap()
            .contains("expired")
    );
    let retained =
        structured(&client.tool("noh_job", json!({"job_id":ids[17],"include_result":true})));
    assert_eq!(retained["state"], "failed");
    client.close_input();
    client.wait_exit();
}

fn media_request(f: &Fixture) -> Value {
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x96:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        f.path("source.mp4")
    ]);
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=25:d=1",
        "-c:v",
        "mpeg4",
        f.path("other.mp4")
    ]);
    let wav = f.wav("audio.wav", 3.44);
    json!({"videos":[f.path("source.mp4"),f.path("other.mp4")],"wav":wav,
        "output":f.path("result.mp4"),"ffmpeg":f.ffmpeg,"fade_in":0.2,"fade_out":0.2})
}

#[test]
fn cancellation_and_client_eof_stop_active_work_and_preserve_unrelated_files() {
    let f = Fixture::new();
    let mut request = media_request(&f);
    // Multiple inspections provide active work without expensive large fixtures.
    request["videos"] = json!(vec![f.path("source.mp4"); 64]);
    request["force_encode"] = json!(true);
    std::fs::create_dir(f.path(".noh-unrelated")).unwrap();
    std::fs::write(f.path(".noh-unrelated/keep.txt"), b"keep").unwrap();
    for shutdown in ["cancel", "eof", "broken-output"] {
        request["output"] = json!(f.path(&format!("{shutdown}.mp4")));
        let mut client = Client::spawn(Some(&f.ffmpeg));
        client.initialize("2025-11-25");
        let job = start(&mut client, "noh_preview", json!({"request":request}));
        let progress = structured(&client.tool(
            "noh_job",
            json!({
                "job_id":job,"after_revision":1,"wait_ms":1000
            }),
        ));
        assert_eq!(progress["state"], "running", "{progress}");
        let busy = client.tool(
            "noh_inspect",
            json!({"path":f.path("source.mp4"),"kind":"video"}),
        );
        assert_eq!(busy["isError"], true, "{busy}");
        if shutdown == "cancel" {
            let cancelled = client.tool("noh_cancel", json!({"job_id":job}));
            assert_ne!(cancelled["isError"], true, "{cancelled}");
            let result = structured(&terminal(&mut client, &job));
            assert_eq!(result["state"], "cancelled", "{result}");
        }
        if shutdown == "broken-output" {
            client.close_output();
            // Retain stdin while forcing a response into the closed output pipe.
            client.send(&json!({"jsonrpc":"2.0","id":9999,"method":"ping"}));
        } else {
            client.close_input();
        }
        // The client's wrapper owns the full server/worker/FFmpeg tree.
        client.wait_exit();
        assert!(!std::path::Path::new(request["output"].as_str().unwrap()).exists());
        let remaining: Vec<_> = std::fs::read_dir(f.folder.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().starts_with(".noh-"))
            .collect();
        assert_eq!(remaining, [std::ffi::OsString::from(".noh-unrelated")]);
        assert_eq!(
            std::fs::read(f.path(".noh-unrelated/keep.txt")).unwrap(),
            b"keep"
        );
    }
}

#[test]
fn shared_diagnosis_export_preview_errors_and_no_overwrite() {
    let f = Fixture::new();
    let mut request = media_request(&f);
    let mut client = Client::spawn(Some(&f.ffmpeg));
    client.initialize("2025-11-25");
    let inspect = start(
        &mut client,
        "noh_inspect",
        json!({"path":f.path("source.mp4"),"kind":"video"}),
    );
    let metadata = structured(&terminal(&mut client, &inspect));
    assert_eq!(metadata["state"], "succeeded");
    assert_eq!(metadata["result"]["kind"], "metadata");
    let quick = start(
        &mut client,
        "noh_diagnose",
        json!({"request":request,"level":"quick"}),
    );
    let quick_result = structured(&terminal(&mut client, &quick));
    assert_eq!(quick_result["result"]["diagnosis"]["plan"], Value::Null);
    let denied = client.tool(
        "noh_export",
        json!({"request":request,"diagnosis_job_id":quick}),
    );
    assert_eq!(denied["isError"], true);
    let exact = start(&mut client, "noh_diagnose", json!({"request":request}));
    let exact_result = structured(&terminal(&mut client, &exact));
    let diagnosis = &exact_result["result"]["diagnosis"];
    assert_eq!(diagnosis["level"], "exact");
    let treatments: Vec<_> = diagnosis["plan"]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["treatment"].as_str().unwrap())
        .collect();
    assert_eq!(treatments, ["copy", "convert"]);
    let cli = f.cli(args![
        f.path("source.mp4"),
        f.path("audio.wav"),
        "--video",
        f.path("other.mp4"),
        "--output",
        f.path("result.mp4"),
        "--fade-in",
        "0.2",
        "--fade-out",
        "0.2",
        "--partial-fades",
        "--diagnose=exact",
        "--json"
    ]);
    let cli: Value = serde_json::from_slice(&cli.stdout).unwrap();
    assert_eq!(cli["version"], 1);
    let noh::engine::Event::Inspected(Ok(noh::inspection::InspectionResult::Plan(cli_diagnosis))) =
        serde_json::from_value(cli["result"].clone()).unwrap()
    else {
        panic!("{cli}")
    };
    assert_eq!(
        serde_json::to_value(cli_diagnosis.plan).unwrap(),
        diagnosis["plan"]
    );
    request["output"] = json!(f.path("renamed.MP4"));
    let export = start(
        &mut client,
        "noh_export",
        json!({"request":request,"diagnosis_job_id":exact}),
    );
    let exported = structured(&terminal(&mut client, &export));
    assert_eq!(exported["state"], "succeeded");
    assert_eq!(exported["result"]["output"], request["output"]);
    let unchanged = structured(&client.tool(
        "noh_job",
        json!({
            "job_id":export,"after_revision":exported["revision"],"include_result":true
        }),
    ));
    assert_eq!(unchanged["changed"], false);
    assert!(unchanged.get("result").is_none());
    let repeated =
        structured(&client.tool("noh_job", json!({"job_id":export,"include_result":true})));
    assert_eq!(repeated["result"], exported["result"]);
    let late_cancel = structured(&client.tool("noh_cancel", json!({"job_id":export})));
    assert_eq!(late_cancel["state"], "succeeded");
    let output = f.path("renamed.MP4");
    assert!(!f.hashes(&output, false, true, false).is_empty());
    support::near(f.duration(&output), 3.44, 0.13);
    let original = std::fs::read(&output).unwrap();
    let retry = start(
        &mut client,
        "noh_export",
        json!({"request":request,"diagnosis_job_id":exact}),
    );
    let failed = terminal(&mut client, &retry);
    assert_eq!(failed["isError"], true);
    assert_eq!(structured(&failed)["error"]["code"], "error.output_exists");
    assert_eq!(std::fs::read(&output).unwrap(), original);
    request["output"] = json!(f.path("preview.mp4"));
    let preview = start(&mut client, "noh_preview", json!({"request":request}));
    let previewed = structured(&terminal(&mut client, &preview));
    assert_eq!(previewed["state"], "succeeded");
    assert!(
        !f.hashes(&f.path("preview.mp4"), false, true, false)
            .is_empty()
    );
    request["fade_in"] = json!(0.3);
    request["output"] = json!(f.path("changed.mp4"));
    let changed = client.tool(
        "noh_export",
        json!({"request":request,"diagnosis_job_id":exact}),
    );
    // Validation may happen at admission or in the checked worker.
    let changed = if changed["isError"] == true {
        changed
    } else {
        terminal(
            &mut client,
            structured(&changed)["job_id"].as_str().unwrap(),
        )
    };
    assert_eq!(changed["isError"], true);
    assert_eq!(
        structured(&changed)["error"]["code"],
        "error.stale_inspection"
    );
    let mut other = Client::spawn(Some(&f.ffmpeg));
    other.initialize("2025-11-25");
    assert_eq!(
        other.tool("noh_job", json!({"job_id":exact}))["isError"],
        true
    );
    other.close_input();
    other.wait_exit();
    client.close_input();
    client.wait_exit();
    f.clean();
}

#[test]
fn timed_images_share_cli_mcp_diagnosis_export_and_preview() {
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x96:r=25:d=1",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        f.path("video.mp4")
    ]);
    for (name, color) in [("same image.png", "red"), ("photo.jpg", "blue")] {
        f.ff(args![
            "-f",
            "lavfi",
            "-i",
            format!("color=c={color}:s=96x64"),
            "-frames:v",
            "1",
            "-update",
            "1",
            f.path(name)
        ]);
    }
    let wav = f.wav("audio.wav", 3.44);
    let mut request = json!({
        "items":[
            {"kind":"image","path":f.path("same image.png"),"duration":0.4},
            {"kind":"video","path":f.path("video.mp4")},
            {"kind":"image","path":f.path("same image.png"),"duration":0.8},
            {"kind":"image","path":f.path("photo.jpg"),"duration":0.6}
        ],
        "wav":wav,"output":f.path("mcp-export.mp4"),"ffmpeg":f.ffmpeg,
        "fade_in":0.2,"fade_out":0.2
    });
    let cli_arguments = args![
        "--soundtrack",
        wav,
        "--image",
        f.path("same image.png"),
        "0.4",
        "--video",
        f.path("video.mp4"),
        "--image",
        f.path("same image.png"),
        "0.8",
        "--image",
        f.path("photo.jpg"),
        "0.6",
        "--fade-in",
        "0.2",
        "--fade-out",
        "0.2",
        "--partial-fades"
    ];
    let mut client = Client::spawn(Some(&f.ffmpeg));
    client.initialize("2025-11-25");
    let inspect = start(
        &mut client,
        "noh_inspect",
        json!({"path":f.path("photo.jpg"),"kind":"image"}),
    );
    let metadata = structured(&terminal(&mut client, &inspect));
    assert_eq!(metadata["state"], "succeeded", "{metadata}");
    assert_eq!(metadata["result"]["kind"], "metadata");
    assert!(metadata["result"]["info"]["width"].as_u64().unwrap() > 0);
    let exact = start(&mut client, "noh_diagnose", json!({"request":request}));
    let result = structured(&terminal(&mut client, &exact));
    assert_eq!(result["state"], "succeeded", "{result}");
    let diagnosis = &result["result"]["diagnosis"];
    assert_eq!(diagnosis["version"], 1);
    assert_eq!(diagnosis["request"]["items"], request["items"]);
    assert!(diagnosis["request"].get("videos").is_none());
    let clips = diagnosis["plan"]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 4);
    assert_eq!(clips[0]["path"], clips[2]["path"]);
    support::near(clips[0]["source_seconds"].as_f64().unwrap(), 0.4, 0.001);
    support::near(clips[2]["source_seconds"].as_f64().unwrap(), 0.8, 0.001);
    for index in [0, 2, 3] {
        assert_eq!(clips[index]["treatment"], "convert");
    }
    let mut cli = cli_arguments.clone();
    cli.extend(args![
        "--output",
        f.path("mcp-export.mp4"),
        "--diagnose=exact",
        "--json"
    ]);
    let cli: Value = serde_json::from_slice(&f.cli(cli).stdout).unwrap();
    let noh::engine::Event::Inspected(Ok(noh::inspection::InspectionResult::Plan(cli_diagnosis))) =
        serde_json::from_value(cli["result"].clone()).unwrap()
    else {
        panic!("{cli}")
    };
    assert_eq!(
        serde_json::to_value(cli_diagnosis.plan).unwrap(),
        diagnosis["plan"]
    );
    let export = start(
        &mut client,
        "noh_export",
        json!({"request":request,"diagnosis_job_id":exact}),
    );
    let exported = structured(&terminal(&mut client, &export));
    assert_eq!(exported["state"], "succeeded", "{exported}");
    let mut cli = cli_arguments.clone();
    cli.extend(args!["--output", f.path("cli-export.mp4")]);
    f.cli(cli);
    assert_eq!(
        f.hashes(&f.path("mcp-export.mp4"), false, true, false),
        f.hashes(&f.path("cli-export.mp4"), false, true, false)
    );
    support::near(f.duration(&f.path("mcp-export.mp4")), 3.44, 0.13);
    request["output"] = json!(f.path("mcp-preview.mp4"));
    let preview = start(&mut client, "noh_preview", json!({"request":request}));
    let previewed = structured(&terminal(&mut client, &preview));
    assert_eq!(previewed["state"], "succeeded", "{previewed}");
    let mut cli = cli_arguments;
    cli.extend(args!["--output", f.path("cli-preview.mp4"), "--preview"]);
    f.cli(cli);
    assert_eq!(
        f.hashes(&f.path("mcp-preview.mp4"), false, true, false),
        f.hashes(&f.path("cli-preview.mp4"), false, true, false)
    );
    support::near(f.duration(&f.path("mcp-preview.mp4")), 3.44, 0.13);
    request["items"][0]["duration"] = json!(0.8);
    request["output"] = json!(f.path("changed-duration.mp4"));
    let stale = client.tool(
        "noh_export",
        json!({"request":request,"diagnosis_job_id":exact}),
    );
    assert_eq!(stale["isError"], true, "{stale}");
    assert_eq!(
        structured(&stale)["error"]["code"],
        "error.stale_inspection"
    );
    assert!(!f.path("changed-duration.mp4").exists());
    client.close_input();
    client.wait_exit();
    f.clean();
}
