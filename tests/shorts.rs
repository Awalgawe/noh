//! Selected-interval presentation timing, decoded frame identity and audio.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use noh::{
    engine::Event,
    jobs::Job,
    shorts::{Framing, ShortRequest},
};
use std::{path::Path, time::Duration};
use support::Fixture;

fn request(f: &Fixture, source: &Path, output: &str, start_ms: u64, end_ms: u64) -> ShortRequest {
    ShortRequest {
        source: source.into(),
        output: f.path(output),
        ffmpeg: f.ffmpeg.clone(),
        start_ms,
        end_ms,
        framing: Framing::Pad,
        captions: None,
        preview: true,
    }
}

fn run(f: &Fixture, request: &ShortRequest) -> Vec<Event> {
    let job = Job::short_with_worker(request.clone(), f.exe.clone(), || {});
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(55))
            .expect("Short test deadline");
        let terminal = event.is_terminal();
        events.push(event);
        if terminal {
            break;
        }
    }
    drop(job);
    f.clean();
    events
}

fn success(events: &[Event]) -> f64 {
    let Some(Event::Done(Ok(result))) = events.last() else {
        panic!("{events:?}")
    };
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Progress { percent: 100, .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Warning(m) if m.code == "short.reencode"))
    );
    result.duration
}

fn raw_packets(f: &Fixture, path: &Path, decode: bool, audio: bool) -> Vec<(f64, f64)> {
    let mut args = args![
        "-copyts",
        "-i",
        path,
        "-map",
        if audio { "0:a:0" } else { "0:v:0" }
    ];
    if decode {
        args.extend(args![
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "filter"
        ]);
    } else {
        args.extend(args!["-c", "copy"]);
    }
    args.extend(args!["-f", "framehash", "-"]);
    let text = support::text(&f.ff(args).stdout);
    let (n, d) = text
        .lines()
        .find_map(|l| l.strip_prefix("#tb 0: "))
        .unwrap()
        .split_once('/')
        .unwrap();
    let tb = n.parse::<f64>().unwrap() / d.parse::<f64>().unwrap();
    let mut result: Vec<_> = support::rows(&text)
        .iter()
        .map(|r| {
            let pts = r[2].parse::<f64>().unwrap() * tb;
            (pts, pts + r[3].parse::<f64>().unwrap() * tb)
        })
        .collect();
    result.sort_by(|a, b| a.0.total_cmp(&b.0));
    result
}

fn shades(f: &Fixture, path: &Path) -> Vec<u8> {
    f.ff(args![
        "-i",
        path,
        "-vf",
        "crop=8:8:(iw-8)/2:(ih-8)/2,scale=1:1,format=gray",
        "-fps_mode",
        "passthrough",
        "-f",
        "rawvideo",
        "-"
    ])
    .stdout
}

#[test]
fn between_keyframe_cut_keeps_selected_b_frame_pictures_and_final_duration() {
    let f = Fixture::new();
    let source = f.path("bframes.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "nullsrc=s=128x72:r=25:d=2",
        "-vf",
        "geq=lum='16+N*4':cb=128:cr=128",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        "-g",
        "50",
        "-crf",
        "10",
        &source
    ]);
    let mut req = request(&f, &source, "cut.mp4", 135, 1775);
    req.preview = false;
    let events = run(&f, &req);
    support::near(success(&events), 1.64, 0.001);
    let original = shades(&f, &source);
    let cut = shades(&f, &req.output);
    assert_eq!(cut.len(), 41);
    for (got, expected) in cut.iter().zip(&original[4..45]) {
        assert!(
            got.abs_diff(*expected) <= 3,
            "Decoded picture mismatch: {got}, {expected}"
        );
    }
    for decode in [false, true] {
        let packets = raw_packets(&f, &req.output, decode, false);
        assert_eq!(packets.len(), 41);
        support::near(packets[0].0, 0.025, 0.0001);
        support::near(packets.last().unwrap().1, 1.64, 0.0001);
    }
    support::near(f.duration(&req.output), 1.64, 0.011);
    assert!(events.iter().any(|e| matches!(e, Event::Warning(m) if m.code=="short.boundaries" && m.args==["0.025","1.640","1.640"])));
}

