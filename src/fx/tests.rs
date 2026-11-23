//! Engine tests. The GPU test needs an OpenGL 3.3 context (a hidden window) and is
//! ignored by default: run it with `cargo test --lib --features effects -- --include-ignored`.
use super::catalog::{Catalog, scan};
use super::line::{Line, split_words};
use super::plugin::PluginScene;
use super::shaders::{Inst, Upload};
use super::text::Fonts;
use std::path::{Path, PathBuf};

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Atlas uploads go nowhere: layout and animation do not need a GPU.
struct NoGpu;
impl Upload for NoGpu {
    fn upload_cov(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
    fn upload_msdf(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
}

fn lines() -> Vec<Line> {
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    vec![
        split_words(0.5, 2.5, &w("Ever since the impact"), " "),
        split_words(3.0, 5.0, &w("nothing feels the same tonight"), " "),
        split_words(5.5, 6.5, &w("夜空に光る"), " "),
    ]
}

/// Every folder of assets/effects loads, Classique among them (built in through
/// `catalog::CLASSIC_JSON`). A style is added by its folder alone: the list of
/// bundled styles is not written here. The engine's other features are exercised
/// by the test-only effects in tests/fixtures/fx (never packaged).
#[test]
fn bundled_effects_load() {
    let bundled = scan(&[repo("assets/effects")]);
    assert!(bundled.rejected.is_empty(), "{:?}", bundled.rejected);
    let folders = std::fs::read_dir(repo("assets/effects"))
        .unwrap()
        .filter(|e| e.as_ref().unwrap().path().is_dir())
        .count();
    assert_eq!(bundled.effects.len(), folders, "every folder is an effect");
    assert!(bundled.effects.iter().any(|e| e.effect.id == "noh.classic"));
    let catalog = scan(&[repo("assets/effects"), repo("tests/fixtures/fx")]);
    assert!(catalog.rejected.is_empty(), "{:?}", catalog.rejected);
    let mut ids: Vec<&str> = catalog
        .effects
        .iter()
        .map(|e| e.effect.id.as_str())
        .filter(|id| id.starts_with("test."))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "test.compose",
            "test.glow",
            "test.motion",
            "test.window",
            "test.wipe"
        ]
    );
    for e in &catalog.effects {
        assert!(
            !e.effect.presets.is_empty(),
            "{} has no preset",
            e.effect.id
        );
        assert_eq!(e.effect.presets.len(), e.effect.preset_names.len());
        assert!(!e.effect.names.is_empty(), "{} has no name", e.effect.id);
    }
}

#[test]
fn invalid_effects_are_refused_with_a_message() {
    let catalog = scan(&[repo("tests/fixtures/effects-invalid")]);
    assert!(
        catalog.effects.is_empty(),
        "{:?}",
        catalog
            .effects
            .iter()
            .map(|e| &e.effect.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(catalog.rejected.len(), 10);
    for r in &catalog.rejected {
        assert!(!r.message.trim().is_empty(), "{}", r.dir.display());
    }
}

/// The command line and the MCP live in `bin/`: their bundled effects are the
/// application's, one folder up; `noh.exe` finds them beside itself.
#[test]
fn bundled_effects_are_the_application_folder_s_from_bin_too() {
    let app = Path::new("C:/Apps/NOH");
    for exe in ["noh.exe", "bin/noh-cli.exe", "bin/noh-mcp.exe"] {
        assert_eq!(
            super::catalog::bundled_dir_for(&app.join(exe)),
            Some(app.join("effects")),
            "{exe}"
        );
    }
}

#[test]
fn duplicate_ids_refuse_both_and_missing_roots_are_ignored() {
    let temp = tempfile::tempdir().unwrap();
    for name in ["a", "b"] {
        let dir = temp.path().join(name);
        std::fs::create_dir(&dir).unwrap();
        std::fs::copy(
            repo("tests/fixtures/fx/wipe/effect.json"),
            dir.join("effect.json"),
        )
        .unwrap();
    }
    let Catalog { effects, rejected } = scan(&[temp.path().into(), temp.path().join("missing")]);
    assert!(effects.is_empty());
    assert_eq!(rejected.len(), 2);
    assert!(rejected.iter().all(|r| r.message.contains("test.wipe")));
}

fn frames(scene: &mut PluginScene, fonts: &mut Fonts) -> Vec<(Vec<Inst>, f32, Vec<Inst>)> {
    (0..140)
        .map(|f| {
            let (g, gain, m, _) = scene.frame(fonts, &NoGpu, f as f64 / 20.0);
            (g, gain, m)
        })
        .collect()
}

/// Non-regression for format changes: the instances every preset of Classique and
/// of the test effects draws
/// (both orientations, the frames() grid) hash to the values recorded for this
/// system: tests/fixtures/fx-instances.txt on Windows, fx-instances-linux.txt on
/// Linux. `NOH_FX_GOLDEN_WRITE=1` rewrites this system's file.
///
/// One file per system because the bits differ: measured on 5796938, Linux and
/// Windows draw the same instance counts everywhere, but 6 of 32 cases differ in
/// glyph x positions by at most 9.8e-4 px (text layout, not NOH's own maths:
/// moving NOH's sin/cos/pow to libm changed nothing). A tolerance cannot be
/// hashed, and an exact guard per system catches any real change. A system with
/// no recorded file (macOS for now) is reported and not compared.
#[test]
fn bundled_presets_match_the_recorded_instances() {
    let fnv = |bytes: &[u8], mut h: u64| {
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    };
    // The instance as recorded: every field before `aux` (the shader hook inputs
    // added later, which these effects never read), so the record still proves
    // they draw the same bytes. `aux` has its own tests.
    let recorded_part = |insts: &[Inst]| -> Vec<u8> {
        let keep = std::mem::offset_of!(Inst, aux);
        insts
            .iter()
            .flat_map(|i| bytemuck::bytes_of(i)[..keep].to_vec())
            .collect()
    };
    let mut fonts = Fonts::load().unwrap();
    let catalog = scan(&[repo("assets/effects"), repo("tests/fixtures/fx")]);
    let mut effects: Vec<_> = catalog.effects.iter().collect();
    effects.sort_by(|a, b| a.effect.id.cmp(&b.effect.id));
    let mut out = String::new();
    for e in effects {
        for (preset, _) in &e.effect.presets {
            for (w, h) in [(1920, 1080), (1080, 1920)] {
                let mut scene = PluginScene::new(e.effect.clone(), w, h, lines(), Some(preset));
                let mut hash = 0xcbf2_9ce4_8422_2325u64;
                for (g, gain, m) in frames(&mut scene, &mut fonts) {
                    hash = fnv(&recorded_part(&g), hash);
                    hash = fnv(&gain.to_bits().to_le_bytes(), hash);
                    hash = fnv(&recorded_part(&m), hash);
                }
                out.push_str(&format!("{} {preset} {w}x{h} {hash:016x}\n", e.effect.id));
            }
        }
    }
    let path = repo(&match std::env::consts::OS {
        "windows" => "tests/fixtures/fx-instances.txt".to_owned(),
        os => format!("tests/fixtures/fx-instances-{os}.txt"),
    });
    if std::env::var_os("NOH_FX_GOLDEN_WRITE").is_some() {
        std::fs::write(&path, &out).unwrap();
        return;
    }
    // Only a system with no record yet skips (macOS: none could be written here).
    // Elsewhere a missing file fails, so the guard cannot vanish with the file.
    if std::env::consts::OS == "macos" && !path.exists() {
        eprintln!(
            "no recorded instances for {}: not compared ({} would hold them)",
            std::env::consts::OS,
            path.display()
        );
        return;
    }
    let recorded = std::fs::read_to_string(&path).expect("recorded instances");
    let recorded = recorded.replace("\r\n", "\n");
    // Every recorded line is drawn again, the same. An effect with no line yet (a
    // style added by its folder alone) is drawn by the other tests and recorded
    // later by a run with NOH_FX_GOLDEN_WRITE on each system.
    let id = |line: &str| line.split(' ').next().unwrap_or("").to_string();
    let known: std::collections::HashSet<String> = recorded.lines().map(id).collect();
    let now: Vec<&str> = out.lines().filter(|l| known.contains(&id(l))).collect();
    let unrecorded: std::collections::BTreeSet<String> =
        out.lines().map(id).filter(|i| !known.contains(i)).collect();
    if !unrecorded.is_empty() {
        eprintln!("not recorded yet, not compared: {unrecorded:?}");
    }
    for (now, before) in now.iter().zip(recorded.lines()) {
        assert_eq!(*now, before, "bundled effects draw different instances");
    }
    assert_eq!(now.len(), recorded.lines().count());
}

#[test]
fn every_bundled_preset_draws_deterministically() {
    let mut fonts = Fonts::load().unwrap();
    let catalog = scan(&[repo("assets/effects"), repo("tests/fixtures/fx")]);
    for e in &catalog.effects {
        for (preset, _) in &e.effect.presets {
            for (w, h) in [(1920, 1080), (1080, 1920)] {
                let mut a = PluginScene::new(e.effect.clone(), w, h, lines(), Some(preset));
                let mut b = PluginScene::new(e.effect.clone(), w, h, lines(), Some(preset));
                let fa = frames(&mut a, &mut fonts);
                let fb = frames(&mut b, &mut fonts);
                let key = |v: &Vec<(Vec<Inst>, f32, Vec<Inst>)>| {
                    v.iter()
                        .map(|(g, gain, m)| {
                            (
                                bytemuck::cast_slice::<Inst, u8>(g).to_vec(),
                                gain.to_bits(),
                                bytemuck::cast_slice::<Inst, u8>(m).to_vec(),
                            )
                        })
                        .collect::<Vec<_>>()
                };
                assert!(
                    key(&fa) == key(&fb),
                    "{} {preset} {w}x{h} is not deterministic",
                    e.effect.id
                );
                // Something is drawn while a line is on screen, nothing outside lines.
                assert!(
                    !fa[30].2.is_empty(),
                    "{} {preset} {w}x{h}: nothing at 1.5 s",
                    e.effect.id
                );
                // Before the first line nothing is drawn, unless a window shows it
                // ahead (within its lead).
                let ahead = !PluginScene::new(e.effect.clone(), w, h, lines(), Some(preset))
                    .visible_lines(0.0)
                    .is_empty();
                assert!(
                    ahead || (fa[0].0.is_empty() && fa[0].2.is_empty()),
                    "{} {preset}: drawn before the first line",
                    e.effect.id
                );
            }
        }
    }
}

/// Exact reference for the YUV mix (as `samerender_split.py`): float64, limited
/// range, chroma from the 2x2 mean of the premultiplied overlay, rounded to nearest.
fn mix_reference(i420: &[u8], ov: &[u8], w: usize, h: usize, kr: f64, kb: f64) -> Vec<u8> {
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let kg = 1.0 - kr - kb;
    let px = |x: usize, y: usize| {
        let o = (y * w + x) * 4;
        [
            ov[o] as f64 / 255.0,
            ov[o + 1] as f64 / 255.0,
            ov[o + 2] as f64 / 255.0,
            ov[o + 3] as f64 / 255.0,
        ]
    };
    let q = |v: f64| v.round().clamp(0.0, 255.0) as u8;
    let mut out = i420.to_vec();
    for y in 0..h {
        for x in 0..w {
            let c = px(x, y);
            out[y * w + x] = q(16.0 * c[3]
                + 219.0 * (kr * c[0] + kg * c[1] + kb * c[2])
                + i420[y * w + x] as f64 * (1.0 - c[3]));
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            let mut c = [0.0; 4];
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let p = px((2 * x + dx).min(w - 1), (2 * y + dy).min(h - 1));
                for i in 0..4 {
                    c[i] += p[i] / 4.0;
                }
            }
            let yp = kr * c[0] + kg * c[1] + kb * c[2];
            let (ui, vi) = (w * h + y * cw + x, w * h + cw * ch + y * cw + x);
            out[ui] = q(128.0 * c[3]
                + 224.0 * (c[2] - yp) / (2.0 * (1.0 - kb))
                + i420[ui] as f64 * (1.0 - c[3]));
            out[vi] = q(128.0 * c[3]
                + 224.0 * (c[0] - yp) / (2.0 * (1.0 - kr))
                + i420[vi] as f64 * (1.0 - c[3]));
        }
    }
    out
}

/// The hidden GL context belongs to the first thread that uses it: every GPU
/// check runs inside this one test.
#[test]
#[ignore = "needs FFmpeg on PATH and an OpenGL 3.3 GPU"]
fn gpu_checks() {
    gpu_mix_matches_the_float64_reference_in_both_matrices();
    a_blurred_glyph_is_smooth_and_not_cut_at_its_sprite();
    an_installed_font_draws_on_the_gpu_like_the_bundled_face();
    a_failing_encoder_returns_its_diagnostics();
    super::engine_features_tests::gpu_engine_features();
    let other = std::thread::spawn(|| {
        super::gl::GlRenderer::headless(8, 8, 8.0, 8.0, super::shaders::Output::PremulGamma).err()
    });
    let refused = other
        .join()
        .unwrap()
        .expect("a second thread must be refused");
    assert!(refused.contains("another thread"), "{refused}");
}

fn gpu_mix_matches_the_float64_reference_in_both_matrices() {
    use super::gl::{GlRenderer, Matrix, YuvMix};
    use super::shaders::Output;
    use glow::HasContext;
    // Odd sizes exercise the half-size chroma edge.
    let (w, h) = (161usize, 91usize);
    let r =
        GlRenderer::headless(w as u32, h as u32, w as f32, h as f32, Output::PremulGamma).unwrap();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 24) as u8
    };
    let size = w * h + 2 * w.div_ceil(2) * h.div_ceil(2);
    let i420: Vec<u8> = (0..size).map(|_| next()).collect();
    // Premultiplied overlay: a quarter fully transparent, the rest random alpha and colour <= alpha.
    let mut ov = vec![0u8; w * h * 4];
    for (i, p) in ov.chunks_mut(4).enumerate() {
        if i % 4 == 0 {
            continue;
        }
        let a = next();
        for c in &mut p[..3] {
            *c = ((next() as u32 * a as u32) / 255) as u8;
        }
        p[3] = a;
    }
    let gl = r.context().clone();
    let tex = unsafe {
        let t = gl.create_texture().unwrap();
        gl.bind_texture(glow::TEXTURE_2D, Some(t));
        gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w as i32,
            h as i32,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&ov)),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAX_LEVEL, 0);
        gl.bind_texture(glow::TEXTURE_2D, None);
        t
    };
    for matrix in [Matrix::Bt601, Matrix::Bt709] {
        let (kr, kb) = matrix.coefficients();
        let mut mix = YuvMix::new(&r, matrix).unwrap();
        let mut out = vec![0u8; mix.frame_bytes()];
        mix.mix(&i420, tex, &mut out).unwrap();
        let reference = mix_reference(&i420, &ov, w, h, kr as f64, kb as f64);
        let off = out.iter().zip(&reference).filter(|(a, b)| a != b).count();
        let worst = out
            .iter()
            .zip(&reference)
            .map(|(a, b)| (*a as i16 - *b as i16).abs())
            .max()
            .unwrap();
        // float32 on the GPU against float64: only codes on a rounding edge may differ, by one.
        assert!(worst <= 1, "{matrix:?}: a code differs by {worst}");
        assert!(
            off * 1000 <= out.len(),
            "{matrix:?}: {off} of {} codes differ",
            out.len()
        );
        // Where the overlay is fully transparent the luma bytes pass unchanged.
        for i in (0..w * h).step_by(4) {
            assert_eq!(out[i], i420[i], "{matrix:?}: transparent luma {i} changed");
        }
    }
    unsafe { gl.delete_texture(tex) };
}

