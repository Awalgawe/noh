//! Reframing the picture at the same format (`video()` in a scene): one framing
//! function, from the scene's `video()` at an instant to a transformation of the
//! source picture, that the three paths apply to their own image: the export to
//! the source's YUV planes at full size (`fx::gl::YuvMix`), the libmpv preview to
//! its rendered texture (a transformed quad), the FFmpeg player to its decoded
//! image (an egui mesh of the same quad). « The same framing, not the same
//! pixels »: each path samples its own image, the framing is the same function of
//! the same instant.

/// The largest zoom into the picture, over the scale at which it just covers
/// the output.
pub const MAX_VIDEO_SCALE: f32 = 1.5;
/// A light turn only (radians).
pub const MAX_VIDEO_ROT: f32 = 0.35;
/// The longest side the FFmpeg player decodes when a scene reframes (Q6), and
/// when none does (the player's size today).
pub const PLAYER_REFRAMED_MAX: u32 = 1280;
pub const PLAYER_PLAIN_MAX: u32 = 640;

/// How a frame frames the picture, as the scene's `video()` builds it.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoDraw {
    /// 1: the picture just covers the output; 0.25 to `MAX_VIDEO_SCALE`.
    pub scale: f32,
    /// Where the focus point goes in the output (fractions of the output).
    pub at: [f32; 2],
    /// The point of the picture put at `at` (fractions of the picture).
    pub focus: [f32; 2],
    /// Radians, clockwise, about that point.
    pub rot: f32,
    /// The part of the picture drawn (fractions of the picture): x0, y0, x1, y1.
    pub crop: [f32; 4],
}

impl Default for VideoDraw {
    fn default() -> Self {
        Self {
            scale: 1.0,
            at: [0.5, 0.5],
            focus: [0.5, 0.5],
            rot: 0.0,
            crop: [0.0, 0.0, 1.0, 1.0],
        }
    }
}

impl VideoDraw {
    pub fn check(&self) -> Result<(), String> {
        let v = [
            self.scale,
            self.at[0],
            self.at[1],
            self.focus[0],
            self.focus[1],
            self.rot,
            self.crop[0],
            self.crop[1],
            self.crop[2],
            self.crop[3],
        ];
        if !v.iter().all(|x| x.is_finite()) {
            return Err("video: a number is not finite".into());
        }
        if !(0.25..=MAX_VIDEO_SCALE).contains(&self.scale) {
            return Err(format!(
                "video scale {} out of range (0.25 to {MAX_VIDEO_SCALE})",
                self.scale
            ));
        }
        if self.rot.abs() > MAX_VIDEO_ROT {
            return Err(format!(
                "video rotation {} out of range (at most {MAX_VIDEO_ROT})",
                self.rot
            ));
        }
        if self.focus.iter().any(|f| !(0.0..=1.0).contains(f))
            || self.at.iter().any(|a| !(-0.5..=1.5).contains(a))
        {
            return Err("video focus (0 to 1) or position (-0.5 to 1.5) out of range".into());
        }
        let c = self.crop;
        if c.iter().any(|x| !(0.0..=1.0).contains(x)) || c[2] - c[0] < 0.05 || c[3] - c[1] < 0.05 {
            return Err(
                "video crop out of range (inside 0 to 1, at least 0.05 wide and high)".into(),
            );
        }
        Ok(())
    }

    /// The framing for an output of `out` and a source of `src`, in pixels.
    pub fn framing(&self, out: (f32, f32), src: (f32, f32)) -> Framing {
        let s = self.scale * (out.0 / src.0).max(out.1 / src.1);
        Framing {
            scale: s,
            rot: self.rot,
            center: [self.at[0] * out.0, self.at[1] * out.1],
            focus: [self.focus[0] * src.0, self.focus[1] * src.1],
            crop: [
                self.crop[0] * src.0,
                self.crop[1] * src.1,
                self.crop[2] * src.0,
                self.crop[3] * src.1,
            ],
            out,
            src,
        }
    }
}

