//! Native geometry and caption-window regressions for vertical short renders.
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use noh::{
    captions::CaptionStyle,
    engine::Event,
    jobs::Job,
    shorts::{Framing, OUTPUT_HEIGHT, OUTPUT_WIDTH, ShortCaptions, ShortRequest},
};
use std::{path::Path, time::Duration};
use support::Fixture;

fn request(f: &Fixture, source: &Path, output: &Path, framing: Framing) -> ShortRequest {
    ShortRequest {
        source: source.to_path_buf(),
        output: output.to_path_buf(),
        ffmpeg: f.ffmpeg.clone(),
        start_ms: 0,
        end_ms: 400,
        framing,
        captions: None,
        preview: false,
    }
}

fn render(f: &Fixture, request: &ShortRequest) -> Vec<Event> {
    let job = Job::short_with_worker(request.clone(), f.exe.clone(), || {});
    let mut events = Vec::new();
    loop {
        let event = job
            .events
            .recv_timeout(Duration::from_secs(55))
            .expect("short job exceeded its bounded test deadline");
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

fn assert_rendered(events: &[Event]) {
    assert!(
        matches!(events.last(), Some(Event::Done(Ok(_)))),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Warning(message) if message.code == "short.reencode"
        )),
        "short rendering should report its video re-encode: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Progress { percent: 100, .. })),
        "successful publication should finish progress: {events:?}"
    );
}

fn geometry(f: &Fixture, path: &Path) -> (u32, u32, String) {
    let hash = f.packets(path, false, false, false);
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

fn gray_frames(f: &Fixture, path: &Path, width: usize, height: usize) -> Vec<u8> {
    let raw = f
        .ff(args![
            "-i",
            path,
            "-map",
            "0:v:0",
            "-vf",
            format!("scale={width}:{height}:flags=area,format=gray"),
            "-fps_mode",
            "passthrough",
            "-f",
            "rawvideo",
            "-"
        ])
        .stdout;
    assert_eq!(raw.len() % (width * height), 0);
    raw
}

#[test]
fn centered_padding_and_crop_preserve_distinctive_source_regions() {
    let f = Fixture::new();
    let source = f.path("three-regions.mkv");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=red:s=320x180:r=10:d=0.4",
        "-vf",
        "drawbox=x=90:y=0:w=140:h=180:color=lime:t=fill,drawbox=x=230:y=0:w=90:h=180:color=blue:t=fill",
        "-c:v",
        "ffv1",
        &source
    ]);

    let padded = f.path("regions-padded.mp4");
    let padded_request = request(&f, &source, &padded, Framing::Pad);
    let events = render(&f, &padded_request);
    assert_rendered(&events);
    assert_eq!(
        geometry(&f, &padded),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT, "1/1".into())
    );
    let frame = &gray_frames(&f, &padded, 54, 96)[..54 * 96];
    assert!(
        frame[4 * 54 + 27] < 20,
        "padding above the source should be black"
    );
    assert!(
        frame[92 * 54 + 27] < 20,
        "padding below the source should be black"
    );
    let row = &frame[48 * 54..49 * 54];
    assert!(
        row[4] < 110,
        "left source region should remain red: {}",
        row[4]
    );
    assert!(
        row[27] > 100,
        "center source region should remain green: {}",
        row[27]
    );
    assert!(
        row[50] < 70,
        "right source region should remain blue: {}",
        row[50]
    );

    let cropped = f.path("regions-cropped.mp4");
    let events = render(&f, &request(&f, &source, &cropped, Framing::Crop));
    assert_rendered(&events);
    assert_eq!(
        geometry(&f, &cropped),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT, "1/1".into())
    );
    let frame = &gray_frames(&f, &cropped, 54, 96)[..54 * 96];
    for x in [4, 27, 50] {
        assert!(
            frame[48 * 54 + x] > 100,
            "centered crop should retain the green center region at x={x}: {}",
            frame[48 * 54 + x]
        );
    }
}