/// A colour with a non-ASCII character of the right byte length is refused, not
/// a panic (byte slicing inside « é »).
#[test]
fn a_non_ascii_colour_is_refused_without_a_panic() {
    let source = std::fs::read_to_string(repo("assets/effects/classique/effect.json")).unwrap();
    assert!(source.contains("\"#FFFFFF\""));
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("effect.json"),
        source.replacen("\"#FFFFFF\"", "\"#aé12b\"", 1),
    )
    .unwrap();
    let e = super::plugin::Effect::load(temp.path())
        .err()
        .expect("a non-ASCII colour was accepted");
    assert!(
        e.0.contains("bad colour") && e.0.contains("#aé12b"),
        "{}",
        e.0
    );
}

#[test]
fn shader_paths_must_stay_inside_the_package() {
    let source = std::fs::read_to_string(repo("tests/fixtures/fx/wipe/effect.json")).unwrap();
    let mut json: serde_json::Value = serde_json::from_str(&source).unwrap();
    for rel in [
        r"\outside.wgsl",
        "/outside.wgsl",
        r"C:\outside.wgsl",
        r"\\server\share\x.wgsl",
        "a/../x.wgsl",
        "./x.wgsl",
        "",
        // Windows-only separators are refused on every system: a package must
        // load the same everywhere (on Linux these are plain file names).
        r"shaders\tint.wgsl",
        "shaders:tint.wgsl",
    ] {
        let temp = tempfile::tempdir().unwrap();
        json["shader"] = serde_json::Value::from(rel);
        std::fs::write(temp.path().join("effect.json"), json.to_string()).unwrap();
        let e = super::plugin::Effect::load(temp.path())
            .err()
            .unwrap_or_else(|| panic!("{rel:?} was accepted"));
        assert!(e.0.contains("inside the package"), "{rel:?}: {}", e.0);
    }
}

#[test]
fn karaoke_wipes_a_word_cut_into_syllables_one_syllable_at_a_time() {
    use super::line::{Syllable, Word};
    let effect = super::catalog::load_one(&repo("tests/fixtures/fx/wipe"), "test.wipe").unwrap();
    let mut fonts = Fonts::load().unwrap();
    let cut = || {
        vec![
            Syllable {
                first_char: 0,
                start: 1.0,
                end: 1.7,
            },
            Syllable {
                first_char: 4,
                start: 1.7,
                end: 2.6,
            },
        ]
    };
    // The fill glyphs of "Something", left to right: S(ung), W(iping) or R(est).
    let mut glyphs = |syllables: Vec<Syllable>, t: f64| -> String {
        let line = Line {
            start: 1.0,
            end: 2.6,
            words: vec![Word {
                text: "Something".into(),
                start: 1.0,
                end: 2.6,
                syllables,
            }],
            sep: " ".into(),
            breaks: Vec::new(),
        };
        let mut scene = PluginScene::new(effect.clone(), 1920, 1080, vec![line], Some("jaune"));
        let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, t);
        let mut fill: Vec<_> = main.iter().filter(|i| i.color[0] > 0.001).collect();
        fill.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
        fill.iter()
            .map(|i| {
                if i.color2[3] > 0.0 {
                    'W'
                } else if i.color[2] < 0.1 * i.color[3] {
                    'S'
                } else {
                    'R'
                }
            })
            .collect()
    };
    assert_eq!(
        glyphs(cut(), 1.35),
        "WWWWRRRRR",
        "the first syllable wipes, the second waits"
    );
    assert_eq!(
        glyphs(cut(), 2.0),
        "SSSSWWWWW",
        "the first is sung, the second wipes"
    );
    assert_eq!(
        glyphs(Vec::new(), 2.0),
        "WWWWWWWWW",
        "a word without syllables wipes as one"
    );
}

/// FFmpeg #2 fails: its own error text must be in the returned log (the NVENC
/// fallback reads it there).
fn a_failing_encoder_returns_its_diagnostics() {
    let ffmpeg = crate::find_ffmpeg(None).unwrap();
    let temp = tempfile::tempdir().unwrap();
    let effect = super::catalog::load_one(&repo("tests/fixtures/fx/wipe"), "test.wipe").unwrap();
    let scene = PluginScene::new(effect, 64, 36, lines(), Some("jaune"));
    let mut first = crate::command(&ffmpeg);
    first.args([
        "-hide_banner",
        "-nostdin",
        "-f",
        "lavfi",
        "-i",
        "testsrc=size=64x36:rate=24:duration=1",
        "-fps_mode",
        "passthrough",
        "-enc_time_base:v",
        "filter",
    ]);
    let ff2 = ffmpeg.clone();
    let out = temp.path().join("out.mp4");
    let second = move |tb: &str, _: f64| {
        let mut c = crate::command(&ff2);
        c.args([
            "-hide_banner",
            "-nostdin",
            "-f",
            "matroska",
            "-i",
            "pipe:0",
            "-enc_time_base:v",
            tb,
            "-c:v",
            "libx264",
            "-preset",
            "no-such-preset-noh",
        ])
        .arg(&out);
        c
    };
    let job = super::export::Job {
        first,
        second: Box::new(second),
        end: None,
        workspace: temp.path().into(),
        width: 64,
        height: 36,
        matrix: super::gl::Matrix::Bt601,
        scene,
        deadline: std::time::Duration::from_secs(60),
    };
    match super::export::run(job, |_| {}) {
        Ok((status, log, _)) => assert!(
            !status.success() && log.contains("no-such-preset-noh"),
            "{status:?}\n{log}"
        ),
        Err(f) => assert!(
            f.log.contains("no-such-preset-noh"),
            "{}\n{}",
            f.message,
            f.log
        ),
    }
}

/// Measurement dump for the "same render" check (plan-integration §5), run on
/// demand: `NOH_FX_DUMP=<dir> NOH_FX_DUMP_CLIP=<video> NOH_FX_DUMP_SRT=<srt>`.
/// For each effect and instant: the source frame (I420, BT.601 as NOH's pieces),
/// the layer (premultiplied RGBA, the preview's own output) and the GPU mix.
#[test]
#[ignore = "measurement dump: needs NOH_FX_DUMP, FFmpeg on PATH and a GPU"]
fn dump_same_render_frames() {
    use super::gl::{GlRenderer, Matrix, YuvMix};
    use super::shaders::Output;
    let Some(dir) = std::env::var_os("NOH_FX_DUMP").map(PathBuf::from) else {
        return;
    };
    let clip = PathBuf::from(std::env::var_os("NOH_FX_DUMP_CLIP").expect("NOH_FX_DUMP_CLIP"));
    let srt =
        std::fs::read_to_string(std::env::var_os("NOH_FX_DUMP_SRT").expect("NOH_FX_DUMP_SRT"))
            .unwrap();
    let track = crate::subtitle_srt::parse(&srt, 3_600_000).unwrap();
    let lines = super::line::from_track(&track);
    std::fs::create_dir_all(&dir).unwrap();
    let ffmpeg = crate::find_ffmpeg(None).unwrap();
    let (w, h) = (1916u32, 1080u32);
    let mut fonts = Fonts::load().unwrap();
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
    let mut mix = YuvMix::new(&gpu, Matrix::Bt601).unwrap();
    let catalog = scan(&[repo("assets/effects"), repo("tests/fixtures/fx")]);
    let mut list = String::new();
    for (id, preset) in [
        ("test.wipe", "rose"),
        ("test.glow", "arc-en-ciel"),
        ("test.motion", ""),
    ] {
        let Some(entry) = catalog.find(id) else {
            continue;
        };
        let preset = if preset.is_empty() {
            entry.effect.presets[0].0.clone()
        } else {
            preset.to_string()
        };
        gpu.set_hooks(entry.effect.shader.as_deref(), [0.0; 16])
            .unwrap();
        let mut scene = PluginScene::new(entry.effect.clone(), w, h, lines.clone(), Some(&preset));
        for t in [16.2, 49.0, 80.5, 81.7, 121.3] {
            let tag = format!("{}-{}-{:.1}", id.trim_start_matches("noh."), preset, t);
            let src = std::process::Command::new(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-ss",
                    &format!("{}", t % 9.5),
                    "-i",
                ])
                .arg(&clip)
                .args([
                    "-frames:v",
                    "1",
                    "-vf",
                    &format!("scale={w}:{h},format=yuv420p"),
                    "-f",
                    "rawvideo",
                    "-",
                ])
                .output()
                .unwrap()
                .stdout;
            assert_eq!(src.len(), mix.frame_bytes(), "{tag}");
            let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, t);
            let (overlay, _) = gpu.frame(&g, gain, &m, true).unwrap();
            let mut out = vec![0u8; mix.frame_bytes()];
            mix.mix(&src, gpu.output_texture(), &mut out).unwrap();
            std::fs::write(dir.join(format!("{tag}.in.yuv")), &src).unwrap();
            std::fs::write(dir.join(format!("{tag}.overlay_premul.rgba")), &overlay).unwrap();
            std::fs::write(dir.join(format!("{tag}.mixed.yuv")), &out).unwrap();
            list.push_str(&format!("{tag}\n"));
        }
    }
    std::fs::write(dir.join("list.txt"), list).unwrap();
}

