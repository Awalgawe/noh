//! Vector shapes of a motion scene, cut into triangles on the processor with no
//! library: curves are flattened into segments; a fill is one contour that does
//! not cross itself, cut by ear clipping; a stroke is a quad per segment, round
//! joins and, when asked, round caps. Positions are canvas pixels. The caps are
//! set from the measured cost of the heaviest legal frame on the reference
//! machine: lowered until that frame held the bar, never raised to fit a scene.
use std::collections::HashMap;
use std::sync::Arc;

/// Paths one frame may draw.
pub const MAX_PATHS: usize = 64;
/// Points written into one path (a line or a move is one, a quadratic curve
/// two, a cubic three).
pub const MAX_POINTS: usize = 256;
/// Points of a filled path once its curves are flattened: the crossing test
/// and the cutting grow faster than their number.
pub const MAX_FILL_POINTS: usize = 128;
/// Points of all the filled paths of a frame, once flattened.
pub const MAX_FILL_TOTAL: usize = 512;
/// Segments of all paths of a frame once flattened (fills and strokes).
pub const MAX_SEGMENTS: usize = 1024;
/// Colour stops of a gradient.
pub const MAX_STOPS: usize = 8;
/// Surface the paths of a frame may cover, in frames: the sum of their boxes
/// (a stroke's grown by half its width), clipped to the frame.
pub const MAX_SHAPE_AREA: f32 = 4.0;
/// How far a flattened curve may stray from the true one, in pixels.
pub const TOLERANCE_PX: f32 = 0.2;
const MAX_CURVE_STEPS: usize = 64;

