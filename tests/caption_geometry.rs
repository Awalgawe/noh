//! Geometry and stream-timeline regressions for direct caption burns.
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
use std::{path::Path, time::Duration};
use support::Fixture;

fn request(f: &Fixture, source: &Path, subtitles: &Path, output: &Path) -> CaptionRequest {
    CaptionRequest {
        source: source.to_path_buf(),
        subtitles: subtitles.to_path_buf(),
        output: output.to_path_buf(),
        ffmpeg: f.ffmpeg.clone(),
        style: CaptionStyle::default(),
        preview: false,
    }
}

fn burn(f: &Fixture, request: &CaptionRequest) -> Vec<Event> {
    let job = Job::burn_with_worker(request.clone(), f.exe.clone(), || {});
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(55))
            .expect("caption job exceeded its bounded test deadline");
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

fn assert_burned(events: &[Event]) {
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Warning(message) if message.code == "caption.reencode"
        )),
        "the irreversible video encode should be reported: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Progress { percent: 100, .. })),
        "successful publication should finish progress: {events:?}"
    );
}

fn frame_geometry(f: &Fixture, path: &Path, decode: bool) -> (u32, u32, String) {
    let hash = f.packets(path, false, decode, false);
    let dimensions = hash
        .lines()
        .find_map(|line| line.strip_prefix("#dimensions 0: "))
        .expect("video dimensions in framehash header");
    let (width, height) = dimensions.split_once('x').unwrap();
    let sar = hash
        .lines()
        .find_map(|line| line.strip_prefix("#sar 0: "))
        .unwrap_or("1/1")
        .to_owned();
    (width.parse().unwrap(), height.parse().unwrap(), sar)
}

fn decoded_gray(f: &Fixture, path: &Path, width: usize, height: usize) -> Vec<u8> {
    let frames = f
        .ff(args![
            "-i",
            path,
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-vf",
            "format=gray",
            "-f",
            "rawvideo",
            "-"
        ])
        .stdout;
    assert_eq!(frames.len() % (width * height), 0);
    frames
}

fn decoded_pts(f: &Fixture, path: &Path, audio: bool) -> Vec<f64> {
    let packets = support::text(
        &f.ff(args![
            "-i",
            path,
            "-map",
            if audio { "0:a:0" } else { "0:v:0" },
            "-fps_mode",
            "passthrough",
            "-enc_time_base",
            "filter",
            "-f",
            "framehash",
            "-"
        ])
        .stdout,
    );
    let time_base = packets
        .lines()
        .find_map(|line| line.strip_prefix("#tb 0: "))
        .expect("framehash stream time base");
    let (numerator, denominator) = time_base.split_once('/').unwrap();
    let scale = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
    let mut points: Vec<_> = support::rows(&packets)
        .into_iter()
        .map(|row| row[2].parse::<f64>().unwrap() * scale)
        .collect();
    points.sort_by(f64::total_cmp);
    points
}

fn framehash_audio_end(f: &Fixture, path: &Path) -> (String, f64) {
    let packets = f.packets(path, false, false, true);
    let time_base = packets
        .lines()
        .find_map(|line| line.strip_prefix("#tb 0: "))
        .expect("audio stream time base");
    let (numerator, denominator) = time_base.split_once('/').unwrap();
    let scale = numerator.parse::<f64>().unwrap() / denominator.parse::<f64>().unwrap();
    let codec = packets
        .lines()
        .find_map(|line| line.strip_prefix("#codec_id 0: "))
        .unwrap_or_default()
        .to_owned();
    let end = support::rows(&packets)
        .iter()
        .map(|row| (row[2].parse::<i64>().unwrap() + row[3].parse::<i64>().unwrap()) as f64 * scale)
        .fold(f64::NEG_INFINITY, f64::max);
    (codec, end)
}