/// test.wipe with `layout.word_lead` set (lyrics step 7, gap 1).
fn wipe_with_lead(lead: serde_json::Value) -> Result<super::plugin::Effect, String> {
    let dir = repo("tests/fixtures/fx/wipe");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    json["layout"]["word_lead"] = lead;
    super::plugin::Effect::from_json(&dir, json.to_string().as_bytes()).map_err(|e| e.0)
}

#[test]
fn word_lead_moves_words_and_syllables_earlier_but_not_the_line() {
    use super::line::{Syllable, Word};
    let effect = wipe_with_lead(serde_json::json!(0.3)).unwrap();
    let mut fonts = Fonts::load().unwrap();
    let line = |syllables: Vec<Syllable>| Line {
        start: 1.0,
        end: 2.6,
        words: vec![Word {
            text: "Something".into(),
            start: 1.0,
            end: 2.6,
            syllables,
        }],
        sep: " ".into(),
        breaks: Vec::new(),
    };
    let cut = vec![
        Syllable {
            first_char: 0,
            start: 1.0,
            end: 1.7,
        },
        Syllable {
            first_char: 4,
            start: 1.7,
            end: 2.6,
        },
    ];
    let mut draw = |l: Line, t: f64| -> Vec<Inst> {
        let mut scene = PluginScene::new(effect.clone(), 1920, 1080, vec![l], Some("jaune"));
        let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, t);
        let mut fill: Vec<Inst> = main.into_iter().filter(|i| i.color[0] > 0.001).collect();
        fill.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
        fill
    };
    let kinds = |fill: &[Inst]| -> String {
        fill.iter()
            .map(|i| {
                if i.color2[3] > 0.0 {
                    'W'
                } else if i.color[2] < 0.1 * i.color[3] {
                    'S'
                } else {
                    'R'
                }
            })
            .collect()
    };
    // 0.3 s early: the karaoke test's instants (1.35, 2.0) move to 1.05 and 1.7.
    assert_eq!(kinds(&draw(line(cut.clone()), 1.05)), "WWWWRRRRR");
    assert_eq!(kinds(&draw(line(cut.clone()), 1.7)), "SSSSWWWWW");
    // The line itself keeps its times: nothing before its start, even within the lead.
    assert!(draw(line(cut), 0.9).is_empty());
    // The shader hooks' word progress reads the led times: (1.5 - 0.7) / 1.6.
    let fill = draw(line(Vec::new()), 1.5);
    assert!(
        (fill[0].extra[2] - 0.5).abs() < 1e-4,
        "{}",
        fill[0].extra[2]
    );
}

#[test]
fn word_lead_outside_0_to_0_3_is_refused() {
    for bad in [
        serde_json::json!(0.31),
        serde_json::json!(-0.01),
        serde_json::json!("80ms"),
    ] {
        let refused = wipe_with_lead(bad.clone()).unwrap_err();
        assert!(refused.contains("layout.word_lead"), "{bad}: {refused}");
    }
    assert!(wipe_with_lead(serde_json::json!(0)).is_ok());
}

/// A word `enter` colour track from white to a sung colour over `duration`.
fn colour_enter(duration: f64) -> super::plugin::Effect {
    let json = serde_json::json!({
        "engine": 1, "id": "test.colour", "name": {"en": "Colour"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text",
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": {"font": "sans", "size": 64},
        "animate": [{"unit": "word", "slot": "enter", "property": "color", "duration": duration,
                     "keys": [[0, "#FFFFFF"], [1, "#FFD34D"]]}],
        "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
    });
    super::plugin::Effect::from_json(Path::new(""), json.to_string().as_bytes()).unwrap()
}

#[test]
fn colour_keys_blend_over_a_fixed_duration_and_hold() {
    let mut fonts = Fonts::load().unwrap();
    let line = split_words(1.0, 3.0, &["la".to_string()], " ");
    let mut first = |t: f64| -> [f32; 4] {
        let mut scene = PluginScene::new(
            colour_enter(0.12),
            1920,
            1080,
            vec![line.clone()],
            Some("p"),
        );
        let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, t);
        main[0].color
    };
    // Premultiplied linear output: compare through the same conversion.
    let lin = |c: u8| -> f32 {
        let c = c as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let close = |got: [f32; 4], rgb: [u8; 3]| {
        for k in 0..3 {
            assert!((got[k] - lin(rgb[k])).abs() < 2e-3, "{got:?} vs {rgb:?}");
        }
    };
    close(first(1.0), [255, 255, 255]);
    // Halfway through 120 ms: each sRGB channel halfway (D3 → 0xE9, 4D → 0xA6).
    close(first(1.06), [255, 0xE9, 0xA6]);
    // After the slot the last key holds for the rest of the word.
    close(first(2.5), [0xFF, 0xD3, 0x4D]);
}

/// test.window's effect with `layout` keys added.
fn window_with(extra: serde_json::Value) -> super::plugin::Effect {
    let dir = repo("tests/fixtures/fx/window");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    for (k, v) in extra.as_object().unwrap() {
        json["layout"][k] = v.clone();
    }
    super::plugin::Effect::from_json(&dir, json.to_string().as_bytes()).unwrap()
}

#[test]
fn shown_spans_preroll_never_cuts_the_previous_line_and_hold_waits_for_the_next() {
    use super::plugin::shown_spans;
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    // Back to back (cue end = next line start, as the mockup's fixture); values
    // exact in binary so the spans compare exactly.
    let back_to_back = vec![
        split_words(0.5, 3.0, &w("On a gardé la lumière"), " "),
        split_words(3.0, 5.5, &w("même quand la nuit tombait"), " "),
    ];
    let spans = shown_spans(&back_to_back, 0.25, 0.0);
    assert_eq!(spans[0], (0.25, 3.0), "the first line pre-rolls");
    assert_eq!(
        spans[1],
        (3.0, 5.5),
        "the second waits for the first to end"
    );
    // A gap of 1 s: the pre-roll fits in it; hold keeps the first until then.
    let gap = vec![
        split_words(0.5, 2.0, &w("a b"), " "),
        split_words(3.0, 4.0, &w("c d"), " "),
    ];
    assert_eq!(shown_spans(&gap, 0.25, 0.0), [(0.25, 2.0), (2.75, 4.0)]);
    assert_eq!(shown_spans(&gap, 0.25, 10.0), [(0.25, 2.75), (2.75, 14.0)]);
    assert_eq!(shown_spans(&gap, 0.0, 0.5), [(0.5, 2.5), (3.0, 4.5)]);
    // Overlapping cues: the second starts when the first ends.
    let overlap = vec![
        split_words(1.0, 3.0, &w("a"), " "),
        split_words(2.0, 4.0, &w("b"), " "),
    ];
    assert_eq!(shown_spans(&overlap, 0.0, 0.0), [(1.0, 3.0), (3.0, 4.0)]);
}

#[test]
fn a_held_line_stays_active_with_the_next_line_below_until_it_shows() {
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let lines = vec![
        split_words(0.5, 2.0, &w("first line"), " "),
        split_words(3.0, 4.0, &w("second line"), " "),
    ];
    let held = PluginScene::new(
        window_with(serde_json::json!({"hold": 5, "preroll": 0.2})),
        1920,
        1080,
        lines.clone(),
        None,
    );
    let plain = PluginScene::new(window_with(serde_json::json!({})), 1920, 1080, lines, None);
    use super::plugin::{ACTIVE, NEXT};
    // In the gap: without hold the next line takes slot 0 (NEXT); with hold the
    // sung line stays in slot 0 (ACTIVE) and the next one waits in slot +1.
    let gap = 2.5;
    assert_eq!(plain.visible_lines(gap)[0], (1, NEXT, 0));
    let v = held.visible_lines(gap);
    assert_eq!(v[0], (0, ACTIVE, 0));
    assert!(v.contains(&(1, NEXT, 1)), "{v:?}");
    // From the next line's pre-roll (3.0 - 0.2) it is the active one.
    assert_eq!(held.visible_lines(2.85)[0], (1, ACTIVE, 0));
}

#[test]
fn a_line_enter_runs_from_its_preroll_and_its_exit_at_the_held_end() {
    let json = |layout: serde_json::Value| {
        let mut j = serde_json::json!({
            "engine": 1, "id": "test.span", "name": {"en": "Span"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
            "layout": {"font": "sans", "size": 64},
            "animate": [
                {"unit": "line", "slot": "enter", "property": "opacity", "duration": 0.4,
                 "keys": [[0, 0], [1, 1]]},
                {"unit": "line", "slot": "exit", "property": "opacity", "duration": 0.2,
                 "keys": [[0, 1], [1, 0]]}
            ],
            "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
        });
        for (k, v) in layout.as_object().unwrap() {
            j["layout"][k] = v.clone();
        }
        super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).unwrap()
    };
    let mut fonts = Fonts::load().unwrap();
    let line = split_words(1.0, 2.0, &["la".to_string()], " ");
    let mut alpha = |effect: super::plugin::Effect, t: f64| -> Option<f32> {
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![line.clone()], Some("p"));
        let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, t);
        main.first().map(|i| i.color[3])
    };
    let pre = json(serde_json::json!({"preroll": 0.3, "hold": 1.0}));
    assert_eq!(alpha(pre.clone(), 0.65), None, "before the pre-roll");
    // 0.9 is 0.2 into the 0.4 s enter that starts at 0.7.
    assert!((alpha(pre.clone(), 0.9).unwrap() - 0.5).abs() < 1e-3);
    // Held to 3.0: fully shown at 2.5, the exit halfway at 2.9.
    assert!((alpha(pre.clone(), 2.5).unwrap() - 1.0).abs() < 1e-3);
    assert!((alpha(pre.clone(), 2.9).unwrap() - 0.5).abs() < 1e-3);
    assert_eq!(alpha(pre, 3.0), None, "after the held end");
    // Without them the line keeps its cue: the enter starts at 1.0.
    let plain = json(serde_json::json!({}));
    assert!((alpha(plain.clone(), 1.2).unwrap() - 0.5).abs() < 1e-3);
    assert!((alpha(plain, 1.9).unwrap() - 0.5).abs() < 1e-3);
}

/// A line-mode effect with the given `layout` (font sans, size 64, one fill pass).
fn plain_effect(layout: serde_json::Value) -> Result<super::plugin::Effect, String> {
    let mut j = serde_json::json!({
        "engine": 1, "id": "test.plain", "name": {"en": "Plain"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text",
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": {"font": "sans", "size": 64},
        "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
    });
    for (k, v) in layout.as_object().unwrap() {
        j["layout"][k] = v.clone();
    }
    super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).map_err(|e| e.0)
}

/// The mockup's fourth line with its word times.
fn vitrines() -> Line {
    use super::line::Word;
    let w = |text: &str, start: f64, end: f64| Word {
        text: text.into(),
        start,
        end,
        syllables: Vec::new(),
    };
    Line {
        start: 8.2,
        end: 11.0,
        words: vec![
            w("faisaient", 8.2, 8.6),
            w("trembler", 8.6, 9.2),
            w("les", 9.2, 9.35),
            w("vitrines", 9.35, 10.6),
        ],
        sep: " ".into(),
        breaks: Vec::new(),
    }
}

#[test]
fn chunks_slice_a_cue_in_order_and_hand_over_at_the_next_preroll() {
    let effect =
        plain_effect(serde_json::json!({"chunk": {"max": 3, "rows": "word"}, "preroll": 0.06}))
            .unwrap();
    let scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some("p"));
    // « faisaient trembler les » until 9.35 - 0.06, then « vitrines » to the cue end.
    assert_eq!(scene.line_at(9.28), Some(0));
    assert_eq!(scene.line_at(9.30), Some(1));
    assert_eq!(scene.line_at(10.9), Some(1));
    assert_eq!(scene.line_at(11.0), None);
    // From its pre-roll, 60 ms before its first word.
    assert_eq!(scene.line_at(8.13), None);
    assert_eq!(scene.line_at(8.15), Some(0));
}

#[test]
fn chunk_rows_put_each_word_on_its_own_row() {
    let mut fonts = Fonts::load().unwrap();
    let mut rows = |layout: serde_json::Value| -> usize {
        let effect = plain_effect(layout).unwrap();
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some("p"));
        let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, 8.5);
        let mut ys: Vec<i32> = main.iter().map(|i| i.center[1].round() as i32).collect();
        ys.sort();
        ys.dedup_by(|a, b| (*a - *b).abs() < 20);
        ys.len()
    };
    assert_eq!(
        rows(serde_json::json!({"chunk": {"max": 3, "rows": "word"}})),
        3
    );
    assert_eq!(rows(serde_json::json!({"chunk": {"max": 3}})), 1);
}

