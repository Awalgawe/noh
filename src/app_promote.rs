//! « Promote NOH »: the setting, its checkbox on the export lines, and the
//! « made with NOH » mark painted over the preview (`noh::mark` holds the one
//! description exports burn with libass; the preview draws the same numbers).
use super::*;
use egui::{Color32, FontFamily, FontId, Painter, Rect, Vec2};
use noh::mark::{Corner, Layout};

/// The setting's file, beside the language file (`<config>/noh/promote`):
/// `on` or `off`, then the corner (`auto` when absent). Off by default.
pub fn path(language: &std::path::Path) -> std::path::PathBuf {
    language.with_file_name("promote")
}

/// The box's state and the corner, as last saved.
pub fn read(path: Option<&std::path::Path>) -> (bool, Corner) {
    let text = path
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    let mut words = text.split_whitespace();
    let on = words.next() == Some("on");
    let corner = words
        .next()
        .and_then(|w| serde_json::from_value(serde_json::Value::String(w.into())).ok())
        .unwrap_or_default();
    (on, corner)
}

fn save(path: &std::path::Path, on: bool, corner: Corner) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let corner = serde_json::to_value(corner)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "auto".into());
    std::fs::write(path, format!("{} {corner}", if on { "on" } else { "off" }))
}

impl NohApp {
    /// Tick or untick the box: remembered, and the exports already made, made
    /// without (or with) the mark, are no longer current. Previews are not
    /// touched: the mark is drawn over them live.
    pub(super) fn set_promote(&mut self, on: bool) {
        if self.promote == on {
            return;
        }
        self.promote = on;
        self.outdate_marked_exports();
        self.save_promote();
    }

    /// Choose the mark's corner: remembered. With the box ticked, exports
    /// made in another corner are no longer current (`auto` and the corner it
    /// stands for are the same on each picture); unticked, nothing changes.
    pub(super) fn set_promote_corner(&mut self, corner: Corner) {
        if self.promote_corner == corner {
            return;
        }
        let before = self.promote_corner;
        self.promote_corner = corner;
        let moved = [(1920, 1080), (1080, 1920)]
            .iter()
            .any(|&(w, h)| before.resolve(w, h) != corner.resolve(w, h));
        if self.promote && moved {
            self.outdate_marked_exports();
        }
        self.save_promote();
    }

    fn outdate_marked_exports(&mut self) {
        self.outdate_exports();
        for a in [
            &mut self.shorts.export_artifact,
            &mut self.captions.export_artifact,
        ]
        .into_iter()
        .flatten()
        {
            a.revision = a.revision.wrapping_sub(1);
        }
    }

    fn save_promote(&mut self) {
        let (on, corner) = (self.promote, self.promote_corner);
        let saved = self
            .locale
            .path
            .as_deref()
            .ok_or_else(|| std::io::Error::other("No user configuration directory"))
            .and_then(|language| save(&path(language), on, corner));
        if let Err(error) = saved {
            self.notice = Some(Message::new(
                "error.promote_preferences",
                &[error.to_string()],
            ));
        }
    }

    /// The mark over the preview's picture `picture`, for a `canvas`-sized
    /// export with these safe insets. Nothing without the setting.
    pub(super) fn paint_promote(
        &self,
        painter: &Painter,
        picture: Rect,
        canvas: (u32, u32),
        safe: Option<[u32; 4]>,
    ) {
        if self.promote {
            paint(painter, picture, canvas, safe, self.promote_corner);
        }
    }
}

