//! Real caption rendering through the CLI worker, using synthetic media only.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
use noh::{
    captions::{CaptionRequest, CaptionStyle},
    engine::Event,
    jobs::Job,
};
use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};
use support::Fixture;

fn request(f: &Fixture) -> CaptionRequest {
    CaptionRequest {
        scene: None,
        effect: None,
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        output: f.path("captioned.mp4"),
        ffmpeg: f.ffmpeg.clone(),
        style: CaptionStyle::default(),
        preview: false,
        promote: false,
    }
}
/// The longest a burn may say nothing before the test calls it hung. A burn
/// is silent while its captions are drawn and encoded, from the encoder
/// being chosen to FFmpeg's first progress line: about 1 s here for a burn
/// alone, up to 11 s in a debug build when this file's tests start side by
/// side, and more than the former 45 s at times on the slowest CI runner,
/// where the whole file takes 4 to 8 times longer than here. The limit is
/// four times the former 45 s, so that a slow machine is not taken for a
/// hang.
const SILENCE: Duration = Duration::from_secs(180);

fn burn(f: &Fixture, req: &CaptionRequest) -> Vec<Event> {
    let job = Job::burn_with_worker(req.clone(), f.exe.clone(), || {});
    let started = Instant::now();
    let mut events = Vec::new();
    loop {
        // A burn that hangs says which one, when, and what it had reported.
        let event = job.events.recv_timeout(SILENCE).unwrap_or_else(|e| {
            panic!(
                "Caption job deadline: {e:?} after {SILENCE:?} of silence, {:?} into the burn of {:?} to {:?}; reports so far: {events:?}",
                started.elapsed(),
                std::fs::read_to_string(&req.subtitles)
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect::<String>(),
                req.output.file_name().unwrap_or_default()
            )
        });
        let done = event.is_terminal();
        events.push(event);
        if done {
            break;
        }
    }
    drop(job);
    f.clean();
    events
}
fn success(events: &[Event]) {
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(
        matches!(
            events.get(events.len() - 2),
            Some(Event::Progress { percent: 100, .. })
        ),
        "{events:?}"
    );
    assert!(
        events.iter().any(
            |event| matches!(event, Event::Warning(message) if message.code=="caption.reencode")
        )
    );
}
fn black(f: &Fixture, output: &Path, width: usize, height: usize, seconds: usize) {
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        format!("color=black:s={width}x{height}:r=10:d={seconds}"),
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        output
    ]);
}
fn gray(f: &Fixture, source: &Path) -> Vec<u8> {
    f.ff(args![
        "-i",
        source,
        "-map",
        "0:v:0",
        "-pix_fmt",
        "gray",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-"
    ])
    .stdout
}
fn pts(f: &Fixture, source: &Path, audio: bool) -> Vec<f64> {
    let packets = f.packets(source, false, false, audio);
    let tb = packets
        .lines()
        .find_map(|l| l.strip_prefix("#tb 0: "))
        .unwrap();
    let (n, d) = tb.split_once('/').unwrap();
    let step = n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap();
    let mut times: Vec<_> = support::rows(&packets)
        .into_iter()
        .map(|r| r[2].parse::<f64>().unwrap() * step)
        .collect();
    times.sort_by(f64::total_cmp);
    times
}
#[test]
fn pre_cancelled_captions_touch_no_files() {
    let f = Fixture::new();
    let job = Job::burn_with_cancel(
        request(&f),
        f.path("missing-worker"),
        Arc::new(AtomicBool::new(true)),
        || {},
    );
    assert!(matches!(
        job.events.recv_timeout(Duration::from_secs(3)).unwrap(),
        Event::Cancelled
    ));
    drop(job);
    assert_eq!(std::fs::read_dir(f.folder.path()).unwrap().count(), 0);
}
#[test]
fn literal_unicode_captions_follow_cues_and_stay_inside_margins() {
    let f = Fixture::new();
    let mut req = request(&f);
    req.source = f.path("L'été, 日本語.mp4");
    req.subtitles = f.path("L'été, 字幕.srt");
    black(&f, &req.source, 640, 360, 3);
    let texts = [
        r"{\pos(0,0)\alpha&HFF&} \N literal",
        "Français Straße español",
        "日本語 한국어 中文",
        "e\u{301} and tabs\there",
        "LongWordWithoutSpacesLongWordWithoutSpacesLongWordWithoutSpaces",
    ];
    let mut srt = String::new();
    for (i, text) in texts.iter().enumerate() {
        let start = i * 500 + 100;
        let end = start + 300;
        srt.push_str(&format!(
            "{}\n00:00:{:02},{:03} --> 00:00:{:02},{:03}\n{text}\n\n",
            i + 1,
            start / 1000,
            start % 1000,
            end / 1000,
            end % 1000
        ));
    }
    std::fs::write(&req.subtitles, srt).unwrap();
    let original = std::fs::read(&req.source).unwrap();
    success(&burn(&f, &req));
    let frames = gray(&f, &req.output);
    assert_eq!(frames.len(), 30 * 640 * 360);
    for (index, frame) in frames.as_chunks::<{ 640 * 360 }>().0.iter().enumerate() {
        let visible: Vec<_> = frame
            .iter()
            .enumerate()
            .filter_map(|(i, p)| (*p > 180).then_some((i % 640, i / 640)))
            .collect();
        let should_show = index < 25 && (1..4).contains(&(index % 5));
        assert_eq!(!visible.is_empty(), should_show, "caption at frame {index}");
        if should_show {
            assert!(
                visible
                    .iter()
                    .all(|(x, y)| *x > 8 && *x < 632 && *y > 8 && *y < 352),
                "clipped text at frame {index}"
            );
        }
    }
    assert_eq!(std::fs::read(&req.source).unwrap(), original);
    let existing = std::fs::read(&req.output).unwrap();
    let duplicate = burn(&f, &req);
    assert!(matches!(duplicate.last(),Some(Event::Done(Err(e))) if e.code=="error.output_exists"));
    assert_eq!(std::fs::read(&req.output).unwrap(), existing);
}
#[test]
fn invalid_tracks_and_unsupported_or_overflowing_text_never_publish() {
    let f = Fixture::new();
    let req = request(&f);
    black(&f, &req.source, 320, 180, 1);
    for (text, code) in [
        ("not SRT".to_string(), "error.caption_track"),
        (
            "1\n00:00:00,100 --> 00:00:00,900\n\u{E000}\n".into(),
            "error.caption_layout",
        ),
        (
            "1\n00:00:00,100 --> 00:00:02,000\nText\n".into(),
            "error.caption_track",
        ),
    ] {
        std::fs::write(&req.subtitles, text).unwrap();
        let events = burn(&f, &req);
        assert!(
            matches!(events.last(),Some(Event::Done(Err(e))) if e.code==code),
            "{events:?}"
        );
        assert!(!req.output.exists());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::Progress { percent: 100, .. }))
        );
    }
    // libass's own limits (Task 43): a cue too tall for the picture, and one that
    // collapses at libass's centiseconds. Classique draws them (the tall one with a
    // warning that captions leave the picture); libass, without OpenGL, refuses
    // them as before.
    // One long line of 600 words, too tall for 320 x 180 once wrapped (whether
    // typed lines or one line: Classique keeps a cue's typed lines).
    let tall = format!(
        "1\n00:00:00,100 --> 00:00:00,900\n{}\n",
        std::iter::repeat_n("Text", 600)
            .collect::<Vec<_>>()
            .join(" ")
    );
    for (text, refused, warning) in [
        (
            tall,
            "error.caption_layout",
            Some("warning.effect_off_frame"),
        ),
        (
            "1\n00:00:00,101 --> 00:00:00,102\nText\n".to_string(),
            "error.caption_track",
            None,
        ),
    ] {
        let _ = std::fs::remove_file(&req.output);
        std::fs::write(&req.subtitles, text).unwrap();
        let events = burn(&f, &req);
        let warned = |code: &str| {
            events
                .iter()
                .any(|e| matches!(e, Event::Warning(m) if m.code == code))
        };
        if warned("warning.effect_fallback") {
            assert!(
                matches!(events.last(), Some(Event::Done(Err(e))) if e.code == refused),
                "{events:?}"
            );
            assert!(!req.output.exists());
        } else {
            assert!(
                matches!(events.last(), Some(Event::Done(Ok(_)))),
                "{events:?}"
            );
            if let Some(code) = warning {
                assert!(warned(code), "{events:?}");
            }
        }
    }
}