#[test]
fn chunk_settings_outside_their_range_are_refused() {
    for (bad, field) in [
        (serde_json::json!({"max": 1}), "layout.chunk.max"),
        (serde_json::json!({"max": 5}), "layout.chunk.max"),
        (
            serde_json::json!({"max": 3, "rows": "letter"}),
            "layout.chunk.rows",
        ),
        (serde_json::json!({"max": 3, "size": 2}), "layout.chunk"),
    ] {
        let refused = plain_effect(serde_json::json!({"chunk": bad})).unwrap_err();
        assert!(refused.contains(field), "{bad}: {refused}");
    }
    // The cue's typed lines cannot survive chunking: refused rather than dropped.
    let refused = plain_effect(serde_json::json!({
        "chunk": {"max": 3}, "line_breaks": "keep"
    }))
    .unwrap_err();
    assert!(refused.contains("not with line_breaks keep"), "{refused}");
}

#[test]
fn chunks_end_with_their_cue_and_the_next_cue_starts_at_its_preroll() {
    let w = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let effect = plain_effect(serde_json::json!({"chunk": {"max": 2}, "preroll": 0.25})).unwrap();
    let scene = PluginScene::new(
        effect,
        1920,
        1080,
        vec![
            split_words(1.0, 2.0, &w("a b c"), " "),
            split_words(3.0, 4.0, &w("d e"), " "),
        ],
        Some("p"),
    );
    // « a b » then « c » to the cue end (2.0); « d e » from 3.0 - 0.25.
    assert_eq!(scene.line_at(1.9), Some(1));
    assert_eq!(scene.line_at(2.0), None);
    assert_eq!(scene.line_at(2.7), None);
    assert_eq!(scene.line_at(2.75), Some(2));
}

#[test]
fn word_values_give_the_sung_duration_and_the_longest_word() {
    // Opacity = 0.5 for the longest-sung word, and its duration sets the scale.
    let j = serde_json::json!({
        "engine": 1, "id": "test.word", "name": {"en": "Word"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text",
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": {"font": "sans", "size": 64, "word_lead": 0.1},
        "animate": [
            {"unit": "word", "slot": "loop", "property": "opacity",
             "value": {"sub": [1, {"mul": ["@word_longest", 0.5]}]}},
            {"unit": "word", "slot": "loop", "property": "scale", "value": {"add": [1, "@word_duration"]}}
        ],
        "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
    });
    let effect = super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).unwrap();
    let mut fonts = Fonts::load().unwrap();
    let mut scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some("p"));
    let (_, _, main, _) = scene.frame(&mut fonts, &NoGpu, 9.0);
    let mut by_x: Vec<&Inst> = main.iter().collect();
    by_x.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
    // « faisaient » (0.4 s) first, « vitrines » (1.25 s, the longest) last.
    let (first, last) = (by_x[0], by_x[by_x.len() - 1]);
    assert!((first.xform[2] - 1.4).abs() < 1e-4, "{:?}", first.xform);
    assert!((last.xform[2] - 2.25).abs() < 1e-4, "{:?}", last.xform);
    assert!(last.color[3] < 0.51 && first.color[3] > 0.99);
}

#[test]
fn an_unknown_at_value_lists_the_host_and_word_values() {
    let refused = plain_effect(serde_json::json!({"size": "@word_length"})).unwrap_err();
    assert!(refused.contains("word_duration, word_longest"), "{refused}");
}

#[test]
fn compose_role_colours_read_the_word_values() {
    let dir = repo("tests/fixtures/fx/compose");
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    let mut fonts = Fonts::load().unwrap();
    // White instances drawn at 10 s (every word shown: the fixture shows words
    // from their start) with all three roles coloured `colour`.
    let mut white = |colour: serde_json::Value| -> usize {
        let mut json = base.clone();
        for role in ["carrier", "second", "others"] {
            json["layout"]["compose"]["roles"][role]["color"] = colour.clone();
        }
        let effect = super::plugin::Effect::from_json(&dir, json.to_string().as_bytes()).unwrap();
        let preset = effect.presets[0].0.clone();
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some(&preset));
        let main = scene.frame(&mut fonts, &NoGpu, 10.0).2;
        main.iter()
            .filter(|i| i.color[0] > 0.5 && i.color[3] > 0.5)
            .count()
    };
    // Beyond what the fixture draws white anyway, only the letters of « vitrines »,
    // the longest-sung word.
    let black = white(serde_json::json!("#000000"));
    let longest = white(serde_json::json!({"mix": ["#000000", "#FFFFFF", "@word_longest"]}));
    assert_eq!(longest - black, "vitrines".len(), "{longest} vs {black}");
}

#[test]
fn word_size_enlarges_the_longest_word_and_its_row_reflows() {
    let mut fonts = Fonts::load().unwrap();
    // Bloc's rows: one word per row, the longest-sung word × 1.15.
    let draw = |fonts: &mut Fonts, size: serde_json::Value| -> Vec<Inst> {
        let effect = plain_effect(serde_json::json!({
            "chunk": {"max": 3, "rows": "word"}, "word_size": size
        }))
        .unwrap();
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some("p"));
        scene.frame(fonts, &NoGpu, 9.4).2
    };
    // At 9.4 the chunk shown is « vitrines » alone: compare its width, first to
    // last glyph centre (sprite sizes carry atlas padding that does not scale).
    let plain = draw(&mut fonts, serde_json::json!(1));
    let big = draw(
        &mut fonts,
        serde_json::json!({"add": [1, {"mul": ["@word_longest", 0.15]}]}),
    );
    let h = |v: &[Inst]| {
        let xs = v.iter().map(|i| i.center[0]);
        xs.clone().fold(f32::MIN, f32::max) - xs.fold(f32::MAX, f32::min)
    };
    let ratio = h(&big) / h(&plain);
    assert!((ratio - 1.15).abs() < 0.03, "{ratio}");
    // In « faisaient trembler les » (8.5) the longest is « trembler »: its row is
    // taller, so the three rows spread further apart than with no factor.
    let spread = |v: &[Inst]| {
        let ys = v.iter().map(|i| i.center[1]);
        ys.clone().fold(f32::MIN, f32::max) - ys.fold(f32::MAX, f32::min)
    };
    let mut at = |size: serde_json::Value| -> f32 {
        let effect = plain_effect(
            serde_json::json!({"chunk": {"max": 3, "rows": "word"}, "word_size": size}),
        )
        .unwrap();
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![vitrines()], Some("p"));
        spread(&scene.frame(&mut fonts, &NoGpu, 8.5).2)
    };
    let (a, b) = (
        at(serde_json::json!(1)),
        at(serde_json::json!({"add": [1, {"mul": ["@word_longest", 0.15]}]})),
    );
    assert!(b - a > 3.0, "{a} {b}");
}

/// A line-mode effect with `animate` tracks and passes (size 64, sans).
fn tracked_effect(
    animate: serde_json::Value,
    passes: serde_json::Value,
) -> Result<super::plugin::Effect, String> {
    let j = serde_json::json!({
        "engine": 1, "id": "test.tracked", "name": {"en": "Tracked"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text",
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": {"font": "sans", "size": 64},
        "animate": animate,
        "render": {"passes": passes}
    });
    super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).map_err(|e| e.0)
}

fn fill_only() -> serde_json::Value {
    serde_json::json!([{"kind": "fill", "color": "#FFFFFF"}])
}

/// Fill glyphs of « ab cd » at 1.5 s, left to right.
fn drawn(effect: super::plugin::Effect) -> Vec<Inst> {
    let mut fonts = Fonts::load().unwrap();
    let line = split_words(1.0, 2.0, &["ab".to_string(), "cd".to_string()], " ");
    let mut scene = PluginScene::new(effect, 1920, 1080, vec![line], Some("p"));
    let mut v = scene.frame(&mut fonts, &NoGpu, 1.5).2;
    v.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
    v
}

#[test]
fn line_offsets_move_every_letter_in_the_line_em() {
    let plain = drawn(tracked_effect(serde_json::json!([]), fill_only()).unwrap());
    let moved = drawn(
        tracked_effect(
            serde_json::json!([
                {"unit": "line", "slot": "loop", "property": "offset_y", "value": 0.5},
                {"unit": "line", "slot": "loop", "property": "offset_x", "value": -0.25}
            ]),
            fill_only(),
        )
        .unwrap(),
    );
    for (a, b) in plain.iter().zip(&moved) {
        assert!(
            (b.center[1] - a.center[1] - 32.0).abs() < 1e-3,
            "{a:?} {b:?}"
        );
        assert!(
            (b.center[0] - a.center[0] + 16.0).abs() < 1e-3,
            "{a:?} {b:?}"
        );
    }
}

#[test]
fn line_tracking_spreads_letters_about_the_row_centre_spaces_included() {
    let plain = drawn(tracked_effect(serde_json::json!([]), fill_only()).unwrap());
    let spread = drawn(
        tracked_effect(
            serde_json::json!([
                {"unit": "line", "slot": "loop", "property": "tracking", "value": 0.1}
            ]),
            fill_only(),
        )
        .unwrap(),
    );
    // « ab cd »: 5 characters; a, b, c, d sit at 0, 1, 3, 4 → (k − 2.5) × 0.1 em × 64.
    let want = [-16.0, -9.6, 3.2, 9.6];
    for ((a, b), w) in plain.iter().zip(&spread).zip(want) {
        assert!(
            (b.center[0] - a.center[0] - w).abs() < 1e-3,
            "{} vs {w}",
            b.center[0] - a.center[0]
        );
    }
}

#[test]
fn line_blur_reaches_the_fill_and_the_shadow() {
    let passes = serde_json::json!([
        {"kind": "shadow", "color": "#000000", "blur": 2, "offset_y": 0.05},
        {"kind": "fill", "color": "#FFFFFF"}
    ]);
    let v = drawn(
        tracked_effect(
            serde_json::json!([{"unit": "line", "slot": "loop", "property": "blur", "value": 8}]),
            passes,
        )
        .unwrap(),
    );
    let shadows: Vec<&Inst> = v.iter().filter(|i| i.color[0] < 1e-6).collect();
    let fills: Vec<&Inst> = v.iter().filter(|i| i.color[0] > 0.5).collect();
    assert!(!shadows.is_empty() && !fills.is_empty());
    assert!(shadows.iter().all(|i| (i.params[1] - 10.0).abs() < 1e-4));
    assert!(fills.iter().all(|i| (i.params[1] - 8.0).abs() < 1e-4));
}

#[test]
fn line_properties_that_would_not_draw_are_refused() {
    for (unit, prop, says) in [
        ("line", "wipe", "not drawn on the line unit"),
        ("line", "color", "not drawn on the line unit"),
        ("word", "tracking", "\"tracking\" is a line unit property"),
    ] {
        let refused = tracked_effect(
            serde_json::json!([{"unit": unit, "slot": "loop", "property": prop, "value": 1}]),
            fill_only(),
        )
        .unwrap_err();
        assert!(refused.contains(says), "{unit} {prop}: {refused}");
    }
}

#[test]
fn line_offsets_and_tracking_are_refused_in_compose_mode() {
    let dir = repo("tests/fixtures/fx/compose");
    let base: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("effect.json")).unwrap()).unwrap();
    for prop in ["offset_x", "offset_y", "tracking", "reveal_y"] {
        let mut json = base.clone();
        json["animate"].as_array_mut().unwrap().push(
            serde_json::json!({"unit": "line", "slot": "loop", "property": prop, "value": 0.1}),
        );
        let refused = super::plugin::Effect::from_json(&dir, json.to_string().as_bytes())
            .map(|_| ())
            .unwrap_err()
            .0;
        assert!(refused.contains("in compose mode"), "{prop}: {refused}");
    }
    let mut json = base.clone();
    json["animate"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"unit": "line", "slot": "loop", "property": "blur", "value": 1}));
    assert!(super::plugin::Effect::from_json(&dir, json.to_string().as_bytes()).is_ok());
}