/// A framing in pixels: an output pixel `p` shows the source pixel
/// `R(-rot) (p - center) / scale + focus`, black outside `crop`. Positions are
/// measured from the top left, down and right, in both images.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Framing {
    pub scale: f32,
    pub rot: f32,
    pub center: [f32; 2],
    pub focus: [f32; 2],
    /// x0, y0, x1, y1 in source pixels.
    pub crop: [f32; 4],
    pub out: (f32, f32),
    pub src: (f32, f32),
}

impl Framing {
    /// No reframing: the source as it is (same size), as the export draws today.
    pub fn identity(size: (f32, f32)) -> Self {
        VideoDraw::default().framing(size, size)
    }

    /// The output position to the source position.
    pub fn to_source(&self, p: [f32; 2]) -> [f32; 2] {
        let (dx, dy) = (p[0] - self.center[0], p[1] - self.center[1]);
        let (c, n) = (self.rot.cos(), self.rot.sin());
        [
            (c * dx + n * dy) / self.scale + self.focus[0],
            (-n * dx + c * dy) / self.scale + self.focus[1],
        ]
    }

    /// The source position to the output position.
    pub fn to_output(&self, s: [f32; 2]) -> [f32; 2] {
        let (dx, dy) = (
            (s[0] - self.focus[0]) * self.scale,
            (s[1] - self.focus[1]) * self.scale,
        );
        let (c, n) = (self.rot.cos(), self.rot.sin());
        [
            c * dx - n * dy + self.center[0],
            n * dx + c * dy + self.center[1],
        ]
    }

    /// The inverse as a 2 x 2 matrix, column-major, and the offsets it needs:
    /// `source = inv * (p - center) + focus` (what a shader is given).
    pub fn inverse(&self) -> [f32; 4] {
        let (c, n) = (self.rot.cos(), self.rot.sin());
        let s = self.scale;
        [c / s, -n / s, n / s, c / s]
    }

    /// The same framing for a plane `k` times smaller in each direction than the
    /// images it was made for (chroma planes: 2).
    pub fn for_plane(&self, k: f32) -> Self {
        let d = |v: [f32; 2]| [v[0] / k, v[1] / k];
        Self {
            scale: self.scale,
            rot: self.rot,
            center: d(self.center),
            focus: d(self.focus),
            crop: self.crop.map(|v| v / k),
            out: (self.out.0 / k, self.out.1 / k),
            src: (self.src.0 / k, self.src.1 / k),
        }
    }

    /// The drawn part of the picture as a quad in the output, corner by corner
    /// (top left, top right, bottom right, bottom left), each with its texture
    /// coordinate in the source (0 to 1, from the top left): what the previews
    /// draw (a textured quad, an egui mesh), whatever size their image is.
    pub fn quad(&self) -> [([f32; 2], [f32; 2]); 4] {
        let c = self.crop;
        [[c[0], c[1]], [c[2], c[1]], [c[2], c[3]], [c[0], c[3]]]
            .map(|s| (self.to_output(s), [s[0] / self.src.0, s[1] / self.src.1]))
    }

    /// The same quad in a view (an egui rectangle, a preview target): `view`
    /// is its left, top, width and height, the output scaled to fill it. The
    /// texture coordinates do not depend on the size of the preview's image.
    pub fn quad_in(&self, view: [f32; 4]) -> [([f32; 2], [f32; 2]); 4] {
        let (kx, ky) = (view[2] / self.out.0, view[3] / self.out.1);
        self.quad()
            .map(|(p, uv)| ([view[0] + p[0] * kx, view[1] + p[1] * ky], uv))
    }

    /// Whether the drawn part of the picture covers the whole output.
    pub fn covers(&self) -> bool {
        let (w, h) = self.out;
        [[0.0, 0.0], [w, 0.0], [0.0, h], [w, h]].iter().all(|&p| {
            let s = self.to_source(p);
            let c = self.crop;
            s[0] >= c[0] - 0.5 && s[0] <= c[2] + 0.5 && s[1] >= c[1] - 0.5 && s[1] <= c[3] + 0.5
        })
    }