#[test]
fn sar_and_quarter_turn_metadata_produce_correct_square_pixel_geometry() {
    let f = Fixture::new();
    let sar_source = f.path("sar-source.mp4");
    let sar_srt = f.path("sar.srt");
    let sar_output = f.path("sar-burn.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=10:d=2",
        "-vf",
        "setsar=2/1",
        "-c:v",
        "libx264",
        &sar_source
    ]);
    std::fs::write(&sar_srt, "1\n00:00:00,200 --> 00:00:01,400\nSAR\n\n").unwrap();
    let events = burn(&f, &request(&f, &sar_source, &sar_srt, &sar_output));
    assert_burned(&events);
    let (width, height, sar) = frame_geometry(&f, &sar_output, true);
    assert_eq!((width, height, sar.as_str()), (640, 180, "1/1"));

    let raw = f.path("rotation-raw.mp4");
    let rotated = f.path("rotation-source.mp4");
    let rotation_srt = f.path("rotation.srt");
    let rotation_output = f.path("rotation-burn.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=2",
        "-vf",
        "drawbox=x=0:y=0:w=50:h=180:color=white:t=fill",
        "-c:v",
        "libx264",
        &raw
    ]);
    f.ff(args![
        "-display_rotation:v:0",
        "90",
        "-i",
        &raw,
        "-c",
        "copy",
        &rotated
    ]);
    std::fs::write(
        &rotation_srt,
        "1\n00:00:01,000 --> 00:00:01,500\nRotation\n\n",
    )
    .unwrap();
    let events = burn(&f, &request(&f, &rotated, &rotation_srt, &rotation_output));
    assert_burned(&events);
    let (width, height, sar) = frame_geometry(&f, &rotation_output, true);
    assert_eq!((width, height, sar.as_str()), (180, 320, "1/1"));
    let frames = decoded_gray(&f, &rotation_output, width as usize, height as usize);
    let frame = &frames[..width as usize * height as usize];
    let row_mean = |y: usize| -> f64 {
        let start = y * width as usize;
        frame[start..start + width as usize]
            .iter()
            .map(|pixel| f64::from(*pixel))
            .sum::<f64>()
            / f64::from(width)
    };
    let top = (8..42).map(row_mean).sum::<f64>() / 34.0;
    let bottom = (278..312).map(row_mean).sum::<f64>() / 34.0;
    assert!(
        (top > 180.0) ^ (bottom > 180.0),
        "rotation was not applied once: top={top}, bottom={bottom}"
    );
}

#[test]
fn vfr_decoded_presentation_times_survive_and_unknown_last_duration_is_reported() {
    let f = Fixture::new();
    let source = f.path("vfr-source.mkv");
    let srt = f.path("vfr.srt");
    let output = f.path("vfr-burn.mp4");
    // Keep the nominal frame duration below Matroska's millisecond time base.
    // FFmpeg 6 may otherwise infer a 100 ms duration where FFmpeg 7 reports zero.
    // The explicit PTS still span 2.15 seconds and include a 350 ms VFR gap.
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=10000:d=0.002",
        "-vf",
        "setpts=1000*N+2500*gte(N\\,10)",
        "-fps_mode",
        "vfr",
        "-c:v",
        "ffv1",
        &source
    ]);
    let packets = f.packets(&source, false, false, false);
    let last = support::rows(&packets)
        .pop()
        .expect("fixture has video packets");
    assert_eq!(
        last[3], "0",
        "fixture must expose an unknown final packet duration: {packets}"
    );
    std::fs::write(&srt, "1\n00:00:01,300 --> 00:00:01,700\nVFR\n\n").unwrap();
    let events = burn(&f, &request(&f, &source, &srt, &output));
    assert_burned(&events);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Warning(message) if message.code == "caption.unknown_duration"
        )),
        "unknown final frame duration must be surfaced: {events:?}"
    );
    let before = decoded_pts(&f, &source, false);
    let after = decoded_pts(&f, &output, false);
    assert_eq!(before.len(), 20);
    assert_eq!(before.len(), after.len());
    assert!(
        before.windows(2).any(|pair| pair[1] - pair[0] > 0.15),
        "fixture must contain a VFR gap: {before:?}"
    );
    for (expected, actual) in before.iter().zip(after) {
        support::near(actual, *expected, 0.0011);
    }
}