#[test]
fn irregular_vfr_cut_keeps_all_selected_frames_without_tail_extension() {
    let f = Fixture::new();
    let source = f.path("irregular.mkv");
    let expression = "if(eq(N,0),0,if(eq(N,1),123,if(eq(N,2),171,if(eq(N,3),489,701))))";
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "nullsrc=s=128x72:r=10:d=0.5",
        "-vf",
        "geq=lum='32+N*32':cb=128:cr=128,settb=1/1000",
        "-frames:v",
        "5",
        "-fps_mode",
        "passthrough",
        "-enc_time_base:v",
        "filter",
        "-c:v",
        "ffv1",
        "-bsf:v",
        format!("setts=pts='{expression}':dts='{expression}':duration=100"),
        &source
    ]);
    let original = shades(&f, &source);
    for preview in [false, true] {
        let mut req = request(
            &f,
            &source,
            if preview { "preview.mp4" } else { "full.mp4" },
            140,
            740,
        );
        req.preview = preview;
        support::near(success(&run(&f, &req)), 0.600, 0.0001);
        for decode in [false, true] {
            let packets = raw_packets(&f, &req.output, decode, false);
            assert_eq!(packets.len(), 3);
            for ((pts, _), expected) in packets.iter().zip([0.031, 0.349, 0.561]) {
                support::near(*pts, expected, 0.0001);
            }
            support::near(packets.last().unwrap().1, 0.600, 0.0001);
        }
        let cut = shades(&f, &req.output);
        assert_eq!(cut.len(), 3);
        for (a, b) in cut.iter().zip(&original[2..]) {
            assert!(a.abs_diff(*b) <= 3);
        }
        support::near(f.duration(&req.output), 0.60, 0.011);
    }
    // No picture has a presentation timestamp in this small gap. Never publish
    // an empty clip or reach back to an earlier frame to fake success.
    let empty = request(&f, &source, "gap.mp4", 200, 300);
    let events = run(&f, &empty);
    assert!(
        matches!(events.last(), Some(Event::Done(Err(_)))),
        "{events:?}"
    );
    assert!(!empty.output.exists());
}

#[test]
fn delayed_audio_keeps_common_positive_origin_and_is_absent_before_its_start() {
    let f = Fixture::new();
    let source = f.path("delayed.mkv");
    f.ff(args![
        "-itsoffset",
        "3",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=128x72:r=25:d=2",
        "-itsoffset",
        "4",
        "-f",
        "lavfi",
        "-i",
        "aevalsrc='if(eq(n,24000),0.9,0)':s=48000:d=1",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-bf",
        "3",
        "-c:a",
        "pcm_s16le",
        &source
    ]);
    let req = request(&f, &source, "delayed-cut.mp4", 135, 1775);
    let events = run(&f, &req);
    let measured = success(&events);
    let audio_packets = raw_packets(&f, &req.output, false, true);
    let audio_end = audio_packets
        .iter()
        .map(|(_, end)| *end)
        .fold(0.0, f64::max);
    support::near(measured, audio_end.max(1.64), 0.0001);
    // AAC's last packet can include padding; container and video bounds are
    // checked separately rather than silently widening their tolerances.
    assert!(audio_end <= 1.64 + 1024.0 / 48000.0);
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::Warning(m) if m.code == "short.audio_padding"))
    );
    support::near(f.duration(&req.output), 1.64, 0.011);
    let video = raw_packets(&f, &req.output, true, false);
    assert_eq!(video.len(), 41);
    support::near(video[0].0, 0.025, 0.0001);
    support::near(video.last().unwrap().1, 1.64, 0.0001);
    let audio = raw_packets(&f, &req.output, true, true);
    let decoded = f
        .ff(args![
            "-i",
            &req.output,
            "-map",
            "0:a:0",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "-"
        ])
        .stdout;
    let samples = decoded.as_chunks::<2>().0;
    let peak = samples
        .iter()
        .enumerate()
        .max_by_key(|(_, b)| i16::from_le_bytes(**b).unsigned_abs())
        .unwrap()
        .0;
    support::near(audio[0].0 + peak as f64 / 48000.0, 1.365, 0.002);
    let before = request(&f, &source, "before-audio.mp4", 135, 775);
    support::near(success(&run(&f, &before)), 0.64, 0.001);
    let streams = support::text(
        &f.ff(args![
            "-i",
            &before.output,
            "-map",
            "0:v:0",
            "-map",
            "0:a:0?",
            "-c",
            "copy",
            "-f",
            "framehash",
            "-"
        ])
        .stdout,
    );
    assert!(!streams.contains(": audio"), "{streams}");
}

#[test]
fn invalid_or_missing_intervals_never_publish_and_existing_output_is_preserved() {
    let f = Fixture::new();
    let source = f.path("source.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=128x72:r=10:d=1",
        "-c:v",
        "libx264",
        &source
    ]);
    for (start, end) in [(1000, 1200), (0, 2000), (200, 200)] {
        let req = request(&f, &source, "invalid.mp4", start, end);
        let events = run(&f, &req);
        assert!(
            matches!(events.last(), Some(Event::Done(Err(e))) if e.code=="error.short_range"),
            "{events:?}"
        );
        assert!(!req.output.exists());
    }
    let req = request(&f, &source, "retained.mp4", 0, 500);
    std::fs::write(&req.output, b"existing user data").unwrap();
    let events = run(&f, &req);
    assert!(
        matches!(events.last(), Some(Event::Done(Err(e))) if e.code=="error.output_exists"),
        "{events:?}"
    );
    assert_eq!(std::fs::read(&req.output).unwrap(), b"existing user data");
}