#[test]
fn every_face_resolves_to_its_own_file_and_variable_ones_draw_at_their_weight() {
    use super::text::{Face, layout};
    let mut fonts = Fonts::load().unwrap();
    let file_len = |fonts: &mut Fonts, face: Face| -> usize {
        let laid = layout(
            fonts,
            &["Hamburg".to_string()],
            " ",
            &[],
            face,
            64.0,
            None,
            1.2,
        );
        let id = laid.glyphs[0].font_id;
        assert!(
            laid.glyphs.iter().all(|g| g.font_id == id),
            "{face:?}: one file"
        );
        fonts.bytes[&id].0.bytes().len()
    };
    let len = |rel: &str| std::fs::metadata(repo(rel)).unwrap().len() as usize;
    // Two « Jost » files are loaded: the wordmark's ExtraBold must stay the static one.
    assert_eq!(
        file_len(&mut fonts, Face::Jost),
        len("assets/Jost-ExtraBold.ttf")
    );
    assert_eq!(
        file_len(&mut fonts, Face::JostMedium),
        len("assets/Jost-Variable.ttf")
    );
    assert_eq!(
        file_len(&mut fonts, Face::InterSemi),
        len("assets/Inter-Variable.ttf")
    );
    assert_eq!(
        file_len(&mut fonts, Face::CondensedExtraBold),
        len("assets/BarlowCondensed-ExtraBold.ttf")
    );
    assert_eq!(
        file_len(&mut fonts, Face::SerifItalic),
        len("assets/NotoSerif-Italic.ttf")
    );
    // Shaped at the set weight: Jost 500 is narrower than the ExtraBold, wider
    // than the variable file's thinnest instance would be (advances vary with wght).
    let width = |fonts: &mut Fonts, face: Face| {
        layout(
            fonts,
            &["Hamburg".to_string()],
            " ",
            &[],
            face,
            64.0,
            None,
            1.2,
        )
        .glyphs
        .iter()
        .map(|g| g.advance)
        .sum::<f32>()
    };
    let (medium, extra) = (
        width(&mut fonts, Face::JostMedium),
        width(&mut fonts, Face::Jost),
    );
    assert!(medium < extra, "{medium} {extra}");
    assert_eq!(
        layout(
            &mut fonts,
            &["H".to_string()],
            " ",
            &[],
            Face::InterSemi,
            64.0,
            None,
            1.2
        )
        .glyphs[0]
            .weight
            .0,
        600
    );
}

#[test]
fn a_variable_face_is_outlined_at_its_weight_in_the_msdf_atlas() {
    use super::text::{Face, layout};
    let mut fonts = Fonts::load().unwrap();
    let laid = layout(
        &mut fonts,
        &["H".to_string()],
        " ",
        &[],
        Face::JostMedium,
        64.0,
        None,
        1.2,
    );
    let g = &laid.glyphs[0];
    // The MSDF bbox of « H » at 500 is narrower than at the file's heaviest (800)
    // or wider than its lightest; compare with the outline ttf-parser gives at 500.
    let (data, index) = &fonts.bytes[&g.font_id];
    let mut face = ttf_parser::Face::parse(data.bytes(), *index).unwrap();
    let gid = ttf_parser::GlyphId(g.glyph_id);
    let default = face.glyph_bounding_box(gid).unwrap();
    face.set_variation(ttf_parser::Tag::from_bytes(b"wght"), 500.0)
        .unwrap();
    let at_500 = face.glyph_bounding_box(gid).unwrap();
    assert_ne!(
        default.width(),
        at_500.width(),
        "the axis moves the outline"
    );
    assert_eq!(fonts.weights[&g.font_id], 500.0);
    let mut atlas = super::text::Atlas::new(48.0);
    let s = atlas.msdf_sprite(&fonts, &NoGpu, g).unwrap();
    // Sprite width in font units (minus the MSDF padding) matches the 500 bbox.
    let range = super::shaders::MSDF_RANGE;
    let units = (s.w / s.ppt - 2.0 * range) * f32::from(face.units_per_em()) / 48.0;
    assert!(
        (units - f32::from(at_500.width())).abs() < f32::from(face.units_per_em()) * 0.03,
        "{units} vs {}",
        at_500.width()
    );
}

#[test]
fn reveal_clips_the_whole_block_from_the_top_on_every_pass() {
    let passes = serde_json::json!([
        {"kind": "shadow", "color": "#000000", "offset_y": 0.05},
        {"kind": "fill", "color": "#FFFFFF"}
    ]);
    let effect = |reveal: f64| {
        tracked_effect(
            serde_json::json!([
                {"unit": "line", "slot": "loop", "property": "reveal_y", "value": reveal}
            ]),
            passes.clone(),
        )
        .unwrap()
    };
    let none = drawn(tracked_effect(serde_json::json!([]), passes.clone()).unwrap());
    assert!(
        none.iter().all(|i| i.xform[3] == 0.0),
        "no reveal track: no clip"
    );
    let full = drawn(effect(1.0));
    // Fully shown: no clip at all.
    assert!(
        full.iter().all(|i| i.xform[3] == 0.0),
        "{:?}",
        full.iter().map(|i| i.xform[3]).collect::<Vec<_>>()
    );
    let half = drawn(effect(0.5));
    // Half shown: one line of text, so the clip is about halfway down each glyph
    // quad and the same canvas y for the shadows as for the fills.
    let canvas_y = |i: &Inst| {
        let h = i.size[1] * i.xform[2];
        i.center[1] - h * 0.5 + i.xform[3] * h
    };
    let ys: Vec<f32> = half.iter().map(canvas_y).collect();
    let (lo, hi) = ys
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
    assert!(hi - lo < 1e-2, "one clip line for every pass: {ys:?}");
    assert!(half.iter().any(|i| i.xform[3] > 0.0 && i.xform[3] < 1.0));
    let refused = tracked_effect(
        serde_json::json!([{"unit": "word", "slot": "loop", "property": "reveal_y", "value": 1}]),
        fill_only(),
    )
    .unwrap_err();
    assert!(refused.contains("is a line unit property"), "{refused}");
}

#[test]
fn a_wipe_draws_its_sung_side_at_alpha2_and_the_rest_at_alpha() {
    use super::line::Word;
    let effect = tracked_effect(
        serde_json::json!([
            {"unit": "word", "slot": "active", "property": "wipe", "keys": [[0, 0], [1, 1]]},
            {"unit": "word", "slot": "past", "property": "wipe", "value": 1}
        ]),
        serde_json::json!([
            {"kind": "fill", "color": "#FFFFFF", "color2": "#FFD34D", "alpha": 0.42, "alpha2": 1}
        ]),
    )
    .unwrap();
    let mut fonts = Fonts::load().unwrap();
    let line = Line {
        start: 1.0,
        end: 3.0,
        words: vec![
            Word {
                text: "la".into(),
                start: 1.0,
                end: 2.0,
                syllables: Vec::new(),
            },
            Word {
                text: "lu".into(),
                start: 2.0,
                end: 3.0,
                syllables: Vec::new(),
            },
        ],
        sep: " ".into(),
        breaks: Vec::new(),
    };
    let mut scene = PluginScene::new(effect, 1920, 1080, vec![line], Some("p"));
    let mut v = scene.frame(&mut fonts, &NoGpu, 2.5).2;
    v.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
    // « la » is sung (fully the second colour, opaque); « lu » is wiping: its rest
    // side at 0.42, its sung side opaque.
    assert!((v[0].color[3] - 1.0).abs() < 1e-4, "{:?}", v[0].color);
    let wiping = v
        .iter()
        .find(|i| i.extra[0] >= 0.0)
        .expect("a glyph mid-wipe");
    assert!((wiping.color[3] - 0.42).abs() < 1e-4, "{:?}", wiping.color);
    assert!((wiping.color2[3] - 1.0).abs() < 1e-4, "{:?}", wiping.color2);
}

#[test]
fn a_second_colour_on_a_shadow_or_glow_pass_is_refused() {
    // color2: fill and outline_only only; alpha2: also the glow, never the shadow.
    for (kind, key, refusal) in [
        (
            "shadow",
            "color2",
            Some("fill and outline_only passes only"),
        ),
        ("glow", "color2", Some("fill and outline_only passes only")),
        (
            "shadow",
            "alpha2",
            Some("fill, outline_only and glow passes only"),
        ),
        ("glow", "alpha2", None),
    ] {
        let value = if key == "color2" {
            serde_json::json!("#FFD34D")
        } else {
            serde_json::json!(1)
        };
        let mut pass = serde_json::json!({"kind": kind, "color": "#000000"});
        pass[key] = value;
        let loaded = tracked_effect(serde_json::json!([]), serde_json::json!([pass]));
        match refusal {
            Some(text) => {
                let refused = loaded.unwrap_err();
                assert!(
                    refused.contains(&format!("{key}: {text}")),
                    "{kind} {key}: {refused}"
                );
            }
            None => assert!(loaded.is_ok(), "{kind} {key}: {:?}", loaded.err()),
        }
    }
}

#[test]
fn a_glow_with_alpha2_splits_at_the_wipe_and_one_without_does_not() {
    use super::line::Word;
    let line = || Line {
        start: 1.0,
        end: 3.0,
        words: vec![
            Word {
                text: "la".into(),
                start: 1.0,
                end: 2.0,
                syllables: Vec::new(),
            },
            Word {
                text: "lu".into(),
                start: 2.0,
                end: 3.0,
                syllables: Vec::new(),
            },
        ],
        sep: " ".into(),
        breaks: Vec::new(),
    };
    let glows = |glow: serde_json::Value| {
        let effect = tracked_effect(
            serde_json::json!([
                {"unit": "word", "slot": "active", "property": "wipe", "keys": [[0, 0], [1, 1]]},
                {"unit": "word", "slot": "past", "property": "wipe", "value": 1}
            ]),
            serde_json::json!([glow, {"kind": "fill", "color": "#FFFFFF"}]),
        )
        .unwrap();
        let mut fonts = Fonts::load().unwrap();
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![line()], Some("p"));
        let mut v = scene.frame(&mut fonts, &NoGpu, 2.5).0;
        v.sort_by(|a, b| a.center[0].total_cmp(&b.center[0]));
        v
    };
    // gain 0.5 scales both sides: unsung 0.2 -> 0.1, sung 0.8 -> 0.4.
    let v = glows(serde_json::json!(
        {"kind": "glow", "color": "#FFD34D", "alpha": 0.2, "alpha2": 0.8, "gain": 0.5}
    ));
    assert_eq!(v.len(), 4, "a glow instance per letter");
    // « la » is sung: wholly the sung side, no split.
    for i in &v[..2] {
        assert!(i.extra[0] < 0.0 && (i.color[3] - 0.4).abs() < 1e-5, "{i:?}");
    }
    // « lu » is half sung: one glyph splits, its rest side 0.1 and sung side 0.4.
    let wiping = v[2..]
        .iter()
        .find(|i| i.extra[0] >= 0.0)
        .expect("a glow instance mid-wipe");
    assert!((wiping.color[3] - 0.1).abs() < 1e-5, "{wiping:?}");
    assert!((wiping.color2[3] - 0.4).abs() < 1e-5, "{wiping:?}");
    // Without alpha2 the glow keeps one opacity and no split, as before.
    let plain = glows(serde_json::json!(
        {"kind": "glow", "color": "#FFD34D", "alpha": 0.2, "gain": 0.5}
    ));
    for i in &plain {
        assert!(i.extra[0] < 0.0 && (i.color[3] - 0.1).abs() < 1e-5, "{i:?}");
    }
}