#[derive(Clone, Debug, PartialEq)]
pub enum Seg {
    Move([f32; 2]),
    Line([f32; 2]),
    Quad([f32; 2], [f32; 2]),
    Cubic([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

impl Seg {
    fn points(&self) -> usize {
        match self {
            Seg::Move(_) | Seg::Line(_) => 1,
            Seg::Quad(..) => 2,
            Seg::Cubic(..) => 3,
            Seg::Close => 0,
        }
    }
}

/// A colour stop: offset (0 to 1), colour, opacity.
pub type Stop = (f32, String, f32);

/// A fill's gradient in canvas pixels: 1 linear or 2 radial, its from or
/// centre, its to, its radius, its stops.
pub type GradientPx<'a> = (u8, [f32; 2], [f32; 2], f32, &'a [Stop]);

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    None,
    Solid(String),
    /// From and to, fractions of the frame (moved with the path).
    Linear([f32; 2], [f32; 2], Vec<Stop>),
    /// Centre (fractions of the frame) and radius (fraction of the short side).
    Radial([f32; 2], f32, Vec<Stop>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PathDraw {
    /// Points as fractions of the frame: x of its width, y of its height.
    pub segs: Vec<Seg>,
    pub fill: Paint,
    /// Colour and width (fraction of the short side).
    pub stroke: Option<(String, f32)>,
    pub round_caps: bool,
    /// The part of the stroke drawn, from 0 to 1 of its length.
    pub trim: [f32; 2],
    pub opacity: f32,
    /// Turned (radians, clockwise) and scaled about `pivot`, then moved by
    /// `offset` (fractions of the frame).
    pub pivot: [f32; 2],
    pub rot: f32,
    pub scale: f32,
    pub offset: [f32; 2],
}

impl Default for PathDraw {
    fn default() -> Self {
        Self {
            segs: Vec::new(),
            fill: Paint::None,
            stroke: None,
            round_caps: false,
            trim: [0.0, 1.0],
            opacity: 1.0,
            pivot: [0.5, 0.5],
            rot: 0.0,
            scale: 1.0,
            offset: [0.0, 0.0],
        }
    }
}

fn stops_ok(stops: &[Stop]) -> Result<(), String> {
    if stops.len() < 2 || stops.len() > MAX_STOPS {
        return Err(format!(
            "a gradient takes 2 to {MAX_STOPS} stops, not {}",
            stops.len()
        ));
    }
    let mut last = -1.0;
    for (o, c, a) in stops {
        super::draw::hex(c)?;
        if !o.is_finite() || !a.is_finite() || !(0.0..=1.0).contains(o) || *o < last {
            return Err("gradient stops: offsets from 0 to 1, in order".into());
        }
        last = *o;
    }
    Ok(())
}

impl PathDraw {
    pub fn points(&self) -> usize {
        self.segs.iter().map(Seg::points).sum()
    }

    pub fn check(&self) -> Result<(), String> {
        let mut nums = vec![
            self.opacity,
            self.rot,
            self.scale,
            self.trim[0],
            self.trim[1],
        ];
        nums.extend(self.pivot);
        nums.extend(self.offset);
        for s in &self.segs {
            match s {
                Seg::Move(p) | Seg::Line(p) => nums.extend(p),
                Seg::Quad(a, b) => nums.extend([a[0], a[1], b[0], b[1]]),
                Seg::Cubic(a, b, c) => nums.extend([a[0], a[1], b[0], b[1], c[0], c[1]]),
                Seg::Close => {}
            }
        }
        if nums.iter().any(|v| !v.is_finite()) {
            return Err("path: a number is not finite".into());
        }
        if self.points() > MAX_POINTS {
            return Err(format!("a path of more than {MAX_POINTS} points"));
        }
        if !(0.01..=10.0).contains(&self.scale) {
            return Err("path scale out of range (0.01 to 10)".into());
        }
        if !(0.0..=1.0).contains(&self.trim[0])
            || !(0.0..=1.0).contains(&self.trim[1])
            || self.trim[0] > self.trim[1]
        {
            return Err("path trim out of range (0 <= start <= end <= 1)".into());
        }
        if let Some((c, w)) = &self.stroke {
            super::draw::hex(c)?;
            if !(0.0005..=0.25).contains(w) {
                return Err("stroke width out of range (0.0005 to 0.25 of the short side)".into());
            }
        }
        match &self.fill {
            Paint::None => {}
            Paint::Solid(c) => {
                super::draw::hex(c)?;
            }
            Paint::Linear(a, b, st) => {
                if a.iter().chain(b).any(|v| !v.is_finite()) {
                    return Err("gradient: a number is not finite".into());
                }
                stops_ok(st)?;
            }
            Paint::Radial(c, r, st) => {
                if c.iter().any(|v| !v.is_finite()) || !(0.0001..=4.0).contains(r) {
                    return Err("radial gradient: centre or radius out of range".into());
                }
                stops_ok(st)?;
            }
        }
        if self.fill != Paint::None {
            let moves = self
                .segs
                .iter()
                .filter(|s| matches!(s, Seg::Move(_)))
                .count();
            if moves > 1 {
                return Err(
                    "a filled path has one contour (no holes): use one path per shape".into(),
                );
            }
        }
        Ok(())
    }

    /// Canvas pixels of a point given in fractions, the path's transform applied.
    fn place(&self, p: [f32; 2], w: f32, h: f32) -> [f32; 2] {
        let (px, py) = (self.pivot[0] * w, self.pivot[1] * h);
        let (dx, dy) = (p[0] * w - px, p[1] * h - py);
        let (c, s) = (self.rot.cos(), self.rot.sin());
        [
            px + self.scale * (c * dx - s * dy) + self.offset[0] * w,
            py + self.scale * (s * dx + c * dy) + self.offset[1] * h,
        ]
    }

    /// The fill's gradient in canvas pixels, moved with the path.
    pub fn gradient_px(&self, w: f32, h: f32) -> Option<GradientPx<'_>> {
        let short = w.min(h);
        match &self.fill {
            Paint::Linear(a, b, st) => {
                Some((1, self.place(*a, w, h), self.place(*b, w, h), 0.0, st))
            }
            Paint::Radial(c, r, st) => Some((
                2,
                self.place(*c, w, h),
                [0.0; 2],
                r * short * self.scale,
                st,
            )),
            _ => None,
        }
    }
}

fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn len(a: [f32; 2]) -> f32 {
    a[0].hypot(a[1])
}
fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

fn steps(dd: f32) -> usize {
    ((0.75 * dd / TOLERANCE_PX).sqrt().ceil() as usize).clamp(1, MAX_CURVE_STEPS)
}

/// The path's contours in canvas pixels, curves flattened; each with whether
/// it is closed.
pub fn flatten(p: &PathDraw, w: f32, h: f32) -> Vec<(Vec<[f32; 2]>, bool)> {
    let mut out: Vec<(Vec<[f32; 2]>, bool)> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut start = [0.0f32; 2];
    let at = |q: [f32; 2]| p.place(q, w, h);
    for s in &p.segs {
        match s {
            Seg::Move(q) => {
                if cur.len() > 1 {
                    out.push((std::mem::take(&mut cur), false));
                }
                cur.clear();
                start = at(*q);
                cur.push(start);
            }
            Seg::Line(q) => {
                if cur.is_empty() {
                    cur.push(start);
                }
                cur.push(at(*q));
            }
            Seg::Quad(c, q) => {
                if cur.is_empty() {
                    cur.push(start);
                }
                let (p0, p1, p2) = (*cur.last().unwrap(), at(*c), at(*q));
                let n = steps(len([
                    p0[0] - 2.0 * p1[0] + p2[0],
                    p0[1] - 2.0 * p1[1] + p2[1],
                ]));
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    cur.push([
                        u * u * p0[0] + 2.0 * u * t * p1[0] + t * t * p2[0],
                        u * u * p0[1] + 2.0 * u * t * p1[1] + t * t * p2[1],
                    ]);
                }
            }
            Seg::Cubic(c1, c2, q) => {
                if cur.is_empty() {
                    cur.push(start);
                }
                let (p0, p1, p2, p3) = (*cur.last().unwrap(), at(*c1), at(*c2), at(*q));
                let d1 = len([p0[0] - 2.0 * p1[0] + p2[0], p0[1] - 2.0 * p1[1] + p2[1]]);
                let d2 = len([p1[0] - 2.0 * p2[0] + p3[0], p1[1] - 2.0 * p2[1] + p3[1]]);
                let n = steps(d1.max(d2) * 1.5);
                for k in 1..=n {
                    let t = k as f32 / n as f32;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.push([
                        a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0],
                        a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1],
                    ]);
                }
            }
            Seg::Close => {
                if cur.len() > 1 {
                    out.push((std::mem::take(&mut cur), true));
                }
                cur.clear();
            }
        }
    }
    if cur.len() > 1 {
        out.push((cur, false));
    }
    // Consecutive repeats, and a closing point equal to the first, are dropped.
    for (c, closed) in &mut out {
        c.dedup_by(|a, b| len(sub(*a, *b)) < 1e-3);
        if *closed && c.len() > 1 && len(sub(c[0], *c.last().unwrap())) < 1e-3 {
            c.pop();
        }
    }
    out
}