#[test]
fn irregular_vfr_preserves_off_grid_pts_and_the_known_final_frame_duration() {
    let f = Fixture::new();
    let source = f.path("off-grid.mkv");
    let srt = f.path("off-grid.srt");
    let expression = "if(eq(N,0),0,if(eq(N,1),123,if(eq(N,2),171,if(eq(N,3),489,701))))";
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=0.5",
        "-vf",
        "settb=1/1000",
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
    std::fs::write(
        &srt,
        "1\n00:00:00,200 --> 00:00:00,800\nIrregular timestamps\n\n",
    )
    .unwrap();
    let expected = [0.0, 0.123, 0.171, 0.489, 0.701];
    let before = decoded_pts(&f, &source, false);
    assert_eq!(before.len(), expected.len());
    for (a, b) in before.iter().zip(expected) {
        support::near(*a, b, 0.00001);
    }
    for preview in [false, true] {
        let output = f.path(if preview {
            "off-grid-preview.mp4"
        } else {
            "off-grid-burn.mp4"
        });
        let mut req = request(&f, &source, &srt, &output);
        req.preview = preview;
        assert_burned(&burn(&f, &req));
        let after = decoded_pts(&f, &output, false);
        assert_eq!(after.len(), expected.len());
        for (a, b) in after.iter().zip(expected) {
            support::near(*a, b, 0.00001);
        }
        let packets = f.packets(&output, false, false, false);
        let (a, b) = packets
            .lines()
            .find_map(|l| l.strip_prefix("#tb 0: "))
            .unwrap()
            .split_once('/')
            .unwrap();
        let step = a.parse::<f64>().unwrap() / b.parse::<f64>().unwrap();
        let end = support::rows(&packets)
            .iter()
            .map(|row| {
                (row[2].parse::<i64>().unwrap() + row[3].parse::<i64>().unwrap()) as f64 * step
            })
            .fold(0.0, f64::max);
        support::near(end, 0.801, 0.0011);
        support::near(f.duration(&output), 0.801, 0.011);
    }
}

#[test]
fn first_non_aac_audio_is_converted_and_its_long_tail_remains_audible() {
    let f = Fixture::new();
    let source = f.path("pcm-two-audio-source.mkv");
    let srt = f.path("audio.srt");
    let output = f.path("audio-burn.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=10:d=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=880:sample_rate=48000:duration=1",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-map",
        "2:a:0",
        "-c:v",
        "libx264",
        "-c:a",
        "pcm_s16le",
        &source
    ]);
    std::fs::write(&srt, "1\n00:00:00,200 --> 00:00:01,200\nAudio\n\n").unwrap();
    let events = burn(&f, &request(&f, &source, &srt, &output));
    assert_burned(&events);
    for warning in ["caption.audio_convert", "caption.multiple_audio"] {
        assert!(
            events.iter().any(|event| matches!(
                event,
                Event::Warning(message) if message.code == warning
            )),
            "missing {warning} warning: {events:?}"
        );
    }
    let (codec, end) = framehash_audio_end(&f, &output);
    assert_eq!(codec, "aac");
    assert!(
        (3.95..=4.1).contains(&end),
        "audio tail timestamp lost: {end}"
    );
    let decoded_tail = f
        .ff(args![
            "-i",
            &output,
            "-ss",
            "3.5",
            "-t",
            "0.25",
            "-map",
            "0:a:0",
            "-acodec",
            "pcm_s16le",
            "-f",
            "s16le",
            "-"
        ])
        .stdout;
    assert!(decoded_tail.len() > 1000);
    let audible = decoded_tail
        .as_chunks::<2>()
        .0
        .iter()
        .any(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs() > 500);
    assert!(audible, "decoded audio tail is silent");
}

#[test]
fn common_positive_container_origin_matches_playback_zero_caption_times() {
    let f = Fixture::new();
    let source = f.path("common-offset-source.mkv");
    let srt = f.path("common-offset.srt");
    let output = f.path("common-offset-burn.mp4");
    f.ff(args![
        "-itsoffset",
        "5",
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=2",
        "-itsoffset",
        "5",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
        "-map",
        "0:v:0",
        "-map",
        "1:a:0",
        "-c:v",
        "libx264",
        "-c:a",
        "pcm_s16le",
        &source
    ]);
    std::fs::write(&srt, "1\n00:00:00,100 --> 00:00:00,600\nOrigin\n\n").unwrap();
    let source_pts = decoded_pts(&f, &source, false);
    assert_eq!(source_pts.first().copied(), Some(0.0));
    let events = burn(&f, &request(&f, &source, &srt, &output));
    assert_burned(&events);
    let output_pts = decoded_pts(&f, &output, false);
    assert_eq!(source_pts.len(), output_pts.len());
    for (expected, actual) in source_pts.iter().zip(output_pts) {
        support::near(actual, *expected, 0.0011);
    }
    let frames = decoded_gray(&f, &output, 320, 180);
    for index in 1..6 {
        let frame = &frames[index * 320 * 180..(index + 1) * 320 * 180];
        assert!(
            frame[120 * 320..].iter().any(|pixel| *pixel > 180),
            "time-zero SRT did not appear at playback t={:.1}",
            index as f64 / 10.0
        );
    }
}
