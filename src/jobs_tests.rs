use super::*;

#[test]
fn caption_and_short_publication_recheck_inputs_and_reject_invalid_worker_results() {
    let folder = tempfile::tempdir().unwrap();
    let script = folder.path().join("caption_worker.rs");
    let worker = folder.path().join(if cfg!(windows) {
        "caption_worker.exe"
    } else {
        "caption_worker"
    });
    let source = folder.path().join("source.mp4");
    let srt = folder.path().join("reviewed.srt");
    std::fs::write(&script, r##"use std::{env,fs,path::Path};
fn main() {
 let args:Vec<_>=env::args().collect();
 let stage=Path::new(&args[1]).parent().unwrap().join("export.mp4");
 let scenario=args[2].as_str();
 if scenario=="directory" {fs::create_dir(&stage).unwrap();}
 else if scenario!="missing" {fs::write(&stage,if scenario=="empty" {b"".as_slice()}else{b"rendered".as_slice()}).unwrap();}
 println!("{}",r#"{"event":"diagnostic","data":"staged"}"#);
 for _ in 0..match scenario {"ten-warnings"=>10,"eleven-warnings"=>11,_=>0} {
   println!("{}",r#"{"event":"warning","data":{"code":"short.reencode","args":[]}}"#);
 }
 if scenario=="wrong-terminal" {println!("{}",r#"{"event":"subtitled","data":{"Ok":{"output":"ignored","track":{"language":null,"duration_ms":1000,"cues":[]}}}}"#);}
 else if scenario=="zero-duration" {println!("{}",r#"{"event":"done","data":{"Ok":{"output":"untrusted.mp4","duration":0.0}}}"#);}
 else {println!("{}",r#"{"event":"done","data":{"Ok":{"output":"untrusted.mp4","duration":1.0}}}"#);}
}
"##).unwrap();
    let mut compiler = crate::command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    compiler.arg(&script).arg("-o").arg(&worker);
    #[cfg(all(windows, target_env = "gnu"))]
    compiler.args(["-C", "linker=rust-lld", "-C", "linker-flavor=ld.lld"]);
    let (status, log) = crate::process::capture(compiler, Duration::from_secs(60), true).unwrap();
    assert!(status.success(), "{log}");
    for short in [false, true] {
        for scenario in [
            "success",
            "cancel",
            "late-cancel",
            "collision",
            "source-change",
            "srt-change",
            "directory",
            "missing",
            "empty",
            "wrong-terminal",
            "zero-duration",
            "ten-warnings",
            "eleven-warnings",
        ] {
            std::fs::write(&source, b"source").unwrap();
            std::fs::write(&srt, b"track").unwrap();
            let output = folder.path().join(format!("{short}-{scenario}.mp4"));
            let operation = if short {
                Operation::Short(ShortRequest {
                    source: source.clone(),
                    output: output.clone(),
                    ffmpeg: worker.clone(),
                    start_ms: 0,
                    end_ms: 1000,
                    framing: Default::default(),
                    captions: Some(crate::shorts::ShortCaptions {
                        subtitles: srt.clone(),
                        style: Default::default(),
                    }),
                    preview: false,
                })
            } else {
                Operation::Captions(CaptionRequest {
                    source: source.clone(),
                    subtitles: srt.clone(),
                    output: output.clone(),
                    ffmpeg: worker.clone(),
                    style: Default::default(),
                    preview: false,
                })
            };
            let cancel = AtomicBool::new(false);
            let mut published = false;
            let result = run_with_command(
                &worker,
                &operation,
                &cancel,
                |wire| {
                    let mut command = crate::command(&worker);
                    command.arg(wire).arg(scenario);
                    command
                },
                |event| match event {
                    Event::Diagnostic(ref line) if line == "staged" => match scenario {
                        "cancel" => cancel.store(true, Ordering::Release),
                        "collision" => std::fs::write(&output, b"other publisher").unwrap(),
                        "source-change" => std::fs::write(&source, b"modified source").unwrap(),
                        "srt-change" => std::fs::write(&srt, b"modified track").unwrap(),
                        _ => {}
                    },
                    Event::Progress { percent: 100, .. } => {
                        published = true;
                        assert_eq!(std::fs::read(&output).unwrap(), b"rendered");
                        if scenario == "late-cancel" {
                            cancel.store(true, Ordering::Release);
                        }
                    }
                    _ => {}
                },
            );
            if matches!(scenario, "success" | "late-cancel")
                || (short && scenario == "ten-warnings")
            {
                assert!(
                    matches!(result,Ok(Outcome::Export(ref value)) if value.output==output),
                    "{scenario}: {:?}",
                    result.err()
                );
                assert!(published);
            } else {
                let expected = match scenario {
                    "cancel" => "status.export_cancelled",
                    "collision" => "error.output_exists",
                    "source-change" | "srt-change" => "error.stale_inspection",
                    "wrong-terminal" => "error.protocol",
                    "ten-warnings" | "eleven-warnings" => "error.protocol",
                    _ if short => "error.short_result",
                    _ => "error.caption_result",
                };
                assert_eq!(result.err().unwrap().code, expected, "{scenario}");
                assert!(!published);
                if scenario == "collision" {
                    assert_eq!(std::fs::read(&output).unwrap(), b"other publisher");
                } else {
                    assert!(!output.exists());
                }
            }
            assert!(!std::fs::read_dir(folder.path()).unwrap().any(|entry| {
                entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".noh-")
            }));
        }
    }
}

#[test]
fn short_wire_is_optional_for_v1_and_roundtrips_without_changing_other_operations() {
    let wire: WorkerRequest = serde_json::from_value(serde_json::json!({
        "version":1,"request":null,"workspace":"stage"
    }))
    .unwrap();
    assert!(wire.shorts.is_none());
    let request = ShortRequest {
        source: "source.mp4".into(),
        output: "out.mp4".into(),
        ffmpeg: "ffmpeg".into(),
        start_ms: 135,
        end_ms: 1775,
        framing: crate::shorts::Framing::Crop,
        captions: None,
        preview: true,
    };
    let wire = WorkerRequest {
        shorts: Some(request.clone()),
        ..wire
    };
    let decoded: WorkerRequest =
        serde_json::from_slice(&serde_json::to_vec(&wire).unwrap()).unwrap();
    assert_eq!(decoded.version, 1);
    assert_eq!(decoded.shorts, Some(request));
    assert!(
        decoded.request.is_none()
            && decoded.inspection.is_none()
            && decoded.subtitles.is_none()
            && decoded.captions.is_none()
            && decoded.expected.is_none()
    );
}

#[test]
fn pre_cancelled_short_never_validates_or_launches_a_worker() {
    let folder = tempfile::tempdir().unwrap();
    let job = Job::short_with_cancel(
        ShortRequest {
            source: folder.path().join("missing-source.mp4"),
            output: folder.path().join("out.mp4"),
            ffmpeg: folder.path().join("missing-ffmpeg"),
            start_ms: 0,
            end_ms: 0,
            framing: Default::default(),
            captions: None,
            preview: false,
        },
        folder.path().join("missing-worker"),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(
        job.events.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Cancelled
    ));
    drop(job);
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[test]
fn cancellation_on_either_side_of_publication_preserves_commit_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let staged = folder.path().join("staged.mp4");
    let output = folder.path().join("output.mp4");
    std::fs::write(&staged, b"owned").unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        commit(&staged, &output, &cancel).unwrap_err().code,
        "status.export_cancelled"
    );
    assert!(!output.exists());
    cancel.store(false, Ordering::Release);
    let result = commit(&staged, &output, &cancel);
    cancel.store(true, Ordering::Release);
    assert!(result.is_ok());
    assert_eq!(std::fs::read(&output).unwrap(), b"owned");
    std::fs::write(&staged, b"new").unwrap();
    cancel.store(false, Ordering::Release);
    assert_eq!(
        commit(&staged, &output, &cancel).unwrap_err().code,
        "error.output_exists"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"owned");
}

#[test]
fn publication_and_cancellation_use_the_commit_point_across_threads() {
    use std::sync::Barrier;
    for cancel_first in [true, false] {
        let folder = tempfile::tempdir().unwrap();
        let staged = folder.path().join("staged.mp4");
        let output = folder.path().join("final.mp4");
        std::fs::write(&staged, b"complete").unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Barrier::new(2));
        let (tx, rx) = mpsc::channel();
        let task = {
            let cancel = cancel.clone();
            let gate = gate.clone();
            let output = output.clone();
            thread::spawn(move || {
                if cancel_first {
                    gate.wait();
                }
                let result = commit(&staged, &output, &cancel);
                tx.send(()).unwrap();
                if !cancel_first {
                    gate.wait();
                }
                result
            })
        };
        if cancel_first {
            cancel.store(true, Ordering::Release);
            gate.wait();
        } else {
            rx.recv_timeout(Duration::from_secs(5)).unwrap();
            cancel.store(true, Ordering::Release);
            gate.wait();
        }
        let result = task.join().unwrap();
        assert_eq!(result.is_ok(), !cancel_first);
        assert_eq!(output.exists(), !cancel_first);
    }
}

#[cfg(windows)]
#[test]
fn blocked_inspection_and_assembly_cancel_confirmed_descendants_and_owned_staging() {
    let folder = tempfile::tempdir().unwrap();
    let script = folder.path().join("worker.ps1");
    let descendant = folder.path().join("child.ps1");
    std::fs::write(
        &descendant,
        r#"param($Signal)
$ready = [Threading.EventWaitHandle]::OpenExisting($Signal)
[void]$ready.Set()
[Threading.Thread]::Sleep([Threading.Timeout]::Infinite)
"#,
    )
    .unwrap();
    std::fs::write(&script, r#"param($RequestFile, $Phase)
$ErrorActionPreference = 'Stop'
$wire = Get-Content -Raw -LiteralPath $RequestFile | ConvertFrom-Json
$signal = 'Local\noh-' + [Guid]::NewGuid().ToString()
$ready = [Threading.EventWaitHandle]::new($false, [Threading.EventResetMode]::ManualReset, $signal)
$start = [Diagnostics.ProcessStartInfo]::new()
$start.FileName = Join-Path $PSHOME 'powershell.exe'
$start.Arguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + (Join-Path $PSScriptRoot 'child.ps1') + '" ' + $signal
$start.UseShellExecute = $false
$start.CreateNoWindow = $true
$child = [Diagnostics.Process]::Start($start)
if (!$ready.WaitOne(10000)) { throw 'Child handshake timed out' }
@{event='diagnostic';data=$wire.workspace} | ConvertTo-Json -Compress
@{event='progress';data=@{percent=40;phase=@{code=$Phase;args=@([string]$child.Id)}}} | ConvertTo-Json -Compress -Depth 5
[Threading.Thread]::Sleep([Threading.Timeout]::Infinite)
"#).unwrap();
    let source = folder.path().join("source.mp4");
    std::fs::write(&source, b"fixture").unwrap();
    let ffmpeg = crate::inspection::resolve_ffmpeg(Path::new("powershell.exe")).unwrap();
    for phase in [
        "progress.analyze_clip",
        "progress.assemble",
        "subtitle.transcribing",
        "caption.preparing",
        "caption.rendering",
        "short.preparing",
        "short.rendering",
    ] {
        let output = folder.path().join(if phase.starts_with("subtitle.") {
            "result.srt"
        } else {
            "result.mp4"
        });
        let request = ExportRequest {
            items: vec![source.clone().into()],
            wav: source.clone(),
            output: output.clone(),
            ffmpeg: ffmpeg.clone(),
            fade_in: 0.0,
            fade_out: 0.0,
            partial_fades: true,
            preview: false,
            clip_audio: false,
            force_encode: false,
        };
        let operation = if phase == "progress.analyze_clip" {
            Operation::Inspect(InspectionRequest::Plan {
                request,
                level: crate::inspection::Level::Exact,
            })
        } else if phase.starts_with("subtitle.") {
            Operation::Subtitles(SubtitleRequest {
                source: source.clone(),
                output: output.clone(),
                ffmpeg: ffmpeg.clone(),
                transcriber: ffmpeg.clone(),
                model: source.clone(),
                vad_model: source.clone(),
                language: "auto".into(),
            })
        } else if phase.starts_with("short.") {
            Operation::Short(ShortRequest {
                source: source.clone(),
                output: output.clone(),
                ffmpeg: ffmpeg.clone(),
                start_ms: 0,
                end_ms: 1000,
                framing: Default::default(),
                captions: None,
                preview: false,
            })
        } else if phase.starts_with("caption.") {
            Operation::Captions(CaptionRequest {
                source: source.clone(),
                subtitles: source.clone(),
                output: output.clone(),
                ffmpeg: ffmpeg.clone(),
                style: Default::default(),
                preview: false,
            })
        } else {
            Operation::Export(request, None)
        };
        let cancel = AtomicBool::new(false);
        let mut workspace = None;
        let mut handle = std::ptr::null_mut();
        unsafe extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
            fn WaitForSingleObject(handle: *mut std::ffi::c_void, ms: u32) -> u32;
            fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        }
        let result = run_with_command(
            &ffmpeg,
            &operation,
            &cancel,
            |request_file| {
                let mut command = crate::command(&ffmpeg);
                command
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-File",
                    ])
                    .arg(&script)
                    .arg(request_file)
                    .arg(phase);
                command
            },
            |event| match event {
                Event::Diagnostic(path) => workspace = Some(PathBuf::from(path)),
                Event::Progress { phase, .. } => {
                    handle = unsafe { OpenProcess(0x00100000, 0, phase.args[0].parse().unwrap()) };
                    assert!(!handle.is_null());
                    assert_eq!(unsafe { WaitForSingleObject(handle, 0) }, 258);
                    cancel.store(true, Ordering::Release);
                }
                _ => {}
            },
        );
        assert!(
            matches!(&result, Err(e) if e.code == "status.export_cancelled"),
            "{:?}; diagnostic: {workspace:?}",
            result.err()
        );
        assert!(!handle.is_null());
        assert_eq!(unsafe { WaitForSingleObject(handle, 5000) }, 0);
        unsafe {
            CloseHandle(handle);
        }
        assert!(!workspace.unwrap().exists());
        assert!(!output.exists());
        assert_eq!(std::fs::read(&source).unwrap(), b"fixture");
    }
}

#[cfg(windows)]
#[test]
fn subtitle_protocol_and_publication_preserve_validated_track_and_cancel_semantics() {
    let folder = tempfile::tempdir().unwrap();
    let script = folder.path().join("subtitle_worker.rs");
    let source = folder.path().join("source.wav");
    let model = folder.path().join("model.bin");
    let worker = folder.path().join("subtitle_worker.exe");
    std::fs::write(&source, b"source").unwrap();
    std::fs::write(&script, r##"use std::{env,fs,path::Path};
fn main() {
 let args:Vec<_>=env::args().collect();
 let directory=Path::new(&args[1]).parent().unwrap();
 let scenario=args[2].as_str();
 let srt=if scenario=="mismatch" {"wrong text"} else {"1\n00:00:00,000 --> 00:00:01,000\nText\n\n"};
 if scenario=="directory" {fs::create_dir(directory.join("subtitle.srt")).unwrap();}
 else if scenario!="missing" {fs::write(directory.join("subtitle.srt"),srt).unwrap();}
 println!("{}",r#"{"event":"diagnostic","data":"staged"}"#);
 let result=if scenario=="wrong-terminal" {r#"{"event":"done","data":{"Ok":{"output":"ignored.mp4","duration":1.0}}}"#.to_string()}
 else {r#"{"event":"subtitled","data":{"Ok":{"output":"untrusted.srt","track":{"language":"en","duration_ms":1000,"cues":[{"start_ms":0,"end_ms":END,"text":"Text"}]}}}}"#.replace("END",if scenario=="bad-track" {"2000"}else{"1000"})};
 let result=if scenario=="long-track" {result.replace("\"duration_ms\":1000","\"duration_ms\":7200001")}else{result};
 println!("{result}");
}
"##).unwrap();
    // Use a tiny Rust fixture, without depending on script execution policy or
    // changing machine settings. The compiler is already required for these tests.
    let mut compiler = crate::command(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    compiler.arg(&script).arg("-o").arg(&worker);
    #[cfg(all(windows, target_env = "gnu"))]
    compiler.args(["-C", "linker=rust-lld", "-C", "linker-flavor=ld.lld"]);
    let (status, diagnostic) =
        crate::process::capture(compiler, Duration::from_secs(60), true).unwrap();
    assert!(status.success(), "Fixture compilation failed: {diagnostic}");
    for scenario in [
        "success",
        "late-cancel",
        "cancel",
        "collision",
        "model-change",
        "mismatch",
        "missing",
        "directory",
        "long-track",
        "bad-track",
        "wrong-terminal",
    ] {
        std::fs::write(&model, b"model").unwrap();
        let output = folder.path().join(format!("{scenario}.srt"));
        let request = SubtitleRequest {
            source: source.clone(),
            output: output.clone(),
            ffmpeg: worker.clone(),
            transcriber: worker.clone(),
            model: model.clone(),
            vad_model: model.clone(),
            language: "auto".into(),
        };
        let cancel = AtomicBool::new(false);
        let mut published = false;
        let result = run_with_command(
            &worker,
            &Operation::Subtitles(request),
            &cancel,
            |wire| {
                let mut command = crate::command(&worker);
                command.arg(wire).arg(scenario);
                command
            },
            |event| match event {
                Event::Diagnostic(ref line) if line == "staged" => match scenario {
                    "cancel" => cancel.store(true, Ordering::Release),
                    "collision" => std::fs::write(&output, b"other publisher").unwrap(),
                    "model-change" => std::fs::write(&model, b"modified model").unwrap(),
                    _ => {}
                },
                Event::Progress { percent: 100, .. } => {
                    published = true;
                    assert!(output.is_file());
                    if scenario == "late-cancel" {
                        cancel.store(true, Ordering::Release);
                    }
                }
                Event::Diagnostic(line) => eprintln!("Worker diagnostic: {line}"),
                _ => {}
            },
        );
        if matches!(scenario, "success" | "late-cancel") {
            let Ok(Outcome::Subtitles(result)) = result else {
                panic!(
                    "Subtitle success expected for {scenario}: {:?}",
                    result.err()
                );
            };
            assert_eq!(result.output, output);
            assert_eq!(
                std::fs::read_to_string(&output).unwrap(),
                result.track.to_srt().unwrap()
            );
            assert!(published);
        } else {
            let error = result.err().expect("Invalid worker result must fail");
            let expected = match scenario {
                "cancel" => "status.export_cancelled",
                "collision" => "error.output_exists",
                "model-change" => "error.subtitle_config",
                "wrong-terminal" => "error.protocol",
                "long-track" => "error.subtitle_limit",
                _ => "error.subtitle_result",
            };
            assert_eq!(error.code, expected, "{scenario}: {error}");
            assert!(!published);
            if scenario == "collision" {
                assert_eq!(std::fs::read(&output).unwrap(), b"other publisher");
            } else {
                assert!(!output.exists());
            }
        }
        assert!(!std::fs::read_dir(folder.path()).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".noh-")
        }));
    }
}