/// Whether two segments cross, overlap or touch (the caller leaves out
/// neighbours, which share an end): a corner lying on another edge makes a
/// contour that is not simple, and cutting it would go wrong.
fn crosses(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let (r, s) = (sub(b, a), sub(d, c));
    let den = cross(r, s);
    let ca = sub(c, a);
    if den.abs() < 1e-9 {
        // Parallel: crossing only if collinear and overlapping.
        if cross(ca, r).abs() > 1e-3 * len(r).max(1.0) {
            return false;
        }
        let rr = r[0] * r[0] + r[1] * r[1];
        if rr < 1e-12 {
            return false;
        }
        let t0 = (ca[0] * r[0] + ca[1] * r[1]) / rr;
        let t1 = t0 + (s[0] * r[0] + s[1] * r[1]) / rr;
        let (lo, hi) = (t0.min(t1), t0.max(t1));
        return hi >= -1e-6 && lo <= 1.0 + 1e-6;
    }
    let t = cross(ca, s) / den;
    let u = cross(ca, r) / den;
    let e = 1e-6;
    t >= -e && t <= 1.0 + e && u >= -e && u <= 1.0 + e
}

/// Whether a closed contour crosses itself. The edges are swept in the order
/// of their left end: an edge is only tested against those whose x range
/// overlaps its own (the same answer as testing every pair, sooner).
pub fn self_crossing(pts: &[[f32; 2]]) -> bool {
    let n = pts.len();
    let mut order: Vec<usize> = (0..n).collect();
    let lo = |i: usize| pts[i][0].min(pts[(i + 1) % n][0]);
    let hi = |i: usize| pts[i][0].max(pts[(i + 1) % n][0]);
    order.sort_by(|&a, &b| lo(a).total_cmp(&lo(b)));
    for (k, &i) in order.iter().enumerate() {
        let (a, b, right) = (pts[i], pts[(i + 1) % n], hi(i));
        for &j in &order[k + 1..] {
            if lo(j) > right {
                break;
            }
            let (x, y) = (i.min(j), i.max(j));
            if y == x + 1 || (x == 0 && y == n - 1) {
                continue; // neighbours share an end
            }
            if crosses(a, b, pts[j], pts[(j + 1) % n]) {
                return true;
            }
        }
    }
    false
}