#[test]
fn choice_labels_name_each_choice_and_unknown_ones_are_refused() {
    let json = |labels: serde_json::Value| {
        serde_json::json!({
            "engine": 1, "id": "test.labels", "name": {"en": "Labels"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "params": [{"id": "size", "type": "choice", "default": "medium",
                        "choices": ["small", "medium", "large"], "choice_labels": labels,
                        "level": 1, "label": {"en": "Size"}}],
            "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
            "layout": {"font": "sans", "size": 64},
            "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
        })
        .to_string()
    };
    let effect = super::plugin::Effect::from_json(
        Path::new(""),
        json(serde_json::json!({"small": {"fr": "Petite", "de": "Klein"}, "large": {"fr": "Grande"}}))
            .as_bytes(),
    )
    .unwrap();
    let p = &effect.params[0];
    assert_eq!(p.choice_labels[0]["fr"], "Petite");
    assert!(
        p.choice_labels[1].is_empty(),
        "medium has no names: its own text shows"
    );
    assert_eq!(p.choice_labels[2]["fr"], "Grande");
    let refused = super::plugin::Effect::from_json(
        Path::new(""),
        json(serde_json::json!({"huge": {"en": "Huge"}})).as_bytes(),
    )
    .map(|_| ())
    .unwrap_err()
    .0;
    assert!(
        refused.contains("\"huge\" is not one of its choices"),
        "{refused}"
    );
}

/// Gap 13 cost: frame time of a Respiration-like line (line blur 19 px at 720p,
/// shadow 7/304 of the height) at 1080p and 2160p. Prints, asserts nothing.
#[test]
#[ignore = "measurement: needs NOH_FX_BLUR_COST and a GPU"]
fn blur_cost_of_a_full_blurred_line() {
    use super::gl::GlRenderer;
    use super::shaders::Output;
    if std::env::var_os("NOH_FX_BLUR_COST").is_none() {
        return;
    }
    let mut fonts = Fonts::load().unwrap();
    for (w, h) in [(1920u32, 1080u32), (3840, 2160)] {
        let hf = f64::from(h);
        let j = serde_json::json!({
            "engine": 1, "id": "test.cost", "name": {"en": "Cost"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
            "layout": {"font": "jost_medium", "size": 32.0 / 304.0 * hf},
            "animate": [{"unit": "line", "slot": "loop", "property": "blur", "value": 19.0 / 720.0 * hf}],
            "render": {"passes": [
                {"kind": "shadow", "color": "#000000", "alpha": 0.5, "offset_y": 0.06, "blur": 7.0 / 304.0 * hf},
                {"kind": "fill", "color": "#FFFFFF"}
            ]}
        });
        let effect =
            super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).unwrap();
        let w_ = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        let line = split_words(1.0, 9.0, &w_("On a gardé la lumière"), " ");
        let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
        let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
        let mut times = Vec::new();
        for k in 0..40 {
            let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 2.0 + f64::from(k) * 0.01);
            let (_, t) = gpu.frame(&g, gain, &m, true).unwrap();
            if k >= 10 {
                times.push(t.gpu_ms.unwrap_or(f64::NAN));
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "blur cost {w}x{h}: gpu median {:.3} ms, max {:.3} ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

/// Gap 13: a blurred glyph is a smooth gaussian, not a grid of copies, and its
/// blur runs past the sprite instead of stopping at it.
fn a_blurred_glyph_is_smooth_and_not_cut_at_its_sprite() {
    use super::gl::GlRenderer;
    use super::shaders::Output;
    let (w, h) = (320u32, 200u32);
    let mut fonts = Fonts::load().unwrap();
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
    let effect = tracked_effect(
        serde_json::json!([{"unit": "line", "slot": "loop", "property": "blur", "value": 12}]),
        fill_only(),
    )
    .unwrap();
    let line = split_words(1.0, 2.0, &["I".to_string()], " ");
    let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
    let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 1.5);
    assert_eq!(m.len(), 1);
    let (cx, cy, half) = (m[0].center[0], m[0].center[1], m[0].size[0] * 0.5);
    let (px, _) = gpu.frame(&g, gain, &m, true).unwrap();
    // Alpha along the row through the glyph's centre (top row first).
    let row = cy.round() as usize;
    let alpha: Vec<u8> = (0..w as usize)
        .map(|x| px[(row * w as usize + x) * 4 + 3])
        .collect();
    let peak = (0..alpha.len()).max_by_key(|&x| alpha[x]).unwrap();
    // One hump: never rising again on either side (ghost copies would).
    for x in peak..alpha.len() - 1 {
        assert!(alpha[x + 1] <= alpha[x] + 1, "rises at {x}: {alpha:?}");
    }
    for x in 1..=peak {
        assert!(alpha[x - 1] <= alpha[x] + 1, "rises at {x}: {alpha:?}");
    }
    // With blur on, the reveal clip still cuts at its line. The engine clips the
    // glyph, then blurs it (CSS does filter, then clip-path): the blur may spread
    // up to 3 sigma (6 px) above the line, never further.
    let effect = tracked_effect(
        serde_json::json!([
            {"unit": "line", "slot": "loop", "property": "blur", "value": 6},
            {"unit": "line", "slot": "loop", "property": "reveal_y", "value": 0.5}
        ]),
        fill_only(),
    )
    .unwrap();
    let line = split_words(1.0, 2.0, &["I".to_string()], " ");
    let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
    let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 1.5);
    let quad_h = m[0].size[1] * m[0].xform[2];
    let clip = m[0].center[1] - quad_h * 0.5 + m[0].xform[3] * quad_h;
    let (px, _) = gpu.frame(&g, gain, &m, true).unwrap();
    let col = m[0].center[0].round() as usize;
    let above = (clip - 3.0 * 6.0 - 2.0).floor() as usize;
    let below = (clip + 4.0).ceil() as usize;
    assert_eq!(
        px[(above * w as usize + col) * 4 + 3],
        0,
        "drawn more than 3 sigma above the clip"
    );
    let column: Vec<u8> = (0..h as usize)
        .map(|y| px[(y * w as usize + col) * 4 + 3])
        .collect();
    assert!(
        px[(below * w as usize + col) * 4 + 3] > 0,
        "nothing below the clip at {below} (clip {clip}, centre {:?}, size {:?}, xform {:?}): {column:?}",
        m[0].center,
        m[0].size,
        m[0].xform
    );
    // With blur on, a wipe still splits where it did: the sung colour (red) left of
    // the split, the rest (blue) right of it, along the glyph's middle row.
    let mut wipe = |blur: f64| {
        let effect = tracked_effect(
            serde_json::json!([
                {"unit": "line", "slot": "loop", "property": "blur", "value": blur},
                {"unit": "word", "slot": "active", "property": "wipe", "keys": [[0, 0], [1, 1]]}
            ]),
            serde_json::json!([{"kind": "fill", "color": "#0000FF", "color2": "#FF0000"}]),
        )
        .unwrap();
        let line = split_words(1.0, 2.0, &["MMM".to_string()], " ");
        let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
        let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 1.5);
        let (px, _) = gpu.frame(&g, gain, &m, true).unwrap();
        let row = m[1].center[1].round() as usize;
        // The first column, left to right, where blue outweighs red.
        (0..w as usize)
            .find(|&x| {
                let p = &px[(row * w as usize + x) * 4..];
                p[3] > 40 && p[2] > p[0]
            })
            .unwrap()
    };
    let (sharp, soft) = (wipe(0.0), wipe(2.0));
    assert!(
        (sharp as i32 - soft as i32).abs() <= 3,
        "split moved: {sharp} vs {soft}"
    );
    // Still drawn a little past where the sprite used to end.
    let past = (cx + half + 3.0).round() as usize;
    assert!(
        alpha[past] >= 1,
        "cut at the sprite: {} at {past}, {alpha:?}",
        alpha[past]
    );
}

/// Gap 13 measurements for the round: continuity across the blur layer's
/// resolution steps and at the direct path's edge, half-pixel motion at a large
/// sigma, cost with one blur group per letter, and the export path's alpha.
#[test]
#[ignore = "measurement: needs NOH_FX_BLUR_PROBE and a GPU"]
fn blur_layer_measurements() {
    use super::gl::GlRenderer;
    use super::shaders::Output;
    if std::env::var_os("NOH_FX_BLUR_PROBE").is_none() {
        return;
    }
    let (w, h) = (640u32, 240u32);
    let mut fonts = Fonts::load().unwrap();
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
    let words = |s: &str| s.split_whitespace().map(str::to_string).collect::<Vec<_>>();
    let mut alpha = |tracks: serde_json::Value| -> Vec<f32> {
        let effect = tracked_effect(tracks, fill_only()).unwrap();
        let line = split_words(1.0, 2.0, &words("ab cd"), " ");
        let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
        let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 1.5);
        let (px, _) = gpu.frame(&g, gain, &m, true).unwrap();
        px.chunks(4).map(|p| f32::from(p[3])).collect()
    };
    let blur = |s: f64| serde_json::json!([{"unit": "line", "slot": "loop", "property": "blur", "value": s}]);
    let diff = |a: &[f32], b: &[f32]| -> (f32, f32) {
        let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| (x - y).abs()).collect();
        (
            d.iter().sum::<f32>() / d.len() as f32,
            d.iter().fold(0.0f32, |m, v| m.max(*v)),
        )
    };
    // Values on the quarter-pixel key, straddling each threshold: the direct path
    // ends below a quarter pixel; the layer halves past 4, 8 and 16.
    for steps in [
        [0.0, 0.25, 0.5, 0.75, 1.0],
        [3.5, 3.75, 4.0, 4.25, 4.5],
        [7.5, 7.75, 8.0, 8.25, 8.5],
        [15.5, 15.75, 16.0, 16.25, 16.5],
    ] {
        let imgs: Vec<Vec<f32>> = steps.iter().map(|s| alpha(blur(*s))).collect();
        for k in 0..steps.len() - 1 {
            let (mean, max) = diff(&imgs[k], &imgs[k + 1]);
            eprintln!(
                "sigma {} -> {}: mean |d| {mean:.3}, max |d| {max:.0}",
                steps[k],
                steps[k + 1]
            );
        }
    }
    for sigma in [8.0, 40.0] {
        let a = alpha(blur(sigma));
        let b = alpha(serde_json::json!([
            {"unit": "line", "slot": "loop", "property": "blur", "value": sigma},
            {"unit": "line", "slot": "loop", "property": "offset_x", "value": 0.5 / 64.0}
        ]));
        let (mean, max) = diff(&a, &b);
        eprintln!("sigma {sigma}: half-pixel shift mean |d| {mean:.3}, max |d| {max:.0}");
    }
    // Export path: the same frame through Output::StraightSrgb has the same alpha.
    let preview = alpha(blur(6.0));
    let mut export = GlRenderer::headless(w, h, w as f32, h as f32, Output::StraightSrgb).unwrap();
    let effect = tracked_effect(blur(6.0), fill_only()).unwrap();
    let line = split_words(1.0, 2.0, &words("ab cd"), " ");
    let mut scene = PluginScene::new(effect, w, h, vec![line], Some("p"));
    let (g, gain, m, _) = scene.frame(&mut fonts, &export, 1.5);
    let (px, _) = export.frame(&g, gain, &m, true).unwrap();
    let straight: Vec<f32> = px.chunks(4).map(|p| f32::from(p[3])).collect();
    let (mean, max) = diff(&straight, &preview);
    eprintln!("export vs preview alpha: mean |d| {mean:.3}, max |d| {max:.0}");
    // One blur group per letter: a staggered letter blur on a 1080p line.
    for (cw, ch) in [(1920u32, 1080u32), (3840, 2160)] {
        let mut big =
            GlRenderer::headless(cw, ch, cw as f32, ch as f32, Output::PremulGamma).unwrap();
        let effect = tracked_effect(
            serde_json::json!([{"unit": "letter", "slot": "enter", "property": "blur", "duration": 2.0,
                                "stagger": 0.05, "keys": [[0, 30], [1, 0]]}]),
            fill_only(),
        )
        .unwrap();
        let line = split_words(1.0, 9.0, &words("On a gardé la lumière ce soir"), " ");
        let mut scene = PluginScene::new(effect, cw, ch, vec![line], Some("p"));
        let mut times = Vec::new();
        for k in 0..40 {
            let (g, gain, m, _) = scene.frame(&mut fonts, &big, 1.2 + f64::from(k) * 0.01);
            let (_, t) = big.frame(&g, gain, &m, true).unwrap();
            if k >= 10 {
                times.push(t.gpu_ms.unwrap_or(f64::NAN));
            }
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "letter-staggered blur {cw}x{ch}: gpu median {:.3} ms, max {:.3} ms",
            times[times.len() / 2],
            times[times.len() - 1]
        );
    }
}

#[test]
fn cubic_curves_follow_x_cubed_and_its_mirror() {
    // A line enter of 1 s, opacity 0 -> 1, sampled at 20 %, 50 % and 80 %.
    let opacity = |curve: &str, x: f64| -> f32 {
        let effect = tracked_effect(
            serde_json::json!([{"unit": "line", "slot": "enter", "property": "opacity",
                                "duration": 1.0, "curve": curve, "keys": [[0, 0], [1, 1]]}]),
            fill_only(),
        )
        .unwrap();
        let mut fonts = Fonts::load().unwrap();
        let line = split_words(1.0, 3.0, &["la".to_string()], " ");
        let mut scene = PluginScene::new(effect, 1920, 1080, vec![line], Some("p"));
        scene.frame(&mut fonts, &NoGpu, 1.0 + x).2[0].color[3]
    };
    for x in [0.2, 0.5, 0.8] {
        let (i, o) = (opacity("ease_in_cubic", x), opacity("ease_out_cubic", x));
        assert!((f64::from(i) - x * x * x).abs() < 1e-5, "in {x}: {i}");
        assert!(
            (f64::from(o) - (1.0 - (1.0 - x).powi(3))).abs() < 1e-5,
            "out {x}: {o}"
        );
    }
}