#[test]
fn platform_presets_keep_rendered_multiline_ink_inside_guides_at_both_sizes() {
    use noh::{
        captions::{CaptionPlacement, CaptionSize},
        safe_area::SafeArea,
    };
    let f = Fixture::new();
    let mut req = request(&f);
    black(&f, &req.source, 1080, 1920, 1);
    std::fs::write(&req.subtitles,
        "1\n00:00:00,000 --> 00:00:01,000\nFrançais Straße español 日本語 한국어 中文\nKeep captions clear of buttons\n").unwrap();
    req.style.size = CaptionSize::Large;
    for area in [
        SafeArea::YoutubeShorts,
        SafeArea::Tiktok,
        SafeArea::Universal,
    ] {
        for placement in [CaptionPlacement::Bottom, CaptionPlacement::Top] {
            for preview in [false, true] {
                req.style.safe_area = area;
                req.style.placement = placement;
                req.preview = preview;
                req.output = f.path(&format!("{area:?}-{placement:?}-{preview}.mp4"));
                success(&burn(&f, &req));
                let (w, h) = if preview { (302, 540) } else { (1080, 1920) };
                let pixels = f
                    .ff(args![
                        "-i",
                        &req.output,
                        "-frames:v",
                        "1",
                        "-pix_fmt",
                        "gray",
                        "-f",
                        "rawvideo",
                        "-"
                    ])
                    .stdout;
                assert_eq!(pixels.len() % (w * h), 0);
                let [left, top, right, bottom] =
                    area.insets(w as u32, h as u32).unwrap().map(|v| v as usize);
                let mut count = 0;
                for (i, value) in pixels.iter().take(w * h).enumerate() {
                    if *value > 80 {
                        let (x, y) = (i % w, i / w);
                        assert!(
                            x >= left && x < w - right && y >= top && y < h - bottom,
                            "{area:?} {placement:?} preview={preview}: ink outside safe area at {x},{y}"
                        );
                        count += 1;
                    }
                }
                assert!(count > 100, "captions must actually render");
            }
        }
    }
}
#[test]
fn delayed_video_keeps_presentation_times_and_original_aac_packets() {
    let f = Fixture::new();
    let req = request(&f);
    f.ff(args![
        "-itsoffset",
        "1",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-c:v",
        "libx264",
        "-bf",
        "2",
        "-fps_mode",
        "passthrough",
        "-c:a",
        "aac",
        &req.source
    ]);
    std::fs::write(
        &req.subtitles,
        "1\n00:00:01,100 --> 00:00:01,400\nDelayed video\n\n",
    )
    .unwrap();
    success(&burn(&f, &req));
    let before = pts(&f, &req.source, false);
    let after = pts(&f, &req.output, false);
    assert_eq!(before.len(), after.len());
    assert!(before[0] > 0.99, "{before:?}");
    for (a, b) in before.iter().zip(after) {
        support::near(b, *a, 0.00001);
    }
    assert_eq!(
        f.hashes(&req.source, false, false, true),
        f.hashes(&req.output, false, false, true)
    );
    let before_audio = pts(&f, &req.source, true);
    let after_audio = pts(&f, &req.output, true);
    assert_eq!(before_audio.len(), after_audio.len());
    for (a, b) in before_audio.iter().zip(after_audio) {
        support::near(b, *a, 0.00003);
    }
    assert!(f.duration(&req.output) > 3.99, "audio tail lost");
    // Task 43: the picture ends where the source's does, to the tick, through the
    // caption engine's pipe and its -itsoffset (fx/export.rs compensates FFmpeg
    // 7.1's last-frame duration); a different FFmpeg behaviour fails here.
    let video_end = |path: &std::path::Path| {
        let packets = f.packets(path, false, false, false);
        let (a, b) = packets
            .lines()
            .find_map(|l| l.strip_prefix("#tb 0: "))
            .unwrap()
            .split_once('/')
            .unwrap();
        let step = a.parse::<f64>().unwrap() / b.parse::<f64>().unwrap();
        support::rows(&packets)
            .iter()
            .map(|row| {
                (row[2].parse::<i64>().unwrap() + row[3].parse::<i64>().unwrap()) as f64 * step
            })
            .fold(0.0, f64::max)
    };
    support::near(video_end(&req.output), video_end(&req.source), 0.00005);
    let frames = gray(&f, &req.output);
    for (index, frame) in frames.as_chunks::<{ 320 * 180 }>().0.iter().enumerate() {
        // Classique draws about 10 px letters at 320 x 180, unhinted: their
        // brightest pixel is about 170 (libass's hinted ones reach 211), on black.
        assert_eq!(
            frame.iter().any(|p| *p > 120),
            (1..4).contains(&index),
            "frame {index}"
        );
    }
}
#[test]
fn cli_preview_and_export_share_composition_and_report_json_success() {
    let f = Fixture::new();
    let req = request(&f);
    black(&f, &req.source, 1280, 720, 1);
    std::fs::write(&req.subtitles,"1\n00:00:00,100 --> 00:00:00,900\n日本語の字幕 Long text which should retain identical line breaks between export and preview\n\n").unwrap();
    let mut masks = Vec::new();
    for preview in [false, true] {
        let path = f.path(if preview { "preview.mp4" } else { "full.mp4" });
        let mut args = args![
            "--burn-subtitles",
            &req.source,
            "--srt",
            &req.subtitles,
            "--output",
            &path,
            "--caption-placement",
            "top",
            "--json"
        ];
        if preview {
            args.push("--preview".into());
        }
        let reply = f.cli(args);
        let json: serde_json::Value = serde_json::from_slice(&reply.stdout).unwrap();
        assert_eq!(json["result"]["event"], "done");
        assert!(json["result"]["data"]["Ok"].is_object(), "{json}");
        let sampled = f
            .ff(args![
                "-i",
                &path,
                "-ss",
                "0.3",
                "-frames:v",
                "1",
                "-vf",
                "scale=320:180",
                "-pix_fmt",
                "gray",
                "-f",
                "rawvideo",
                "-"
            ])
            .stdout;
        assert_eq!(sampled.len(), 320 * 180);
        let occupied_rows: Vec<bool> = sampled
            .as_chunks::<320>()
            .0
            .iter()
            .map(|row| row.iter().any(|p| *p > 100))
            .collect();
        let start = occupied_rows.iter().position(|p| *p).unwrap();
        let end = occupied_rows.iter().rposition(|p| *p).unwrap();
        assert!(end < 90, "top placement failed");
        masks.push((start, end));
    }
    assert!(
        masks[0].0.abs_diff(masks[1].0) <= 1 && masks[0].1.abs_diff(masks[1].1) <= 1,
        "{masks:?}"
    );
}