/// Strictly inside: a corner lying on the ear's border does not block it
/// (cutting there still leaves a valid polygon; counting it as inside left
/// shapes with aligned corners, such as a comb, without ears).
fn in_triangle(p: [f32; 2], a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> bool {
    let d1 = cross(sub(b, a), sub(p, a));
    let d2 = cross(sub(c, b), sub(p, b));
    let d3 = cross(sub(a, c), sub(p, c));
    d1 > 1e-6 && d2 > 1e-6 && d3 > 1e-6
}

/// Ear clipping of a simple polygon; triangles as a flat list of corners.
pub fn ear_clip(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    ear_clip_counted(pts).0
}

/// The same, with the number of triangles taken although they were not ears
/// (a full round found none: near-degenerate input). Such a triangle may paint
/// outside the shape; the caller says so.
pub fn ear_clip_counted(pts: &[[f32; 2]]) -> (Vec<[f32; 2]>, usize) {
    let n = pts.len();
    if n < 3 {
        return (Vec::new(), 0);
    }
    let area: f32 = (0..n).map(|i| cross(pts[i], pts[(i + 1) % n])).sum();
    if area.abs() < 1e-6 {
        return (Vec::new(), 0);
    }
    let mut forced = 0;
    // Counter-clockwise in these axes (positive area).
    let mut idx: Vec<usize> = if area > 0.0 {
        (0..n).collect()
    } else {
        (0..n).rev().collect()
    };
    let mut out = Vec::with_capacity(3 * (n - 2));
    let mut i = 0;
    let mut misses = 0;
    // Only a reflex corner can lie inside a candidate ear: the others are
    // skipped. A corner can only stop being reflex (when a neighbour goes).
    let corner = |idx: &[usize], k: usize| {
        let m = idx.len();
        let (a, b, c) = (
            pts[idx[(k + m - 1) % m]],
            pts[idx[k]],
            pts[idx[(k + 1) % m]],
        );
        cross(sub(b, a), sub(c, b)) <= 1e-9
    };
    let mut reflex: Vec<bool> = vec![false; n];
    for k in 0..idx.len() {
        reflex[idx[k]] = corner(&idx, k);
    }
    while idx.len() > 3 {
        let m = idx.len();
        let (ip, ic, inx) = (idx[(i + m - 1) % m], idx[i % m], idx[(i + 1) % m]);
        let (a, b, c) = (pts[ip], pts[ic], pts[inx]);
        let convex = !reflex[ic];
        let ear = convex
            && idx
                .iter()
                .all(|&k| !reflex[k] || k == ip || k == inx || !in_triangle(pts[k], a, b, c));
        if ear || misses > m {
            // A full round without an ear (numerical trouble): take it anyway,
            // and count it.
            forced += usize::from(!ear);
            out.extend([a, b, c]);
            idx.remove(i % m);
            misses = 0;
            let m = idx.len();
            let at = i % (m + 1);
            let (kp, kn) = ((at + m - 1) % m, at % m);
            reflex[idx[kp]] = corner(&idx, kp);
            reflex[idx[kn]] = corner(&idx, kn);
            i = i.saturating_sub(1);
        } else {
            i += 1;
            misses += 1;
        }
    }
    out.extend([pts[idx[0]], pts[idx[1]], pts[idx[2]]]);
    (out, forced)
}

/// The part of the contours between `a` and `b` (fractions of their total
/// length), as open polylines.
fn trimmed(contours: &[(Vec<[f32; 2]>, bool)], a: f32, b: f32) -> Vec<(Vec<[f32; 2]>, bool)> {
    if a <= 0.0 && b >= 1.0 {
        return contours.to_vec();
    }
    let lines: Vec<Vec<[f32; 2]>> = contours
        .iter()
        .map(|(c, closed)| {
            let mut v = c.clone();
            if *closed {
                v.push(c[0]);
            }
            v
        })
        .collect();
    let total: f32 = lines
        .iter()
        .map(|l| l.windows(2).map(|w| len(sub(w[1], w[0]))).sum::<f32>())
        .sum();
    let (from, to) = (a * total, b * total);
    let mut out = Vec::new();
    let mut run = 0.0f32;
    for l in &lines {
        let mut cur: Vec<[f32; 2]> = Vec::new();
        for w in l.windows(2) {
            let d = len(sub(w[1], w[0]));
            let (s0, s1) = (run, run + d);
            run = s1;
            if s1 < from || s0 > to || d <= 0.0 {
                continue;
            }
            let lerp = |s: f32| {
                let t = ((s - s0) / d).clamp(0.0, 1.0);
                [
                    w[0][0] + (w[1][0] - w[0][0]) * t,
                    w[0][1] + (w[1][1] - w[0][1]) * t,
                ]
            };
            if cur.is_empty() {
                cur.push(lerp(from.max(s0)));
            }
            cur.push(lerp(to.min(s1)));
        }
        if cur.len() > 1 {
            out.push((cur, false));
        }
    }
    out
}

fn arc(out: &mut Vec<[f32; 2]>, c: [f32; 2], r: f32, a0: f32, sweep: f32) {
    let step = 2.0 * (1.0 - (TOLERANCE_PX / r).min(1.0)).acos().max(0.05);
    let k = ((sweep.abs() / step).ceil() as usize).clamp(1, 64);
    for i in 0..k {
        let (t0, t1) = (
            a0 + sweep * i as f32 / k as f32,
            a0 + sweep * (i + 1) as f32 / k as f32,
        );
        out.extend([
            c,
            [c[0] + r * t0.cos(), c[1] + r * t0.sin()],
            [c[0] + r * t1.cos(), c[1] + r * t1.sin()],
        ]);
    }
}

/// Triangles of a stroke of half width `hw` along the polylines.
pub fn stroke(lines: &[(Vec<[f32; 2]>, bool)], hw: f32, round_caps: bool) -> Vec<[f32; 2]> {
    let total: usize = lines.iter().map(|(l, _)| l.len()).sum();
    let mut out = Vec::with_capacity(total * 9);
    for (l, closed) in lines {
        let n = l.len();
        if n < 2 {
            continue;
        }
        let segs = if *closed { n } else { n - 1 };
        let dirs: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let d = sub(l[(i + 1) % n], l[i]);
                let m = len(d).max(1e-9);
                [d[0] / m, d[1] / m]
            })
            .collect();
        let dir = |i: usize| dirs[i];
        for i in 0..segs {
            let (a, b) = (l[i], l[(i + 1) % n]);
            let d = dir(i);
            let nn = [-d[1] * hw, d[0] * hw];
            let (a0, a1, b0, b1) = (
                [a[0] + nn[0], a[1] + nn[1]],
                [a[0] - nn[0], a[1] - nn[1]],
                [b[0] + nn[0], b[1] + nn[1]],
                [b[0] - nn[0], b[1] - nn[1]],
            );
            out.extend([a0, b0, b1, a0, b1, a1]);
        }
        // Round joins: the outer wedge between the two segments' sides.
        let joins: Vec<usize> = if *closed {
            (0..n).collect()
        } else {
            (1..n - 1).collect()
        };
        for i in joins {
            let (d0, d1) = (dir((i + n - 1) % n), dir(i));
            let turn = cross(d0, d1).atan2(d0[0] * d1[0] + d0[1] * d1[1]);
            // The wedge left open between the two quads is at most
            // hw * (1 - cos(turn / 2)) deep: under the tolerance, no join.
            if hw * (1.0 - (turn * 0.5).cos()) < TOLERANCE_PX * 0.5 {
                continue;
            }
            let side = -turn.signum();
            let a0 = (side * d0[0]).atan2(-side * d0[1]);
            arc(&mut out, l[i], hw, a0, turn);
        }
        if round_caps && !*closed {
            let d = dir(0);
            arc(&mut out, l[0], hw, d[0].atan2(-d[1]), std::f32::consts::PI);
            let d = dir(n - 2);
            arc(
                &mut out,
                l[n - 1],
                hw,
                (-d[0]).atan2(d[1]),
                std::f32::consts::PI,
            );
        }
    }
    out
}

