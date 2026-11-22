//! The pass run before a scene is shown or exported: evaluate the scene at every
//! instant the output will draw, check each frame as `check` does, ask every
//! frame again in another order (a scene is a pure function of `t`), and report.
//! It refuses on counted quantities only; measured time is reported, never a
//! reason to refuse. It does not replace the check of each frame evaluated
//! afterwards: between two of its instants a script may still exceed a cap.
use super::check::{Checker, pass_count};
use super::draw::{BoxF, Draw, layout_key, measure};
use super::script::{Frame, MAX_CALLS, MAX_OPS, ScriptScene};
use crate::fx::text::Fonts;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Bars of the script's own time on the reference machine: warnings, never refusals.
pub const SCRIPT_P95_MS: f64 = 2.0;
pub const SCRIPT_WORST_MS: f64 = 4.0;

/// The frame rate the validation grid falls back on when the source does not
/// say its own, and what the report then says.
pub const FALLBACK_FPS: f64 = 30.0;
pub const UNKNOWN_FPS: &str = "the video's frame rate is unknown: the scene was checked at 30 frames per second; every exported frame is still checked";

/// A source's nominal frame rate when it is known (capped at 120).
pub fn known_fps(fps: f64) -> Option<f64> {
    (fps.is_finite() && fps > 0.0).then(|| fps.min(120.0))
}

/// The instants of an output that starts at `from`, ends before `to` and runs at
/// `fps` frames per second: what a montage or a short draws.
pub fn grid(from: f64, to: f64, fps: f64) -> Vec<f64> {
    let n = ((to - from) * fps).round().max(1.0) as usize;
    (0..n).map(|i| from + i as f64 / fps).collect()
}

#[derive(Debug, Default)]
pub struct Report {
    pub frames: usize,
    /// Counted: these decide.
    pub max_ops: u64,
    pub max_ops_at: f64,
    pub max_draws: usize,
    pub max_particles: usize,
    pub max_glyphs: usize,
    /// Surface covered, in frames (every pass counted), and where it peaks.
    pub max_area: f32,
    pub max_area_at: f64,
    pub max_blurs: usize,
    pub max_passes: usize,
    pub max_paths: usize,
    pub max_segments: usize,
    pub max_shape_area: f32,
    pub max_calls: u32,
    pub max_reads: u32,
    /// Shader layers in a frame, and the pixels they compute (in frames).
    pub max_shader_layers: usize,
    pub max_shader_area: f32,
    pub blocks: usize,
    pub peak_text_scale: f32,
    /// Measured on this machine: reported only.
    pub script_ms_p95: f64,
    pub script_ms_worst: f64,
    pub script_ms_worst_at: f64,
    pub warnings: Vec<String>,
    /// Text leaving the frame or the safe area, texts overlapping, fills cut
    /// with a forced triangle: said, never corrected. Each with its first
    /// instant and its number of frames.
    pub layout_warnings: Vec<String>,
    /// What this pass did not look at, so that silence is not read as « fine ».
    pub not_checked: Vec<String>,
}

/// What a frame draws, reduced to a number: two frames that draw the same thing
/// have the same one (keeping every frame whole would cost too much memory).
fn fingerprint(f: &Frame) -> u64 {
    let mut h = DefaultHasher::new();
    format!("{:?}{:?}{:?}", f.draws, f.layers, f.video).hash(&mut h);
    h.finish()
}

fn p95(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * 0.95).round() as usize]
}

/// Notes said once each, with their first instant and how often they came.
#[derive(Default)]
struct Notes(Vec<(String, f64, usize)>);

impl Notes {
    fn add(&mut self, msg: String, t: f64) {
        if let Some(x) = self.0.iter_mut().find(|x| x.0 == msg) {
            x.2 += 1;
        } else if self.0.len() < 40 {
            self.0.push((msg, t, 1));
        }
    }
}

/// Where each visible, unrotated text falls: out of the frame, out of the safe
/// area, over another well visible text. Returns how many rotated texts it left out.
fn place_texts(
    f: &Frame,
    t: f64,
    fonts: &RefCell<Fonts>,
    boxes: &mut HashMap<String, BoxF>,
    safe: [f32; 4],
    (w, h): (f32, f32),
    notes: &mut Notes,
) -> usize {
    let mut rotated = 0;
    let mut placed: Vec<(&str, BoxF)> = Vec::new();
    for d in &f.draws {
        let Draw::Text(td) = d else { continue };
        if td.opacity <= 0.02 || td.dissolve >= 0.98 {
            continue;
        }
        if td.block.rot != 0.0 {
            rotated += 1;
            continue;
        }
        let b = &td.block;
        let key = format!(
            "{}|{}|{}|{}|{}",
            layout_key(b, w, h),
            b.x,
            b.y,
            b.anchor,
            td.scale
        );
        let bx = *boxes
            .entry(key)
            .or_insert_with(|| measure(&mut fonts.borrow_mut(), b, td.scale, w, h).0);
        let label: String = b.text.chars().take(24).collect();
        if bx.x0 < -0.002 || bx.y0 < -0.002 || bx.x1 > 1.002 || bx.y1 > 1.002 {
            notes.add(format!("{label:?} leaves the frame"), t);
        } else if bx.x0 < safe[0] - 0.002
            || bx.y0 < safe[1] - 0.002
            || bx.x1 > safe[2] + 0.002
            || bx.y1 > safe[3] + 0.002
        {
            notes.add(format!("{label:?} leaves the safe area"), t);
        }
        // A cross-fade, where one leaves as the other arrives, is not an overlap.
        if td.opacity < 0.5 || td.dissolve > 0.5 {
            continue;
        }
        for (other, o) in &placed {
            let (ix, iy) = (
                bx.x1.min(o.x1) - bx.x0.max(o.x0),
                bx.y1.min(o.y1) - bx.y0.max(o.y0),
            );
            if ix > 0.004 && iy > 0.004 && *other != b.text {
                let ol: String = other.chars().take(24).collect();
                notes.add(format!("{label:?} overlaps {ol:?}"), t);
            }
        }
        placed.push((&b.text, bx));
    }
    rotated
}

