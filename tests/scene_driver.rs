//! A driver that dies compiling a layer shader takes the worker down, never the
//! caller: the scene job fails with what happened and publishes nothing. The
//! death is forced where the compilation happens (`ShaderSet::compile`) through
//! NOH_TEST_SHADER_ABORT, in the worker this test binary starts; one test per
//! binary keeps the variable from reaching other tests.
#![cfg(feature = "motion")]
#[path = "../tools/process.rs"]
mod process;
#[macro_use]
#[allow(dead_code)]
mod support;
use noh::{
    engine::Event,
    jobs::Job,
    safe_area::SafeArea,
    scene_request::{SceneMode, SceneRequest},
};
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
use support::Fixture;

#[test]
#[ignore = "needs an OpenGL 3.3 GPU and FFmpeg on PATH"]
fn a_driver_that_dies_on_a_shader_fails_the_scene_job_cleanly() {
    let f = Fixture::new();
    f.ff(args![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=320x180:r=10:d=1",
        "-c:v",
        "libx264",
        "-pix_fmt",
        "yuv420p",
        f.path("source.mp4")
    ]);
    std::fs::write(
        f.path("reviewed.srt"),
        "1\n00:00:00,100 --> 00:00:00,900\nla\n",
    )
    .unwrap();
    std::fs::write(
        f.path("scene.noh-motion.rhai"),
        "// noh-motion 0\nfn frame(t, ctx) { draw(layer().shader(\"plain\")); }\n",
    )
    .unwrap();
    std::fs::create_dir(f.path("scene.noh-motion.shaders")).unwrap();
    std::fs::write(
        f.path("scene.noh-motion.shaders").join("plain.wgsl"),
        "fn layer(i: LayerIn) -> vec4<f32> { return vec4<f32>(i.uv, 0.0, 1.0); }",
    )
    .unwrap();
    // SAFETY: the only test of this binary, before it starts any thread.
    unsafe { std::env::set_var("NOH_TEST_SHADER_ABORT", "1") };
    let request = SceneRequest {
        scene: f.path("scene.noh-motion.rhai"),
        source: f.path("source.mp4"),
        subtitles: f.path("reviewed.srt"),
        ffmpeg: f.ffmpeg.clone(),
        safe_area: SafeArea::None,
        from_ms: None,
        to_ms: None,
        mode: SceneMode::Check,
        bare: false,
        grid: false,
        output: f.path("check"),
    };
    let job = Job::scene_with_cancel(
        request,
        f.exe.clone(),
        Arc::new(AtomicBool::new(false)),
        || {},
    );
    let last = loop {
        let e = job
            .events
            .recv_timeout(Duration::from_secs(120))
            .expect("silent job");
        if e.is_terminal() {
            break e;
        }
    };
    match last {
        Event::Done(Err(e)) => assert_eq!(e.code, "error.scene_worker_stopped", "{e:?}"),
        other => panic!("expected the worker's death to be said, got {other:?}"),
    }
    assert!(!f.path("check").exists());
}