/// A path cut into triangles, with what it counts.
#[derive(Debug, Default)]
pub struct Tess {
    pub fill: Vec<[f32; 2]>,
    pub stroke: Vec<[f32; 2]>,
    /// Segments once flattened (fill and stroke).
    pub segments: usize,
    /// Points of its filled contour, once flattened.
    pub fill_points: usize,
    /// Triangles of the fill taken although they were not ears.
    pub forced: usize,
    /// Its box in pixels, clipped to the frame, as a share of the frame.
    pub area: f32,
}

/// Cut a path into triangles; refuses a fill that crosses itself or has too
/// many points.
pub fn tessellate(p: &PathDraw, w: f32, h: f32) -> Result<Tess, String> {
    let contours = flatten(p, w, h);
    let mut t = Tess::default();
    let short = w.min(h);
    let hw = p
        .stroke
        .as_ref()
        .map_or(0.0, |(_, sw)| sw * short * p.scale / 2.0);
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (c, closed) in &contours {
        for q in c {
            (x0, y0, x1, y1) = (x0.min(q[0]), y0.min(q[1]), x1.max(q[0]), y1.max(q[1]));
        }
        t.segments += if *closed {
            c.len()
        } else {
            c.len().saturating_sub(1)
        };
    }
    if p.fill != Paint::None
        && let Some((c, _)) = contours.first()
    {
        if c.len() > MAX_FILL_POINTS {
            return Err(format!(
                "a filled path of {} points once its curves are flattened exceeds {MAX_FILL_POINTS}",
                c.len()
            ));
        }
        if self_crossing(c) {
            return Err("a filled path crosses itself (or a corner touches another edge)".into());
        }
        (t.fill, t.forced) = ear_clip_counted(c);
        t.fill_points = c.len();
        if p.stroke.is_some() {
            t.segments += c.len();
        }
    }
    if hw > 0.0 {
        let lines = trimmed(&contours, p.trim[0], p.trim[1]);
        t.stroke = stroke(&lines, hw, p.round_caps);
    }
    if x0 <= x1 {
        let g = hw;
        let (cx0, cy0) = ((x0 - g).max(0.0), (y0 - g).max(0.0));
        let (cx1, cy1) = ((x1 + g).min(w), (y1 + g).min(h));
        t.area = ((cx1 - cx0).max(0.0) * (cy1 - cy0).max(0.0)) / (w * h);
    }
    Ok(t)
}

