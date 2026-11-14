//! The font picker's rows, each name drawn in its own face (maintainer, 2026-11-12):
//! the face the effect will draw with (an installed family at the effect face's
//! weight and style, by the engine's own query), rasterized off the UI thread
//! when the row first shows, into a small white alpha image the row tints. Until
//! then, and for a symbol font or a face that cannot draw its own name, the row
//! keeps the interface font. egui's own fonts are never touched.
use eframe::egui;
use noh::fx::text::{Face, Fonts, InstalledFonts, Label};
use std::collections::HashMap;
use std::sync::mpsc;

/// One row's drawing: the font setting's value (`jost_extrabold`,
/// `system:Arial`), the effect's own face (the weight and style an installed
/// family is drawn at), the text, and its size in pixels × 100.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Key {
    pub value: String,
    pub own: String,
    pub text: String,
    pub px100: u32,
}

impl Key {
    pub fn new(value: &str, own: &str, text: &str, px: f32) -> Self {
        Self {
            value: value.into(),
            own: own.into(),
            text: text.into(),
            px100: (px * 100.0).round() as u32,
        }
    }
}

pub enum Row {
    Pending,
    /// The name's image and its size in points.
    Drawn(egui::TextureHandle, egui::Vec2),
    Symbol,
    Plain,
}

struct Job {
    key: Key,
    installed: Option<InstalledFonts>,
}

#[derive(Default)]
pub struct Rows {
    cache: HashMap<Key, Row>,
    worker: Option<(mpsc::Sender<Job>, mpsc::Receiver<(Key, Label)>)>,
    /// Draw on the UI thread before the frame (capture mode, tests).
    sync_fonts: Option<Fonts>,
    pub sync: bool,
    /// Never draw: rows keep the interface font (tests that read row texts).
    pub plain: bool,
}

/// `key`'s name drawn by `fonts` (`Label::Unreadable` for a family not installed).
/// An installed family is drawn with fonts of its own, dropped right after: the
/// engine maps an installed file while it draws with it (rewriting the file then
/// fails on Windows, measured with Rust's own file calls), so a family only
/// browsed in the list is not kept.
pub fn draw(fonts: &mut Fonts, key: &Key, installed: Option<&InstalledFonts>) -> Label {
    let own = Face::named(&key.own).unwrap_or(Face::Sans);
    if !key.value.starts_with("system:") {
        let face = Face::named(&key.value).unwrap_or(own);
        return fonts.label(face, &key.text, key.px100 as f32 / 100.0);
    }
    let (Some(db), Ok(mut once)) = (installed, Fonts::load()) else {
        return Label::Unreadable;
    };
    once.set_installed(db.clone());
    match once.face(&key.value, own) {
        (face, None) => once.label(face, &key.text, key.px100 as f32 / 100.0),
        _ => Label::Unreadable,
    }
}

impl Rows {
    pub fn peek(&self, key: &Key) -> Option<&Row> {
        self.cache.get(key)
    }

    /// Take the worker's finished drawings into textures.
    pub fn poll(&mut self, ctx: &egui::Context) {
        let done: Vec<(Key, Label)> = match &self.worker {
            Some((_, rx)) => rx.try_iter().collect(),
            None => Vec::new(),
        };
        for (key, label) in done {
            let row = row_of(ctx, &key, label);
            self.cache.insert(key, row);
        }
    }

    /// Ask for `key`'s drawing (once): at once in sync mode, else queued.
    pub fn want(&mut self, ctx: &egui::Context, key: Key, installed: Option<InstalledFonts>) {
        if self.cache.contains_key(&key) || self.plain {
            return;
        }
        if self.sync || std::env::var_os("NOH_CAPTURE_UI").is_some() {
            if self.sync_fonts.is_none() {
                self.sync_fonts = Fonts::load().ok();
            }
            let label = match &mut self.sync_fonts {
                Some(fonts) => guarded(fonts, &key, installed.as_ref()),
                None => Label::Unreadable,
            };
            let row = row_of(ctx, &key, label);
            self.cache.insert(key, row);
            return;
        }
        let (tx, _) = self.worker.get_or_insert_with(|| spawn(ctx.clone()));
        if tx
            .send(Job {
                key: key.clone(),
                installed,
            })
            .is_ok()
        {
            self.cache.insert(key, Row::Pending);
        } else {
            self.cache.insert(key, Row::Plain);
        }
    }
}

