//! The GUI reads each clip with the quick, header-only inspection and lays caption
//! effects out on the export canvas from it, before any analysis. That read must
//! carry the picture size (a missing size made a 2x2 canvas: no caption visible).
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;

use support::Fixture;

#[test]
fn the_quick_read_gives_the_export_canvas() {
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=640x360:rate=25:duration=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        f.path("wide.mp4")
    ]);
    // A phone clip: landscape pixels with a quarter-turn display matrix (set on a
    // stream copy; on an encode FFmpeg would rotate the pixels themselves).
    f.ff(args![
        "-display_rotation:v:0",
        "90",
        "-i",
        f.path("wide.mp4"),
        "-c",
        "copy",
        f.path("phone.mp4")
    ]);
    for (name, size, canvas) in [
        ("wide.mp4", (640, 360), (640, 360)),
        ("phone.mp4", (640, 360), (360, 640)),
    ] {
        let path = f.path(name);
        let info = noh::media::preview_info(&f.ffmpeg, &path).unwrap();
        assert_eq!((info.width, info.height), size, "{name}");
        assert_eq!(
            noh::plan::export_canvas(&[path.into()], &[info]),
            Some(canvas),
            "{name}"
        );
    }
}