/// Validate `scene` at `instants` (in order, as the output draws them) against
/// the checker of the same inputs. `safe` is the surface text should stay in.
pub fn validate(
    scene: &ScriptScene,
    checker: &mut Checker,
    fonts: &RefCell<Fonts>,
    instants: &[f64],
    safe: [f32; 4],
    (w, h): (u32, u32),
) -> Result<Report, String> {
    let mut rep = Report {
        frames: instants.len(),
        ..Report::default()
    };
    let mut notes = Notes::default();
    let mut boxes = HashMap::new();
    let mut rotated = 0usize;
    let mut kept: Vec<u64> = Vec::with_capacity(instants.len());
    let mut times = Vec::with_capacity(instants.len());
    for &t in instants {
        let f = scene.frame(t)?;
        let c = checker.check(&f, t)?;
        rotated += place_texts(
            &f,
            t,
            fonts,
            &mut boxes,
            safe,
            (w as f32, h as f32),
            &mut notes,
        );
        if let Some(v) = &f.video
            && !v
                .framing((w as f32, h as f32), (w as f32, h as f32))
                .covers()
        {
            notes.add(
                "the picture does not cover the frame (black shows at an edge)".into(),
                t,
            );
        }
        if c.forced > 0 {
            notes.add(
                "a filled path could not be cut cleanly (a triangle that is not an ear was taken): it may paint outside its shape".into(),
                t,
            );
        }
        if c.area > rep.max_area {
            (rep.max_area, rep.max_area_at) = (c.area, t);
        }
        if f.ops > rep.max_ops {
            (rep.max_ops, rep.max_ops_at) = (f.ops, t);
        }
        if f.run_ms > rep.script_ms_worst {
            (rep.script_ms_worst, rep.script_ms_worst_at) = (f.run_ms, t);
        }
        rep.max_glyphs = rep.max_glyphs.max(c.glyphs);
        rep.max_particles = rep.max_particles.max(c.particles);
        rep.max_blurs = rep.max_blurs.max(c.blurs);
        rep.max_passes = rep.max_passes.max(pass_count(&f));
        rep.max_paths = rep.max_paths.max(c.paths);
        rep.max_segments = rep.max_segments.max(c.segments);
        rep.max_shape_area = rep.max_shape_area.max(c.shape_area);
        rep.max_draws = rep.max_draws.max(f.draws.len());
        rep.max_calls = rep.max_calls.max(f.calls);
        rep.max_reads = rep.max_reads.max(f.reads);
        let (shaders, shader_area) = super::check::shader_load(&f);
        rep.max_shader_layers = rep.max_shader_layers.max(shaders);
        rep.max_shader_area = rep.max_shader_area.max(shader_area);
        times.push(f.run_ms);
        kept.push(fingerprint(&f));
    }
    // A pure function of t: asked in another order, every frame is the same.
    let n = instants.len();
    for k in 0..n {
        let i = (k * 7919 + 13) % n;
        let f = scene.frame(instants[i])?;
        if fingerprint(&f) != kept[i] {
            return Err(format!(
                "at t={:.3}: the frame differs when asked in another order (the script keeps state)",
                instants[i]
            ));
        }
    }
    let blocks = checker.blocks();
    rep.blocks = blocks.len();
    rep.peak_text_scale = blocks.iter().map(|b| b.1).fold(0.0, f32::max);
    rep.script_ms_p95 = p95(&mut times);
    if rep.script_ms_p95 > SCRIPT_P95_MS {
        rep.warnings.push(format!(
            "script p95 {:.2} ms is above {SCRIPT_P95_MS} ms on this machine",
            rep.script_ms_p95
        ));
    }
    if rep.script_ms_worst > SCRIPT_WORST_MS {
        rep.warnings.push(format!(
            "script worst frame {:.2} ms (t={:.3}) is above {SCRIPT_WORST_MS} ms on this machine",
            rep.script_ms_worst, rep.script_ms_worst_at
        ));
    }
    if rotated > 0 {
        rep.not_checked.push(format!(
            "rotated text ({rotated} draw(s)): not checked against the frame, the safe area or other texts"
        ));
    }
    rep.not_checked.push(
        "overlaps between texts when either is under half opacity or more than half dissolved"
            .into(),
    );
    rep.layout_warnings = notes
        .0
        .iter()
        .map(|(m, t, n)| format!("{m} (first at {t:.2} s, {n} frame(s))"))
        .collect();
    debug_assert!(rep.max_ops <= MAX_OPS && rep.max_calls <= MAX_CALLS);
    Ok(rep)
}