/// Tessellations kept by content: a path that does not change is not cut again.
#[derive(Default)]
pub struct ShapeCache {
    map: HashMap<String, Arc<Tess>>,
    pub hits: usize,
    pub misses: usize,
}

impl ShapeCache {
    pub fn get(&mut self, p: &PathDraw, w: f32, h: f32) -> Result<Arc<Tess>, String> {
        let key = format!("{p:?}");
        if let Some(t) = self.map.get(&key) {
            self.hits += 1;
            return Ok(t.clone());
        }
        self.misses += 1;
        let t = Arc::new(tessellate(p, w, h)?);
        if self.map.len() >= 4096 {
            self.map.clear();
        }
        self.map.insert(key, t.clone());
        Ok(t)
    }
}

/// The counted checks of a frame's paths: number, segments, surface.
/// Returns the segments, the surface, and the triangles forced in fills.
pub fn check_paths(
    paths: &[&PathDraw],
    cache: &mut ShapeCache,
    w: f32,
    h: f32,
) -> Result<(usize, f32, usize), String> {
    if paths.len() > MAX_PATHS {
        return Err(format!("{} paths exceed {MAX_PATHS}", paths.len()));
    }
    let (mut segs, mut area, mut fills, mut forced) = (0usize, 0.0f32, 0usize, 0usize);
    for p in paths {
        let t = cache.get(p, w, h)?;
        segs += t.segments;
        area += t.area;
        fills += t.fill_points;
        forced += t.forced;
    }

    if fills > MAX_FILL_TOTAL {
        return Err(format!(
            "{fills} points of filled paths once flattened exceed {MAX_FILL_TOTAL}"
        ));
    }
    if segs > MAX_SEGMENTS {
        return Err(format!(
            "{segs} segments once flattened exceed {MAX_SEGMENTS}"
        ));
    }
    if area > MAX_SHAPE_AREA {
        return Err(format!(
            "the paths' boxes cover {area:.2} frames, more than {MAX_SHAPE_AREA}"
        ));
    }
    Ok((segs, area, forced))
}