/// Paint the mark of a `canvas`-sized picture into `picture`: the same
/// geometry the export's libass script draws, scaled to the monitor. The
/// frame's edges land on physical pixels and its stroke is never thinner than
/// one; the letters are the logo font's, at the wordmark's pens.
pub fn paint(
    painter: &Painter,
    picture: Rect,
    canvas: (u32, u32),
    safe: Option<[u32; 4]>,
    corner: Corner,
) {
    let (cw, ch) = canvas;
    if cw == 0 || ch == 0 || picture.width() <= 0.0 {
        return;
    }
    let m = Layout::new(cw, ch, safe, corner);
    let k = picture.width() / cw as f32;
    let ppp = painter.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let at = |x: f32, y: f32| picture.min + Vec2::new(x, y) * k;
    let alpha = |o: f32| (o * 255.0).round() as u8;
    // The frame: four bands, each at least one physical pixel.
    let stroke = (m.stroke as f32 * k).max(1.0 / ppp);
    let (a, b) = (at(m.x0 as f32, m.y0 as f32), at(m.x1 as f32, m.y1 as f32));
    let (x0, y0, x1, y1) = (snap(a.x), snap(a.y), snap(b.x), snap(b.y));
    let ring = Color32::from_white_alpha(alpha(noh::mark::STROKE));
    for band in [
        Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y0 + stroke)),
        Rect::from_min_max(egui::pos2(x0, y1 - stroke), egui::pos2(x1, y1)),
        Rect::from_min_max(
            egui::pos2(x0, y0 + stroke),
            egui::pos2(x0 + stroke, y1 - stroke),
        ),
        Rect::from_min_max(
            egui::pos2(x1 - stroke, y0 + stroke),
            egui::pos2(x1, y1 - stroke),
        ),
    ] {
        painter.rect_filled(band, 0.0, ring);
    }
    // The letters, white at their opacity, on their baselines.
    let white = Color32::from_white_alpha(alpha(noh::mark::LETTERS));
    let logo = |size: f32| FontId::new(size, FontFamily::Name("logo".into()));
    let place = |text: &str, font: FontId, pen: egui::Pos2| {
        let galley = painter.layout_no_wrap(text.into(), font, white);
        let origin = galley
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|g| row.pos + g.pos.to_vec2()));
        if let Some(origin) = origin {
            painter.galley(pen - origin.to_vec2(), galley, white);
        }
    };
    let noh = logo(1000.0 * m.unit() * k);
    for (letter, x) in m.letters() {
        place(letter, noh.clone(), at(x, m.baseline));
    }
    // MADE WITH: each letter at its pen, the font's advance plus the tracking.
    let size = 1000.0 * m.made_scale * k;
    let made = logo(size);
    let (mut pen, tracking) = m.made_pen();
    for c in noh::mark::MADE_TEXT.chars() {
        place(&c.to_string(), made.clone(), at(pen, m.made_baseline));
        let advance = painter
            .layout_no_wrap(c.to_string(), made.clone(), white)
            .rows
            .first()
            .and_then(|row| row.glyphs.first().map(|g| g.advance_width))
            .unwrap_or(0.0);
        pen += advance / k + tracking;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(revision: u64) -> Option<app_state::Artifact> {
        Some(app_state::Artifact {
            path: "made.mp4".into(),
            revision,
            bytes: None,
        })
    }

    #[test]
    fn ticking_the_box_outdates_the_exports_and_never_the_previews() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = NohApp::default();
        app.locale.path = Some(folder.path().join("noh").join("language"));
        let revision = app.revision;
        app.export_artifact = artifact(revision);
        app.preview_artifact = artifact(revision);
        app.captions.export_artifact = artifact(app.captions.revision);
        app.shorts.export_artifact = artifact(app.shorts.revision);
        assert!(!app.promote);
        app.set_promote(true);
        assert!(app.promote && app.notice.is_none());
        // The exports were made without the mark: no longer current.
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        assert_ne!(
            app.captions.export_artifact.as_ref().unwrap().revision,
            app.captions.revision
        );
        assert_ne!(
            app.shorts.export_artifact.as_ref().unwrap().revision,
            app.shorts.revision
        );
        // The preview render carries no mark (drawn live): still current, and
        // the project's revision did not move, so nothing renders again.
        assert_eq!(app.revision, revision);
        assert_eq!(
            app.preview_artifact.as_ref().unwrap().revision,
            app.revision
        );
        // Remembered.
        let file = path(app.locale.path.as_deref().unwrap());
        assert!(read(Some(&file)).0);
        app.set_promote(false);
        assert!(!read(Some(&file)).0);
    }

    #[test]
    fn the_setting_is_off_unless_the_file_says_on() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("promote");
        assert_eq!(read(None), (false, Corner::Auto));
        assert_eq!(read(Some(&file)), (false, Corner::Auto));
        std::fs::write(&file, "maybe").unwrap();
        assert!(!read(Some(&file)).0);
        std::fs::write(
            &file, "on
",
        )
        .unwrap();
        assert_eq!(read(Some(&file)), (true, Corner::Auto));
        save(&file, true, Corner::TopRight).unwrap();
        assert_eq!(read(Some(&file)), (true, Corner::TopRight));
    }

    #[test]
    fn moving_the_mark_outdates_exports_only_while_it_is_on() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = NohApp::default();
        app.locale.path = Some(folder.path().join("noh").join("language"));
        let revision = app.revision;
        app.export_artifact = artifact(revision);
        app.preview_artifact = artifact(revision);
        // Off: the corner is remembered, nothing is outdated.
        app.set_promote_corner(Corner::TopRight);
        assert_eq!(app.export_artifact.as_ref().unwrap().revision, revision);
        // On, and the box ticked made the old export stale; a fresh one now.
        app.set_promote(true);
        app.export_artifact = artifact(app.revision);
        // Auto and the corner it stands for on a landscape picture... differ
        // on a vertical one, so the move counts.
        app.set_promote_corner(Corner::BottomRight);
        assert_ne!(app.export_artifact.as_ref().unwrap().revision, app.revision);
        // The preview render never carries the mark: still current.
        assert_eq!(
            app.preview_artifact.as_ref().unwrap().revision,
            app.revision
        );
        assert_eq!(app.revision, revision);
        let file = path(app.locale.path.as_deref().unwrap());
        assert_eq!(read(Some(&file)), (true, Corner::BottomRight));
    }
}