    /// How much larger than the output the picture is drawn, at most: the side
    /// a preview must decode or render at so that its zoom adds no blur (the
    /// source's own size never helps beyond it).
    pub fn zoom(&self) -> f32 {
        let cover = (self.out.0 / self.src.0).max(self.out.1 / self.src.1);
        (self.scale / cover).max(1.0)
    }
}

/// An image of `channels` bytes per pixel resampled through `f` (bilinear
/// between pixel centres, as the GPU samples a texture), `black` outside the
/// crop: what a look draws under the scene, and the reference the export's
/// GPU pass is held to.
pub fn resample(src: &[u8], channels: usize, f: &Framing, black: &[u8]) -> Vec<u8> {
    let (sw, sh) = (f.src.0 as usize, f.src.1 as usize);
    let (w, h) = (f.out.0 as usize, f.out.1 as usize);
    let mut out = Vec::with_capacity(w * h * channels);
    let at = |x: usize, y: usize, c: usize| f32::from(src[(y * sw + x) * channels + c]);
    for y in 0..h {
        for x in 0..w {
            let s = f.to_source([x as f32 + 0.5, y as f32 + 0.5]);
            let c = f.crop;
            if s[0] < c[0] || s[1] < c[1] || s[0] > c[2] || s[1] > c[3] {
                out.extend_from_slice(&black[..channels]);
                continue;
            }
            // Texel space: centres at whole numbers, clamped to the edge.
            let (tx, ty) = (
                (s[0] - 0.5).clamp(0.0, (sw - 1) as f32),
                (s[1] - 0.5).clamp(0.0, (sh - 1) as f32),
            );
            let (x0, y0) = (tx.floor() as usize, ty.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
            let (fx, fy) = (tx - x0 as f32, ty - y0 as f32);
            for ch in 0..channels {
                let top = at(x0, y0, ch) * (1.0 - fx) + at(x1, y0, ch) * fx;
                let bottom = at(x0, y1, ch) * (1.0 - fx) + at(x1, y1, ch) * fx;
                out.push((top * (1.0 - fy) + bottom * fy).round().clamp(0.0, 255.0) as u8);
            }
        }
    }
    out
}

/// The longest side the FFmpeg player decodes (Q6): its usual 640 px without
/// reframing; with it, the view's side in physical pixels times the zoom the
/// scene needs, at most 1 280 px, never below the usual size.
pub fn player_side(reframes: bool, view_px: u32, zoom: f32) -> u32 {
    if !reframes {
        return PLAYER_PLAIN_MAX;
    }
    let want = (view_px as f32 * zoom.max(1.0)).ceil() as u32;
    want.clamp(PLAYER_PLAIN_MAX, PLAYER_REFRAMED_MAX)
}

/// The size libmpv renders the whole, unframed picture at for the preview, when
/// a scene reframes: the view in physical pixels times the zoom the scene needs
/// (so that drawing it zoomed adds no blur), never above the source's own size
/// (more would not show more), never below the view.
pub fn libmpv_size(view: (u32, u32), zoom: f32, src: (u32, u32)) -> (u32, u32) {
    let side = |v: u32, s: u32| {
        let want = (v as f32 * zoom.max(1.0)).ceil() as u32;
        want.min(s.max(v)).max(v)
    };
    (side(view.0, src.0), side(view.1, src.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 2], b: [f32; 2]) -> bool {
        (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3
    }

    #[test]
    fn the_framing_goes_both_ways_and_its_quad_lands_on_the_output() {
        let v = VideoDraw {
            scale: 1.3,
            at: [0.45, 0.55],
            focus: [0.6, 0.4],
            rot: 0.2,
            crop: [0.1, 0.0, 0.9, 1.0],
        };
        let f = v.framing((1920.0, 1080.0), (1920.0, 1080.0));
        for p in [[0.0, 0.0], [960.0, 540.0], [1919.0, 1079.0], [123.0, 987.0]] {
            assert!(near(f.to_output(f.to_source(p)), p), "{p:?}");
        }
        // The focus point lands at `at`.
        assert!(near(f.to_output(f.focus), f.center));
        // The quad's corners are the crop's corners, carried to the output.
        let q = f.quad();
        assert!(near(q[0].1, [0.1, 0.0]) && near(q[2].1, [0.9, 1.0]));
        assert!(near(f.to_source(q[0].0), [192.0, 0.0]));
        // A chroma plane: the same framing at half size.
        let half = f.for_plane(2.0);
        let p = [300.0, 200.0];
        let full = f.to_source(p);
        assert!(near(
            half.to_source([150.0, 100.0]),
            [full[0] / 2.0, full[1] / 2.0]
        ));
        // The quad in a view half the size, offset: the same corners, scaled.
        let v = f.quad_in([10.0, 20.0, 960.0, 540.0]);
        for (a, b) in v.iter().zip(&q) {
            assert!(near(a.0, [10.0 + b.0[0] / 2.0, 20.0 + b.0[1] / 2.0]) && a.1 == b.1);
        }
        // The identity draws the source as it is.
        let id = Framing::identity((1920.0, 1080.0));
        assert!(near(id.to_source([17.0, 33.0]), [17.0, 33.0]) && id.covers());
    }

    #[test]
    fn a_framing_that_leaves_black_is_said_and_the_zoom_is_counted() {
        let small = VideoDraw {
            scale: 0.8,
            ..VideoDraw::default()
        };
        assert!(!small.framing((1920.0, 1080.0), (1920.0, 1080.0)).covers());
        let turned = VideoDraw {
            rot: 0.3,
            ..VideoDraw::default()
        };
        assert!(!turned.framing((1920.0, 1080.0), (1920.0, 1080.0)).covers());
        let zoomed = VideoDraw {
            scale: 1.5,
            rot: 0.3,
            ..VideoDraw::default()
        };
        let z = zoomed.framing((1920.0, 1080.0), (1920.0, 1080.0));
        assert!(z.covers());
        assert!((z.zoom() - 1.5).abs() < 1e-5);
        assert!(
            (VideoDraw::default()
                .framing((960.0, 540.0), (1920.0, 1080.0))
                .zoom()
                - 1.0)
                .abs()
                < 1e-5
        );
    }

    #[test]
    fn libmpv_renders_larger_by_the_zoom_up_to_the_source() {
        assert_eq!(libmpv_size((960, 540), 1.0, (1920, 1080)), (960, 540));
        assert_eq!(libmpv_size((960, 540), 1.5, (1920, 1080)), (1440, 810));
        assert_eq!(libmpv_size((1600, 900), 1.5, (1920, 1080)), (1920, 1080));
        // A source smaller than the view: the view.
        assert_eq!(libmpv_size((960, 540), 1.5, (640, 360)), (960, 540));
    }

    #[test]
    fn the_player_decodes_larger_only_when_a_scene_reframes() {
        assert_eq!(player_side(false, 1600, 1.5), 640);
        assert_eq!(player_side(true, 400, 1.0), 640);
        assert_eq!(player_side(true, 600, 1.5), 900);
        assert_eq!(player_side(true, 1600, 1.5), 1280);
    }

    #[test]
    fn video_settings_out_of_range_are_refused() {
        let ok = VideoDraw::default();
        assert!(ok.check().is_ok());
        for bad in [
            VideoDraw {
                scale: 1.6,
                ..ok.clone()
            },
            VideoDraw {
                scale: 0.2,
                ..ok.clone()
            },
            VideoDraw {
                rot: 0.4,
                ..ok.clone()
            },
            VideoDraw {
                focus: [1.2, 0.5],
                ..ok.clone()
            },
            VideoDraw {
                at: [2.0, 0.5],
                ..ok.clone()
            },
            VideoDraw {
                crop: [0.0, 0.0, 0.03, 1.0],
                ..ok.clone()
            },
            VideoDraw {
                scale: f32::NAN,
                ..ok.clone()
            },
        ] {
            assert!(bad.check().is_err(), "{bad:?}");
        }
    }
}