/// Circle as four cubic curves, centre in fractions of the frame, radius in
/// fractions of the short side.
pub fn ellipse_segs(cx: f32, cy: f32, rx: f32, ry: f32, w: f32, h: f32) -> Vec<Seg> {
    let short = w.min(h);
    let (ax, ay) = (rx * short / w, ry * short / h);
    let k = 0.552_284_8;
    let p = |x: f32, y: f32| [cx + x * ax, cy + y * ay];
    vec![
        Seg::Move(p(1.0, 0.0)),
        Seg::Cubic(p(1.0, k), p(k, 1.0), p(0.0, 1.0)),
        Seg::Cubic(p(-k, 1.0), p(-1.0, k), p(-1.0, 0.0)),
        Seg::Cubic(p(-1.0, -k), p(-k, -1.0), p(0.0, -1.0)),
        Seg::Cubic(p(k, -1.0), p(1.0, -k), p(1.0, 0.0)),
        Seg::Close,
    ]
}

/// Rectangle with round corners: centre, width (of the frame's width), height
/// (of its height), corner radius (of the short side).
pub fn rounded_rect_segs(cx: f32, cy: f32, rw: f32, rh: f32, r: f32, w: f32, h: f32) -> Vec<Seg> {
    let short = w.min(h);
    let (rx, ry) = ((r * short / w).min(rw / 2.0), (r * short / h).min(rh / 2.0));
    let (x0, y0, x1, y1) = (cx - rw / 2.0, cy - rh / 2.0, cx + rw / 2.0, cy + rh / 2.0);
    let k = 0.552_284_8;
    vec![
        Seg::Move([x0 + rx, y0]),
        Seg::Line([x1 - rx, y0]),
        Seg::Cubic(
            [x1 - rx + k * rx, y0],
            [x1, y0 + ry - k * ry],
            [x1, y0 + ry],
        ),
        Seg::Line([x1, y1 - ry]),
        Seg::Cubic(
            [x1, y1 - ry + k * ry],
            [x1 - rx + k * rx, y1],
            [x1 - rx, y1],
        ),
        Seg::Line([x0 + rx, y1]),
        Seg::Cubic(
            [x0 + rx - k * rx, y1],
            [x0, y1 - ry + k * ry],
            [x0, y1 - ry],
        ),
        Seg::Line([x0, y0 + ry]),
        Seg::Cubic(
            [x0, y0 + ry - k * ry],
            [x0 + rx - k * rx, y0],
            [x0 + rx, y0],
        ),
        Seg::Close,
    ]
}