#[test]
fn sar_and_quarter_turn_metadata_are_normalized_before_framing() {
    let f = Fixture::new();
    let sar_source = f.path("wide-sar-source.mkv");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=gray:s=320x180:r=10:d=0.4",
        "-vf",
        "setsar=2/1",
        "-c:v",
        "ffv1",
        &sar_source
    ]);
    let sar_output = f.path("wide-sar-short.mp4");
    let events = render(&f, &request(&f, &sar_source, &sar_output, Framing::Pad));
    assert_rendered(&events);
    assert_eq!(
        geometry(&f, &sar_output),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT, "1/1".into())
    );
    let frame = &gray_frames(&f, &sar_output, 108, 192)[..108 * 192];
    let occupied_rows = (0..192).filter(|&y| frame[y * 108 + 54] > 25).count();
    assert!(
        (24..=38).contains(&occupied_rows),
        "2:1 SAR should produce a centered 640x180 displayed image, got {occupied_rows} rows"
    );

    let raw = f.path("rotation-raw.mp4");
    let rotated = f.path("rotation-source.mp4");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=0.4",
        "-vf",
        "drawbox=x=0:y=0:w=60:h=180:color=white:t=fill",
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
    let rotation_output = f.path("rotation-short.mp4");
    let events = render(&f, &request(&f, &rotated, &rotation_output, Framing::Pad));
    assert_rendered(&events);
    assert_eq!(
        geometry(&f, &rotation_output),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT, "1/1".into())
    );
    let frame = &gray_frames(&f, &rotation_output, 108, 192)[..108 * 192];
    let mean_rows = |first: usize| -> f64 {
        let mut sum = 0u64;
        for y in first..first + 20 {
            sum += frame[y * 108..(y + 1) * 108]
                .iter()
                .map(|&value| u64::from(value))
                .sum::<u64>();
        }
        sum as f64 / (108.0 * 20.0)
    };
    let top = mean_rows(0);
    let bottom = mean_rows(172);
    assert!(
        (top > 150.0) ^ (bottom > 150.0),
        "the raw left stripe should rotate into one horizontal edge: top={top}, bottom={bottom}"
    );
}

#[test]
fn captions_clip_to_the_selected_interval_and_preview_after_composition() {
    let f = Fixture::new();
    let source = f.path("caption-source.mkv");
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "color=black:s=320x180:r=10:d=1",
        "-c:v",
        "ffv1",
        &source
    ]);
    let srt = f.path("reviewed.srt");
    std::fs::write(
        &srt,
        "1\n00:00:00,150 --> 00:00:00,300\nCROSS LEFT\n\n\
         2\n00:00:00,300 --> 00:00:00,450\nMIDDLE\n\n\
         3\n00:00:00,450 --> 00:00:00,700\nCROSS RIGHT\n\n\
         4\n00:00:00,700 --> 00:00:00,800\nAFTER\n\n",
    )
    .unwrap();

    let output = f.path("caption-preview.mp4");
    let mut preview = request(&f, &source, &output, Framing::Pad);
    preview.start_ms = 200;
    preview.end_ms = 600;
    preview.captions = Some(ShortCaptions {
        subtitles: srt.clone(),
        style: CaptionStyle::default(),
    });
    preview.preview = true;
    let events = render(&f, &preview);
    assert_rendered(&events);
    assert_eq!(geometry(&f, &output), (360, 640, "1/1".into()));

    let frames = gray_frames(&f, &output, 360, 640);
    let frame_size = 360 * 640;
    assert!(
        frames.len() >= frame_size * 4,
        "expected selected video frames"
    );
    let caption_pixels = |frame: &[u8]| {
        frame[550 * 360..625 * 360]
            .iter()
            .filter(|&&pixel| pixel > 180)
            .count()
    };
    for index in [0, 1, 3] {
        assert!(
            caption_pixels(&frames[index * frame_size..(index + 1) * frame_size]) > 10,
            "expected a clipped cue in preview frame {index}"
        );
    }
    let last_frame = &frames[3 * frame_size..4 * frame_size];
    assert!(
        last_frame[630 * 360..].iter().all(|&pixel| pixel < 60),
        "caption should respect the preview bottom margin"
    );

    let outside = f.path("outside-selection.srt");
    std::fs::write(
        &outside,
        "1\n00:00:00,000 --> 00:00:00,200\nTOUCH START\n\n\
         2\n00:00:00,600 --> 00:00:00,800\nTOUCH END\n\n",
    )
    .unwrap();
    let no_cues_output = f.path("no-cues.mp4");
    let mut no_cues = request(&f, &source, &no_cues_output, Framing::Pad);
    no_cues.start_ms = 200;
    no_cues.end_ms = 600;
    no_cues.captions = Some(ShortCaptions {
        subtitles: outside,
        style: CaptionStyle::default(),
    });
    let events = render(&f, &no_cues);
    assert_rendered(&events);
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::Warning(message) if message.code == "short.no_captions"
        )),
        "a valid selection with no intersecting cues should render without captions: {events:?}"
    );
    assert_eq!(
        geometry(&f, &no_cues_output),
        (OUTPUT_WIDTH, OUTPUT_HEIGHT, "1/1".into())
    );
}