/// A face that panics the rasterizer gives the interface font, never a crash.
fn guarded(fonts: &mut Fonts, key: &Key, installed: Option<&InstalledFonts>) -> Label {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(fonts, key, installed)))
        .unwrap_or(Label::Unreadable)
}

fn spawn(ctx: egui::Context) -> (mpsc::Sender<Job>, mpsc::Receiver<(Key, Label)>) {
    let (jobs, inbox) = mpsc::channel::<Job>();
    let (results, done) = mpsc::channel();
    std::thread::spawn(move || {
        let mut fonts = Fonts::load().ok();
        for job in inbox {
            let label = match &mut fonts {
                Some(fonts) => guarded(fonts, &job.key, job.installed.as_ref()),
                None => Label::Unreadable,
            };
            if results.send((job.key, label)).is_err() {
                break;
            }
            ctx.request_repaint();
        }
    });
    (jobs, done)
}

fn row_of(ctx: &egui::Context, key: &Key, label: Label) -> Row {
    match label {
        Label::Drawn { w, h, alpha } => {
            let rgba: Vec<u8> = alpha.iter().flat_map(|&a| [a, a, a, a]).collect();
            let image = egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba);
            let texture = ctx.load_texture(
                format!("font-row-{}-{}", key.value, key.px100),
                image,
                egui::TextureOptions::LINEAR,
            );
            let ppp = ctx.pixels_per_point();
            Row::Drawn(texture, egui::vec2(w as f32 / ppp, h as f32 / ppp))
        }
        Label::Symbol => Row::Symbol,
        Label::Unreadable => Row::Plain,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The path the app takes: queued, drawn by the worker, taken by `poll`; a
    /// family that is not installed comes back as the interface font.
    #[test]
    fn rows_are_drawn_off_the_ui_thread_and_failures_keep_the_ui_font() {
        let ctx = egui::Context::default();
        let mut rows = Rows::default();
        let drawn = Key::new("inter_semibold", "sans", "Inter SemiBold", 14.0);
        let absent = Key::new("system:Absent Family", "sans", "Absent Family", 14.0);
        rows.want(&ctx, drawn.clone(), None);
        rows.want(&ctx, absent.clone(), None);
        assert!(matches!(rows.peek(&drawn), Some(Row::Pending)));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while matches!(rows.peek(&drawn), Some(Row::Pending))
            || matches!(rows.peek(&absent), Some(Row::Pending))
        {
            assert!(std::time::Instant::now() < deadline, "the worker answers");
            std::thread::sleep(std::time::Duration::from_millis(10));
            rows.poll(&ctx);
        }
        assert!(matches!(rows.peek(&drawn), Some(Row::Drawn(..))));
        assert!(matches!(rows.peek(&absent), Some(Row::Plain)));
    }

    /// A row's drawing depends on the effect's own face (the weight an installed
    /// family is drawn at): two effects never share one cached image.
    #[test]
    fn rows_are_cached_per_effect_face_and_drawn_at_once_in_sync_mode() {
        let ctx = egui::Context::default();
        let mut rows = Rows {
            sync: true,
            ..Rows::default()
        };
        let bold = Key::new("jost_medium", "jost_extrabold", "Jost Medium", 14.0);
        let regular = Key::new("jost_medium", "sans", "Jost Medium", 14.0);
        assert_ne!(bold, regular);
        rows.want(&ctx, bold.clone(), None);
        assert!(matches!(rows.peek(&bold), Some(Row::Drawn(..))));
        assert!(rows.peek(&regular).is_none());
        // A family that is not installed keeps the interface font.
        let absent = Key::new("system:Absent Family", "sans", "Absent Family", 14.0);
        rows.want(&ctx, absent.clone(), None);
        assert!(matches!(rows.peek(&absent), Some(Row::Plain)));
        // Plain mode never draws.
        let mut plain = Rows {
            plain: true,
            ..Rows::default()
        };
        plain.want(&ctx, bold.clone(), None);
        assert!(plain.peek(&bold).is_none());
    }

    /// An installed font only browsed in the list can be updated or removed
    /// while NOH runs (a user may update or uninstall it): drawing its row
    /// leaves no file open. The control shows the check can fail: the face an
    /// effect draws with keeps its file mapped, and rewriting it then fails on
    /// Windows. Rust's remove_file succeeds either way here, so only the rewrite
    /// tells them apart; an uninstall from Windows itself was not tried.
    #[test]
    fn a_browsed_installed_font_can_be_updated_right_after_its_row_is_drawn() {
        let bundled = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets/BarlowCondensed-ExtraBold.ttf");
        let installed = |dir: &std::path::Path| {
            let file = dir.join("Installed.ttf");
            std::fs::copy(&bundled, &file).unwrap();
            let db: InstalledFonts =
                std::sync::Arc::new(noh::fx::text::system_fonts(Some(&[dir.into()])));
            let family = noh::fx::text::families_of(&db)
                .into_iter()
                .next()
                .expect("the copied file is read");
            (file, db, format!("system:{family}"), family)
        };
        let rewrite = |file: &std::path::Path| {
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(file)
                .map(|_| ())
        };
        let browsed = tempfile::tempdir().unwrap();
        let (file, db, value, family) = installed(browsed.path());
        let key = Key::new(&value, "condensed_extrabold", &family, 14.0);
        let mut fonts = Fonts::load().unwrap();
        assert!(
            matches!(draw(&mut fonts, &key, Some(&db)), Label::Drawn { .. }),
            "the row is drawn in its own face"
        );
        rewrite(&file).expect("a browsed font can be updated");
        std::fs::remove_file(&file).expect("a browsed font can be removed");
        if cfg!(windows) {
            let used = tempfile::tempdir().unwrap();
            let (file, db, value, _) = installed(used.path());
            let mut kept = Fonts::load().unwrap().with_installed(db);
            assert_eq!(kept.face(&value, Face::CondensedExtraBold).1, None);
            assert!(
                rewrite(&file).is_err(),
                "control: the face an effect draws with holds its file"
            );
            drop(kept);
            rewrite(&file).expect("released with its fonts");
        }
    }

    /// The list's cost on this machine's real fonts (maintainer, 2026-11-12: « temps
    /// de chargement »): reading the installed fonts once, then drawing each row.
    /// `cargo test --features gui -- --ignored --nocapture font_rows_cost`.
    #[test]
    #[ignore = "measures this machine's installed fonts"]
    fn font_rows_cost_on_the_installed_fonts() {
        use std::time::Instant;
        let start = Instant::now();
        let db: InstalledFonts = std::sync::Arc::new(noh::fx::text::system_fonts(None));
        let read = start.elapsed();
        let families = noh::fx::text::families_of(&db);
        let mut fonts = Fonts::load().unwrap();
        let mut each = Vec::new();
        let mut drawn = 0;
        let mut kept_ui_font = Vec::new();
        for family in &families {
            let key = Key::new(&format!("system:{family}"), "sans", family, 14.0);
            let t = Instant::now();
            match draw(&mut fonts, &key, Some(&db)) {
                Label::Drawn { .. } => drawn += 1,
                Label::Symbol => kept_ui_font.push(format!("{family} (symbols)")),
                Label::Unreadable => kept_ui_font.push(format!("{family} (unreadable)")),
            }
            each.push(t.elapsed());
        }
        each.sort();
        let total: std::time::Duration = each.iter().sum();
        println!(
            "faces {} families {} | read {:?} | rows drawn {drawn}, total {:?}, median {:?}, p95 {:?}, max {:?}",
            db.len(),
            families.len(),
            read,
            total,
            each[each.len() / 2],
            each[each.len() * 95 / 100],
            each[each.len() - 1],
        );
        println!("interface font kept: {}", kept_ui_font.join(", "));
    }
}