/// A temp folder holding unmodified bundled files stands for the installed fonts.
fn installed(files: &[&str]) -> (tempfile::TempDir, Fonts) {
    let dir = tempfile::tempdir().unwrap();
    for f in files {
        std::fs::copy(repo(&format!("assets/{f}")), dir.path().join(f)).unwrap();
    }
    let db = super::text::system_fonts(Some(&[dir.path().to_path_buf()]));
    let fonts = Fonts::load()
        .unwrap()
        .with_installed(std::sync::Arc::new(db));
    (dir, fonts)
}

/// A plugin whose layout font is its font parameter (default: a bundled face).
fn font_effect(default: &str) -> Result<super::plugin::Effect, String> {
    let j = serde_json::json!({
        "engine": 1, "id": "test.font", "name": {"en": "Font"}, "version": "0.1.0",
        "license": "GPL-3.0-only", "target": "text",
        "params": [{"id": "font", "type": "font", "default": default, "level": 2,
                    "label": {"en": "Font"}}],
        "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
        "layout": {"font": "$font", "size": 64},
        "animate": [],
        "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
    });
    super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes()).map_err(|e| e.0)
}

fn font_scene(value: &str) -> PluginScene {
    font_scene_at(value, 1920, 1080)
}

fn font_scene_at(value: &str, w: u32, h: u32) -> PluginScene {
    let values = [("font".to_string(), serde_json::json!(value))]
        .into_iter()
        .collect();
    let line = split_words(1.0, 3.0, &["AB".to_string()], " ");
    PluginScene::new(
        font_effect("condensed_extrabold").unwrap(),
        w,
        h,
        vec![line],
        Some("p"),
    )
    .with_values(&values)
    .unwrap()
}

#[test]
fn an_installed_family_draws_from_its_own_file_in_both_atlases() {
    use super::text::{Face, FontData, layout};
    let (dir, mut fonts) = installed(&["BarlowCondensed-ExtraBold.ttf"]);
    let (face, missing) = fonts.face("system:Barlow Condensed", Face::CondensedExtraBold);
    assert!(missing.is_none());
    assert!(matches!(face, Face::System(_)), "{face:?}");
    let laid = |fonts: &mut Fonts, face| {
        layout(fonts, &["AB".to_string()], "", &[], face, 64.0, None, 1.0)
    };
    let mine = laid(&mut fonts, face);
    let shipped = laid(&mut fonts, Face::CondensedExtraBold);
    let id = mine.glyphs[0].font_id;
    // Its own face, not the bundled one of the same family, from the same design.
    assert_ne!(id, shipped.glyphs[0].font_id);
    assert_eq!(mine.glyphs[0].glyph_id, shipped.glyphs[0].glyph_id);
    assert_eq!(mine.width, shipped.width);
    // Its bytes are the installed file's, shared (mapped), not the bundled copy.
    let (data, _) = &fonts.bytes[&id];
    assert!(matches!(data, FontData::Shared(_)));
    let file = std::fs::read(dir.path().join("BarlowCondensed-ExtraBold.ttf")).unwrap();
    assert_eq!(data.bytes(), &file[..]);
    let mut atlas = super::text::Atlas::new(48.0);
    let g = &mine.glyphs[0];
    assert!(atlas.cov_sprite(&mut fonts, &NoGpu, g, 1.0).is_some());
    assert!(atlas.msdf_sprite(&fonts, &NoGpu, g).is_some());
    // Asked again: the same face, not a second copy.
    let (again, _) = fonts.face("system:Barlow Condensed", Face::CondensedExtraBold);
    assert_eq!(again, face);
    // A scene with it draws, and nothing is reported missing.
    let mut scene = font_scene("system:Barlow Condensed");
    let main = scene.frame(&mut fonts, &NoGpu, 2.0).2;
    assert_eq!(main.len(), 2);
    assert_eq!(scene.font_missing(), None);
}

#[test]
fn a_missing_family_draws_the_plugin_face_and_is_reported() {
    use super::text::{Face, Lack};
    let (_dir, mut fonts) = installed(&["BarlowCondensed-ExtraBold.ttf"]);
    assert_eq!(
        fonts.face("system:Nope Sans", Face::CondensedExtraBold),
        (
            Face::CondensedExtraBold,
            Some(Lack::Missing("Nope Sans".to_string()))
        )
    );
    // Installed fonts not read yet: the plugin's face for now, not « missing ».
    let mut bare = Fonts::load().unwrap();
    assert_eq!(
        bare.face("system:Barlow Condensed", Face::Jost),
        (
            Face::Jost,
            Some(Lack::NotRead("Barlow Condensed".to_string()))
        )
    );
    let mut early = font_scene("system:Barlow Condensed");
    early.frame(&mut bare, &NoGpu, 2.0);
    assert_eq!(early.font_missing(), None);
    assert!(early.font_unread());
    // The scene draws its own face (the same instances as asking for it) and says so.
    let mut scene = font_scene("system:Nope Sans");
    let main = scene.frame(&mut fonts, &NoGpu, 2.0).2;
    assert_eq!(scene.font_missing(), Some("Nope Sans"));
    assert!(!scene.font_unread());
    let mut own = font_scene("condensed_extrabold");
    let shipped = own.frame(&mut fonts, &NoGpu, 2.0).2;
    assert_eq!(
        bytemuck::cast_slice::<Inst, u8>(&main),
        bytemuck::cast_slice::<Inst, u8>(&shipped)
    );
    assert_eq!(own.font_missing(), None);
}

#[test]
fn a_font_removed_after_the_read_is_missing_and_leaves_nothing_behind() {
    use super::text::{Face, Lack};
    let (dir, mut fonts) = installed(&["BarlowCondensed-ExtraBold.ttf"]);
    std::fs::remove_file(dir.path().join("BarlowCondensed-ExtraBold.ttf")).unwrap();
    let before = fonts.system.db().len();
    for _ in 0..2 {
        assert_eq!(
            fonts.face("system:Barlow Condensed", Face::CondensedExtraBold),
            (
                Face::CondensedExtraBold,
                Some(Lack::Missing("Barlow Condensed".to_string()))
            )
        );
        assert_eq!(fonts.system.db().len(), before);
    }
}

#[test]
fn an_installed_font_never_takes_over_a_bundled_face_fallback() {
    use super::text::{Face, layout};
    // A wide-coverage file installed and chosen BEFORE any text needs a fallback
    // (the order a user makes): a character Jost lacks must still be drawn from
    // the bundled file, not from the installed copy. Two separate font systems,
    // so no cache carries one answer to the other; the file is named by family.
    let jost =
        ttf_parser::Face::parse(include_bytes!("../../assets/Jost-ExtraBold.ttf"), 0).unwrap();
    let noto =
        ttf_parser::Face::parse(include_bytes!("../../assets/NotoSans-Regular.ttf"), 0).unwrap();
    let c = ['Ж', 'Ω', 'ש', 'ạ', 'ǅ', '₹', 'Ꭰ', 'ⴰ']
        .into_iter()
        .find(|c| jost.glyph_index(*c).is_none() && noto.glyph_index(*c).is_some())
        .expect("a character Noto Sans has and Jost lacks");
    let text = format!("A{c}");
    let families = |fonts: &mut Fonts| {
        let laid = layout(fonts, &[text.clone()], "", &[], Face::Jost, 64.0, None, 1.0);
        laid.glyphs
            .iter()
            .map(|g| {
                fonts.system.db().face(g.font_id).unwrap().families[0]
                    .0
                    .clone()
            })
            .collect::<Vec<_>>()
    };
    let (_plain_dir, mut plain) = installed(&["NotoSans-Regular.ttf"]);
    let shipped = families(&mut plain);
    // The character really falls back (to a bundled family other than Jost).
    assert!(shipped.iter().any(|f| f != "Jost"), "{shipped:?}");
    let (_dir, mut chosen) = installed(&["NotoSans-Regular.ttf"]);
    let (face, lack) = chosen.face("system:Noto Sans", Face::Sans);
    assert!(
        lack.is_none() && matches!(face, Face::System(_)),
        "{face:?}"
    );
    let with_installed = families(&mut chosen);
    assert_eq!(with_installed, shipped, "fallback for {c:?}");
    assert!(
        with_installed
            .iter()
            .all(|f| !f.starts_with("NOH installed")),
        "{with_installed:?}"
    );
}

#[test]
fn a_variable_installed_font_is_drawn_at_the_plugin_weight() {
    use super::text::Face;
    let (_dir, mut fonts) = installed(&["Jost-Variable.ttf"]);
    // Jost ExtraBold (800) is the plugin's face: the variable file is asked at 800.
    let (face, missing) = fonts.face("system:Jost", Face::Jost);
    assert!(
        missing.is_none() && matches!(face, Face::System(_)),
        "{face:?}"
    );
    assert_eq!(fonts.face_info(face).1.0, 800);
    let laid = super::text::layout(
        &mut fonts,
        &["H".to_string()],
        "",
        &[],
        face,
        64.0,
        None,
        1.0,
    );
    assert_eq!(fonts.weights[&laid.glyphs[0].font_id], 800.0);
}

/// GPU: an installed copy of a bundled file draws the same pixels as the bundled face.
fn an_installed_font_draws_on_the_gpu_like_the_bundled_face() {
    use super::gl::GlRenderer;
    use super::shaders::Output;
    let (w, h) = (320u32, 200u32);
    let (_dir, mut fonts) = installed(&["BarlowCondensed-ExtraBold.ttf"]);
    let mut gpu = GlRenderer::headless(w, h, w as f32, h as f32, Output::PremulGamma).unwrap();
    let mut draw = |value: &str| {
        let mut scene = font_scene_at(value, w, h);
        let (g, gain, m, _) = scene.frame(&mut fonts, &gpu, 2.0);
        assert_eq!(scene.font_missing(), None, "{value}");
        gpu.frame(&g, gain, &m, true).unwrap().0
    };
    let installed_px = draw("system:Barlow Condensed");
    let bundled_px = draw("condensed_extrabold");
    assert!(installed_px.chunks(4).any(|p| p[3] > 200), "ink drawn");
    assert_eq!(installed_px, bundled_px);
}

#[test]
fn a_font_param_defaults_to_a_bundled_face_and_takes_installed_names() {
    for default in ["system:Barlow Condensed", "comic"] {
        let refused = font_effect(default).unwrap_err();
        assert!(refused.contains("a bundled face"), "{default}: {refused}");
    }
    let effect = font_effect("condensed_extrabold").unwrap();
    let p = &effect.params[0];
    for (value, ok) in [
        ("jost_medium", true),
        ("system:Barlow Condensed", true),
        ("system:", false),
        ("system:  ", false),
        ("comic", false),
    ] {
        assert_eq!(p.accepts(&serde_json::json!(value)), ok, "{value}");
    }
}

#[test]
#[ignore = "measurement: reads this machine's installed fonts (run with --nocapture)"]
fn installed_fonts_read_time() {
    let started = std::time::Instant::now();
    let db = super::text::system_fonts(None);
    let read = started.elapsed();
    let families = super::text::families_of(&db);
    eprintln!(
        "installed fonts: {} faces, {} families, read in {read:?}",
        db.len(),
        families.len()
    );
}