/// Step 7 B: the caption burn draws an effect chosen by id (the command line and
/// the MCP resolve it the same way); a font setting naming an installed family
/// this machine lacks is drawn in the effect's own face and said, once.
#[test]
#[ignore = "needs an OpenGL 3.3 GPU and FFmpeg on PATH"]
fn a_chosen_effect_burns_and_warns_when_its_installed_font_is_missing() {
    use noh::effect_choice::{Chosen, Flags, Given, Wanted, resolve_in};
    let f = Fixture::new();
    black(&f, &f.path("source.mp4"), 640, 360, 3);
    std::fs::write(
        f.path("reviewed.srt"),
        "1\n00:00:00,500 --> 00:00:02,500\nOn a gardé la lumière\n",
    )
    .unwrap();
    words_beside(&f.path("reviewed.srt"));
    let family = "NOH Missing Family For Tests";
    let wanted = Wanted {
        id: "noh.drop".into(),
        preset: None,
        values: vec![("font".into(), Given::Text(format!("system:{family}")))],
    };
    let roots = [Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/effects")];
    let Ok(Chosen::Plugin(choice)) = resolve_in(
        &wanted,
        CaptionStyle::default(),
        Flags::default(),
        false,
        &roots,
    ) else {
        panic!("noh.drop resolves to its bundled folder");
    };
    let mut req = request(&f);
    req.effect = Some(choice);
    let events = burn(&f, &req);
    success(&events);
    let warned: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Warning(m) if m.code == "warning.effect_font_missing" => Some(m.args.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(warned, vec![vec![family.to_string()]], "{events:?}");
    // The effect drew with the word times kept beside the subtitles.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Diagnostic(d) if d.contains("word times: 1 cues"))),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .all(|e| !matches!(e, Event::Warning(m) if m.code == "warning.effect_fallback")),
        "a chosen effect has no libass fallback: {events:?}"
    );
    assert!(f.path("captioned.mp4").exists());
}

/// A lyrics file with hand-placed word times beside `srt`, for its one cue
/// "On a gardé la lumière" at 0.5..2.5 s.
fn words_beside(srt: &Path) {
    use noh::subtitle_track::{SubtitleCue, SubtitleTrack, WordSource, WordTime};
    let words = [
        ("On", 500),
        ("a", 800),
        ("gardé", 1100),
        ("la", 1500),
        ("lumière", 1800),
    ]
    .map(|(text, start_ms)| WordTime {
        text: text.into(),
        start_ms,
        end_ms: None,
        source: WordSource::Hand,
        syllables: None,
    })
    .to_vec();
    let track = SubtitleTrack {
        voices: Vec::new(),
        language: None,
        duration_ms: 3_000,
        cues: vec![SubtitleCue {
            voices: Vec::new(),
            start_ms: 500,
            end_ms: 2_500,
            text: "On a gardé la lumière".into(),
            words: Some(words),
        }],
    };
    std::fs::write(
        noh::subtitle_words::lyrics_path(srt),
        noh::subtitle_words::to_lyrics_json(&track, None),
    )
    .unwrap();
}
