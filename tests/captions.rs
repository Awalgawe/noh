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
    time::Duration,
};
use support::Fixture;

fn request(f: &Fixture) -> CaptionRequest {
    CaptionRequest {
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        output: f.path("captioned.mp4"),
        ffmpeg: f.ffmpeg.clone(),
        style: CaptionStyle::default(),
        preview: false,
    }
}
fn burn(f: &Fixture, req: &CaptionRequest) -> Vec<Event> {
    let job = Job::burn_with_worker(req.clone(), f.exe.clone(), || {});
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(45))
            .expect("Caption job deadline");
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
            format!(
                "1\n00:00:00,100 --> 00:00:00,900\n{}\n",
                std::iter::repeat_n("Text", 70)
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
            "error.caption_layout",
        ),
        (
            "1\n00:00:00,101 --> 00:00:00,102\nText\n".into(),
            "error.caption_track",
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
    let frames = gray(&f, &req.output);
    for (index, frame) in frames.as_chunks::<{ 320 * 180 }>().0.iter().enumerate() {
        assert_eq!(
            frame.iter().any(|p| *p > 180),
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
