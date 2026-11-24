//! « Promote NOH »: the setting, its checkbox on the export lines, and the
//! « made with NOH » mark painted over the preview (`noh::mark` holds the one
//! description exports burn with libass; the preview draws the same numbers).
use super::*;
use egui::{Color32, FontFamily, FontId, Painter, Pos2, Rect, Vec2};
use noh::mark::Layout;

/// The setting's file, beside the language file (`<config>/noh/promote`): its
/// presence with `on` means the box is ticked. Off by default.
pub fn path(language: &std::path::Path) -> std::path::PathBuf {
    language.with_file_name("promote")
}

pub fn read(path: Option<&std::path::Path>) -> bool {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .is_some_and(|s| s.trim() == "on")
}

fn save(path: &std::path::Path, on: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, if on { "on" } else { "off" })
}

impl NohApp {
    /// Change the setting: remembered, and the exports already made, made
    /// without (or with) the mark, are no longer current. Previews are not
    /// touched: the mark is drawn over them live.
    pub(super) fn set_promote(&mut self, on: bool) {
        if self.promote == on {
            return;
        }
        self.promote = on;
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
        let saved = self
            .locale
            .path
            .as_deref()
            .ok_or_else(|| std::io::Error::other("No user configuration directory"))
            .and_then(|language| save(&path(language), on));
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
            paint(painter, picture, canvas, safe);
        }
    }
}

/// Paint the mark of a `canvas`-sized picture into `picture` (the same
/// geometry the export's libass script uses, scaled).
pub fn paint(painter: &Painter, picture: Rect, canvas: (u32, u32), safe: Option<[u32; 4]>) {
    let (cw, ch) = canvas;
    if cw == 0 || ch == 0 || picture.width() <= 0.0 {
        return;
    }
    let m = Layout::new(cw, ch, safe);
    let k = picture.width() / cw as f32;
    let at = |x: f32, y: f32| picture.min + Vec2::new(x, y) * k;
    let white = Color32::from_white_alpha((noh::mark::OPACITY * 255.0).round() as u8);
    let halo = Color32::from_black_alpha(76);
    let font = FontId::new(m.size * k, FontFamily::Name("logo".into()));
    let small = FontId::new(m.small * k, FontFamily::Name("logo".into()));
    // A soft halo: the letters drawn dark around their place, then white.
    let ring: Vec<Vec2> = (0..8)
        .map(|i| {
            let a = i as f32 * std::f32::consts::FRAC_PI_4;
            Vec2::new(a.cos(), a.sin()) * m.halo * k
        })
        .collect();
    let mut glyphs: Vec<(String, FontId, Pos2)> = m
        .letters()
        .map(|(letter, x)| (letter.to_string(), font.clone(), at(x, m.baseline)))
        .collect();
    // « made with », letter by letter at its spacing.
    let mut x = m.left;
    for c in noh::mark::small_text().chars() {
        let galley = painter.layout_no_wrap(c.to_string(), small.clone(), white);
        glyphs.push((c.to_string(), small.clone(), at(x, m.small_baseline)));
        x += galley.size().x / k + m.small_spacing;
    }
    for color in [halo, white] {
        for (text, font, pen) in &glyphs {
            let galley = painter.layout_no_wrap(text.clone(), font.clone(), color);
            // The pen position on the baseline, as SVG text and libass do.
            let Some(origin) = galley
                .rows
                .first()
                .and_then(|row| row.glyphs.first().map(|g| row.pos + g.pos.to_vec2()))
            else {
                continue;
            };
            let offsets: &[Vec2] = if color == halo { &ring } else { &[Vec2::ZERO] };
            for offset in offsets {
                painter.galley(*pen + *offset - origin.to_vec2(), galley.clone(), color);
            }
        }
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
        assert!(read(Some(&file)));
        app.set_promote(false);
        assert!(!read(Some(&file)));
    }

    #[test]
    fn the_setting_is_off_unless_the_file_says_on() {
        let folder = tempfile::tempdir().unwrap();
        let file = folder.path().join("promote");
        assert!(!read(None));
        assert!(!read(Some(&file)));
        std::fs::write(&file, "maybe").unwrap();
        assert!(!read(Some(&file)));
        std::fs::write(&file, "on\n").unwrap();
        assert!(read(Some(&file)));
    }
}