#[test]
fn an_installed_font_named_in_a_layout_or_a_role_is_refused() {
    // Only a font parameter may name an installed family (its default stays a
    // bundled face, drawn when the family is missing).
    for font in [
        serde_json::json!("system:Arial"),
        serde_json::json!({"by_offset": ["jost_extrabold", "system:Arial"]}),
    ] {
        let j = serde_json::json!({
            "engine": 1, "id": "test.literal", "name": {"en": "L"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
            "layout": {"font": font, "size": 64},
            "animate": [],
            "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
        });
        let refused = super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes())
            .err()
            .map(|e| e.0)
            .expect("refused");
        assert!(
            refused.contains("layout.font: an installed font is chosen through a font parameter"),
            "{font}: {refused}"
        );
    }
    // Nor through another type of parameter (a choice listing an installed
    // family): the export would not read the installed fonts for it.
    for (default, choices) in [
        (
            "jost_extrabold",
            serde_json::json!(["jost_extrabold", "system:Arial"]),
        ),
        (
            "system:Arial",
            serde_json::json!(["system:Arial", "jost_extrabold"]),
        ),
    ] {
        let j = serde_json::json!({
            "engine": 1, "id": "test.choice_font", "name": {"en": "C"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "params": [{"id": "face", "type": "choice", "default": default, "choices": choices,
                        "level": 1, "label": {"en": "Face"}}],
            "presets": [{"id": "p", "name": {"en": "P"}, "values": {}}],
            "layout": {"font": "$face", "size": 64},
            "animate": [],
            "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
        });
        let refused = super::plugin::Effect::from_json(Path::new(""), j.to_string().as_bytes())
            .err()
            .map(|e| e.0)
            .expect("refused");
        assert!(
            refused.contains("an installed font is chosen through a font parameter only"),
            "{default}: {refused}"
        );
    }
    // An open choice (no list) takes any text: not an installed family, from a
    // preset nor from the settings.
    let open = |preset: serde_json::Value| {
        serde_json::json!({
            "engine": 1, "id": "test.open_choice", "name": {"en": "O"}, "version": "0.1.0",
            "license": "GPL-3.0-only", "target": "text",
            "params": [{"id": "face", "type": "choice", "default": "jost_extrabold",
                        "level": 1, "label": {"en": "Face"}}],
            "presets": [{"id": "p", "name": {"en": "P"}, "values": preset}],
            "layout": {"font": "$face", "size": 64},
            "animate": [],
            "render": {"passes": [{"kind": "fill", "color": "#FFFFFF"}]}
        })
        .to_string()
    };
    let refused = super::plugin::Effect::from_json(
        Path::new(""),
        open(serde_json::json!({"face": "system:Arial"})).as_bytes(),
    )
    .err()
    .map(|e| e.0)
    .expect("refused");
    assert!(
        refused.contains(
            "presets[0].values.face: an installed font is chosen through a font parameter only"
        ),
        "{refused}"
    );
    let effect =
        super::plugin::Effect::from_json(Path::new(""), open(serde_json::json!({})).as_bytes())
            .map_err(|e| e.0)
            .unwrap();
    let values = [("face".to_string(), serde_json::json!("system:Arial"))]
        .into_iter()
        .collect();
    let line = split_words(1.0, 3.0, &["AB".to_string()], " ");
    let refused = PluginScene::new(effect, 1920, 1080, vec![line], Some("p"))
        .with_values(&values)
        .err()
        .expect("refused");
    assert!(refused.contains("not a valid choice"), "{refused}");
    let path = repo("tests/fixtures/fx/compose/effect.json");
    let mut j: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // The fixture's carrier role, wherever its « roles » object sits.
    fn carrier(v: &serde_json::Value, at: String) -> Option<String> {
        let o = v.as_object()?;
        if o.get("roles").and_then(|r| r.get("carrier")).is_some() {
            return Some(format!("{at}/roles/carrier"));
        }
        o.iter().find_map(|(k, v)| carrier(v, format!("{at}/{k}")))
    }
    let pointer = carrier(&j, String::new()).expect("a carrier role");
    j.pointer_mut(&pointer).unwrap()["font"] = serde_json::json!("system:Arial");
    let refused = super::plugin::Effect::from_json(&path, j.to_string().as_bytes())
        .err()
        .map(|e| e.0)
        .expect("refused");
    assert!(
        refused.contains(".font: an installed font is chosen through a font parameter"),
        "{refused}"
    );
}

#[test]
fn the_bundled_plugins_take_an_installed_font_in_one_level_2_setting() {
    // Each step 7 plugin: one font setting, level 2, defaulting to the face it
    // drew before (so its recorded instances do not move).
    let bundled = scan(&[repo("assets/effects")]);
    for (id, face) in [
        ("noh.karaoke-simple", "inter_semibold"),
        ("noh.drop", "jost_extrabold"),
        ("noh.sweep", "jost_extrabold"),
        ("noh.breath", "jost_medium"),
        ("noh.block", "condensed_extrabold"),
    ] {
        let e = &bundled
            .effects
            .iter()
            .find(|e| e.effect.id == id)
            .unwrap()
            .effect;
        let fonts: Vec<_> = e.params.iter().filter(|p| p.kind == "font").collect();
        assert_eq!(fonts.len(), 1, "{id}");
        assert_eq!(fonts[0].level, 2, "{id}");
        assert_eq!(fonts[0].default, serde_json::json!(face), "{id}");
        let values = [("font".to_string(), serde_json::json!("system:Nope Sans"))]
            .into_iter()
            .collect();
        let scene = PluginScene::new(e.clone(), 1920, 1080, lines(), Some(&e.presets[0].0))
            .with_values(&values)
            .unwrap();
        assert!(scene.wants_installed_fonts(), "{id}");
        let plain = PluginScene::new(e.clone(), 1920, 1080, lines(), Some(&e.presets[0].0));
        assert!(!plain.wants_installed_fonts(), "{id}");
    }
}

/// The maintainer's choice B (2026-11-12): a word wider than the box is never cut between
/// letters. It stays whole on its own line, centred on the box (cosmic-text alone
/// would start it at the box's left edge), and the layout says so.
#[test]
fn a_word_wider_than_the_box_stays_whole_centred_and_is_said() {
    use super::text::{Face, layout};
    let mut fonts = Fonts::load().unwrap();
    let long = "Supercalifragilisticexpialidocious";
    let words: Vec<String> = ["on", long, "ici"].iter().map(|s| s.to_string()).collect();
    let laid = layout(
        &mut fonts,
        &words,
        " ",
        &[],
        Face::Sans,
        40.0,
        Some(200.0),
        1.2,
    );
    assert!(laid.too_wide);
    let word: Vec<_> = laid.glyphs.iter().filter(|g| g.word == 1).collect();
    assert!(word.len() >= long.len() - 2, "the word is drawn whole");
    assert!(word.iter().all(|g| g.line == word[0].line), "on one line");
    let x0 = word.iter().map(|g| g.x).fold(f32::MAX, f32::min);
    let x1 = word
        .iter()
        .map(|g| g.x + g.advance)
        .fold(f32::MIN, f32::max);
    assert!(x0 < 0.0 && x1 > 200.0, "wider than the box: {x0}..{x1}");
    assert!(((x0 + x1) * 0.5 - 100.0).abs() < 1.0, "centred: {x0}..{x1}");
    // Its neighbours stay inside the box, on their own lines.
    for g in laid.glyphs.iter().filter(|g| g.word != 1) {
        assert_ne!(g.line, word[0].line);
        assert!(g.x >= -0.5 && g.x + g.advance <= 200.5);
    }
    // A right-to-left line wider than the box is centred too (cosmic-text puts
    // it at [box - line, box]).
    let rtl: Vec<String> = ["שלום", &"אבגדהוזחטיכלמנסעפצקרשת".repeat(2)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let laid = layout(
        &mut fonts,
        &rtl,
        " ",
        &[],
        Face::Sans,
        40.0,
        Some(200.0),
        1.2,
    );
    assert!(laid.too_wide);
    let word: Vec<_> = laid.glyphs.iter().filter(|g| g.word == 1).collect();
    let x0 = word.iter().map(|g| g.x).fold(f32::MAX, f32::min);
    let x1 = word
        .iter()
        .map(|g| g.x + g.advance)
        .fold(f32::MIN, f32::max);
    assert!(
        ((x0 + x1) * 0.5 - 100.0).abs() < 1.0,
        "RTL centred: {x0}..{x1}"
    );
    let fits: Vec<String> = ["on", "a", "la"].iter().map(|s| s.to_string()).collect();
    assert!(
        !layout(
            &mut fonts,
            &fits,
            " ",
            &[],
            Face::Sans,
            40.0,
            Some(200.0),
            1.2
        )
        .too_wide
    );
}

/// The same in a scene, for Classique and a plugin: the too-wide word's letters
/// sit symmetrically about the box centre (the canvas centre here), and the scene
/// says it (`too_wide`), with or without a safe area.
#[test]
fn a_too_wide_word_is_centred_and_said_by_classique_and_a_plugin() {
    let mut fonts = Fonts::load().unwrap();
    let long = "SupercalifragilisticexpialidociousSupercalifragilisticexpialidocious";
    let words = vec![long.to_string()];
    let drop = super::catalog::load_one(&repo("assets/effects/drop"), "noh.drop").unwrap();
    for (name, effect) in [
        ("classique", super::catalog::builtin_classic()),
        ("drop", drop),
    ] {
        let mut s = PluginScene::new(
            effect,
            1920,
            1080,
            vec![split_words(0.0, 4.0, &words, " ")],
            None,
        );
        let (_, _, main, _) = s.frame(&mut fonts, &NoGpu, 3.0);
        assert!(s.too_wide(), "{name}");
        let xs: Vec<f32> = main.iter().map(|i| i.center[0]).collect();
        assert!(!xs.is_empty(), "{name}");
        let (a, b) = (
            xs.iter().copied().fold(f32::MAX, f32::min),
            xs.iter().copied().fold(f32::MIN, f32::max),
        );
        assert!(((a + b) * 0.5 - 960.0).abs() < 6.0, "{name}: {a}..{b}");
        let fits = vec!["lumière".to_string()];
        let mut s = PluginScene::new(
            s.effect.clone(),
            1920,
            1080,
            vec![split_words(0.0, 4.0, &fits, " ")],
            None,
        );
        s.frame(&mut fonts, &NoGpu, 3.0);
        assert!(!s.too_wide(), "{name}: a word that fits");
    }
}

/// The font picker's rows (maintainer, 2026-11-12): a face's name drawn in that face,
/// at the weight the effect asks for; a face lacking its name's glyphs, or a
/// symbol font, is said so (the row keeps the interface font).
#[test]
fn a_face_draws_its_own_name_or_says_why_it_cannot() {
    use super::text::{Face, Label};
    let mut fonts = Fonts::load().unwrap();
    let ink = |l: &Label| match l {
        Label::Drawn { w, h, alpha } => {
            assert_eq!(alpha.len(), (*w * *h) as usize);
            alpha.iter().filter(|a| **a > 128).count()
        }
        other => panic!("not drawn: {other:?}"),
    };
    for face in [
        Face::Jost,
        Face::JostMedium,
        Face::CondensedExtraBold,
        Face::SerifItalic,
    ] {
        let l = fonts.label(face, "Jost ExtraBold", 28.0);
        assert!(ink(&l) > 50, "{face:?}");
    }
    // Barlow Condensed has no Han glyphs: never borrowed from another font.
    assert_eq!(
        fonts.label(Face::CondensedExtraBold, "漢字", 28.0),
        Label::Unreadable
    );
    // An installed family is drawn at the effect face's weight (the engine's own
    // query): SemiBold asked by a bold effect face, Regular by a regular one.
    let dir = tempfile::tempdir().unwrap();
    for f in ["NotoSans-Regular.ttf", "NotoSans-SemiBold.ttf"] {
        std::fs::copy(repo(&format!("assets/{f}")), dir.path().join(f)).unwrap();
    }
    let db = std::sync::Arc::new(super::text::system_fonts(Some(&[dir.path().to_path_buf()])));
    fonts.set_installed(db);
    let (bold, _) = fonts.face("system:Noto Sans", Face::Jost);
    let (regular, _) = fonts.face("system:Noto Sans", Face::Sans);
    let (b, r) = (
        ink(&fonts.label(bold, "Noto Sans", 28.0)),
        ink(&fonts.label(regular, "Noto Sans", 28.0)),
    );
    assert!(
        b > r * 11 / 10,
        "bolder ink for the bold effect: {b} vs {r}"
    );
    // A symbol font (Windows Symbol cmap): checked where this machine has one.
    let wingdings = std::path::Path::new("C:/Windows/Fonts/wingding.ttf");
    if wingdings.is_file() {
        let sym = tempfile::tempdir().unwrap();
        std::fs::copy(wingdings, sym.path().join("wingding.ttf")).unwrap();
        let db = std::sync::Arc::new(super::text::system_fonts(Some(&[sym.path().to_path_buf()])));
        fonts.set_installed(db);
        let (face, lack) = fonts.face("system:Wingdings", Face::Sans);
        assert_eq!(lack, None);
        assert_eq!(fonts.label(face, "Wingdings", 28.0), Label::Symbol);
    }
}