#[cfg(test)]
pub fn hard_polygons(n: usize) -> Vec<(&'static str, Vec<[f32; 2]>)> {
    // (the same shapes are written as scenes by examples/gen_worst_shapes.py)
    let mut out = Vec::new();
    // Spiral: out along one arm, back along a parallel arm.
    let half = n / 2;
    let turns = 3.0f32;
    let arm = |k: usize, r0: f32| {
        let t = k as f32 / (half - 1) as f32;
        let a = t * turns * std::f32::consts::TAU;
        let r = r0 + 0.38 * t;
        [0.5 + r * a.cos() * 0.5625, 0.5 + r * a.sin()]
    };
    let mut sp: Vec<[f32; 2]> = (0..half).map(|k| arm(k, 0.02)).collect();
    sp.extend((0..half).rev().map(|k| arm(k, 0.055)));
    out.push(("spiral", sp));
    // Comb: a spine on the left, teeth across the whole width.
    // (n - 2) / 4 teeth, then two corners closing along an outer spine.
    let teeth = (n - 2) / 4;
    let mut comb = Vec::new();
    for k in 0..teeth {
        let y0 = 0.05 + 0.9 * k as f32 / teeth as f32;
        let y1 = y0 + 0.45 / teeth as f32;
        comb.extend([[0.05, y0], [0.95, y0], [0.95, y1], [0.1, y1]]);
    }
    let (top, bottom) = (comb[0][1], comb[comb.len() - 1][1]);
    comb.extend([[0.02, bottom + 0.01], [0.02, top]]);
    out.push(("comb", comb));
    // Star: n/2 tips, inner points almost at the centre.
    let star = (0..n)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / n as f32;
            let r = if k % 2 == 0 { 0.45 } else { 0.004 };
            [0.5 + r * a.cos() * 0.5625, 0.5 + r * a.sin()]
        })
        .collect();
    out.push(("star", star));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri_area(t: &[[f32; 2]]) -> f32 {
        t.chunks(3)
            .map(|c| cross(sub(c[1], c[0]), sub(c[2], c[0])).abs() / 2.0)
            .sum()
    }

    fn poly(pts: &[[f32; 2]]) -> PathDraw {
        let mut segs = vec![Seg::Move(pts[0])];
        segs.extend(pts[1..].iter().map(|p| Seg::Line(*p)));
        segs.push(Seg::Close);
        PathDraw {
            segs,
            fill: Paint::Solid("#FFFFFF".into()),
            ..Default::default()
        }
    }

    #[test]
    fn fills_cover_their_area() {
        // A unit square in a 100 x 100 frame, and a concave arrow.
        let sq = tessellate(
            &poly(&[[0.1, 0.1], [0.5, 0.1], [0.5, 0.5], [0.1, 0.5]]),
            100.0,
            100.0,
        )
        .unwrap();
        assert!((tri_area(&sq.fill) - 1600.0).abs() < 0.01);
        let arrow = [[0.1, 0.1], [0.9, 0.5], [0.1, 0.9], [0.4, 0.5]];
        let t = tessellate(&poly(&arrow), 100.0, 100.0).unwrap();
        // Shoelace area of the arrow, in pixels.
        let px: Vec<[f32; 2]> = arrow.iter().map(|p| [p[0] * 100.0, p[1] * 100.0]).collect();
        let a: f32 = (0..4)
            .map(|i| cross(px[i], px[(i + 1) % 4]))
            .sum::<f32>()
            .abs()
            / 2.0;
        assert!(
            (tri_area(&t.fill) - a).abs() < 0.01,
            "{} {a}",
            tri_area(&t.fill)
        );
        // A circle of radius 0.25 of the short side in 400 x 400: pi r^2.
        let c = PathDraw {
            segs: ellipse_segs(0.5, 0.5, 0.25, 0.25, 400.0, 400.0),
            fill: Paint::Solid("#FFFFFF".into()),
            ..Default::default()
        };
        let t = tessellate(&c, 400.0, 400.0).unwrap();
        let want = std::f32::consts::PI * 100.0 * 100.0;
        assert!(
            (tri_area(&t.fill) - want).abs() / want < 0.003,
            "{}",
            tri_area(&t.fill)
        );
    }

    #[test]
    fn crossing_fills_and_holes_are_refused() {
        let eight = poly(&[[0.1, 0.1], [0.9, 0.9], [0.9, 0.1], [0.1, 0.9]]);
        assert!(
            tessellate(&eight, 100.0, 100.0)
                .unwrap_err()
                .contains("crosses itself")
        );
        let mut two = poly(&[[0.1, 0.1], [0.5, 0.1], [0.5, 0.5]]);
        two.segs.extend([
            Seg::Move([0.6, 0.6]),
            Seg::Line([0.8, 0.6]),
            Seg::Line([0.8, 0.8]),
            Seg::Close,
        ]);
        assert!(two.check().unwrap_err().contains("one contour"));
    }

    #[test]
    fn a_stroke_covers_length_times_width_and_trims() {
        let mut p = PathDraw {
            segs: vec![Seg::Move([0.1, 0.5]), Seg::Line([0.9, 0.5])],
            stroke: Some(("#FFFFFF".into(), 0.02)),
            ..Default::default()
        };
        let t = tessellate(&p, 1000.0, 1000.0).unwrap();
        assert!((tri_area(&t.stroke) - 800.0 * 20.0).abs() < 0.5);
        p.trim = [0.0, 0.25];
        let t = tessellate(&p, 1000.0, 1000.0).unwrap();
        assert!((tri_area(&t.stroke) - 200.0 * 20.0).abs() < 0.5);
        // Round caps add a disc.
        p.trim = [0.0, 1.0];
        p.round_caps = true;
        let t = tessellate(&p, 1000.0, 1000.0).unwrap();
        let want = 800.0 * 20.0 + std::f32::consts::PI * 100.0;
        assert!(
            (tri_area(&t.stroke) - want).abs() / want < 0.01,
            "{}",
            tri_area(&t.stroke)
        );
    }

    #[test]
    fn curves_stay_within_the_tolerance() {
        let c = PathDraw {
            segs: ellipse_segs(0.5, 0.5, 0.4, 0.4, 1000.0, 1000.0),
            stroke: Some(("#FFFFFF".into(), 0.001)),
            ..Default::default()
        };
        for (pts, _) in flatten(&c, 1000.0, 1000.0) {
            for q in pts {
                let r = len(sub(q, [500.0, 500.0]));
                assert!((r - 400.0).abs() < 0.6, "{r}");
            }
        }
    }
}
