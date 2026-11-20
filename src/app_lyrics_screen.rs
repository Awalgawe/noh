//! The lyrics screen (mockup v5): the lyrics as a text, one field per line, a
//! zoomed track that follows playback, and the monitor beside them. It edits
//! the same session as the lyrics lane (app_lyrics_edit.rs), through the same
//! undo history; each step names itself for « Annuler : … ». The sync game
//! (hold Espace while it is sung) is in `game`.
use super::*;
use app_style::{self as style, space};
use egui::{
    Align, Align2, Color32, FontId, Key, Layout, Modifiers, Rect, Sense, Stroke, pos2, vec2,
};
use noh::{
    subtitle_edit::{self as edit, Edge, Level},
    subtitle_track::{SubtitleCue, SubtitleTrack, WordSource},
    timeline::{Range, Viewport},
};
use std::sync::Arc;

mod game;

const MODEBAR_H: f32 = 56.0;
const FOOT_H: f32 = 44.0;
/// A game's prompt in the narrow window, on one row.
const PROMPT_H: f32 = 30.0;
/// What the zoomed track shows: 12 s, 6 s in the narrow window (388 px would
/// leave 32 px per second, too little to read a word).
const VIEW_MS: u64 = 12_000;
const VIEW_COMPACT_MS: u64 = 6_000;
/// A silence between two lines the text list names (« 5,2 s sans paroles »).
const GAP_NOTE_MS: u64 = 5_000;
/// Alt+← → moves a word's start by this much.
const NUDGE_MS: i64 = 50;
/// Typing stops this long: the text typed so far reaches the lyrics.
const TYPING_PAUSE: Duration = Duration::from_millis(400);
const TOAST: Duration = Duration::from_secs(8);
const FLASH: Duration = Duration::from_secs(3);
/// The time column of the text list.
const TIME_W: f32 = 52.0;
const ROW_H: f32 = 28.0;
const LANE_H: f32 = 84.0;
const EDGE_HIT: f32 = 6.0;

/// The text of the current line while it is typed.
struct Draft {
    cue: usize,
    text: String,
    /// The cue's text this draft started from, or was last written as: when
    /// the cue no longer says it (Ctrl+Z, another edit), the draft starts over.
    base: String,
    /// The last keystroke not yet in the lyrics.
    typed_at: Option<Instant>,
}

/// Where the text cursor is in its line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaretKind {
    Selection,
    Empty,
    Start,
    End,
    /// Between two words: Entrée cuts before word `k`.
    Cut(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Caret {
    cue: usize,
    /// Character index.
    index: usize,
    kind: CaretKind,
    /// On the first (last) row of the field: ↑ (↓) leaves the line.
    top: bool,
    bottom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DragPart {
    Start,
    End,
    Body,
    /// The start of a unit (its index among the cue's units).
    Unit(usize),
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    part: DragPart,
    /// The word under the press: a click without a move chooses it.
    word: Option<usize>,
    anchor_ms: u64,
    anchor_x: f32,
    moved: bool,
}

/// The split of a word being typed in the footer.
struct SylEdit {
    word: usize,
    text: String,
    /// Where a proposed split comes from.
    note: Option<String>,
    /// Why the typed split was refused (a locale key).
    error: Option<&'static str>,
    /// The field takes the keyboard when it opens.
    focus: bool,
}

pub struct State {
    pub open: bool,
    /// The current line.
    pub cur: usize,
    /// Give the keyboard to this line's field, the cursor at this character
    /// (None: at the end).
    focus: Option<(usize, Option<usize>)>,
    draft: Option<Draft>,
    /// The text cursor when its field last had the keyboard.
    caret: Option<Caret>,
    text_focus: bool,
    /// The word chosen in the track.
    word: Option<usize>,
    syl: Option<SylEdit>,
    /// What the track shows, and what the sync game will take.
    pub level: Level,
    /// The sync game in progress.
    game: Option<game::Game>,
    /// What the last game did, until something else is done.
    summary: Option<(game::Summary, u64)>,
    /// The playback speed chosen for the game.
    speed: f64,
    /// Tests stand in for the player's clock: Some(None) is « not playing ».
    #[cfg(test)]
    test_clock: Option<Option<u64>>,
    /// Tests stand in for the engine: whether the one that can slow plays.
    #[cfg(test)]
    test_live: Option<bool>,
    view_start: u64,
    drag: Option<Drag>,
    /// The track as the drag in progress would leave it.
    preview: Option<SubtitleTrack>,
    /// « Ligne supprimée · Annuler » until then.
    toast: Option<Instant>,
    flash: Option<(String, Instant)>,
    wave: app_timeline::WaveCache,
    scroll_to_cur: bool,
    /// A widget had the keyboard at the end of the last frame. egui drops the
    /// focus itself when Échap comes, before any widget is drawn: this tells
    /// that the press was spent on that.
    had_focus: bool,
    /// The keyboard left the current line's field: its text is kept (or the
    /// emptied line goes) once the click that took it is over.
    leave_pending: bool,
    /// The narrow window (under 560 px), as of the last frame.
    compact: bool,
    /// The lyrics as last shown, kept while their file is read again.
    shown: Option<Arc<SubtitleTrack>>,
    /// The height the footer's text needed last frame.
    foot_h: f32,
    /// The wide window's footer did not hold its text on one row last frame.
    foot_wraps: bool,
    /// A game was running when the footer was last drawn.
    foot_in_game: bool,
    /// The height a game's prompt needed last frame, in the narrow window.
    prompt_h: f32,
    /// The monitor showed the short before the screen opened.
    was_short: bool,
    /// The zones drawn last frame, for tests.
    #[cfg(test)]
    drawn: Drawn,
    /// Each line's row in the text list, last frame.
    #[cfg(test)]
    drawn_rows: Vec<Rect>,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct Drawn {
    monitor: Option<Rect>,
    text: Option<Rect>,
    lane: Option<Rect>,
    ruler: Option<Rect>,
    foot: Option<Rect>,
    foot_text: Option<Rect>,
    /// The header's title, its undo arrow and « Terminé », as drawn.
    title: Option<Rect>,
    undo: Option<Rect>,
    done: Option<Rect>,
    /// A game's line being sung: where it is drawn, and its expected unit.
    prompt: Option<Rect>,
    expected: Option<Rect>,
    /// The ruler, the pictures, the waveform and the lines' lane together.
    lanes: Option<Rect>,
    /// The banner shown while a job locks the lyrics.
    read_only: Option<Rect>,
    /// « Commencer ».
    start: Option<Rect>,
    /// The summary's « Reprendre à la ligne N » and « Tout annuler ».
    again: Option<Rect>,
    undo_all: Option<Rect>,
    /// The sync bar, last frame.
    sync: Option<Rect>,
    /// The current line's three buttons.
    cut: Option<Rect>,
    join: Option<Rect>,
    delete: Option<Rect>,
    view: Option<Viewport>,
}

impl State {
    /// How much of the song the track shows.
    fn span(&self, compact: bool) -> u64 {
        self.game_span(compact)
            .unwrap_or(if compact { VIEW_COMPACT_MS } else { VIEW_MS })
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            cur: 0,
            focus: None,
            draft: None,
            caret: None,
            text_focus: false,
            word: None,
            syl: None,
            level: Level::Line,
            game: None,
            summary: None,
            speed: 1.0,
            #[cfg(test)]
            test_clock: None,
            #[cfg(test)]
            test_live: None,
            view_start: 0,
            drag: None,
            preview: None,
            toast: None,
            flash: None,
            wave: Default::default(),
            scroll_to_cur: false,
            had_focus: false,
            leave_pending: false,
            compact: false,
            shown: None,
            foot_h: FOOT_H,
            foot_wraps: false,
            foot_in_game: false,
            prompt_h: PROMPT_H,
            was_short: false,
            #[cfg(test)]
            drawn: Default::default(),
            #[cfg(test)]
            drawn_rows: Vec::new(),
        }
    }
}

/// One block of the track: a word, or a syllable.
struct Unit {
    word: usize,
    text: String,
    start: u64,
    end: u64,
    source: WordSource,
    /// The first unit of its word (a word that is not cut is one unit).
    word_start: bool,
}

/// A cue's words (or, with `syllables`, its syllables), each with its start
/// and its end: the known end, else the next start (the cue's end for the last).
fn shown_units(cue: &SubtitleCue, syllables: bool) -> Vec<Unit> {
    let words = edit::words_or_estimate(cue);
    let mut out = Vec::new();
    for (k, w) in words.iter().enumerate() {
        let next = words.get(k + 1).map_or(cue.end_ms, |n| n.start_ms);
        let word_end = w.end_ms.unwrap_or(next).max(w.start_ms);
        match &w.syllables {
            Some(parts) if syllables => {
                for (j, s) in parts.iter().enumerate() {
                    let end = s
                        .end_ms
                        .unwrap_or_else(|| parts.get(j + 1).map_or(word_end, |n| n.start_ms));
                    out.push(Unit {
                        word: k,
                        text: s.text.clone(),
                        start: s.start_ms,
                        end: end.max(s.start_ms),
                        source: s.source,
                        word_start: j == 0,
                    });
                }
            }
            _ => out.push(Unit {
                word: k,
                text: w.text.clone(),
                start: w.start_ms,
                end: word_end,
                source: w.source,
                word_start: true,
            }),
        }
    }
    out
}

/// Where the text cursor at character `index` stands in `text`.
fn caret_kind(text: &str, index: usize, selection: bool) -> CaretKind {
    if selection {
        return CaretKind::Selection;
    }
    if text.trim().is_empty() {
        return CaretKind::Empty;
    }
    let at = text
        .char_indices()
        .nth(index)
        .map_or(text.len(), |(b, _)| b);
    let (before, after) = text.split_at(at);
    if before.trim().is_empty() {
        return CaretKind::Start;
    }
    if after.trim().is_empty() {
        return CaretKind::End;
    }
    let mut k = before.split_whitespace().count();
    let between = before.ends_with(char::is_whitespace) || after.starts_with(char::is_whitespace);
    if !between {
        k -= 1;
    }
    if k == 0 {
        CaretKind::Start
    } else {
        CaretKind::Cut(k)
    }
}

/// The word the text cursor is in (or just after).
fn word_under(text: &str, index: usize) -> usize {
    let at = text
        .char_indices()
        .nth(index)
        .map_or(text.len(), |(b, _)| b);
    let before = &text[..at];
    let mut k = before.split_whitespace().count();
    if !before.is_empty() && !before.ends_with(char::is_whitespace) {
        k = k.saturating_sub(1);
    }
    k.min(text.split_whitespace().count().saturating_sub(1))
}

/// Take a key press whose modifiers `want` accepts, so no widget sees it.
/// (`consume_key` ignores extra Shift and Alt: Maj+Entrée would pass for Entrée.)
fn take_key(i: &mut egui::InputState, key: Key, want: impl Fn(Modifiers) -> bool) -> bool {
    let mut hit = false;
    i.events.retain(|e| match e {
        egui::Event::Key {
            key: k,
            pressed: true,
            modifiers,
            ..
        } if *k == key && want(*modifiers) => {
            hit = true;
            false
        }
        _ => true,
    });
    hit
}

fn plain(m: Modifiers) -> bool {
    !m.alt && !m.ctrl && !m.shift && !m.command
}

fn alt_only(m: Modifiers) -> bool {
    m.alt && !m.ctrl && !m.shift && !m.command
}

fn x_at(view: Viewport, rect: Rect, ms: u64) -> f32 {
    rect.left() + view.fraction(ms) as f32 * rect.width()
}

fn time_at(view: Viewport, rect: Rect, x: f32) -> u64 {
    view.time(f64::from((x - rect.left()) / rect.width().max(1.0)))
}

fn row_id(i: usize) -> egui::Id {
    egui::Id::new(("lyrics-screen-text", i))
}

fn source_key(source: WordSource) -> &'static str {
    match source {
        WordSource::Whisper => "lyrics_screen.source_whisper",
        WordSource::Hand => "lyrics_screen.source_hand",
        WordSource::Estimated => "lyrics_screen.source_estimated",
    }
}

/// A footer line: `[Entrée]` in the text is drawn as a key cap.
fn key_caps(ui: &mut egui::Ui, text: &str) {
    let t = style::tokens(ui);
    let mut rest = text;
    ui.spacing_mut().item_spacing.x = 0.0;
    // In a wrapping row each word is placed on its own, on a new row when it
    // does not fit: a whole label or a key cap would run past the edge.
    let wrap = ui.layout().main_wrap;
    let fit = |ui: &mut egui::Ui, width: f32| {
        if wrap
            && width > ui.available_width() + 0.5
            && ui.available_width() < ui.max_rect().width()
        {
            ui.end_row();
        }
    };
    let label = |ui: &mut egui::Ui, s: &str| {
        if s.is_empty() {
            return;
        }
        if !wrap {
            ui.label(egui::RichText::new(s).size(13.0).color(t.text_2));
            return;
        }
        for word in s.split_inclusive(' ').flat_map(breakable) {
            let galley =
                ui.painter()
                    .layout_no_wrap(word.to_string(), FontId::proportional(13.0), t.text_2);
            // A row never starts or ends on the space that follows a word.
            let shown = ui
                .painter()
                .layout_no_wrap(
                    word.trim_end().to_string(),
                    FontId::proportional(13.0),
                    t.text_2,
                )
                .size()
                .x;
            fit(ui, shown);
            let width = galley.size().x.min(ui.available_width().max(shown));
            let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
            ui.painter().galley(
                pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
                galley,
                t.text_2,
            );
        }
    };
    while let Some(a) = rest.find('[') {
        let Some(len) = rest[a..].find(']') else {
            break;
        };
        label(ui, &rest[..a]);
        let cap = &rest[a + 1..a + len];
        let galley = ui.painter().layout_no_wrap(
            cap.to_string(),
            FontId::proportional(style::text::SMALL),
            t.text,
        );
        fit(ui, galley.size().x + 12.0);
        let (rect, _) = ui.allocate_exact_size(galley.size() + vec2(12.0, 4.0), Sense::hover());
        ui.painter().rect_stroke(
            rect,
            4,
            Stroke::new(1.0, t.border_strong),
            egui::StrokeKind::Inside,
        );
        ui.painter().line_segment(
            [
                rect.left_bottom() + vec2(3.0, -1.0),
                rect.right_bottom() + vec2(-3.0, -1.0),
            ],
            Stroke::new(1.0, t.border_strong),
        );
        ui.painter()
            .galley(rect.min + vec2(6.0, 2.0), galley, t.text);
        rest = &rest[a + len + 1..];
    }
    label(ui, rest);
}

/// Where a footer row may break inside `word`: nowhere, except in writing
/// without spaces (kana, kanji, hanzi and their punctuation), where each
/// character may start a row. Hangul words are spaced: they stay whole.
fn breakable(word: &str) -> Vec<&str> {
    let open =
        |c: char| ('\u{2E80}'..'\u{AC00}').contains(&c) || ('\u{F900}'..='\u{FFEF}').contains(&c);
    if !word.chars().any(open) {
        return vec![word];
    }
    let mut out = Vec::new();
    let mut start = 0;
    for (at, c) in word.char_indices().skip(1) {
        // A closing mark or a space stays with what it follows.
        if !"、。，．！？：）」』】・ ".contains(c) {
            out.push(&word[start..at]);
            start = at;
        }
    }
    out.push(&word[start..]);
    out
}

/// A gap between two parts of a footer line; none when the row is about to
/// end (the next part starts the next row at the margin).
fn row_gap(ui: &mut egui::Ui, width: f32) {
    if ui.max_rect().right() - ui.cursor().min.x > width {
        ui.add_space(width);
    }
}

/// A small button of the text list's row actions and of the footer (and
/// « Modifier » on the main screen's lyrics chip).
pub(super) fn small_button(
    ui: &mut egui::Ui,
    label: &str,
    enabled: bool,
    kind: style::Kind,
) -> egui::Response {
    let t = style::tokens(ui);
    let quiet = kind == style::Kind::Quiet;
    let fill = if quiet {
        Color32::TRANSPARENT
    } else {
        t.surface_2
    };
    let color = match (enabled, quiet) {
        (false, _) => t.text_3,
        (true, true) => t.text_2,
        (true, false) => t.text,
    };
    let saved = (ui.spacing().button_padding, ui.visuals().disabled_alpha);
    ui.spacing_mut().button_padding = vec2(8.0, 0.0);
    ui.visuals_mut().disabled_alpha = 1.0;
    let response = ui.add_enabled(
        enabled,
        egui::Button::new(
            egui::RichText::new(label)
                .size(style::text::SMALL)
                .color(color),
        )
        .fill(fill)
        .stroke(Stroke::new(1.0, Color32::TRANSPARENT))
        .corner_radius(style::radius::M)
        .min_size(vec2(0.0, 24.0)),
    );
    (
        ui.spacing_mut().button_padding,
        ui.visuals_mut().disabled_alpha,
    ) = saved;
    style::focus(ui, &response);
    response
}

impl NohApp {
    /// Open the lyrics screen on line `cue` (None: the line under the
    /// playhead, else the first). The monitor shows the whole video there.
    /// « Modifier » on the lyrics chip and a click on the lyrics lane open it.
    pub(super) fn open_lyrics_screen(&mut self, cue: Option<usize>) {
        let Some(track) = self.lyrics_track() else {
            return;
        };
        let ph = self.mini_preview.cursor_ms;
        let at = cue
            .or_else(|| {
                track
                    .cues
                    .iter()
                    .position(|c| c.start_ms <= ph && ph < c.end_ms)
            })
            .unwrap_or(0);
        let s = &mut self.lyrics_screen;
        *s = State {
            open: true,
            cur: at.min(track.cues.len().saturating_sub(1)),
            level: s.level,
            speed: s.speed,
            was_short: self.mini_preview.short,
            shown: Some(track.clone()),
            scroll_to_cur: true,
            ..Default::default()
        };
        if self.mini_preview.short {
            self.mini_preview.short = false;
            self.mini_preview.invalidate();
        }
        if let Some(c) = track.cues.get(self.lyrics_screen.cur) {
            self.lyrics_screen.view_start = c.start_ms.saturating_sub(VIEW_MS / 4);
            // A line asked for by name: the playhead goes to its start, so the
            // monitor shows it (a line found under the playhead keeps it).
            if cue.is_some() && !self.mini_playing() {
                self.navigate(c.start_ms, false);
            }
        }
    }

    /// A capture state: the screen open on line `i`, in one of its situations.
    pub(super) fn stage_lyrics_screen(&mut self, i: usize, scene: &str) {
        self.open_lyrics_screen(Some(i));
        let text = self
            .ls_track()
            .and_then(|t| t.cues.get(i).map(|c| c.text.clone()))
            .unwrap_or_default();
        match scene {
            // The text cursor before the last word.
            "-cut" => {
                let at = text
                    .trim_end()
                    .rfind(char::is_whitespace)
                    .map_or(0, |b| text[..b].chars().count() + 1);
                self.lyrics_screen.focus = Some((i, Some(at)));
            }
            "-word" => {
                self.lyrics_screen.level = Level::Word;
                self.lyrics_screen.word =
                    Some(1.min(text.split_whitespace().count().saturating_sub(1)));
            }
            "-syllables" => {
                self.lyrics_screen.level = Level::Syllable;
                self.subtitles.language = "en".into();
                self.ls_open_syllables(0);
            }
            // A game at the Words level, two lines further (as the mockup):
            // three words taken; the fourth being held; a word that went by;
            // and what the footer says after a game.
            "-game" | "-game-hold" | "-game-missed" | "-summary" => {
                let last = self
                    .ls_track()
                    .map_or(0, |t| t.cues.len().saturating_sub(1));
                let line = (i + 2).min(last);
                let scene = scene.strip_prefix("-game").unwrap_or(scene);
                self.stage_lyrics_game(line, Level::Word, 3, scene);
                if scene == "-summary" {
                    self.ls_end_game();
                }
            }
            // A job started while the screen was open: read-only.
            "-read-only" => self.cancelling = true,
            _ => {}
        }
    }

    /// A staged game in the narrow window: the track where following the
    /// playhead would have left it (the staging frames it for the wide one).
    pub(super) fn stage_lyrics_view_compact(&mut self) {
        if self.lyrics_screen.game.is_some() {
            let span = self.lyrics_screen.span(true);
            self.lyrics_screen.view_start =
                self.mini_preview.cursor_ms.saturating_sub(span * 3 / 10);
        }
    }

    /// The line the open screen is on.
    #[cfg(test)]
    pub(super) fn lyrics_screen_line(&self) -> Option<usize> {
        self.lyrics_screen.open.then_some(self.lyrics_screen.cur)
    }

    /// « Terminé »: what was typed is kept (the lyrics are written about a
    /// second after the last change, as everywhere), playback pauses.
    pub(super) fn close_lyrics_screen(&mut self) {
        self.ls_end_game();
        // Text typed while the lyrics file is read again is not in the lyrics
        // yet: the screen stays the moment it takes (« Terminé » works after).
        if self.ls_reading()
            && self
                .lyrics_screen
                .draft
                .as_ref()
                .is_some_and(|d| d.typed_at.is_some())
        {
            return;
        }
        self.ls_leave_row();
        self.end_edit_run();
        self.mini_pause();
        if self.lyrics_screen.was_short && self.project.range.is_some() {
            self.mini_preview.short = true;
            self.mini_preview.invalidate();
        }
        let (level, speed) = (self.lyrics_screen.level, self.lyrics_screen.speed);
        self.lyrics_screen = State {
            level,
            speed,
            ..Default::default()
        };
    }

    /// The lyrics the screen shows: the edited or attached ones, else, only
    /// while the attached file is being read again (its periodic check, a
    /// reload after a change on disk), the ones it last showed. A file that
    /// could not be read gives none: the screen closes, and the main screen
    /// says what is wrong with the file.
    fn ls_track(&self) -> Option<Arc<SubtitleTrack>> {
        self.lyrics_track().or_else(|| {
            self.ls_reading()
                .then(|| self.lyrics_screen.shown.clone())
                .flatten()
        })
    }

    /// The lyrics file is being read again and nothing can be edited for a
    /// moment: the screen shows what it had, typed text waits in its field,
    /// and every other edit is held back rather than lost.
    fn ls_reading(&self) -> bool {
        self.lyrics_track().is_none()
            && self.project.track.path.is_some()
            && self.project.track.loading()
    }

    /// A job runs (an export, a render, lyrics generated again): the screen
    /// is read-only until its end, as the main screen's lane is locked. A
    /// game stops and keeps its takes, a drag or a split being typed is
    /// dropped, and nothing here changes the lyrics the job reads. Playback
    /// and « Terminé » stay.
    fn ls_lock_frame(&mut self) {
        if !self.lyrics_locked() {
            return;
        }
        self.ls_lock_game();
        let s = &mut self.lyrics_screen;
        (s.drag, s.preview, s.syl, s.word, s.toast) = (None, None, None, None, None);
    }

    fn ls_flash(&mut self, key: &str, args: &[String]) {
        let text = self.locale.language.format(key, args);
        self.lyrics_screen.flash = Some((text, Instant::now() + FLASH));
    }

    /// The song time under the playhead: the player's own while it plays.
    fn ls_playhead(&self) -> u64 {
        self.mini_position()
    }

    fn ls_view(&self, compact: bool) -> Option<Viewport> {
        let duration = self.project.duration_ms;
        let span = self.lyrics_screen.span(compact).min(duration);
        Range::place(self.lyrics_screen.view_start, span, duration).map(Viewport)
    }

    /// Bring line `i` into the track's view, centred, when it is outside.
    fn ls_reveal(&mut self, i: usize, compact: bool) {
        let (Some(view), Some(track)) = (self.ls_view(compact), self.ls_track()) else {
            return;
        };
        let Some(c) = track.cues.get(i) else {
            return;
        };
        if c.start_ms < view.0.start_ms || c.end_ms > view.0.end_ms {
            let span = view.0.duration_ms();
            self.lyrics_screen.view_start = ((c.start_ms + c.end_ms) / 2).saturating_sub(span / 2);
        }
    }

    /// The typed text reaches the lyrics: one undo step for the line, however
    /// many pauses the typing had.
    fn ls_commit_draft(&mut self) {
        // A lyrics question waits for its answer, or the lyrics file is being
        // read again: the typed text waits too, and is tried again after.
        // So does it while a job locks the lyrics.
        if self.lyrics_question_pending()
            || self.lyrics_question_open()
            || self.ls_reading()
            || self.lyrics_locked()
        {
            return;
        }
        let Some(d) = &mut self.lyrics_screen.draft else {
            return;
        };
        d.typed_at = None;
        let (i, text) = (d.cue, d.text.trim().to_string());
        let Some(current) = self
            .ls_track()
            .and_then(|t| t.cues.get(i).map(|c| c.text.clone()))
        else {
            return;
        };
        if text.is_empty() || text == current {
            return;
        }
        let label = self
            .locale
            .language
            .format("lyrics_screen.did_text", &[(i + 1).to_string()]);
        self.edit_track_run(
            egui::Id::new(("lyrics-screen-typing", i)).value(),
            label,
            |t| edit::set_text(t, i, &text),
        );
        // What the cue says now: the text, or the old one if it was refused.
        let now = self
            .ls_track()
            .and_then(|t| t.cues.get(i).map(|c| c.text.clone()));
        if let (Some(d), Some(now)) = (&mut self.lyrics_screen.draft, now) {
            d.base = now;
        }
    }

    /// Leaving the current line: its text is kept, or the line goes if it was
    /// left empty. Returns the index of a deleted line.
    fn ls_leave_row(&mut self) -> Option<usize> {
        let empty = self
            .lyrics_screen
            .draft
            .as_ref()
            .filter(|d| d.text.trim().is_empty())
            .map(|d| d.cue);
        match empty {
            Some(i) => {
                self.lyrics_screen.draft = None;
                self.ls_delete(i).then_some(i)
            }
            None => {
                self.ls_commit_draft();
                None
            }
        }
    }

    /// Make line `i` the current one; `focus` gives its field the keyboard,
    /// the cursor at that character (None: at the end).
    fn ls_set_cur(&mut self, mut i: usize, focus: Option<Option<usize>>) {
        // Leaving the line would drop what it holds while nothing can be written.
        if self.ls_reading() {
            return;
        }
        if self.lyrics_screen.cur != i {
            if let Some(gone) = self.ls_leave_row()
                && gone < i
            {
                i -= 1;
            }
            self.end_edit_run();
            self.lyrics_screen.word = None;
            self.lyrics_screen.syl = None;
            self.lyrics_screen.draft = None;
        }
        let Some(track) = self.ls_track() else {
            return;
        };
        if track.cues.is_empty() {
            return;
        }
        let i = i.min(track.cues.len() - 1);
        self.lyrics_screen.cur = i;
        self.lyrics_screen.scroll_to_cur = true;
        if let Some(caret) = focus {
            self.lyrics_screen.focus = Some((i, caret));
        }
        if !self.mini_playing() {
            self.navigate(track.cues[i].start_ms, false);
        }
        let compact = self.lyrics_screen.compact;
        self.ls_reveal(i, compact);
    }

    /// Cut the current line before its word `k`.
    fn ls_cut(&mut self, k: usize) {
        self.ls_commit_draft();
        let i = self.lyrics_screen.cur;
        let l = self.locale.language;
        let label = l.format("lyrics_screen.did_cut", &[(i + 1).to_string()]);
        if self.edit_track_as(label, |t| edit::split_at_word(t, i, k)) {
            self.lyrics_screen.draft = None;
            self.ls_set_cur(i + 1, Some(Some(0)));
        } else {
            self.ls_flash(
                "lyrics_screen.too_short",
                &[l.decimal(edit::MIN_CUE_MS as f64 / 1000.0, 1)],
            );
        }
    }

    /// Join line `i` with the next one, as one line of text (answer Q3).
    /// Returns the line that holds both texts and the character where the
    /// second starts. A line emptied in its field has nothing to join: it goes
    /// (as when it is left empty), and the next line takes its place.
    fn ls_join(&mut self, i: usize) -> Option<(usize, usize)> {
        let emptied = self
            .lyrics_screen
            .draft
            .as_ref()
            .is_some_and(|d| d.cue == i && d.text.trim().is_empty());
        if emptied {
            self.lyrics_screen.draft = None;
            if !self.ls_delete(i) {
                return None;
            }
            // ls_delete moved to the line above: the next line is the one to show.
            let last = self.ls_track()?.cues.len().checked_sub(1)?;
            self.lyrics_screen.cur = i.min(last);
            return Some((i.min(last), 0));
        }
        self.ls_commit_draft();
        let at = self.ls_track()?.cues.get(i)?.text.chars().count() + 1;
        let label = self.locale.language.format(
            "lyrics_screen.did_join",
            &[(i + 1).to_string(), (i + 2).to_string()],
        );
        if !self.edit_track_as(label, |t| edit::join_next_with(t, i, " ")) {
            return None;
        }
        self.lyrics_screen.draft = None;
        self.lyrics_screen.word = None;
        self.lyrics_screen.syl = None;
        Some((i, at))
    }

    /// Delete line `i`, with « Ligne supprimée · Annuler ».
    fn ls_delete(&mut self, i: usize) -> bool {
        let label = self
            .locale
            .language
            .format("lyrics_screen.did_delete", &[(i + 1).to_string()]);
        if !self.edit_track_as(label, |t| edit::delete(t, i).is_some()) {
            return false;
        }
        let s = &mut self.lyrics_screen;
        s.toast = Some(Instant::now() + TOAST);
        s.draft = None;
        s.word = None;
        s.syl = None;
        s.caret = None;
        if s.cur > i || (s.cur == i && i > 0) {
            s.cur -= 1;
        }
        true
    }

    /// A new line in the gap around `ms`; its field takes the keyboard.
    fn ls_add_at(&mut self, ms: u64) -> bool {
        self.ls_leave_row();
        let l = self.locale.language;
        let text = l.text("edit.new_text").to_string();
        let mut at = None;
        self.edit_track_as(l.text("lyrics_screen.did_add").to_string(), |t| {
            at = edit::add_cue(t, ms, &text);
            at.is_some()
        });
        let Some(i) = at else {
            return false;
        };
        self.lyrics_screen.cur = i;
        self.ls_set_cur(i, Some(Some(0)));
        // An empty field over the placeholder text: typing replaces it, and a
        // line left empty goes away.
        self.lyrics_screen.draft = Some(Draft {
            cue: i,
            text: String::new(),
            base: text,
            typed_at: None,
        });
        true
    }

    /// Entrée at the end of line `i`: a new line right after it.
    fn ls_add_after(&mut self, i: usize) {
        let Some(track) = self.ls_track() else {
            return;
        };
        let Some(c) = track.cues.get(i) else {
            return;
        };
        let a = c.end_ms;
        let b = track
            .cues
            .get(i + 1)
            .map_or(track.duration_ms, |n| n.start_ms);
        if b < a + edit::MIN_ADD_GAP_MS || !self.ls_add_at(a + ((b - a) / 2).min(1000)) {
            self.ls_flash("lyrics_screen.no_room", &[]);
        }
    }

    /// Move the start of word `k` of the current line by `delta` ms, by hand.
    fn ls_nudge_word(&mut self, k: usize, delta: i64) {
        self.ls_commit_draft();
        let i = self.lyrics_screen.cur;
        let Some(cue) = self.ls_track().and_then(|t| t.cues.get(i).cloned()) else {
            return;
        };
        if k == 0 {
            self.ls_flash("lyrics_screen.first_word", &[]);
            return;
        }
        let mut probe = cue.clone();
        probe.words = Some(edit::words_or_estimate(&cue));
        let units = edit::units(&probe);
        let Some(unit) = units.iter().position(|u| u.0 == k) else {
            return;
        };
        let to = units[unit].2.saturating_add_signed(delta);
        let word = probe
            .words
            .as_ref()
            .map_or(String::new(), |w| w[k].text.clone());
        let label = self
            .locale
            .language
            .format("lyrics_screen.did_word", &[word]);
        // Presses in a row on the same word are one step.
        self.edit_track_run(
            egui::Id::new(("lyrics-screen-nudge", i, k)).value(),
            label,
            |t| edit::set_unit_start(t, i, unit, to).is_some(),
        );
        self.lyrics_screen.word = Some(k);
    }

    /// The split field for word `k` of the current line: its syllables, or
    /// the language's proposal with where it comes from.
    fn ls_open_syllables(&mut self, k: usize) {
        let i = self.lyrics_screen.cur;
        let Some(word) = self
            .ls_track()
            .and_then(|t| t.cues.get(i).map(edit::words_or_estimate))
            .and_then(|w| w.get(k).cloned())
        else {
            return;
        };
        let (text, note) = match &word.syllables {
            Some(s) => (
                s.iter()
                    .map(|s| s.text.as_str())
                    .collect::<Vec<_>>()
                    .join("-"),
                None,
            ),
            None => {
                let language = self.sung_language();
                let packs = self.language_packs();
                let (parts, source, dictionary) = packs.propose(&word.text, language.as_deref());
                let l = self.locale.language;
                let note = match source {
                    noh::syllables::Source::Script => l.text("words.syl_from_script").to_string(),
                    noh::syllables::Source::Dictionary => l.format(
                        "words.syl_from_dictionary",
                        &[dictionary.unwrap_or_default()],
                    ),
                    noh::syllables::Source::Rule => l.text("words.syl_from_rule").to_string(),
                };
                if parts.len() < 2 {
                    (word.text.clone(), None)
                } else {
                    (parts.join("-"), Some(note))
                }
            }
        };
        self.lyrics_screen.word = None;
        self.lyrics_screen.syl = Some(SylEdit {
            word: k,
            text,
            note,
            error: None,
            focus: true,
        });
    }

    /// Entrée in the split field: hyphens cut, no hyphen joins the word back.
    fn ls_apply_syllables(&mut self) {
        let i = self.lyrics_screen.cur;
        let Some((k, typed)) = self
            .lyrics_screen
            .syl
            .as_ref()
            .map(|s| (s.word, s.text.clone()))
        else {
            return;
        };
        let Some(word) = self
            .ls_track()
            .and_then(|t| t.cues.get(i).map(edit::words_or_estimate))
            .and_then(|w| w.get(k).cloned())
        else {
            self.lyrics_screen.syl = None;
            return;
        };
        let error = match edit::parse_syllables(&typed, &word.text) {
            None => Some("words.syl_letters"),
            Some(p) if p.len() < 2 && word.syllables.is_none() => None,
            Some(p)
                if word.syllables.as_ref().is_some_and(|s| {
                    s.iter()
                        .map(|s| s.text.as_str())
                        .eq(p.iter().map(String::as_str))
                }) =>
            {
                None
            }
            Some(p) => {
                let label = self
                    .locale
                    .language
                    .format("lyrics_screen.did_syllables", &[word.text.clone()]);
                (!self.edit_track_as(label, |t| edit::set_syllables(t, i, k, &p)))
                    .then_some("words.syl_room")
            }
        };
        match (error, &mut self.lyrics_screen.syl) {
            (Some(e), Some(s)) => {
                s.error = Some(e);
                s.focus = true;
            }
            _ => self.lyrics_screen.syl = None,
        }
    }

    /// Once per frame, before anything is drawn: the current line exists, its
    /// draft follows the lyrics, typed text reaches them after a pause, and
    /// the track follows playback.
    fn ls_sync(&mut self, track: &SubtitleTrack, compact: bool) {
        let now = Instant::now();
        let playing = self.mini_playing();
        let ph = self.ls_playhead();
        let duration = self.project.duration_ms;
        let s = &mut self.lyrics_screen;
        s.compact = compact;
        s.cur = s.cur.min(track.cues.len().saturating_sub(1));
        let text = track.cues.get(s.cur).map(|c| c.text.as_str());
        match (&s.draft, text) {
            (Some(d), Some(text)) if d.cue == s.cur && d.base == text => {}
            (_, Some(text)) => {
                s.draft = Some(Draft {
                    cue: s.cur,
                    text: text.to_string(),
                    base: text.to_string(),
                    typed_at: None,
                });
            }
            (_, None) => s.draft = None,
        }
        if s.toast.is_some_and(|t| now >= t) {
            s.toast = None;
        }
        // The game's summary describes the lyrics as the game left them.
        if s.summary
            .is_some_and(|(_, revision)| revision != self.project.edit_revision)
        {
            s.summary = None;
        }
        if s.flash.as_ref().is_some_and(|(_, t)| now >= *t) {
            s.flash = None;
        }
        // The chosen word and the split field belong to words that still exist.
        let words = track
            .cues
            .get(s.cur)
            .map_or(0, |c| c.text.split_whitespace().count());
        if s.word.is_some_and(|k| k >= words) {
            s.word = None;
        }
        if s.syl.as_ref().is_some_and(|e| e.word >= words) {
            s.syl = None;
        }
        let span = s.span(compact).min(duration);
        if playing && s.drag.is_none() && (ph > s.view_start + span * 7 / 10 || ph < s.view_start) {
            s.view_start = ph.saturating_sub(span * 3 / 10);
        }
        s.view_start = s.view_start.min(duration.saturating_sub(span));
        if s.draft
            .as_ref()
            .and_then(|d| d.typed_at)
            .is_some_and(|t| now >= t + TYPING_PAUSE)
        {
            self.ls_commit_draft();
        }
    }

    /// The screen's keys. In a line's field: Entrée cuts or adds, Retour
    /// arrière at the start and Suppr at the end join, ↑ ↓ at a boundary change
    /// line, Alt+← → moves the word under the cursor, Échap leaves the field.
    /// Otherwise: Espace plays or pauses, ← → and Alt+← → act on the chosen
    /// word, Échap closes one layer at a time.
    fn ls_keys(&mut self, ctx: &egui::Context, track: &SubtitleTrack) {
        // During a game every key is the game's, whatever has the focus.
        if self.lyrics_screen.game.is_some() {
            self.ls_game_keys(ctx);
            return;
        }
        if self.lyrics_question_open() || self.lyrics_question_pending() || self.ls_reading() {
            return;
        }
        // Read-only while a job runs: Échap closes, Espace plays or pauses.
        if self.lyrics_locked() {
            if ctx.input_mut(|i| take_key(i, Key::Escape, plain)) {
                self.close_lyrics_screen();
            } else if !ctx.egui_wants_keyboard_input()
                && ctx.memory(|m| m.focused().is_none())
                && ctx.input_mut(|i| take_key(i, Key::Space, plain))
            {
                self.mini_play(ctx);
            }
            return;
        }
        let cur = self.lyrics_screen.cur;
        let count = track.cues.len();
        // Échap, one layer at a time: a drag, the split field, the keyboard
        // focus (a line's field, a button), the chosen word, then the screen.
        if ctx.input_mut(|i| take_key(i, Key::Escape, plain)) {
            let s = &mut self.lyrics_screen;
            if s.drag.is_some() {
                s.drag = None;
                s.preview = None;
            } else if s.syl.is_some() {
                s.syl = None;
            } else if s.had_focus {
                if let Some(id) = ctx.memory(|m| m.focused()) {
                    ctx.memory_mut(|m| m.surrender_focus(id));
                }
            } else if s.word.is_some() {
                s.word = None;
            } else {
                self.close_lyrics_screen();
            }
            return;
        }
        if ctx.memory(|m| m.has_focus(row_id(cur))) {
            let Some(c) = self
                .lyrics_screen
                .caret
                .filter(|c| c.cue == cur && self.lyrics_screen.text_focus)
            else {
                return;
            };
            let length = self
                .lyrics_screen
                .draft
                .as_ref()
                .map_or(0, |d| d.text.chars().count());
            let single = c.kind != CaretKind::Selection;
            let (enter, back, delete, up, down, earlier, later) = ctx.input_mut(|i| {
                (
                    take_key(i, Key::Enter, plain),
                    single && c.index == 0 && cur > 0 && take_key(i, Key::Backspace, plain),
                    single
                        && c.index >= length
                        && cur + 1 < count
                        && take_key(i, Key::Delete, plain),
                    c.top && cur > 0 && take_key(i, Key::ArrowUp, plain),
                    c.bottom && cur + 1 < count && take_key(i, Key::ArrowDown, plain),
                    take_key(i, Key::ArrowLeft, alt_only),
                    take_key(i, Key::ArrowRight, alt_only),
                )
            });
            if enter {
                match c.kind {
                    CaretKind::Cut(k) => self.ls_cut(k),
                    CaretKind::End => self.ls_add_after(cur),
                    CaretKind::Start => self.ls_flash("lyrics_screen.cut_between", &[]),
                    CaretKind::Empty | CaretKind::Selection => {}
                }
            } else if back {
                let empty = self
                    .lyrics_screen
                    .draft
                    .as_ref()
                    .is_some_and(|d| d.text.trim().is_empty());
                if empty {
                    // The empty line goes; the cursor ends the line above.
                    self.ls_set_cur(cur - 1, Some(None));
                } else if let Some((line, at)) = self.ls_join(cur - 1) {
                    self.ls_set_cur(line, Some(Some(at)));
                }
            } else if delete {
                // The cursor stays where it was: at the seam.
                if let Some((line, at)) = self.ls_join(cur) {
                    self.ls_set_cur(line, Some(Some(at.saturating_sub(1).min(c.index))));
                }
            } else if up || down {
                let to = if up { cur - 1 } else { cur + 1 };
                let limit = track.cues[to].text.chars().count();
                self.ls_set_cur(to, Some(Some(c.index.min(limit))));
            } else if earlier || later {
                let k = self
                    .lyrics_screen
                    .draft
                    .as_ref()
                    .map_or(0, |d| word_under(&d.text, c.index));
                self.ls_nudge_word(k, if later { NUDGE_MS } else { -NUDGE_MS });
            }
            return;
        }
        // Another text field (the split field) has its own keys.
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let focus_free = ctx.memory(|m| m.focused().is_none());
        if let Some(k) = self.lyrics_screen.word
            && focus_free
        {
            let (earlier, later, left, right) = ctx.input_mut(|i| {
                (
                    take_key(i, Key::ArrowLeft, alt_only),
                    take_key(i, Key::ArrowRight, alt_only),
                    take_key(i, Key::ArrowLeft, plain),
                    take_key(i, Key::ArrowRight, plain),
                )
            });
            let words = track
                .cues
                .get(cur)
                .map_or(0, |c| c.text.split_whitespace().count());
            if earlier || later {
                self.ls_nudge_word(k, if later { NUDGE_MS } else { -NUDGE_MS });
            } else if left {
                self.lyrics_screen.word = Some(k.saturating_sub(1));
            } else if right && k + 1 < words {
                self.lyrics_screen.word = Some(k + 1);
            }
        }
        if focus_free && ctx.input_mut(|i| take_key(i, Key::Space, plain)) {
            self.mini_play(ctx);
        }
    }

    /// The whole screen, in what `ui` has left under the application's header.
    pub(super) fn lyrics_screen_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(track) = self.ls_track() else {
            // The lyrics went away (their file was removed, or can no longer
            // be read): nothing to edit here, the main screen says why.
            self.close_lyrics_screen();
            return;
        };
        self.lyrics_screen.shown = Some(track.clone());
        let full = ui.available_rect_before_wrap();
        let compact = full.width() < style::COMPACT_BELOW;
        self.ls_sync(&track, compact);
        self.ls_lock_frame();
        self.ls_game_frame(&ctx);
        self.ls_keys(&ctx, &track);
        if !self.lyrics_screen.open {
            return;
        }
        // Keys may have edited the lyrics.
        let Some(track) = self.ls_track() else {
            return;
        };
        let t = style::tokens(ui);
        let pad = if compact { space::L } else { space::XL };
        ui.painter().rect_filled(full, 0, t.bg);
        let bar = Rect::from_min_size(full.min, vec2(full.width(), MODEBAR_H));
        // The narrow window wraps the footer: it is as high as its text was
        // last frame (a word chosen, or a word's split, need three rows). So
        // does the wide one for a text one row cannot hold (the first
        // instructions of a game, in most languages).
        let foot_h = if compact || self.lyrics_screen.foot_wraps {
            self.lyrics_screen.foot_h.max(FOOT_H)
        } else {
            FOOT_H
        };
        let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - foot_h), full.max);
        let body = Rect::from_min_max(bar.left_bottom(), foot.right_top());
        self.ls_modebar(ui, bar, pad, compact, &track);
        let mut body_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-body")
                .max_rect(body)
                .layout(Layout::top_down(Align::Min)),
        );
        body_ui.set_clip_rect(body);
        egui::ScrollArea::vertical()
            .id_salt("lyrics-screen-scroll")
            .auto_shrink([false, false])
            .show(&mut body_ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.ls_body(ui, &track, pad, compact, body.height());
            });
        self.ls_footer(ui, foot, pad, &track);
        self.ls_toasts(&ctx, foot);
        if self
            .lyrics_screen
            .draft
            .as_ref()
            .is_some_and(|d| d.typed_at.is_some())
        {
            ctx.request_repaint_after(TYPING_PAUSE);
        }
        self.lyrics_screen.had_focus = ctx.memory(|m| m.focused().is_some());
        if self.lyrics_screen.toast.is_some() || self.lyrics_screen.flash.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Title, the count of lines and the song, undo and redo naming what they
    /// would do, « Terminé »; under it, the outside-change notice.
    fn ls_modebar(
        &mut self,
        ui: &mut egui::Ui,
        bar: Rect,
        pad: f32,
        compact: bool,
        track: &SubtitleTrack,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        ui.painter().line_segment(
            [bar.left_bottom(), bar.right_bottom()],
            Stroke::new(1.0, t.border),
        );
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-bar")
                .max_rect(bar.shrink2(vec2(pad, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
        );
        let ui = &mut row;
        let _title = ui.label(
            style::strong(l.text("lyrics_screen.title"), style::text::HEADING).color(t.text),
        );
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.title = Some(_title.rect);
        }
        if !compact {
            let song = self
                .wav
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            let lines = l.plural("lyrics_screen.lines", track.cues.len());
            let meta = match song {
                Some(song) => format!("{lines} · {song}"),
                None => lines,
            };
            ui.add_space(space::S);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(meta)
                        .size(style::text::SMALL)
                        .color(t.text_2),
                )
                .truncate(),
            );
        }
        let (undo, redo) = {
            let (u, r) = self.history_labels();
            (u.map(str::to_string), r.map(str::to_string))
        };
        let hover = |what: &Option<String>, plain: &str, named: &str, none: &str| match what {
            None => l.text(none).to_string(),
            Some(w) if w.is_empty() => l.text(plain).to_string(),
            Some(w) => l.format(named, &[w.clone()]),
        };
        // During a game the text, the undo arrows and « Terminé » wait.
        // While a job runs, so do the arrows; « Terminé » stays.
        let game = self.lyrics_screen.game.is_some();
        let held = game || self.lyrics_locked();
        let (undo, redo) = (undo.filter(|_| !held), redo.filter(|_| !held));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let done = style::button(ui, l.text("lyrics_screen.done"), !game, true, 0.0);
            #[cfg(test)]
            {
                self.lyrics_screen.drawn.done = Some(done.rect);
            }
            if done.clicked() {
                self.close_lyrics_screen();
            }
            ui.add_space(space::XS);
            let name = hover(
                &redo,
                "lyrics_screen.redo",
                "lyrics_screen.redo_what",
                "lyrics_screen.redo_none",
            );
            let r = ui
                .add_enabled_ui(redo.is_some(), |ui| {
                    style::icon_button(ui, app_icons::Icon::Redo, &name, style::CONTROL_H)
                })
                .inner;
            if r.clicked() {
                self.ls_commit_draft();
                self.redo();
            }
            let name = hover(
                &undo,
                "lyrics_screen.undo",
                "lyrics_screen.undo_what",
                "lyrics_screen.undo_none",
            );
            let u = ui
                .add_enabled_ui(undo.is_some(), |ui| {
                    style::icon_button(ui, app_icons::Icon::Undo, &name, style::CONTROL_H)
                })
                .inner;
            #[cfg(test)]
            {
                self.lyrics_screen.drawn.undo = Some(u.rect);
            }
            if u.clicked() {
                self.ls_commit_draft();
                self.undo();
                self.lyrics_screen.toast = None;
            }
        });
    }

    /// Under the title: the monitor with its transport beside the text (one
    /// above the other in the narrow window), then the sync bar and the track.
    fn ls_body(
        &mut self,
        ui: &mut egui::Ui,
        track: &Arc<SubtitleTrack>,
        pad: f32,
        compact: bool,
        available: f32,
    ) {
        let l = self.locale.language;
        let width = ui.available_width() - 2.0 * pad;
        let left = ui.max_rect().left() + pad;
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.read_only = None;
        }
        let mut notices = 0.0;
        if self
            .project
            .edit
            .as_ref()
            .is_some_and(|s| s.outside_changed)
        {
            ui.add_space(space::S);
            notices += space::S + 36.0;
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
            let mut banner = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("lyrics-screen-outside")
                    .max_rect(rect.shrink2(vec2(pad, 0.0))),
            );
            style::message(
                &mut banner,
                style::Severity::Warning,
                l.text("edit.outside"),
                true,
            );
        }
        if self.lyrics_locked() {
            ui.add_space(space::S);
            notices += space::S + 36.0;
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::hover());
            let mut banner = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("lyrics-screen-read-only")
                    .max_rect(rect.shrink2(vec2(pad, 0.0))),
            );
            style::message(
                &mut banner,
                style::Severity::Info,
                l.text("lyrics_screen.read_only"),
                true,
            );
            #[cfg(test)]
            {
                self.lyrics_screen.drawn.read_only = Some(rect);
            }
        }
        let wave_h = if compact { 48.0 } else { 64.0 };
        let lanes_h = 20.0 + 18.0 + 4.0 + wave_h + 4.0 + LANE_H;
        let track_h = 16.0 + 40.0 + space::S + lanes_h + space::M;
        ui.add_space(space::M);
        let game = self.lyrics_screen.game.is_some();
        // The narrow window's game: the prompt, then its buttons under it.
        let bar_h = if compact && game {
            self.lyrics_screen.prompt_h + 40.0
        } else {
            40.0
        };
        if compact && game {
            // Nothing scrolls while Espace is held: the line to sing, the
            // track and the footer must fit at once, so the monitor and the
            // text go (the prompt shows the words, and the text cannot be
            // edited meanwhile). Playback does not need the monitor painted:
            // measured with libmpv over 100 s of a game, no picture taken, the
            // song's clock never stopped.
        } else if compact {
            let aspect = self.canvas_aspect();
            let monitor_h = (width / aspect.max(16.0 / 9.0)).min(218.0);
            let (rect, _) = ui.allocate_exact_size(
                vec2(
                    ui.available_width(),
                    monitor_h + space::S + style::CONTROL_H,
                ),
                Sense::hover(),
            );
            let column = Rect::from_min_size(pos2(left, rect.top()), vec2(width, rect.height()));
            self.ls_monitor(ui, column, track);
            ui.add_space(space::M);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), 236.0), Sense::hover());
            let doc = Rect::from_min_size(pos2(left, rect.top()), vec2(width, rect.height()));
            self.ls_text(ui, doc, track);
        } else {
            // What a notice above takes is not the track's to lose.
            let top_h = (available - notices - space::M - track_h).max(240.0);
            let (rect, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), top_h), Sense::hover());
            let column_w = 480.0_f32.min((width - space::L) * 0.52);
            let column = Rect::from_min_size(pos2(left, rect.top()), vec2(column_w, top_h));
            self.ls_monitor(ui, column, track);
            let doc = Rect::from_min_max(
                pos2(column.right() + space::L, rect.top()),
                pos2(left + width, rect.bottom()),
            );
            self.ls_text(ui, doc, track);
        }
        ui.add_space(if compact && game { 0.0 } else { 16.0 });
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), bar_h), Sense::hover());
        let bar = Rect::from_min_size(pos2(left, rect.top()), vec2(width, bar_h));
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.sync = Some(bar);
        }
        self.ls_syncbar(ui, bar, compact);
        ui.add_space(space::S);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), lanes_h), Sense::hover());
        let lanes = Rect::from_min_size(pos2(left, rect.top()), vec2(width, lanes_h));
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.lanes = Some(lanes);
        }
        self.ls_lanes(ui, lanes, track, compact, wave_h);
        ui.add_space(space::M);
    }

    /// The monitor (with the current line even when the captions switch is
    /// off, answer Q2) and, under it, the time and play or pause.
    fn ls_monitor(&mut self, ui: &mut egui::Ui, column: Rect, track: &SubtitleTrack) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let aspect = self.canvas_aspect();
        let room = (column.height() - space::S - style::CONTROL_H).max(60.0);
        let height = room.min(column.width() / aspect);
        let width = (height * aspect).min(column.width());
        let monitor = Rect::from_min_size(
            pos2(column.center().x - width / 2.0, column.top()),
            vec2(width, height),
        );
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.monitor = Some(monitor);
        }
        let painted = self.paint_monitor(ui, monitor);
        if !self.project.apply_subtitles {
            let ph = self.ls_playhead();
            let painter = ui.painter_at(monitor);
            if let Some(cue) = track
                .cues
                .iter()
                .find(|c| c.start_ms <= ph && ph < c.end_ms)
            {
                let size = (monitor.height() * 0.063).clamp(12.0, 20.0);
                let mut job = egui::text::LayoutJob::simple(
                    cue.text.clone(),
                    style::semibold(size),
                    Color32::WHITE,
                    monitor.width() * 0.82,
                );
                job.halign = Align::Center;
                let galley = painter.layout_job(job);
                let at = pos2(
                    monitor.center().x,
                    monitor.bottom() - monitor.height() * 0.08 - galley.size().y,
                );
                for d in [
                    vec2(0.0, 1.5),
                    vec2(1.0, 0.0),
                    vec2(-1.0, 0.0),
                    vec2(0.0, -1.0),
                ] {
                    painter.galley_with_override_text_color(
                        at + d,
                        galley.clone(),
                        Color32::from_black_alpha(200),
                    );
                }
                painter.galley(at, galley, Color32::WHITE);
            }
            // The switch decides the export: say that these lines are not in it.
            let galley = painter.layout_no_wrap(
                l.text("lyrics_screen.not_on_video").to_string(),
                FontId::proportional(style::text::SMALL),
                Color32::WHITE,
            );
            let chip = Rect::from_min_size(
                monitor.min + vec2(8.0, 8.0),
                galley.size() + vec2(24.0, 6.0),
            );
            painter.rect_filled(chip, style::radius::S, Color32::from_black_alpha(160));
            painter.circle_filled(
                pos2(chip.left() + 9.0, chip.center().y),
                3.0,
                Color32::from_rgb(0xE9, 0xC4, 0x6A),
            );
            painter.galley(chip.min + vec2(18.0, 3.0), galley, Color32::WHITE);
        }
        let row = Rect::from_min_size(
            pos2(monitor.left(), monitor.bottom() + space::S),
            vec2(monitor.width(), style::CONTROL_H),
        );
        let duration = self.project.duration_ms;
        let ph = self.ls_playhead();
        let clock = |ms| app_timeline::clock_floor(ms, duration, l);
        let painter = ui.painter();
        let now =
            painter.layout_no_wrap(clock(ph), FontId::proportional(style::text::BODY), t.text);
        let total = painter.layout_no_wrap(
            format!(" / {}", clock(duration)),
            FontId::proportional(style::text::BODY),
            t.text_2,
        );
        // The widest time the song can show, so the button never moves.
        let widest = painter
            .layout_no_wrap(
                clock(duration.max(599_900)),
                FontId::proportional(style::text::BODY),
                t.text,
            )
            .size()
            .x;
        let y = row.center().y - now.size().y / 2.0;
        painter.galley(pos2(row.left(), y), now, t.text);
        let total_w = total.size().x;
        painter.galley(pos2(row.left() + widest, y), total, t.text_2);
        let button = Rect::from_center_size(
            pos2(
                row.left() + widest + total_w + space::M + 16.0,
                row.center().y,
            ),
            vec2(style::CONTROL_H, style::CONTROL_H),
        );
        let name = l.text(if painted.playing {
            "preview.pause"
        } else {
            "preview.play"
        });
        // The game plays and pauses the song itself: the button waits.
        let enabled = self.lyrics_screen.game.is_none();
        let response = ui.interact(
            button,
            ui.id().with("lyrics-screen-play"),
            if enabled {
                Sense::click()
            } else {
                Sense::hover()
            },
        );
        let response = if enabled {
            response.on_hover_text(name)
        } else {
            response
        };
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, name));
        ui.painter().circle_filled(
            button.center(),
            16.0,
            if response.hovered() && enabled {
                t.surface_3
            } else {
                t.surface_2
            },
        );
        app_icons::paint(
            ui.painter(),
            Rect::from_center_size(button.center(), vec2(18.0, 18.0)),
            if painted.playing {
                app_icons::Icon::Pause
            } else {
                app_icons::Icon::Play
            },
            if enabled { t.text } else { t.text_3 },
        );
        style::focus(ui, &response);
        if response.clicked() && enabled {
            self.mini_play(ui.ctx());
        }
    }

    /// The lyrics as a text: one row per line, its time (a click plays from
    /// there), the silences named, and the current line as a field with its
    /// three actions under it.
    fn ls_text(&mut self, ui: &mut egui::Ui, doc: Rect, track: &Arc<SubtitleTrack>) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let ctx = ui.ctx().clone();
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.text = Some(doc);
        }
        ui.painter().rect_filled(doc, style::radius::L, t.surface);
        let head = Rect::from_min_size(doc.min + vec2(12.0, 8.0), vec2(doc.width() - 24.0, 20.0));
        let title = ui.painter().layout_no_wrap(
            l.text("lyrics_screen.text").to_string(),
            style::semibold(style::text::SMALL),
            t.text_2,
        );
        let title_w = title.size().x;
        ui.painter().galley(head.min, title, t.text_2);
        ui.painter().with_clip_rect(head).text(
            head.min + vec2(title_w + space::S, 0.0),
            Align2::LEFT_TOP,
            l.text(if self.lyrics_screen.game.is_some() {
                "lyrics_screen.text_waits"
            } else if self.lyrics_locked() {
                "lyrics_screen.text_locked"
            } else {
                "lyrics_screen.text_hint"
            }),
            FontId::proportional(style::text::SMALL),
            t.text_3,
        );
        let list = Rect::from_min_max(
            pos2(doc.left() + 6.0, head.bottom() + 4.0),
            pos2(doc.right() - 2.0, doc.bottom() - 6.0),
        );
        let mut inner = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-text")
                .max_rect(list)
                .layout(Layout::top_down(Align::Min)),
        );
        inner.set_clip_rect(list);
        let ph = self.ls_playhead();
        let playing_row = track
            .cues
            .iter()
            .position(|c| c.start_ms <= ph && ph < c.end_ms);
        let cur = self.lyrics_screen.cur;
        // During a game, and while a job locks the lyrics, the lines are read
        // and not written: no field, no row buttons.
        let game = self.lyrics_screen.game.is_some() || self.lyrics_locked();
        // What the rows ask for, applied after they are drawn.
        enum Ask {
            Choose(usize, usize),
            Listen(usize),
            Cut(usize),
            Join,
            Delete,
        }
        let mut ask = None;
        let mut left = None;
        let mut draft = self.lyrics_screen.draft.take();
        #[cfg(test)]
        self.lyrics_screen.drawn_rows.clear();
        egui::ScrollArea::vertical()
            .id_salt("lyrics-screen-rows")
            .auto_shrink([false, false])
            .show(&mut inner, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let width = ui.available_width();
                let text_w = (width - TIME_W - 6.0 - 12.0).max(40.0);
                let mut previous = 0;
                for (i, cue) in track.cues.iter().enumerate() {
                    if cue.start_ms >= previous + GAP_NOTE_MS {
                        let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
                        if ui.is_rect_visible(rect) {
                            let y = rect.center().y;
                            let x = rect.left() + TIME_W + 4.0;
                            ui.painter().extend(egui::Shape::dashed_line(
                                &[pos2(x, y), pos2(x + 16.0, y)],
                                Stroke::new(1.0, t.border_strong),
                                3.0,
                                3.0,
                            ));
                            let seconds = l.decimal((cue.start_ms - previous) as f64 / 1000.0, 1);
                            ui.painter().text(
                                pos2(x + 24.0, y),
                                Align2::LEFT_CENTER,
                                l.format("lyrics_screen.gap", &[seconds]),
                                FontId::proportional(style::text::SMALL),
                                t.text_3,
                            );
                        }
                    }
                    previous = cue.end_ms;
                    let synced = cue
                        .words
                        .iter()
                        .flatten()
                        .any(|w| w.source == WordSource::Hand);
                    let playing = playing_row == Some(i);
                    // The time, right-aligned in its column; ✓ once a word of
                    // the line was placed by hand.
                    let time = |ui: &mut egui::Ui, rect: Rect, hovered: bool| {
                        let color = if playing {
                            t.accent_text
                        } else if hovered {
                            t.text
                        } else {
                            t.text_3
                        };
                        let font = if playing {
                            style::semibold(style::text::SMALL)
                        } else {
                            FontId::proportional(style::text::SMALL)
                        };
                        let right = rect.right() - if synced { 12.0 } else { 0.0 };
                        ui.painter().text(
                            pos2(right, rect.top() + ROW_H / 2.0),
                            Align2::RIGHT_CENTER,
                            app_timeline::clock(cue.start_ms, track.duration_ms, l)
                                .rsplit_once([',', '.'])
                                .map_or(String::new(), |(a, _)| a.to_string()),
                            font,
                            color,
                        );
                        if synced {
                            app_icons::paint(
                                ui.painter(),
                                Rect::from_center_size(
                                    pos2(rect.right() - 5.0, rect.top() + ROW_H / 2.0),
                                    vec2(10.0, 10.0),
                                ),
                                app_icons::Icon::Check,
                                t.accent_text,
                            );
                        }
                    };
                    let listen = l.format(
                        "lyrics_screen.listen",
                        &[app_timeline::clock(cue.start_ms, track.duration_ms, l)],
                    );
                    if i != cur || game {
                        let galley = ui.painter().layout(
                            cue.text.clone(),
                            FontId::proportional(style::text::BODY),
                            t.text_2,
                            text_w,
                        );
                        let height = (galley.size().y + 8.0).max(ROW_H);
                        let (rect, row) =
                            ui.allocate_exact_size(vec2(width, height), Sense::click());
                        #[cfg(test)]
                        self.lyrics_screen.drawn_rows.push(rect);
                        let time_rect = Rect::from_min_size(
                            rect.min + vec2(4.0, 0.0),
                            vec2(TIME_W - 8.0, ROW_H),
                        );
                        let listen_at = ui
                            .interact(time_rect, ui.id().with(("listen", i)), Sense::click())
                            .on_hover_text(&listen);
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        row.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &cue.text)
                        });
                        if i == cur {
                            // The line the game is on.
                            ui.painter().rect_filled(rect, style::radius::M, t.seg_on);
                            if std::mem::take(&mut self.lyrics_screen.scroll_to_cur) {
                                ui.scroll_to_rect(rect.expand2(vec2(0.0, 36.0)), None);
                            }
                        } else if (row.hovered() || row.has_focus()) && !game {
                            ui.painter()
                                .rect_filled(rect, style::radius::M, t.surface_2);
                        }
                        style::focus(ui, &row);
                        time(ui, time_rect, listen_at.hovered());
                        let text_at = rect.min + vec2(TIME_W + 6.0, 4.0);
                        if listen_at.clicked() {
                            ask = Some(Ask::Listen(i));
                        } else if row.clicked() {
                            // The cursor goes where the click was.
                            let index = row
                                .interact_pointer_pos()
                                .map_or(cue.text.chars().count(), |p| {
                                    galley.cursor_from_pos(p - text_at).index.into()
                                });
                            ask = Some(Ask::Choose(i, index));
                        }
                        ui.painter().galley(text_at, galley, t.text_2);
                        continue;
                    }
                    // The current line: a field.
                    let Some(d) = draft.as_mut().filter(|d| d.cue == i) else {
                        continue;
                    };
                    let id = row_id(i);
                    if let Some((_, caret)) = self.lyrics_screen.focus.take_if(|f| f.0 == i) {
                        ui.memory_mut(|m| m.request_focus(id));
                        let at = caret.unwrap_or(usize::MAX).min(d.text.chars().count());
                        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
                        state
                            .cursor
                            .set_char_range(Some(egui::text::CCursorRange::one(
                                egui::text::CCursor::new(at),
                            )));
                        // Another line had this place before a cut or a join.
                        state.clear_undoer();
                        egui::TextEdit::store_state(&ctx, id, state);
                    }
                    let focused = ui.memory(|m| m.has_focus(id));
                    let background = ui.painter().add(egui::Shape::Noop);
                    let row = ui
                        .horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            let (time_rect, listen_at) =
                                ui.allocate_exact_size(vec2(TIME_W, ROW_H), Sense::click());
                            let listen_at = listen_at.on_hover_text(&listen);
                            let time_rect = Rect::from_min_size(
                                time_rect.min + vec2(4.0, 0.0),
                                vec2(TIME_W - 8.0, ROW_H),
                            );
                            time(ui, time_rect, listen_at.hovered());
                            if listen_at.clicked() {
                                ask = Some(Ask::Listen(i));
                            }
                            ui.add_space(2.0);
                            // The text sits where the other rows paint theirs.
                            let frame =
                                egui::Frame::NONE.inner_margin(egui::Margin::symmetric(4, 4));
                            let frame = if focused {
                                frame
                                    .fill(t.input)
                                    .stroke(Stroke::new(1.0, t.border_strong))
                                    .corner_radius(style::radius::S)
                            } else {
                                frame
                            };
                            egui::TextEdit::multiline(&mut d.text)
                                .id(id)
                                .desired_rows(1)
                                .desired_width(text_w + 8.0)
                                .frame(frame)
                                .text_color(t.text)
                                .hint_text(l.text("lyrics_screen.type_here"))
                                .return_key(Some(egui::KeyboardShortcut::new(
                                    Modifiers::SHIFT,
                                    Key::Enter,
                                )))
                                .show(ui)
                        })
                        .inner;
                    let field = &row.response.response;
                    let rect = Rect::from_min_max(
                        pos2(ui.max_rect().left(), field.rect.top()),
                        pos2(
                            ui.max_rect().left() + width,
                            field.rect.bottom().max(field.rect.top() + ROW_H),
                        ),
                    );
                    ui.painter().set(
                        background,
                        egui::Shape::rect_filled(rect, style::radius::M, t.seg_on),
                    );
                    if field.changed() {
                        d.typed_at = Some(Instant::now());
                        self.lyrics_screen.word = None;
                    }
                    self.lyrics_screen.text_focus = field.has_focus();
                    if field.has_focus()
                        && let Some(range) = row.cursor_range
                    {
                        let rows = row.galley.rows.len().max(1);
                        let at = row.galley.layout_from_cursor(range.primary).row;
                        self.lyrics_screen.caret = Some(Caret {
                            cue: i,
                            index: range.primary.index.into(),
                            kind: caret_kind(
                                &d.text,
                                range.primary.index.into(),
                                !range.is_empty(),
                            ),
                            top: at == 0,
                            bottom: at + 1 >= rows,
                        });
                    }
                    if field.lost_focus() {
                        left = Some(i);
                    }
                    // Couper ici · Joindre à la suivante · Supprimer.
                    let cut = self
                        .lyrics_screen
                        .caret
                        .filter(|c| c.cue == i)
                        .and_then(|c| match c.kind {
                            CaretKind::Cut(k) => Some(k),
                            _ => None,
                        })
                        .filter(|k| *k < d.text.split_whitespace().count());
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.add_space(TIME_W + 2.0);
                        let b = small_button(
                            ui,
                            l.text("lyrics_screen.cut"),
                            cut.is_some(),
                            style::Kind::Quiet,
                        );
                        #[cfg(test)]
                        {
                            self.lyrics_screen.drawn.cut = Some(b.rect);
                        }
                        match cut {
                            Some(k) => {
                                let word = d
                                    .text
                                    .split_whitespace()
                                    .nth(k)
                                    .unwrap_or_default()
                                    .to_string();
                                if b.on_hover_text(l.format("lyrics_screen.cut_tip", &[word]))
                                    .clicked()
                                {
                                    ask = Some(Ask::Cut(k));
                                }
                            }
                            None => {
                                b.on_disabled_hover_text(l.text("lyrics_screen.cut_place"));
                            }
                        }
                        let join = small_button(
                            ui,
                            l.text("lyrics_screen.join"),
                            i + 1 < track.cues.len(),
                            style::Kind::Quiet,
                        );
                        let delete =
                            small_button(ui, l.text("edit.delete"), true, style::Kind::Quiet);
                        #[cfg(test)]
                        {
                            self.lyrics_screen.drawn.join = Some(join.rect);
                            self.lyrics_screen.drawn.delete = Some(delete.rect);
                        }
                        if join.clicked() {
                            ask = Some(Ask::Join);
                        }
                        if delete.clicked() {
                            ask = Some(Ask::Delete);
                        }
                    });
                    ui.add_space(4.0);
                    #[cfg(test)]
                    self.lyrics_screen.drawn_rows.push(rect);
                    if std::mem::take(&mut self.lyrics_screen.scroll_to_cur) {
                        ui.scroll_to_rect(rect.expand2(vec2(0.0, 36.0)), None);
                    }
                }
            });
        self.lyrics_screen.draft = draft;
        // The keyboard left the field (a click elsewhere): the text is kept and
        // an emptied line goes, unless that click asks for something itself
        // (« Supprimer » on an emptied line must not delete two).
        if left == Some(self.lyrics_screen.cur) {
            self.lyrics_screen.leave_pending = true;
        }
        if self.ls_reading() || game {
            // Nothing can be written for a moment (or a game runs): what was
            // asked is dropped, what was typed stays in its field.
            return;
        }
        if self.lyrics_screen.text_focus {
            self.lyrics_screen.summary = None;
        }
        if ask.is_some() || self.lyrics_screen.text_focus {
            self.lyrics_screen.leave_pending = false;
        } else if self.lyrics_screen.leave_pending && !ctx.input(|i| i.pointer.any_down()) {
            self.lyrics_screen.leave_pending = false;
            self.ls_leave_row();
            self.end_edit_run();
        }
        match ask {
            Some(Ask::Choose(i, index)) => self.ls_set_cur(i, Some(Some(index))),
            Some(Ask::Listen(i)) => {
                self.ls_set_cur(i, None);
                if let Some(start) = self
                    .ls_track()
                    .and_then(|t| t.cues.get(self.lyrics_screen.cur).map(|c| c.start_ms))
                {
                    self.mini_play_from(start, &ctx);
                }
            }
            Some(Ask::Cut(k)) => self.ls_cut(k),
            Some(Ask::Join) => {
                let i = self.lyrics_screen.cur;
                if let Some((line, at)) = self.ls_join(i) {
                    self.ls_set_cur(line, Some(Some(at)));
                }
            }
            Some(Ask::Delete) => {
                let i = self.lyrics_screen.cur;
                self.ls_delete(i);
            }
            None => {}
        }
    }

    /// Above the track: what it shows, and what the sync game will take.
    fn ls_syncbar(&mut self, ui: &mut egui::Ui, bar: Rect, compact: bool) {
        if self.lyrics_screen.game.is_some() {
            self.ls_game_syncbar(ui, bar, compact);
            return;
        }
        let l = self.locale.language;
        let t = style::tokens(ui);
        let ctx = ui.ctx().clone();
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-sync")
                .max_rect(bar)
                .layout(Layout::left_to_right(Align::Center)),
        );
        let ui = &mut row;
        if !compact {
            ui.label(style::strong(l.text("lyrics_screen.sync"), style::text::BODY).color(t.text));
            ui.add_space(space::XS);
        }
        let levels = [Level::Line, Level::Word, Level::Syllable];
        let selected = levels
            .iter()
            .position(|x| *x == self.lyrics_screen.level)
            .unwrap_or(0);
        if let Some(k) = style::segmented(
            ui,
            "lyrics-screen-level",
            &[
                l.text("lyrics_screen.level_lines"),
                l.text("lyrics_screen.level_words"),
                l.text("lyrics_screen.level_syllables"),
            ],
            selected,
        ) {
            self.lyrics_screen.level = levels[k];
            self.lyrics_screen.word = None;
            self.lyrics_screen.syl = None;
            self.lyrics_screen.summary = None;
        }
        // « Commencer à la ligne N »: the game goes from the current line on.
        let ready = self.project.duration_ms > 0
            && self.ls_track().is_some_and(|t| !t.cues.is_empty())
            && !self.ls_reading()
            && !self.lyrics_locked();
        let label = if compact {
            l.text("lyrics_screen.start").to_string()
        } else {
            l.format(
                "lyrics_screen.start_at",
                &[(self.lyrics_screen.cur + 1).to_string()],
            )
        };
        let start = ui
            .with_layout(Layout::right_to_left(Align::Center), |ui| {
                style::button(ui, label, ready, true, 0.0)
            })
            .inner;
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.start = Some(start.rect);
        }
        if start.clicked() {
            self.ls_start_game(&ctx);
        }
    }

    /// The zoomed track: ruler (a click moves the playhead), a thin pictures
    /// lane, the waveform, and the lines as blocks of words or syllables. The
    /// current line's edges, body and word starts drag; « + » adds in a gap.
    fn ls_lanes(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        track: &Arc<SubtitleTrack>,
        compact: bool,
        wave_h: f32,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let Some(view) = self.ls_view(compact) else {
            return;
        };
        let ruler = Rect::from_min_size(rect.min, vec2(rect.width(), 20.0));
        let pictures = Rect::from_min_size(ruler.left_bottom(), vec2(rect.width(), 18.0));
        let wave = Rect::from_min_size(
            pictures.left_bottom() + vec2(0.0, 4.0),
            vec2(rect.width(), wave_h),
        );
        let lane = Rect::from_min_size(
            wave.left_bottom() + vec2(0.0, 4.0),
            vec2(rect.width(), LANE_H),
        );
        #[cfg(test)]
        {
            let d = &mut self.lyrics_screen.drawn;
            (d.lane, d.ruler, d.view) = (Some(lane), Some(ruler), Some(view));
        }
        let visuals = self.project_visuals();
        app_timeline::paint_zoom_lanes(
            ui,
            &mut self.lyrics_screen.wave,
            [ruler, pictures, wave],
            view,
            self.project.duration_ms,
            &visuals,
            self.project.waveform.as_deref(),
            compact,
            l,
        );
        // The ruler seeks: the only place of the track that moves the playhead.
        let seek = ui.interact(
            ruler,
            ui.id().with("lyrics-screen-ruler"),
            Sense::click_and_drag(),
        );
        let game = self.lyrics_screen.game.is_some();
        if (seek.is_pointer_button_down_on() || seek.clicked())
            && !game
            && let Some(p) = seek.interact_pointer_pos()
        {
            self.navigate(time_at(view, ruler, p.x), false);
        }
        // During a game the lane only shows the guides and the takes; while a
        // job locks the lyrics it shows them and takes no gesture.
        let locked = self.lyrics_locked();
        let event = if game || locked {
            self.lyrics_screen.drag = None;
            self.lyrics_screen.preview = None;
            None
        } else {
            self.ls_lane_gestures(ui, lane, view, track)
        };
        let shown = self.lyrics_screen.preview.clone();
        let shown = shown.as_ref().unwrap_or(track);
        let cur = self.lyrics_screen.cur;
        let syllables = self.lyrics_screen.level == Level::Syllable;
        let painter = ui.painter_at(rect);
        painter.rect_filled(lane, style::radius::S, t.surface);
        let first = shown.cues.partition_point(|c| c.end_ms <= view.0.start_ms);
        for (i, cue) in shown
            .cues
            .iter()
            .enumerate()
            .skip(first)
            .take_while(|(_, c)| c.start_ms < view.0.end_ms)
        {
            let (a, b) = (x_at(view, lane, cue.start_ms), x_at(view, lane, cue.end_ms));
            let block = Rect::from_min_max(
                pos2(a, lane.top() + 6.0),
                pos2((b - 1.0).max(a + 3.0), lane.bottom() - 6.0),
            );
            let current = i == cur;
            if game {
                painter.rect_filled(block, style::radius::M, t.surface_2);
                if current {
                    painter.rect_stroke(
                        block,
                        style::radius::M,
                        Stroke::new(1.0, t.border_strong),
                        egui::StrokeKind::Inside,
                    );
                }
                self.ls_game_block(&painter, lane, view, i, block);
                continue;
            }
            painter.rect_filled(
                block,
                style::radius::M,
                if current { t.seg_on } else { t.surface_2 },
            );
            let ink = if current { t.text } else { t.text_2 };
            let inside = painter.with_clip_rect(block.intersect(lane));
            let units = shown_units(cue, syllables);
            for (n, u) in units.iter().enumerate() {
                let (ua, ub) = (x_at(view, lane, u.start), x_at(view, lane, u.end));
                if ub < lane.left() || ua > lane.right() {
                    continue;
                }
                let cell = Rect::from_min_max(
                    pos2(ua, block.top()),
                    pos2(ub.max(ua + 1.0), block.bottom()),
                );
                if current && !syllables && self.lyrics_screen.word == Some(u.word) {
                    inside.rect_filled(cell, 0, t.surface_3);
                    inside.rect_filled(
                        Rect::from_min_max(pos2(cell.left(), cell.bottom() - 3.0), cell.max),
                        0,
                        t.text,
                    );
                }
                // A silence after the unit, when its end is known.
                let next = units.get(n + 1).map_or(cue.end_ms, |x| x.start);
                if next > u.end + 60 {
                    let gap = Rect::from_min_max(
                        pos2(ub, block.top()),
                        pos2(x_at(view, lane, next), block.bottom()),
                    );
                    let hatch = inside.with_clip_rect(gap.intersect(block).intersect(lane));
                    let mut x = gap.left() - gap.height();
                    while x < gap.right() {
                        hatch.line_segment(
                            [pos2(x, gap.bottom()), pos2(x + gap.height(), gap.top())],
                            Stroke::new(1.0, t.surface_3),
                        );
                        x += 6.0;
                    }
                }
                // Where the unit starts: solid for a known time, dashed for
                // an estimate; the line's own edge needs no mark.
                if n > 0 {
                    let (top, bottom) = if u.word_start {
                        (block.top(), block.bottom())
                    } else {
                        (block.top() + 18.0, block.bottom() - 18.0)
                    };
                    let stroke = Stroke::new(1.0, t.border_strong);
                    if u.source == WordSource::Estimated {
                        inside.extend(egui::Shape::dashed_line(
                            &[pos2(ua + 0.5, top), pos2(ua + 0.5, bottom)],
                            stroke,
                            3.0,
                            3.0,
                        ));
                    } else {
                        inside.line_segment([pos2(ua + 0.5, top), pos2(ua + 0.5, bottom)], stroke);
                    }
                }
                if cell.width() > 14.0 {
                    let mut job = egui::text::LayoutJob::simple_singleline(
                        u.text.clone(),
                        FontId::proportional(13.0),
                        ink,
                    );
                    job.wrap.max_width = cell.width() - 8.0;
                    job.wrap.max_rows = 1;
                    job.wrap.break_anywhere = true;
                    let galley = inside.layout_job(job);
                    inside
                        .with_clip_rect(cell.intersect(block).intersect(lane))
                        .galley(
                            pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0),
                            galley,
                            ink,
                        );
                }
                // The handle of a word's start, on the current line.
                if current && !syllables && n > 0 {
                    painter.rect_filled(
                        Rect::from_center_size(pos2(ua, block.center().y), vec2(2.0, 24.0)),
                        1,
                        t.text_2,
                    );
                }
            }
            if current {
                painter.rect_stroke(
                    block,
                    style::radius::M,
                    Stroke::new(2.0, t.text),
                    egui::StrokeKind::Inside,
                );
                for x in [block.left(), block.right()] {
                    painter.rect_filled(
                        Rect::from_center_size(pos2(x, block.center().y), vec2(8.0, 48.0)),
                        4,
                        t.text,
                    );
                }
            }
        }
        // « + » in each gap of half a second or more, wide enough to hit.
        for (a, b) in gaps(shown).into_iter().filter(|_| !game && !locked) {
            let (x0, x1) = (x_at(view, lane, a), x_at(view, lane, b));
            let centre = (x0 + x1) / 2.0;
            if x1 - x0 >= 32.0 && centre > lane.left() && centre < lane.right() {
                let c = pos2(centre, lane.center().y);
                painter.circle_filled(c, 12.0, t.bg);
                painter.circle_stroke(c, 12.0, Stroke::new(1.0, t.border_strong));
                painter.text(
                    c,
                    Align2::CENTER_CENTER,
                    "+",
                    FontId::proportional(15.0),
                    t.text_2,
                );
            }
        }
        // Where Entrée would cut, while the text cursor is between two words.
        if let Some(Caret {
            cue,
            kind: CaretKind::Cut(k),
            ..
        }) = self
            .lyrics_screen
            .caret
            .filter(|_| self.lyrics_screen.text_focus)
            && cue == cur
            && let Some(start) = shown
                .cues
                .get(cur)
                .and_then(|c| edit::words_or_estimate(c).get(k).map(|w| w.start_ms))
        {
            let x = x_at(view, lane, start);
            if x >= lane.left() && x <= lane.right() {
                painter.extend(egui::Shape::dashed_line(
                    &[pos2(x, pictures.top()), pos2(x, lane.bottom())],
                    Stroke::new(2.0, t.accent_text),
                    5.0,
                    4.0,
                ));
                let galley = painter.layout_no_wrap(
                    l.format(
                        "lyrics_screen.cut_mark",
                        &[app_timeline::clock(start, track.duration_ms, l)],
                    ),
                    FontId::proportional(style::text::SMALL),
                    t.text,
                );
                let size = galley.size() + vec2(12.0, 4.0);
                let r = Rect::from_min_size(
                    pos2(
                        (x - size.x / 2.0)
                            .clamp(rect.left(), (rect.right() - size.x).max(rect.left())),
                        ruler.top(),
                    ),
                    size,
                );
                painter.rect_filled(r, 4, t.surface);
                painter.rect_stroke(r, 4, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
                painter.galley(r.min + vec2(6.0, 2.0), galley, t.text);
            }
        }
        // The playhead, over every lane.
        let x = x_at(view, lane, self.ls_playhead());
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, t.text),
            );
            painter.rect_filled(
                Rect::from_min_size(pos2(x - 5.0, rect.top()), vec2(10.0, 8.0)),
                2,
                t.text,
            );
        }
        if let Some(event) = event
            && !self.ls_reading()
        {
            self.ls_lane_event(event, ui.ctx());
        }
    }

    /// Presses and drags on the lyrics lane of the track. A press on another
    /// line chooses it; on the current line it takes an edge, a word start or
    /// the whole line, drawn as it would end and applied on release; a click
    /// on a word chooses it (or opens its split, at the Syllables level).
    fn ls_lane_gestures(
        &mut self,
        ui: &mut egui::Ui,
        lane: Rect,
        view: Viewport,
        track: &Arc<SubtitleTrack>,
    ) -> Option<LaneEvent> {
        let l = self.locale.language;
        let cur = self.lyrics_screen.cur;
        let syllables = self.lyrics_screen.level == Level::Syllable;
        self.lyrics_screen.preview = None;
        let response = ui.interact(
            lane,
            ui.id().with("lyrics-screen-lane"),
            Sense::click_and_drag(),
        );
        let units = track
            .cues
            .get(cur)
            .map(|c| shown_units(c, syllables))
            .unwrap_or_default();
        // The start handles: one per word after the first, at the Words view.
        let handles: Vec<(usize, u64)> = if syllables {
            Vec::new()
        } else {
            units.iter().skip(1).map(|u| (u.word, u.start)).collect()
        };
        enum Hit {
            Add(u64),
            Edge(DragPart),
            Handle(usize),
            Unit(usize),
            Line(usize),
            Nothing,
        }
        let hit = |x: f32| {
            let ms = time_at(view, lane, x);
            if let Some(c) = track.cues.get(cur) {
                let (a, b) = (x_at(view, lane, c.start_ms), x_at(view, lane, c.end_ms));
                if (x - a).abs() <= EDGE_HIT {
                    return Hit::Edge(DragPart::Start);
                }
                if (x - b).abs() <= EDGE_HIT {
                    return Hit::Edge(DragPart::End);
                }
                if let Some((word, _)) = handles
                    .iter()
                    .find(|(_, start)| (x - x_at(view, lane, *start)).abs() <= 5.0)
                {
                    return Hit::Handle(*word);
                }
                if c.start_ms <= ms && ms < c.end_ms {
                    let n = units.iter().rposition(|u| u.start <= ms).unwrap_or(0);
                    return Hit::Unit(units.get(n).map_or(0, |u| u.word));
                }
            }
            if let Some(i) = track
                .cues
                .iter()
                .position(|c| c.start_ms <= ms && ms < c.end_ms)
            {
                return Hit::Line(i);
            }
            for (a, b) in gaps(track) {
                let (x0, x1) = (x_at(view, lane, a), x_at(view, lane, b));
                if x1 - x0 >= 32.0 && (x - (x0 + x1) / 2.0).abs() <= 12.0 {
                    return Hit::Add(a + (b - a) / 2);
                }
            }
            Hit::Nothing
        };
        if let Some(p) = response.hover_pos() {
            let dragging = self.lyrics_screen.drag.is_some();
            ui.ctx().set_cursor_icon(match hit(p.x) {
                Hit::Edge(_) | Hit::Handle(_) => egui::CursorIcon::ResizeHorizontal,
                Hit::Unit(_) if dragging => egui::CursorIcon::Grabbing,
                Hit::Unit(_) => egui::CursorIcon::Grab,
                Hit::Line(_) | Hit::Add(_) => egui::CursorIcon::PointingHand,
                Hit::Nothing => egui::CursorIcon::Default,
            });
            // What a word is, and where its time comes from.
            let ms = time_at(view, lane, p.x);
            if !dragging
                && let Some(c) = track.cues.get(cur)
                && c.start_ms <= ms
                && ms < c.end_ms
                && let Some(u) = units.iter().rev().find(|u| u.start <= ms)
            {
                let tip = if ms >= u.end {
                    l.text(if syllables {
                        "lyrics_screen.silence_syllables"
                    } else {
                        "lyrics_screen.silence_words"
                    })
                    .to_string()
                } else {
                    l.format(
                        "lyrics_screen.unit_tip",
                        &[
                            u.text.clone(),
                            app_timeline::clock_cs(u.start, l),
                            l.text(source_key(u.source)).to_string(),
                        ],
                    )
                };
                response.clone().on_hover_text_at_pointer(tip);
            }
        }
        let mut event = None;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        if ui.input(|i| i.pointer.primary_pressed())
            && response.is_pointer_button_down_on()
            && let Some(p) = pointer
        {
            let drag = |part, word| Drag {
                part,
                word,
                anchor_ms: time_at(view, lane, p.x),
                anchor_x: p.x,
                moved: false,
            };
            match hit(p.x) {
                Hit::Add(ms) => event = Some(LaneEvent::Add(ms)),
                Hit::Edge(part) => self.lyrics_screen.drag = Some(drag(part, None)),
                Hit::Handle(word) => {
                    // The unit index among the cue's units (a cut word has several).
                    let mut probe = track.cues[cur].clone();
                    probe.words = Some(edit::words_or_estimate(&probe));
                    if let Some(unit) = edit::units(&probe).iter().position(|u| u.0 == word) {
                        self.lyrics_screen.drag = Some(drag(DragPart::Unit(unit), None));
                    }
                }
                Hit::Unit(word) => self.lyrics_screen.drag = Some(drag(DragPart::Body, Some(word))),
                Hit::Line(i) => event = Some(LaneEvent::Choose(i)),
                Hit::Nothing => {}
            }
        }
        let Some(mut d) = self.lyrics_screen.drag else {
            return event;
        };
        let down = ui.input(|i| i.pointer.primary_down());
        let x = pointer.map_or(d.anchor_x, |p| p.x);
        d.moved |= (x - d.anchor_x).abs() > 3.0;
        let to_ms = time_at(view, lane, x);
        let delta_ms = to_ms as i64 - d.anchor_ms as i64;
        if d.moved && cur < track.cues.len() {
            let mut preview = (**track).clone();
            let clock = |ms| app_timeline::clock(ms, track.duration_ms, l);
            let tip = match d.part {
                DragPart::Unit(unit) => {
                    let at = edit::set_unit_start(&mut preview, cur, unit, to_ms);
                    let mut probe = preview.cues[cur].clone();
                    probe.words = Some(edit::words_or_estimate(&probe));
                    let text = edit::units(&probe)
                        .get(unit)
                        .and_then(|u| probe.words.as_ref().map(|w| w[u.0].text.clone()))
                        .unwrap_or_default();
                    l.format(
                        "words.drag_tip",
                        &[text, app_timeline::clock_cs(at.unwrap_or(to_ms), l)],
                    )
                }
                DragPart::Start | DragPart::End => {
                    let edge = if d.part == DragPart::Start {
                        Edge::Start
                    } else {
                        Edge::End
                    };
                    let at = edit::set_edge(&mut preview, cur, edge, to_ms);
                    let c = &preview.cues[cur];
                    let length = l.decimal((c.end_ms - c.start_ms) as f64 / 1000.0, 1);
                    format!(
                        "{} {} · {length} s",
                        l.text(if edge == Edge::Start {
                            "edit.start"
                        } else {
                            "edit.end"
                        }),
                        clock(at)
                    )
                }
                DragPart::Body => {
                    edit::move_cue(&mut preview, cur, delta_ms);
                    let c = &preview.cues[cur];
                    format!("{} – {}", clock(c.start_ms), clock(c.end_ms))
                }
            };
            let t = style::tokens(ui);
            let painter = ui.painter_at(lane.expand2(vec2(0.0, 40.0)));
            let galley =
                painter.layout_no_wrap(tip, FontId::proportional(style::text::SMALL), t.text);
            let size = galley.size() + vec2(12.0, 6.0);
            let r = Rect::from_min_size(
                pos2(
                    (x - size.x / 2.0).clamp(lane.left(), (lane.right() - size.x).max(lane.left())),
                    lane.top() - size.y - 2.0,
                ),
                size,
            );
            painter.rect_filled(r, 6, t.surface);
            painter.rect_stroke(r, 6, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);
            painter.galley(r.min + vec2(6.0, 3.0), galley, t.text);
            self.lyrics_screen.preview = Some(preview);
        }
        if !down {
            self.lyrics_screen.drag = None;
            self.lyrics_screen.preview = None;
            event = Some(if d.moved {
                LaneEvent::Drag {
                    part: d.part,
                    delta_ms,
                    to_ms,
                }
            } else {
                match d.word {
                    Some(word) => LaneEvent::Word(word),
                    None => return event,
                }
            });
        } else {
            self.lyrics_screen.drag = Some(d);
        }
        event
    }

    fn ls_lane_event(&mut self, event: LaneEvent, ctx: &egui::Context) {
        self.lyrics_screen.summary = None;
        let i = self.lyrics_screen.cur;
        let l = self.locale.language;
        let line = [(i + 1).to_string()];
        match event {
            LaneEvent::Choose(i) => self.ls_set_cur(i, None),
            LaneEvent::Add(ms) => {
                self.ls_add_at(ms);
            }
            LaneEvent::Word(k) => {
                if self.lyrics_screen.level == Level::Syllable {
                    self.ls_open_syllables(k);
                } else {
                    let s = &mut self.lyrics_screen;
                    s.word = if s.word == Some(k) { None } else { Some(k) };
                    // The arrows now belong to the chosen word.
                    if let Some(id) = ctx.memory(|m| m.focused()) {
                        ctx.memory_mut(|m| m.surrender_focus(id));
                    }
                }
            }
            LaneEvent::Drag {
                part,
                delta_ms,
                to_ms,
            } => {
                self.ls_commit_draft();
                match part {
                    DragPart::Start => {
                        self.edit_track_as(l.format("lyrics_screen.did_start", &line), |t| {
                            edit::set_edge(t, i, Edge::Start, to_ms);
                            true
                        });
                    }
                    DragPart::End => {
                        self.edit_track_as(l.format("lyrics_screen.did_end", &line), |t| {
                            edit::set_edge(t, i, Edge::End, to_ms);
                            true
                        });
                    }
                    DragPart::Body => {
                        self.edit_track_as(l.format("lyrics_screen.did_move", &line), |t| {
                            edit::move_cue(t, i, delta_ms) != 0
                        });
                    }
                    DragPart::Unit(unit) => {
                        let mut word = None;
                        self.edit_track_as(String::new(), |t| {
                            let done = edit::set_unit_start(t, i, unit, to_ms).is_some();
                            let mut probe = t.cues[i].clone();
                            probe.words = Some(edit::words_or_estimate(&probe));
                            word = edit::units(&probe).get(unit).map(|u| u.0);
                            done
                        });
                        // The step names the word it placed.
                        if let (Some(k), Some(track)) = (word, self.ls_track())
                            && let Some(w) = edit::words_or_estimate(&track.cues[i]).get(k)
                        {
                            self.name_last_step(
                                l.format("lyrics_screen.did_word", &[w.text.clone()]),
                            );
                            self.lyrics_screen.word = Some(k);
                        }
                    }
                }
                if !self.mini_playing()
                    && let Some(start) = self
                        .ls_track()
                        .and_then(|t| t.cues.get(i).map(|c| c.start_ms))
                {
                    self.navigate(start, false);
                }
            }
        }
    }

    /// The bottom line says what the keys do where the person is: in a line's
    fn ls_footer(&mut self, ui: &mut egui::Ui, foot: Rect, pad: f32, track: &SubtitleTrack) {
        let t = style::tokens(ui);
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.foot = Some(foot);
        }
        ui.painter().rect_filled(foot, 0, t.surface);
        ui.painter().line_segment(
            [foot.left_top(), foot.right_top()],
            Stroke::new(1.0, t.border),
        );
        let inside = foot.shrink2(vec2(pad, 6.0));
        let compact = self.lyrics_screen.compact;
        // During a game the footer only grows: the track under the eyes of
        // someone holding Espace does not move when a shorter text comes.
        let game = self.lyrics_screen.game.is_some();
        let since_before = game && std::mem::replace(&mut self.lyrics_screen.foot_in_game, game);
        if !game {
            self.lyrics_screen.foot_in_game = false;
        }
        if !compact && !self.lyrics_screen.foot_wraps {
            let mut row = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("lyrics-screen-foot")
                    .max_rect(inside)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            row.set_clip_rect(foot);
            self.ls_footer_text(&mut row, track);
            #[cfg(test)]
            {
                self.lyrics_screen.drawn.foot_text = Some(row.min_rect());
            }
            // Longer than the row: wrapped from the next frame on.
            if row.min_rect().right() > inside.right() + 0.5 {
                self.lyrics_screen.foot_wraps = true;
                ui.ctx().request_repaint();
            }
            return;
        }
        // The narrow window wraps the text, from the top, and the footer is
        // next drawn as high as it was.
        let mut column = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-foot")
                .max_rect(inside)
                .layout(Layout::top_down(Align::Min)),
        );
        // Rows as high as their text and key caps, not as a button.
        column.spacing_mut().interact_size.y = 20.0;
        let text = column
            .horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                self.ls_footer_text(ui, track);
            })
            .response
            .rect;
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.foot_text = Some(text);
        }
        let mut needed = (text.height() + 12.0).max(FOOT_H);
        if since_before {
            needed = needed.max(self.lyrics_screen.foot_h);
        }
        if (needed - self.lyrics_screen.foot_h).abs() > 0.5 {
            self.lyrics_screen.foot_h = needed;
            ui.ctx().request_repaint();
        }
        // The wide window: back on one centred row once the text holds on one.
        if !compact && needed <= FOOT_H {
            self.lyrics_screen.foot_wraps = false;
            ui.ctx().request_repaint();
        }
    }

    fn ls_footer_text(&mut self, ui: &mut egui::Ui, track: &SubtitleTrack) {
        if self.lyrics_locked() {
            // Read-only while a job runs: what still works here.
            key_caps(
                ui,
                self.locale.language.text("lyrics_screen.read_only_keys"),
            );
            return;
        }
        if self.lyrics_screen.game.is_some() {
            self.ls_game_footer(ui);
            return;
        }
        let l = self.locale.language;
        let t = style::tokens(ui);
        let cur = self.lyrics_screen.cur;
        let Some(cue) = track.cues.get(cur) else {
            key_caps(ui, l.text("lyrics_screen.foot_idle"));
            return;
        };
        let words = edit::words_or_estimate(cue);
        if let Some(w) = self.lyrics_screen.word.and_then(|k| words.get(k)) {
            key_caps(
                ui,
                &l.format(
                    "lyrics_screen.foot_word",
                    &[
                        w.text.clone(),
                        app_timeline::clock_cs(w.start_ms, l),
                        l.text(source_key(w.source)).to_string(),
                    ],
                ),
            );
            row_gap(ui, space::L);
            key_caps(
                ui,
                &l.format("lyrics_screen.foot_word_keys", &[l.decimal(0.05, 2)]),
            );
            return;
        }
        if let Some(mut s) = self.lyrics_screen.syl.take() {
            let Some(word) = words.get(s.word) else {
                return;
            };
            key_caps(
                ui,
                &l.format("lyrics_screen.foot_split", &[word.text.clone()]),
            );
            row_gap(ui, space::S);
            let id = egui::Id::new("lyrics-screen-split");
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let field = ui.add(
                egui::TextEdit::singleline(&mut s.text)
                    .id(id)
                    .desired_width(180.0),
            );
            if std::mem::take(&mut s.focus) {
                field.request_focus();
            }
            if field.changed() {
                s.error = None;
                s.note = None;
            }
            row_gap(ui, space::S);
            let (hint, color) = match (s.error, &s.note) {
                (Some(e), _) => (l.text(e).to_string(), t.warn),
                (None, Some(note)) => (note.clone(), t.text_2),
                (None, None) => (
                    l.text("lyrics_screen.foot_split_hint").to_string(),
                    t.text_3,
                ),
            };
            ui.label(
                egui::RichText::new(hint)
                    .size(style::text::SMALL)
                    .color(color),
            );
            let close = ui
                .with_layout(Layout::right_to_left(Align::Center), |ui| {
                    small_button(
                        ui,
                        l.text("lyrics_screen.close"),
                        true,
                        style::Kind::Secondary,
                    )
                    .clicked()
                })
                .inner;
            let apply = field.lost_focus() && enter;
            if !close {
                self.lyrics_screen.syl = Some(s);
                if apply {
                    self.ls_apply_syllables();
                }
            }
            return;
        }
        if let Some((summary, _)) = self.lyrics_screen.summary {
            self.ls_summary_footer(ui, summary, track.cues.len());
            return;
        }
        let caret = self
            .lyrics_screen
            .caret
            .filter(|c| c.cue == cur && self.lyrics_screen.text_focus);
        let text = match caret.map(|c| c.kind) {
            None if self.lyrics_screen.level == Level::Syllable => {
                l.text("lyrics_screen.foot_idle_syllables").to_string()
            }
            None => l.text("lyrics_screen.foot_idle").to_string(),
            Some(CaretKind::Cut(k)) => {
                let word = self
                    .lyrics_screen
                    .draft
                    .as_ref()
                    .and_then(|d| d.text.split_whitespace().nth(k))
                    .unwrap_or_default()
                    .to_string();
                l.format("lyrics_screen.foot_cut", &[word])
            }
            Some(CaretKind::Start) if cur > 0 => l.text("lyrics_screen.foot_start").to_string(),
            Some(CaretKind::Start) => l.text("lyrics_screen.foot_first").to_string(),
            Some(CaretKind::End) => l.text("lyrics_screen.foot_end").to_string(),
            Some(CaretKind::Empty) => l.text("lyrics_screen.foot_empty").to_string(),
            Some(CaretKind::Selection) => l.text("lyrics_screen.foot_selection").to_string(),
        };
        key_caps(ui, &text);
    }

    /// « Ligne supprimée · Annuler », and the short messages that say why a
    /// key did nothing, over the bottom of the screen.
    fn ls_toasts(&mut self, ctx: &egui::Context, foot: Rect) {
        let l = self.locale.language;
        let at = foot.center_top() - vec2(0.0, 12.0);
        if let Some((text, _)) = self.lyrics_screen.flash.clone() {
            egui::Area::new(egui::Id::new("lyrics-screen-flash"))
                .order(egui::Order::Foreground)
                .pivot(Align2::CENTER_BOTTOM)
                .fixed_pos(at)
                .interactable(false)
                .show(ctx, |ui| {
                    style::popup_frame(ui).show(ui, |ui| {
                        ui.set_max_width((foot.width() - 48.0).max(120.0));
                        ui.add(egui::Label::new(text).wrap());
                    });
                });
        } else if self.lyrics_screen.toast.is_some() {
            egui::Area::new(egui::Id::new("lyrics-screen-toast"))
                .order(egui::Order::Foreground)
                .pivot(Align2::CENTER_BOTTOM)
                .fixed_pos(at)
                .show(ctx, |ui| {
                    style::popup_frame(ui).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(l.text("lyrics_screen.deleted"));
                            if style::button(ui, l.text("edit.undo"), true, false, 0.0).clicked() {
                                self.undo();
                                self.lyrics_screen.toast = None;
                            }
                        });
                    });
                });
        }
    }
}

/// What the lane asked for, applied once it is drawn.
enum LaneEvent {
    Choose(usize),
    Add(u64),
    /// A click on word `k` of the current line.
    Word(usize),
    Drag {
        part: DragPart,
        delta_ms: i64,
        to_ms: u64,
    },
}

/// Gaps of half a second or more between lines (and at both ends).
fn gaps(track: &SubtitleTrack) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut previous = 0;
    for cue in &track.cues {
        if cue.start_ms >= previous + edit::MIN_ADD_GAP_MS {
            out.push((previous, cue.start_ms));
        }
        previous = cue.end_ms;
    }
    if track.duration_ms >= previous + edit::MIN_ADD_GAP_MS {
        out.push((previous, track.duration_ms));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:03,000\nSomething had to give\n\n2\n00:00:04,000 --> 00:00:06,000\nand nothing disappears\n\n3\n00:00:40,000 --> 00:00:42,000\nfar away\n\n";

    fn app(folder: &std::path::Path) -> NohApp {
        let mut app = NohApp::default();
        app.locale.language = Language::En;
        let path = folder.join("song.srt");
        std::fs::write(&path, SRT).unwrap();
        let track = noh::subtitle_srt::parse(SRT, 60_000).unwrap();
        app.project.duration_ms = 60_000;
        app.project.track.path = Some(path.clone());
        app.project.track.loaded = Some(app_track::Loaded {
            track: Arc::new(track),
            snapshot: noh::inspection::Snapshot(vec![
                noh::inspection::FileStamp::read(&path).unwrap(),
            ]),
            model: None,
            lyrics_unwritten: None,
            srt_stamp: noh::inspection::FileStamp::read(&path).ok(),
            lyrics_stamp: None,
            pairing: Default::default(),
        });
        // NOH may write this file: no first-edit question in these tests.
        app.trust_lyrics(path);
        app.open_lyrics_screen(Some(0));
        app
    }

    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        ctx
    }

    fn frame_at(app: &mut NohApp, ctx: &egui::Context, size: egui::Vec2, events: Vec<egui::Event>) {
        ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                app_style::apply(ui.ctx());
                app.lyrics_edit_frame(ui.ctx());
                if app.lyrics_screen.open {
                    app.lyrics_screen_ui(ui);
                }
            },
        )
        .textures_delta
        .clear();
    }

    fn frame(app: &mut NohApp, ctx: &egui::Context, events: Vec<egui::Event>) {
        frame_at(app, ctx, vec2(980.0, 802.0), events);
    }

    fn key(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Put the text cursor of line `i` at character `at`, as a click would.
    fn cursor(app: &mut NohApp, ctx: &egui::Context, i: usize, at: usize) {
        app.ls_set_cur(i, Some(Some(at)));
        frame(app, ctx, Vec::new());
        frame(app, ctx, Vec::new());
        assert!(app.lyrics_screen.text_focus, "line {i} has the keyboard");
        assert_eq!(
            app.lyrics_screen.caret.map(|c| (c.cue, c.index)),
            Some((i, at))
        );
    }

    /// A real click: the pointer goes there, presses and releases.
    fn click(app: &mut NohApp, ctx: &egui::Context, pos: Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(app, ctx, vec![egui::Event::PointerMoved(pos)]);
        frame(app, ctx, vec![button(true)]);
        frame(app, ctx, vec![button(false)]);
    }

    fn texts(app: &NohApp) -> Vec<String> {
        app.lyrics_track()
            .unwrap()
            .cues
            .iter()
            .map(|c| c.text.clone())
            .collect()
    }

    #[test]
    fn the_cursor_knows_where_it_stands_in_the_line() {
        let text = "Hold the line";
        assert_eq!(caret_kind(text, 0, false), CaretKind::Start);
        // Inside the first word: nothing before it to cut off.
        assert_eq!(caret_kind(text, 2, false), CaretKind::Start);
        // After « Hold », before or after the space: the cut is before « the ».
        assert_eq!(caret_kind(text, 4, false), CaretKind::Cut(1));
        assert_eq!(caret_kind(text, 5, false), CaretKind::Cut(1));
        // Inside « the »: still before « the ».
        assert_eq!(caret_kind(text, 6, false), CaretKind::Cut(1));
        assert_eq!(caret_kind(text, 9, false), CaretKind::Cut(2));
        // Inside the last word: before it.
        assert_eq!(caret_kind(text, 11, false), CaretKind::Cut(2));
        assert_eq!(caret_kind(text, 13, false), CaretKind::End);
        assert_eq!(caret_kind("  ", 1, false), CaretKind::Empty);
        assert_eq!(caret_kind(text, 3, true), CaretKind::Selection);
        // Accents count as characters, not bytes.
        assert_eq!(caret_kind("été déjà", 4, false), CaretKind::Cut(1));
        assert_eq!(word_under(text, 0), 0);
        assert_eq!(word_under(text, 4), 0);
        assert_eq!(word_under(text, 5), 1);
        assert_eq!(word_under(text, 13), 2);
    }

    #[test]
    fn units_end_at_their_known_end_or_at_the_next_start() {
        let mut cue = SubtitleCue::new(1000, 3000, "Some thing");
        let mut words = edit::words_or_estimate(&cue);
        words[0].end_ms = Some(1400);
        let second = words[1].start_ms;
        cue.words = Some(words);
        let units = shown_units(&cue, false);
        assert_eq!((units[0].start, units[0].end), (1000, 1400));
        assert_eq!((units[1].start, units[1].end), (second, 3000));
        assert!(units.iter().all(|u| u.word_start));
    }

    #[test]
    fn enter_cuts_at_the_cursor_and_backspace_joins_back_as_one_line() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        // Before « to »: "Something had |to give".
        cursor(&mut app, &ctx, 0, 14);
        assert_eq!(app.lyrics_screen.caret.unwrap().kind, CaretKind::Cut(2));
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(
            texts(&app),
            [
                "Something had",
                "to give",
                "and nothing disappears",
                "far away"
            ]
        );
        assert_eq!(app.lyrics_screen.cur, 1);
        assert_eq!(app.history_labels().0, Some("cut line 1"));
        // The cursor starts the new line: Retour arrière joins it back, with
        // a space (one line of text), and the cursor stays at the seam.
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(
            app.lyrics_screen.caret.map(|c| (c.cue, c.index)),
            Some((1, 0))
        );
        frame(&mut app, &ctx, vec![key(Key::Backspace, Modifiers::NONE)]);
        assert_eq!(
            texts(&app),
            [
                "Something had to give",
                "and nothing disappears",
                "far away"
            ]
        );
        assert_eq!(app.lyrics_screen.cur, 0);
        assert_eq!(app.history_labels().0, Some("join lines 1 and 2"));
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(app.lyrics_screen.caret.map(|c| c.index), Some(14));
        // Maj+Entrée is not Entrée: it breaks the line inside the subtitle.
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::SHIFT)]);
        assert_eq!(texts(&app).len(), 3);
    }

    #[test]
    fn suppr_at_the_end_joins_and_enter_at_the_end_adds_a_line() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 0, 21);
        assert_eq!(app.lyrics_screen.caret.unwrap().kind, CaretKind::End);
        frame(&mut app, &ctx, vec![key(Key::Delete, Modifiers::NONE)]);
        assert_eq!(
            texts(&app),
            ["Something had to give and nothing disappears", "far away"]
        );
        // Entrée at the end: a new line in the gap after, its field empty.
        cursor(&mut app, &ctx, 0, 44);
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        assert_eq!(texts(&app).len(), 3);
        assert_eq!(app.lyrics_screen.cur, 1);
        assert_eq!(app.lyrics_screen.draft.as_ref().unwrap().text, "");
        assert_eq!(app.history_labels().0, Some("add a line"));
        // Left empty: it goes away, with « Ligne supprimée ».
        app.ls_set_cur(0, None);
        assert_eq!(texts(&app).len(), 2);
        assert!(app.lyrics_screen.toast.is_some());
    }

    /// Empty the field of line `i` the way selecting all and deleting does.
    fn empty_line(app: &mut NohApp, ctx: &egui::Context, i: usize) {
        cursor(app, ctx, i, 0);
        app.lyrics_screen.draft.as_mut().unwrap().text.clear();
        frame(app, ctx, Vec::new());
        assert_eq!(app.lyrics_screen.caret.unwrap().kind, CaretKind::Empty);
    }

    #[test]
    fn an_emptied_line_goes_away_whichever_way_it_is_joined() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        let rest = ["and nothing disappears", "far away"];
        // Suppr in the emptied field: the line goes, the next one takes its
        // place; its old text does not come back.
        let mut app = app(folder.path());
        empty_line(&mut app, &ctx, 0);
        frame(&mut app, &ctx, vec![key(Key::Delete, Modifiers::NONE)]);
        assert_eq!(texts(&app), rest);
        assert_eq!(app.lyrics_screen.cur, 0);
        assert!(app.lyrics_screen.toast.is_some());
        assert_eq!(app.history_labels().0, Some("delete line 1"));
        // « Joindre à la suivante », clicked for real under the emptied field.
        let mut app = self::app(folder.path());
        empty_line(&mut app, &ctx, 0);
        let join = app
            .lyrics_screen
            .drawn
            .join
            .expect("the row's buttons are drawn");
        click(&mut app, &ctx, join.center());
        assert_eq!(texts(&app), rest);
        assert_eq!(app.lyrics_screen.cur, 0);
        // And « Supprimer » on an emptied line deletes one line, not two.
        let mut app = self::app(folder.path());
        empty_line(&mut app, &ctx, 0);
        let delete = app.lyrics_screen.drawn.delete.unwrap();
        click(&mut app, &ctx, delete.center());
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(texts(&app), rest);
    }

    #[test]
    fn a_click_on_a_line_gives_it_the_keyboard_and_the_buttons_act_on_it() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        frame(&mut app, &ctx, Vec::new());
        let row = app.lyrics_screen.drawn_rows[1];
        click(&mut app, &ctx, row.left_center() + vec2(TIME_W + 40.0, 0.0));
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(app.lyrics_screen.cur, 1);
        assert!(app.lyrics_screen.text_focus);
        // « Couper ici » cuts where the text cursor is.
        cursor(&mut app, &ctx, 1, 12);
        let cut = app.lyrics_screen.drawn.cut.unwrap();
        click(&mut app, &ctx, cut.center());
        assert_eq!(
            texts(&app),
            [
                "Something had to give",
                "and nothing",
                "disappears",
                "far away"
            ]
        );
        assert_eq!(app.lyrics_screen.cur, 2);
        // « Joindre à la suivante » on the first line: one line of text.
        cursor(&mut app, &ctx, 1, 3);
        let join = app.lyrics_screen.drawn.join.unwrap();
        click(&mut app, &ctx, join.center());
        assert_eq!(texts(&app)[1], "and nothing disappears");
        // The time of a line plays from there: it becomes the current one.
        frame(&mut app, &ctx, Vec::new());
        let time = app.lyrics_screen.drawn_rows[2].left_top() + vec2(20.0, ROW_H / 2.0);
        click(&mut app, &ctx, time);
        assert_eq!(app.lyrics_screen.cur, 2);
        assert_eq!(app.mini_preview.cursor_ms, 40_000);
    }

    #[test]
    fn the_narrow_footer_grows_to_hold_its_text_in_every_language() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        for language in Language::ALL {
            let mut app = app(folder.path());
            app.locale.language = language;
            let fits = |app: &mut NohApp, what: &str| {
                for _ in 0..3 {
                    frame_at(app, &ctx, vec2(420.0, 492.0), Vec::new());
                }
                let d = app.lyrics_screen.drawn;
                let (foot, text) = (d.foot.unwrap(), d.foot_text.unwrap());
                assert!(
                    foot.contains_rect(text) && text.right() <= 420.0 - 16.0 + 0.5,
                    "{language:?} {what}: {text:?} in {foot:?}"
                );
                assert!(foot.bottom() <= 492.0 + 0.5 && foot.height() <= 160.0);
            };
            fits(&mut app, "idle");
            app.lyrics_screen.word = Some(1);
            fits(&mut app, "a word chosen");
            app.lyrics_screen.word = None;
            app.lyrics_screen.level = Level::Syllable;
            app.ls_open_syllables(0);
            fits(&mut app, "a word's split");
        }
    }

    #[derive(Debug)]
    struct Dropped(std::path::PathBuf);
    impl egui::DroppedFile for Dropped {
        fn path(&self) -> &std::path::Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            panic!("The UI must not read dropped media")
        }
    }

    #[test]
    fn files_over_the_lyrics_screen_get_no_drop_overlay_and_are_not_taken() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        assert!(app.edit_track(|t| edit::set_text(t, 0, "Something had to go")));
        let video = folder.path().join("clip.mp4");
        // What the whole application paints, with or without a file held
        // over the window, and with one dropped.
        let shapes = |app: &mut NohApp, hovered: bool, dropped: bool| {
            let file = |path: &std::path::Path| Some(path.to_path_buf());
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, 850.0))),
                    hovered_files: if hovered {
                        vec![egui::HoveredFile {
                            path: file(&video),
                            ..Default::default()
                        }]
                    } else {
                        Vec::new()
                    },
                    dropped_files: if dropped {
                        vec![std::sync::Arc::new(Dropped(video.clone()))]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                },
                |ui| {
                    app.draw(ui);
                },
            );
            // No renderer takes the texture updates in a test: dropped unread,
            // they are an error wherever the frame produced one.
            output.textures_delta.clear();
            output.shapes.len()
        };
        shapes(&mut app, false, false);
        let plain = shapes(&mut app, false, false);
        // (The test has no reader for the lyrics file, which the whole frame
        // drops: the edit made above keeps the lyrics, as in a real session.)
        assert!(app.lyrics_screen.open);
        assert_eq!(
            shapes(&mut app, true, false),
            plain,
            "no overlay is painted"
        );
        shapes(&mut app, false, true);
        assert!(app.clips.is_empty(), "the dropped file is not taken");
        assert!(app.lyrics_screen.open);
        // Back on the montage, the same file held over the window is announced.
        app.close_lyrics_screen();
        shapes(&mut app, false, false);
        let montage = shapes(&mut app, false, false);
        assert!(shapes(&mut app, true, false) > montage);
    }

    #[test]
    fn text_typed_while_the_lyrics_file_is_read_again_reaches_the_lyrics_after() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 1, 22);
        // The file is being read again: no lyrics for a moment.
        let path = app.project.track.path.clone().unwrap();
        let loaded = app.project.track.loaded.take();
        app.project
            .track
            .request(path, folder.path().join("song.wav"), 60_000);
        assert!(app.lyrics_track().is_none() && app.ls_reading());
        frame(&mut app, &ctx, vec![egui::Event::Text(" now".into())]);
        assert!(app.lyrics_screen.open, "the screen shows what it had");
        // Nothing can be written: the text waits, a line change does not drop
        // it, and « Terminé » waits too.
        app.ls_commit_draft();
        app.ls_set_cur(0, None);
        app.close_lyrics_screen();
        frame(&mut app, &ctx, vec![key(Key::Enter, Modifiers::NONE)]);
        let d = app.lyrics_screen.draft.as_ref().unwrap();
        assert_eq!((d.cue, d.text.as_str()), (1, "and nothing disappears now"));
        assert!(d.typed_at.is_some() && app.lyrics_screen.open);
        assert_eq!(app.lyrics_screen.cur, 1);
        // The lyrics are back: the text reaches them.
        app.project.track.loaded = loaded;
        frame(&mut app, &ctx, Vec::new());
        app.ls_commit_draft();
        assert_eq!(texts(&app)[1], "and nothing disappears now");
        assert_eq!(texts(&app).len(), 3);
    }

    #[test]
    fn a_lyrics_file_that_can_no_longer_be_read_closes_the_screen() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        frame(&mut app, &ctx, Vec::new());
        assert!(app.lyrics_screen.open);
        // Changed on disk and now malformed: the path stays, no lyrics are
        // loaded, nothing is being read.
        app.project.track.loaded = None;
        app.project.track.error = Some(noh::engine::EngineError {
            code: "subtitle_invalid".into(),
            operation: "read".into(),
            path: app.project.track.path.clone(),
            detail: "".into(),
            technical: "".into(),
        });
        assert!(!app.project.track.loading() && !app.ls_reading());
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.lyrics_screen.open, "no stale lyrics to edit");
    }

    #[test]
    fn a_typed_line_is_one_undo_step_however_often_it_pauses() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 1, 22);
        let before = app.history_labels().0.map(str::to_string);
        frame(&mut app, &ctx, vec![egui::Event::Text(" now".into())]);
        assert!(app.lyrics_screen.draft.as_ref().unwrap().typed_at.is_some());
        // Not in the lyrics until the typing pauses.
        assert_eq!(texts(&app)[1], "and nothing disappears");
        app.ls_commit_draft();
        assert_eq!(texts(&app)[1], "and nothing disappears now");
        frame(&mut app, &ctx, vec![egui::Event::Text("!".into())]);
        app.ls_commit_draft();
        assert_eq!(texts(&app)[1], "and nothing disappears now!");
        assert_eq!(app.history_labels().0, Some("edit the text of line 2"));
        assert!(app.undo());
        assert_eq!(texts(&app)[1], "and nothing disappears");
        assert_eq!(app.history_labels().0.map(str::to_string), before);
        assert_eq!(app.history_labels().1, Some("edit the text of line 2"));
        // The field follows the lyrics again.
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(
            app.lyrics_screen.draft.as_ref().unwrap().text,
            "and nothing disappears"
        );
    }

    #[test]
    fn arrows_change_line_at_a_boundary_and_alt_arrows_move_the_word_under_the_cursor() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 0, 12);
        frame(&mut app, &ctx, vec![key(Key::ArrowDown, Modifiers::NONE)]);
        assert_eq!(app.lyrics_screen.cur, 1);
        frame(&mut app, &ctx, Vec::new());
        frame(&mut app, &ctx, Vec::new());
        assert_eq!(
            app.lyrics_screen.caret.map(|c| (c.cue, c.index)),
            Some((1, 12))
        );
        // After the space: « disappears » is the word under the cursor, and its
        // start moves by 0.05 s.
        let start = |app: &NohApp| {
            edit::words_or_estimate(&app.lyrics_track().unwrap().cues[1])[2].start_ms
        };
        let before = start(&app);
        frame(&mut app, &ctx, vec![key(Key::ArrowRight, Modifiers::ALT)]);
        frame(&mut app, &ctx, vec![key(Key::ArrowRight, Modifiers::ALT)]);
        assert_eq!(start(&app), before + 100);
        assert_eq!(app.lyrics_screen.word, Some(2));
        assert_eq!(app.history_labels().0, Some("place “disappears”"));
        // Both presses are one step.
        assert!(app.undo());
        assert_eq!(start(&app), before);
        frame(&mut app, &ctx, vec![key(Key::ArrowUp, Modifiers::NONE)]);
        assert_eq!(app.lyrics_screen.cur, 0);
    }

    #[test]
    fn escape_closes_one_layer_at_a_time() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 0, 3);
        app.lyrics_screen.word = Some(1);
        let escape = || vec![key(Key::Escape, Modifiers::NONE)];
        frame(&mut app, &ctx, escape());
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.lyrics_screen.text_focus, "first the field");
        assert!(app.lyrics_screen.open && app.lyrics_screen.word.is_some());
        frame(&mut app, &ctx, escape());
        assert!(app.lyrics_screen.word.is_none(), "then the chosen word");
        assert!(app.lyrics_screen.open);
        frame(&mut app, &ctx, escape());
        assert!(!app.lyrics_screen.open, "then the screen");
    }

    #[test]
    fn the_track_shows_the_chosen_line_and_its_edges_drag() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        frame(&mut app, &ctx, Vec::new());
        // The third line is far away: choosing it brings it into the view.
        app.ls_set_cur(2, None);
        frame(&mut app, &ctx, Vec::new());
        let d = app.lyrics_screen.drawn;
        let (lane, view) = (d.lane.unwrap(), d.view.unwrap());
        assert_eq!(view.0.duration_ms(), VIEW_MS);
        assert!(
            view.0.start_ms <= 40_000 && view.0.end_ms >= 42_000,
            "{view:?}"
        );
        // Drag its end from 42 s to 43 s.
        let at = |ms| pos2(x_at(view, lane, ms), lane.center().y);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at(42_000))]);
        frame(&mut app, &ctx, vec![button(at(42_000), true)]);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(at(43_000))]);
        assert_eq!(
            app.lyrics_screen.preview.as_ref().map(|t| t.cues[2].end_ms),
            Some(43_000),
            "drawn as it would end"
        );
        assert_eq!(app.lyrics_track().unwrap().cues[2].end_ms, 42_000);
        frame(&mut app, &ctx, vec![button(at(43_000), false)]);
        assert_eq!(app.lyrics_track().unwrap().cues[2].end_ms, 43_000);
        assert_eq!(app.history_labels().0, Some("change the end of line 3"));
        // The ruler moves the playhead.
        let ruler = app.lyrics_screen.drawn.ruler.unwrap();
        let p = pos2(x_at(view, ruler, 45_000), ruler.center().y);
        frame(&mut app, &ctx, vec![egui::Event::PointerMoved(p)]);
        frame(&mut app, &ctx, vec![button(p, true)]);
        frame(&mut app, &ctx, vec![button(p, false)]);
        assert!(app.mini_preview.cursor_ms.abs_diff(45_000) < 60);
    }

    #[test]
    fn a_click_on_a_word_chooses_it_or_opens_its_split() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        app.lyrics_screen.level = Level::Word;
        app.ls_lane_event(LaneEvent::Word(2), &context());
        assert_eq!(app.lyrics_screen.word, Some(2));
        app.ls_lane_event(LaneEvent::Word(2), &context());
        assert_eq!(app.lyrics_screen.word, None);
        // At the Syllables level the click opens the split field; typed
        // hyphens cut the word, and the word typed whole joins it back.
        app.lyrics_screen.level = Level::Syllable;
        app.ls_lane_event(LaneEvent::Word(0), &context());
        assert_eq!(app.lyrics_screen.syl.as_ref().map(|s| s.word), Some(0));
        app.lyrics_screen.syl.as_mut().unwrap().text = "Some-thing".into();
        app.ls_apply_syllables();
        assert!(app.lyrics_screen.syl.is_none());
        let syllables = |app: &NohApp| {
            edit::words_or_estimate(&app.lyrics_track().unwrap().cues[0])[0]
                .syllables
                .as_ref()
                .map(|s| s.iter().map(|s| s.text.clone()).collect::<Vec<_>>())
        };
        assert_eq!(
            syllables(&app),
            Some(vec!["Some".to_string(), "thing".to_string()])
        );
        assert_eq!(
            app.history_labels().0,
            Some("split “Something” into syllables")
        );
        app.ls_open_syllables(0);
        assert_eq!(app.lyrics_screen.syl.as_ref().unwrap().text, "Some-thing");
        app.lyrics_screen.syl.as_mut().unwrap().text = "Somthing".into();
        app.ls_apply_syllables();
        assert_eq!(
            app.lyrics_screen.syl.as_ref().and_then(|s| s.error),
            Some("words.syl_letters")
        );
        app.lyrics_screen.syl.as_mut().unwrap().text = "Something".into();
        app.ls_apply_syllables();
        assert_eq!(syllables(&app), None);
    }

    #[test]
    fn the_narrow_window_stacks_the_monitor_the_text_and_the_track() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        for language in Language::ALL {
            let mut app = app(folder.path());
            app.locale.language = language;
            // 420 × 540, less the application's header.
            for _ in 0..2 {
                frame_at(&mut app, &ctx, vec2(420.0, 492.0), Vec::new());
            }
            let d = app.lyrics_screen.drawn;
            let (monitor, text) = (d.monitor.unwrap(), d.text.unwrap());
            assert!(monitor.bottom() < text.top(), "{language:?}");
            for r in [monitor, text, d.lane.unwrap()] {
                assert!(
                    r.left() >= 16.0 - 0.5 && r.right() <= 420.0 - 16.0 + 0.5,
                    "{language:?} {r:?}"
                );
            }
            // 6 s instead of 12, so a word stays readable.
            assert_eq!(d.view.unwrap().0.duration_ms(), VIEW_COMPACT_MS);
            let mut wide = app;
            for _ in 0..2 {
                frame_at(&mut wide, &ctx, vec2(980.0, 802.0), Vec::new());
            }
            let d = wide.lyrics_screen.drawn;
            let (monitor, text, lane, foot) = (
                d.monitor.unwrap(),
                d.text.unwrap(),
                d.lane.unwrap(),
                d.foot.unwrap(),
            );
            assert!(monitor.right() < text.left(), "{language:?}");
            // At 980 × 850 everything is on screen with no scrolling.
            assert!(
                text.bottom() < lane.top() && lane.bottom() <= foot.top(),
                "{language:?}"
            );
            assert!(text.right() <= 980.0 - 24.0 + 0.5);
        }
    }

    // ---- The sync game ----

    fn space(pressed: bool, repeat: bool) -> egui::Event {
        egui::Event::Key {
            key: Key::Space,
            physical_key: None,
            pressed,
            repeat,
            modifiers: Modifiers::NONE,
        }
    }

    /// One frame with the player's clock at `ms` (None: not playing).
    fn at(app: &mut NohApp, ctx: &egui::Context, ms: Option<u64>, events: Vec<egui::Event>) {
        app.lyrics_screen.test_clock = Some(ms);
        frame(app, ctx, events);
    }

    /// Hold Espace from `from` to `to`.
    fn hold(app: &mut NohApp, ctx: &egui::Context, from: u64, to: u64) {
        at(app, ctx, Some(from), vec![space(true, false)]);
        at(app, ctx, Some((from + to) / 2), vec![space(true, true)]);
        at(app, ctx, Some(to), vec![space(false, false)]);
    }

    /// A game at `level` from line `line`, its playback started.
    fn game(app: &mut NohApp, ctx: &egui::Context, level: Level, line: usize) {
        app.lyrics_screen.level = level;
        app.lyrics_screen.cur = line;
        frame(app, ctx, Vec::new());
        app.ls_start_game(ctx);
        assert!(app.lyrics_screen.game.is_some());
        // The player gets to where playback was asked from.
        let from = app.lyrics_track().unwrap().cues[line]
            .start_ms
            .saturating_sub(2000);
        at(app, ctx, Some(from), Vec::new());
    }

    fn word(app: &NohApp, line: usize, k: usize) -> (u64, Option<u64>, WordSource) {
        let w = &edit::words_or_estimate(&app.lyrics_track().unwrap().cues[line])[k];
        (w.start_ms, w.end_ms, w.source)
    }

    #[test]
    fn holding_space_takes_words_with_their_ends_as_one_undo_step() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        let original = app.lyrics_track().unwrap();
        let steps = app.history_labels().0.map(str::to_string);
        game(&mut app, &ctx, Level::Word, 0);
        // « Something » held from 1,10 to 1,60 s: a repeat of the key is not
        // a second press.
        hold(&mut app, &ctx, 1100, 1600);
        assert_eq!(word(&app, 0, 0), (1100, Some(1600), WordSource::Hand));
        assert_eq!(app.history_labels().0, Some("sync 1 word"));
        // A breath, then « had »: its own start and end, the silence kept.
        hold(&mut app, &ctx, 1900, 2100);
        assert_eq!(word(&app, 0, 1), (1900, Some(2100), WordSource::Hand));
        assert_eq!(word(&app, 0, 0).1, Some(1600));
        assert_eq!(app.history_labels().0, Some("sync 2 words"));
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!((g.idx, g.holds.len()), (2, 2));
        // Échap stops and keeps what was taken; the footer says what was done.
        at(
            &mut app,
            &ctx,
            Some(2200),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(app.lyrics_screen.game.is_none() && app.lyrics_screen.open);
        let (summary, _) = app.lyrics_screen.summary.unwrap();
        assert_eq!((summary.count, summary.lines, summary.last), (2, 1, 0));
        assert_eq!(word(&app, 0, 1).0, 1900);
        // One undo step for the whole game.
        assert!(app.undo());
        assert_eq!(*app.lyrics_track().unwrap(), *original);
        assert_eq!(app.history_labels().0.map(str::to_string), steps);
        frame(&mut app, &ctx, Vec::new());
        assert!(
            app.lyrics_screen.summary.is_none(),
            "undone: nothing to sum up"
        );
    }

    #[test]
    fn a_take_leaves_the_song_playing_and_the_game_s_end_refreshes_the_monitor() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        // « Sous-titres sur la vidéo » is on: an edit drops the monitor's
        // pictures, which show the captions.
        app.project.apply_subtitles = true;
        // The preview, as every frame of the application keeps it.
        app.poll_mini_preview(&ctx);
        game(&mut app, &ctx, Level::Word, 0);
        app.poll_mini_preview(&ctx);
        let (stops, revision) = (app.mini_preview.invalidations, app.revision);
        for (from, to) in [(1000, 1400), (1500, 1900)] {
            hold(&mut app, &ctx, from, to);
            app.poll_mini_preview(&ctx);
        }
        assert!(app.lyrics_screen.game.is_some());
        assert_eq!(word(&app, 0, 1).0, 1500, "the takes are in the lyrics");
        assert_eq!(
            app.mini_preview.invalidations, stops,
            "the player is not stopped by a take"
        );
        assert_ne!(app.revision, revision, "what was checked is out of date");
        // The game over, the monitor shows the lyrics as they are now.
        app.ls_end_game();
        app.poll_mini_preview(&ctx);
        assert_eq!(app.mini_preview.invalidations, stops + 1);
        app.poll_mini_preview(&ctx);
        assert_eq!(app.mini_preview.invalidations, stops + 1, "once");
        // Outside a game, an edit drops the pictures at once, as before.
        app.edit_track(|t| {
            t.cues[1].text = "and nothing is lost".into();
            true
        });
        assert!(app.mini_preview.invalidations > stops + 1);
    }

    #[test]
    fn a_line_held_takes_its_start_and_end_and_legato_closes_small_gaps() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        game(&mut app, &ctx, Level::Line, 0);
        hold(&mut app, &ctx, 1200, 3400);
        // 60 ms later: one sung stretch, the first line ends where the second starts.
        hold(&mut app, &ctx, 3460, 5800);
        let t = app.lyrics_track().unwrap();
        assert_eq!((t.cues[0].start_ms, t.cues[0].end_ms), (1200, 3460));
        assert_eq!((t.cues[1].start_ms, t.cues[1].end_ms), (3460, 5800));
        assert_eq!(app.history_labels().0, Some("sync 2 lines"));
        // The third line is far: the game goes on, on that line.
        assert_eq!(app.lyrics_screen.game.as_ref().unwrap().idx, 2);
        // The last unit taken ends the game by itself.
        hold(&mut app, &ctx, 40_100, 41_900);
        assert!(app.lyrics_screen.game.is_none());
        assert_eq!(app.lyrics_screen.summary.unwrap().0.count, 3);
        assert_eq!(app.lyrics_screen.cur, 2);
    }

    #[test]
    fn backspace_takes_again_and_twice_goes_one_more_back() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        let original = app.lyrics_track().unwrap();
        game(&mut app, &ctx, Level::Word, 0);
        hold(&mut app, &ctx, 1100, 1500);
        hold(&mut app, &ctx, 1800, 2100);
        hold(&mut app, &ctx, 2200, 2400);
        let back = || vec![key(Key::Backspace, Modifiers::NONE)];
        at(&mut app, &ctx, Some(2500), back());
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!((g.idx, g.holds.len(), g.redos), (2, 2, 1));
        // Playback goes back 1.5 s before the word to take again, and the
        // clock counts once it is there: a press before that is not a take.
        assert!(g.waiting.is_some());
        at(&mut app, &ctx, Some(2500), vec![space(true, false)]);
        at(&mut app, &ctx, Some(2500), vec![space(false, false)]);
        assert!(app.lyrics_screen.game.as_ref().unwrap().hold.is_none());
        assert_eq!(app.history_labels().0, Some("sync 2 words"));
        assert_eq!(
            word(&app, 0, 2).2,
            WordSource::Estimated,
            "« to » is as it was"
        );
        // Twice in a row: one more word back; then all of them.
        at(&mut app, &ctx, Some(700), back());
        at(&mut app, &ctx, Some(300), back());
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!((g.idx, g.holds.len()), (0, 0));
        assert_eq!(*app.lyrics_track().unwrap(), *original);
        // Nothing left to take back: the key still rewinds.
        at(&mut app, &ctx, Some(0), back());
        assert!(app.lyrics_screen.game.is_some());
        // With Espace held, Retour arrière drops that press, not the take before.
        at(&mut app, &ctx, Some(100), Vec::new());
        hold(&mut app, &ctx, 1100, 1500);
        at(&mut app, &ctx, Some(1800), vec![space(true, false)]);
        assert!(app.lyrics_screen.game.as_ref().unwrap().hold.is_some());
        app.lyrics_screen.game.as_mut().unwrap().last_back = None;
        at(&mut app, &ctx, Some(1900), back());
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!((g.hold, g.idx, g.holds.len()), (None, 1, 1));
    }

    #[test]
    fn a_unit_that_goes_by_unheld_makes_the_game_wait_and_space_resumes() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        game(&mut app, &ctx, Level::Word, 0);
        hold(&mut app, &ctx, 1100, 1500);
        // « had » is expected until about 2,2 s; 1.5 s later, nothing held.
        let guide = app.lyrics_screen.game.as_ref().unwrap().guides[1];
        at(&mut app, &ctx, Some(guide.end_ms + 1400), Vec::new());
        assert!(app.lyrics_screen.game.as_ref().unwrap().paused.is_none());
        at(&mut app, &ctx, Some(guide.end_ms + 1600), Vec::new());
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!(g.paused, Some(game::Pause::Missed));
        // Espace resumes just before the missed word: that press is not a take.
        at(&mut app, &ctx, None, vec![space(true, false)]);
        at(&mut app, &ctx, None, vec![space(false, false)]);
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert!(g.paused.is_none() && g.hold.is_none() && g.holds.len() == 1);
        at(
            &mut app,
            &ctx,
            Some(guide.start_ms.saturating_sub(1500)),
            Vec::new(),
        );
        hold(&mut app, &ctx, guide.start_ms, guide.start_ms + 200);
        hold(&mut app, &ctx, 2300, 2450);
        assert_eq!(app.lyrics_screen.game.as_ref().unwrap().holds.len(), 3);
        // « give » is left (expected until 3 s, so not timed out at 4,35 s),
        // and the next line started at 4 s: a press now is for that line.
        // The word was passed; the press is not used.
        at(&mut app, &ctx, Some(4350), vec![space(true, false)]);
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!(
            (g.paused, g.hold, g.holds.len()),
            (Some(game::Pause::Missed), None, 3)
        );
    }

    #[test]
    fn a_game_with_no_take_leaves_the_lyrics_and_the_history_as_they_were() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        let original = app.lyrics_track().unwrap();
        // At the Syllables level the words of the lines to play are cut
        // first; stopping at once takes that back too.
        game(&mut app, &ctx, Level::Syllable, 0);
        at(
            &mut app,
            &ctx,
            Some(100),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(app.lyrics_screen.game.is_none() && app.lyrics_screen.summary.is_none());
        assert_eq!(*app.lyrics_track().unwrap(), *original);
        assert_eq!(app.history_labels(), (None, None));
        // The song ending (playback stops by itself) ends a game and keeps
        // its takes.
        game(&mut app, &ctx, Level::Line, 2);
        hold(&mut app, &ctx, 40_200, 41_000);
        assert!(
            app.lyrics_screen.game.is_none(),
            "the last line ends the game"
        );
        game(&mut app, &ctx, Level::Line, 1);
        hold(&mut app, &ctx, 4100, 5000);
        at(&mut app, &ctx, None, Vec::new());
        assert!(app.lyrics_screen.game.is_none());
        assert_eq!(app.lyrics_track().unwrap().cues[1].start_ms, 4100);
    }

    #[test]
    fn a_job_that_starts_makes_the_screen_read_only_and_the_game_keeps_its_takes() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        game(&mut app, &ctx, Level::Word, 0);
        hold(&mut app, &ctx, 1100, 1600);
        // « had » is being held when a job starts.
        at(&mut app, &ctx, Some(1900), vec![space(true, false)]);
        assert!(app.lyrics_screen.drawn.read_only.is_none());
        app.cancelling = true;
        assert!(app.lyrics_locked());
        at(&mut app, &ctx, Some(2000), Vec::new());
        assert!(app.lyrics_screen.game.is_none() && app.lyrics_screen.open);
        assert_eq!(word(&app, 0, 0), (1100, Some(1600), WordSource::Hand));
        assert_ne!(
            word(&app, 0, 1).2,
            WordSource::Hand,
            "the word being held is not taken under the job"
        );
        assert!(
            app.lyrics_screen.drawn.read_only.is_some(),
            "a banner says why"
        );
        let kept = app.lyrics_track().unwrap();
        // Typing, Entrée, Ctrl+Z, a press on the lane, « Commencer »: nothing.
        let lane = app.lyrics_screen.drawn.lane.unwrap().center();
        at(
            &mut app,
            &ctx,
            None,
            vec![
                space(false, false),
                egui::Event::Text("x".into()),
                key(Key::Enter, Modifiers::NONE),
                key(Key::Z, Modifiers::COMMAND),
                egui::Event::PointerMoved(lane),
            ],
        );
        at(
            &mut app,
            &ctx,
            None,
            vec![egui::Event::PointerButton {
                pos: lane,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(app.lyrics_screen.drag.is_none());
        app.ls_start_game(&ctx);
        assert!(app.lyrics_screen.game.is_none());
        assert!(
            !app.edit_track(|t| edit::set_text(t, 0, "x")),
            "no edit reaches the lyrics a job reads, whatever asks for it"
        );
        assert_eq!(*app.lyrics_track().unwrap(), *kept);
        // Échap still closes the screen; once the job is over, edits are back.
        at(
            &mut app,
            &ctx,
            None,
            vec![
                egui::Event::PointerButton {
                    pos: lane,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::NONE,
                },
                key(Key::Escape, Modifiers::NONE),
            ],
        );
        assert!(!app.lyrics_screen.open);
        assert_eq!(*app.lyrics_track().unwrap(), *kept);
        app.cancelling = false;
        assert!(app.edit_track(|t| edit::set_text(t, 0, "x")));
    }

    #[test]
    fn closing_under_a_job_takes_nothing_back_from_a_game_left_without_a_take() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        game(&mut app, &ctx, Level::Word, 0);
        // One take, taken again: the game has its undo step and no take.
        hold(&mut app, &ctx, 1100, 1600);
        at(
            &mut app,
            &ctx,
            Some(1700),
            vec![key(Key::Backspace, Modifiers::NONE)],
        );
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert!(g.holds.is_empty());
        let copy = app.project.edit.as_ref().map(|s| s.file.clone());
        let steps = app.history_labels().0.map(str::to_string);
        assert!(steps.is_some(), "the game left its undo step");
        // A job starts, and the screen is closed before it draws again.
        app.cancelling = true;
        app.close_lyrics_screen();
        assert!(!app.lyrics_screen.open);
        assert_eq!(
            app.project.edit.as_ref().map(|s| s.file.clone()),
            copy,
            "the copy the job reads is untouched"
        );
        assert_eq!(app.history_labels().0.map(str::to_string), steps);
    }

    #[test]
    fn during_a_game_the_text_the_undo_keys_and_the_track_wait() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        cursor(&mut app, &ctx, 0, 3);
        game(&mut app, &ctx, Level::Word, 0);
        assert!(
            !app.lyrics_screen.text_focus,
            "the field gave the keyboard back"
        );
        hold(&mut app, &ctx, 1100, 1500);
        // Typed text goes nowhere, Ctrl+Z undoes nothing, « Terminé » waits.
        at(
            &mut app,
            &ctx,
            Some(1600),
            vec![
                egui::Event::Text("x".into()),
                key(Key::Z, Modifiers::COMMAND),
            ],
        );
        assert_eq!(texts(&app)[0], "Something had to give");
        assert_eq!(word(&app, 0, 0).0, 1100);
        assert!(app.lyrics_screen.game.is_some());
        // A press on the lane starts no drag.
        let d = app.lyrics_screen.drawn;
        let p = d.lane.unwrap().center();
        at(
            &mut app,
            &ctx,
            Some(1650),
            vec![egui::Event::PointerMoved(p)],
        );
        at(
            &mut app,
            &ctx,
            Some(1700),
            vec![egui::Event::PointerButton {
                pos: p,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
        );
        assert!(app.lyrics_screen.drag.is_none());
    }

    #[test]
    fn the_narrow_window_shows_the_prompt_the_track_and_the_footer_during_a_game() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        for language in Language::ALL {
            let mut app = app(folder.path());
            app.locale.language = language;
            app.lyrics_screen.level = Level::Word;
            app.lyrics_screen.compact = true;
            frame_at(&mut app, &ctx, vec2(420.0, 492.0), Vec::new());
            app.ls_start_game(&ctx);
            app.lyrics_screen.test_clock = Some(Some(0));
            // The first instructions, then a missed word: the two longest footers.
            for missed in [false, true] {
                if missed {
                    app.lyrics_screen.game.as_mut().unwrap().paused = Some(game::Pause::Missed);
                }
                for _ in 0..3 {
                    frame_at(&mut app, &ctx, vec2(420.0, 492.0), Vec::new());
                }
                let d = app.lyrics_screen.drawn;
                let (sync, lane, foot, text) = (
                    d.sync.unwrap(),
                    d.lane.unwrap(),
                    d.foot.unwrap(),
                    d.foot_text.unwrap(),
                );
                // No scrolling while Espace is held: all of it is on screen.
                assert!(sync.top() >= MODEBAR_H, "{language:?} {sync:?}");
                assert!(
                    sync.bottom() <= lane.top() && lane.bottom() <= foot.top(),
                    "{language:?} missed={missed}: {sync:?} {lane:?} {foot:?}"
                );
                assert!(
                    foot.bottom() <= 492.5 && foot.contains_rect(text),
                    "{language:?}"
                );
            }
        }
    }

    #[test]
    fn the_first_edit_question_of_an_srt_pauses_the_game_and_keeps_it_one_undo_step() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        // An .srt NOH has never been allowed to write: its first edit asks.
        let untrusted = |folder: &std::path::Path| {
            let mut app = app(folder);
            app.trust_lyrics(folder.join("another.srt"));
            app
        };
        let mut app = untrusted(folder.path());
        let original = app.lyrics_track().unwrap();
        game(&mut app, &ctx, Level::Word, 0);
        hold(&mut app, &ctx, 1100, 1500);
        assert!(
            app.lyrics_question_open(),
            "the first take is the first edit"
        );
        // The game waits for the answer: no take meanwhile.
        at(&mut app, &ctx, Some(1700), vec![space(true, false)]);
        at(&mut app, &ctx, Some(1800), vec![space(false, false)]);
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!((g.paused, g.holds.len()), (Some(game::Pause::Question), 1));
        // « Modifier le .srt »: the take is kept and written, Espace resumes.
        let path = app.project.track.path.clone().unwrap();
        app.consent_first_edit(path);
        at(&mut app, &ctx, None, Vec::new());
        assert_eq!(
            app.lyrics_screen.game.as_ref().unwrap().paused,
            Some(game::Pause::Answered)
        );
        at(&mut app, &ctx, None, vec![space(true, false)]);
        at(&mut app, &ctx, None, vec![space(false, false)]);
        at(&mut app, &ctx, Some(400), Vec::new());
        hold(&mut app, &ctx, 1900, 2100);
        hold(&mut app, &ctx, 2200, 2400);
        assert_eq!(app.history_labels().0, Some("sync 3 words"));
        at(
            &mut app,
            &ctx,
            Some(2500),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert_eq!(app.lyrics_screen.summary.unwrap().0.count, 3);
        // « Tout annuler », clicked: the whole game goes, first take included.
        frame(&mut app, &ctx, Vec::new());
        let undo_all = app
            .lyrics_screen
            .drawn
            .undo_all
            .expect("the summary's buttons");
        click(&mut app, &ctx, undo_all.center());
        assert_eq!(*app.lyrics_track().unwrap(), *original);
        assert_eq!(app.history_labels().0, None);
        assert!(app.lyrics_screen.summary.is_none());

        // « Annuler » on the question: the game ends, lyrics and history untouched.
        let mut app = untrusted(folder.path());
        let original = app.lyrics_track().unwrap();
        game(&mut app, &ctx, Level::Word, 0);
        hold(&mut app, &ctx, 1100, 1500);
        assert!(app.lyrics_question_open());
        at(&mut app, &ctx, Some(1600), Vec::new());
        app.cancel_first_edit();
        at(&mut app, &ctx, None, Vec::new());
        assert!(app.lyrics_screen.game.is_none() && app.lyrics_screen.summary.is_none());
        assert_eq!(*app.lyrics_track().unwrap(), *original);
        assert_eq!(app.history_labels(), (None, None));
    }

    #[test]
    fn a_syllables_game_cuts_only_the_lines_that_get_a_take() {
        let folder = tempfile::tempdir().unwrap();
        let packs = folder.path().join("languages");
        let dir = packs.join("en");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("language.json"),
            r#"{"id": "en", "name": {}, "version": "1", "syllables": {"vowels": "aeiouy"}}"#,
        )
        .unwrap();
        let mut app = app(folder.path());
        app.set_language_packs(noh::languages::scan(&[packs]));
        app.subtitles.language = "en".into();
        let ctx = context();
        let cut = |app: &NohApp, line: usize| {
            edit::words_or_estimate(&app.lyrics_track().unwrap().cues[line])
                .iter()
                .any(|w| w.syllables.is_some())
        };
        game(&mut app, &ctx, Level::Syllable, 0);
        // The game shows every line's syllables (« Some·thing », « noth·ing »)…
        let g = app.lyrics_screen.game.as_ref().unwrap();
        let texts = |line: usize| -> Vec<&str> {
            g.units
                .iter()
                .filter(|u| u.cue == line)
                .map(|u| u.text.as_str())
                .collect()
        };
        assert!(
            texts(0).len() > 4 && texts(1).len() > 3,
            "{:?} {:?}",
            texts(0),
            texts(1)
        );
        // …but the lyrics are not touched before a take.
        assert!(!cut(&app, 0) && !cut(&app, 1));
        hold(&mut app, &ctx, 1100, 1400);
        at(
            &mut app,
            &ctx,
            Some(1500),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        // One syllable taken on line 1: that line has its splits, the others
        // are as they were.
        assert!(cut(&app, 0));
        assert!(!cut(&app, 1) && !cut(&app, 2));
        assert_eq!(app.lyrics_track().unwrap().cues[1].words, None);
        assert_eq!(app.history_labels().0, Some("sync 1 syllable"));
    }

    #[test]
    fn the_game_waits_when_the_player_that_could_slow_the_song_stops() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        app.lyrics_screen.test_live = Some(true);
        app.lyrics_screen.speed = 0.75;
        game(&mut app, &ctx, Level::Word, 0);
        at(&mut app, &ctx, Some(200), Vec::new());
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert!(g.paused.is_none() && app.lyrics_screen.speed == 0.75);
        hold(&mut app, &ctx, 1100, 1500);
        // The usual player takes over: it cannot slow down.
        app.lyrics_screen.test_live = Some(false);
        at(&mut app, &ctx, Some(1600), vec![space(true, false)]);
        at(&mut app, &ctx, Some(1650), vec![space(false, false)]);
        let g = app.lyrics_screen.game.as_ref().unwrap();
        assert_eq!(g.paused, None, "that Espace resumed the game");
        assert_eq!(g.holds.len(), 1, "and was not a take");
        assert_eq!(app.lyrics_screen.speed, 1.0);
        // The pause itself, before any key.
        let mut app = self::app(folder.path());
        app.lyrics_screen.test_live = Some(true);
        app.lyrics_screen.speed = 0.5;
        game(&mut app, &ctx, Level::Word, 0);
        app.lyrics_screen.test_live = Some(false);
        at(&mut app, &ctx, Some(300), Vec::new());
        assert_eq!(
            app.lyrics_screen.game.as_ref().unwrap().paused,
            Some(game::Pause::Engine)
        );
        assert_eq!(app.lyrics_screen.speed, 1.0);
        // Stopping keeps the take and leaves no speed behind.
        app.ls_end_game();
        assert!(app.lyrics_screen.game.is_none());
    }

    #[test]
    fn the_lyrics_are_written_after_the_game_not_during_it() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        let srt = app.project.track.path.clone().unwrap();
        let on_disk = std::fs::read_to_string(&srt).unwrap();
        game(&mut app, &ctx, Level::Line, 0);
        hold(&mut app, &ctx, 1200, 3400);
        assert!(app.has_unsaved_lyrics());
        // The write is due: it waits for the end of the game.
        app.lyrics_save.write_at = Some(Instant::now() - Duration::from_secs(1));
        at(&mut app, &ctx, Some(3500), Vec::new());
        assert!(app.has_unsaved_lyrics());
        assert_eq!(std::fs::read_to_string(&srt).unwrap(), on_disk);
        at(
            &mut app,
            &ctx,
            Some(3600),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        assert!(app.lyrics_screen.game.is_none());
        app.lyrics_save.write_at = Some(Instant::now() - Duration::from_secs(1));
        frame(&mut app, &ctx, Vec::new());
        assert!(!app.has_unsaved_lyrics(), "written once the game is over");
        assert!(
            std::fs::read_to_string(&srt)
                .unwrap()
                .contains("00:00:01,200 --> 00:00:03,400")
        );
    }

    #[test]
    fn the_summary_offers_to_go_on_at_the_next_line() {
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let ctx = context();
        game(&mut app, &ctx, Level::Line, 0);
        hold(&mut app, &ctx, 1200, 3400);
        at(
            &mut app,
            &ctx,
            Some(3500),
            vec![key(Key::Escape, Modifiers::NONE)],
        );
        frame(&mut app, &ctx, Vec::new());
        let again = app
            .lyrics_screen
            .drawn
            .again
            .expect("the summary's buttons");
        click(&mut app, &ctx, again.center());
        let g = app.lyrics_screen.game.as_ref().expect("a new game");
        assert_eq!(g.units[0].cue, 1, "from the line after the last take");
        assert!(app.lyrics_screen.summary.is_none());
        // The transport's button waits during a game: a click on it does nothing.
        assert!(app.lyrics_screen.game.is_some());
    }

    // ---- Every state fits, in every language ----

    /// The lines of the mockup's song around its longest one.
    const SONG: &[&str] = &[
        "Static on the window, night is running slow",
        "Every street I know is folding into one",
        "I keep the porch light on",
        "for the echo, not for you",
        "Paper boats in rain",
        "sinking where the gutters turn",
        "We were loud enough to burn",
        "now the quiet keeps the score",
        "Count the cars",
        "count the hours",
        "Hold the line, hold the line, we are almost home",
        "almost",
        "Glass in the pocket, light in the seams",
        "I learned your name in someone else's dreams",
    ];

    /// The screens the captures stage, by the same staging.
    const SCENES: &[&str] = &[
        "",
        // A game no unit of which is taken yet: its longest footer.
        "-game-first",
        // The notice of a change made outside NOH: alone, with the read-only
        // notice, and above a game's longest footer.
        "-outside",
        "-read-only-outside",
        "-game-first-outside",
        // The game waiting because the slowed playback stopped: its longest
        // prompt.
        "-game-engine",
        "-cut",
        "-word",
        "-syllables",
        "-game",
        "-game-hold",
        "-game-missed",
        "-summary",
        "-read-only",
    ];

    fn song_app(folder: &std::path::Path, language: Language, scene: &str) -> NohApp {
        let mut app = app(folder);
        app.locale.language = language;
        let cues = SONG
            .iter()
            .enumerate()
            .map(|(i, text)| {
                let start = 1000 + i as u64 * 4000;
                SubtitleCue::new(start, start + 3500, text.to_string())
            })
            .collect();
        app.replace_lyrics(Arc::new(SubtitleTrack {
            language: None,
            duration_ms: 60_000,
            cues,
        }));
        // The lyrics were changed here, then outside NOH.
        let outside = scene.ends_with("-outside");
        if outside {
            app.edit_track(|t| {
                t.cues[0].end_ms += 1;
                true
            });
            app.project.edit.as_mut().unwrap().outside_changed = true;
        }
        let scene = scene.strip_suffix("-outside").unwrap_or(scene);
        // The longest line is the one edited, and the one sung in a game.
        let game = scene.starts_with("-game") || scene == "-summary";
        app.stage_lyrics_screen(
            if game { 8 } else { 10 },
            match scene {
                "-game-first" | "-game-engine" => "-game",
                scene => scene,
            },
        );
        if scene == "-game-engine"
            && let Some(g) = &mut app.lyrics_screen.game
        {
            g.paused = Some(game::Pause::Engine);
        }
        if scene == "-game-first"
            && let Some(g) = &mut app.lyrics_screen.game
        {
            g.holds.clear();
            g.idx = 0;
        }
        if let Some(g) = &app.lyrics_screen.game {
            let at = g.hold.unwrap_or(g.guides[g.idx].start_ms);
            app.lyrics_screen.test_clock = Some(Some(at));
            app.lyrics_screen.test_live = Some(true);
        }
        app
    }

    /// A text as painted: all of it, whatever it was given to be drawn in.
    #[derive(Debug)]
    struct Painted {
        text: String,
        rect: Rect,
        clip: Rect,
        elided: bool,
        rows: usize,
    }

    fn painted(shape: &egui::Shape, clip: Rect, out: &mut Vec<Painted>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| painted(s, clip, out)),
            egui::Shape::Text(t) if !t.galley.text().trim().is_empty() => out.push(Painted {
                text: t.galley.text().to_string(),
                rect: t.galley.rect.translate(t.pos.to_vec2()),
                clip,
                elided: t.galley.elided,
                rows: t.galley.rows.len(),
            }),
            _ => {}
        }
    }

    /// One frame of the screen alone (under the header's height) or of the
    /// whole application, and every text it painted.
    fn texts_at(
        app: &mut NohApp,
        ctx: &egui::Context,
        size: egui::Vec2,
        whole: bool,
    ) -> Vec<Painted> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                if whole {
                    app.draw(ui);
                } else {
                    app_style::apply(ui.ctx());
                    app.lyrics_edit_frame(ui.ctx());
                    app.lyrics_screen_ui(ui);
                }
            },
        );
        output.textures_delta.clear();
        let mut out = Vec::new();
        for clipped in &output.shapes {
            painted(&clipped.shape, clipped.clip_rect, &mut out);
        }
        out
    }

    /// What is wrong with the screen as last drawn in `size` (its top at
    /// `top`), or nothing: a text cut by what it is drawn in, shortened with
    /// an ellipsis, wrapped where one row is meant, outside its zone or the
    /// window, or over another text; the header's ends over one another; the
    /// zones not one under the other.
    fn misfits(app: &NohApp, texts: &[Painted], size: egui::Vec2, top: f32) -> Vec<String> {
        let d = app.lyrics_screen.drawn;
        let compact = size.x < style::COMPACT_BELOW;
        let pad = if compact { space::L } else { space::XL };
        let game = app.lyrics_screen.game.is_some();
        let mut wrong = Vec::new();
        let bar = Rect::from_min_size(pos2(0.0, top), vec2(size.x, MODEBAR_H));
        let (Some(sync), Some(lanes), Some(foot)) = (d.sync, d.lanes, d.foot) else {
            return vec!["a zone was not drawn".into()];
        };
        // These scroll or clip what they show, by design: the lines of the
        // text, the picture's own captions, the track's words under its view.
        let own_clip: Vec<Rect> = if compact && game {
            vec![lanes]
        } else {
            [d.monitor, d.text, Some(lanes)]
                .into_iter()
                .flatten()
                .collect()
        };
        // The song's name is shortened when it is long; nothing else is.
        let lines = app
            .locale
            .language
            .plural("lyrics_screen.lines", SONG.len());
        let window = Rect::from_min_size(Pos2::ZERO, size);
        // (A text drawn by one of them is clipped inside it: told by its clip,
        // since the text itself may be far outside.)
        let within = |zone: &Rect, t: &Painted| {
            zone.contains(t.rect.center())
                || (t.clip.top() >= zone.top() - 1.0
                    && t.clip.bottom() <= zone.bottom() + 1.0
                    && t.clip.left() >= zone.left() - 1.0)
        };
        let fixed: Vec<&Painted> = texts
            .iter()
            .filter(|t| !own_clip.iter().any(|z| within(z, t)))
            .collect();
        let inside = |outer: Rect, inner: Rect| outer.expand(0.5).contains_rect(inner);
        // The narrow window scrolls all that is between its header and its
        // footer, but in a game: what is further down is not on screen yet.
        let body = Rect::from_min_max(bar.left_bottom(), foot.right_top());
        let further_down =
            |t: &Painted| compact && !game && body.contains_rect(t.clip) && !inside(body, t.rect);
        for t in &fixed {
            if t.rect.left() < pad - 0.5 || t.rect.right() > size.x - pad + 0.5 {
                wrong.push(format!("outside the window's margins: {t:?}"));
            }
            if further_down(t) {
                continue;
            }
            if !inside(t.clip, t.rect) {
                wrong.push(format!("cut by what it is drawn in: {t:?}"));
            }
            if !inside(window, t.rect) {
                wrong.push(format!("outside the window: {t:?}"));
            }
            if t.elided && !t.text.starts_with(&lines) {
                wrong.push(format!("shortened: {t:?}"));
            }
            for (name, zone, one_row) in [
                ("header", bar, true),
                ("sync bar", sync, !(compact && game)),
                ("footer", foot, !compact),
            ] {
                if zone.contains(t.rect.center()) {
                    if !inside(zone, t.rect) {
                        wrong.push(format!("out of the {name} {zone:?}: {t:?}"));
                    }
                    // (Why a game waits may take two rows of the prompt.)
                    if one_row && t.rows != 1 && Some(t.clip) != d.prompt {
                        wrong.push(format!("wrapped in the {name}: {t:?}"));
                    }
                }
            }
        }
        for (i, a) in fixed.iter().enumerate() {
            for b in &fixed[i + 1..] {
                if a.rect.shrink(0.5).intersects(b.rect.shrink(0.5)) {
                    wrong.push(format!("one over the other: {a:?} and {b:?}"));
                }
            }
        }
        // The text column scrolls its lines up and down, by design; nothing
        // of a line on screen is cut at its sides, shortened or over another.
        let on_screen = |t: &Painted| t.rect.intersect(t.clip).intersect(body);
        if let Some(doc) = d.text.filter(|_| !(compact && game)) {
            let column: Vec<&Painted> = texts
                .iter()
                .filter(|t| {
                    t.clip.top() >= doc.top() - 1.0
                        && t.clip.bottom() <= doc.bottom() + 1.0
                        && t.clip.left() >= doc.left() - 1.0
                        && t.clip.right() <= doc.right() + 1.0
                })
                // What of it is on screen, the list being scrolled.
                .filter(|t| on_screen(t).is_positive())
                .collect();
            if column.is_empty() {
                wrong.push("no text in the text column".into());
            }
            for t in &column {
                let sides = |outer: Rect| {
                    outer.left() - 0.5 <= t.rect.left() && t.rect.right() <= outer.right() + 0.5
                };
                if !sides(doc) || !sides(t.clip) {
                    wrong.push(format!("text column {doc:?}, cut at its sides: {t:?}"));
                }
                if t.elided {
                    wrong.push(format!("text column, shortened: {t:?}"));
                }
            }
            for (i, a) in column.iter().enumerate() {
                for b in &column[i + 1..] {
                    if on_screen(a)
                        .shrink(0.5)
                        .intersects(on_screen(b).shrink(0.5))
                    {
                        wrong.push(format!("text column, one over the other: {a:?} and {b:?}"));
                    }
                }
            }
            // The current line's buttons, when it shows them.
            if let (Some(cut), Some(join), Some(delete)) = (d.cut, d.join, d.delete)
                && !(doc.left() <= cut.left()
                    && cut.right() <= join.left()
                    && join.right() <= delete.left()
                    && delete.right() <= doc.right() + 0.5)
            {
                wrong.push(format!(
                    "line buttons in {doc:?}: {cut:?} {join:?} {delete:?}"
                ));
            }
        }
        match (d.title, d.undo, d.done) {
            (Some(title), Some(undo), Some(done)) => {
                if !(title.left() >= pad - 0.5
                    && title.right() <= undo.left()
                    && undo.right() <= done.left()
                    && done.right() <= size.x - pad + 0.5)
                {
                    wrong.push(format!("header: {title:?} {undo:?} {done:?}"));
                }
            }
            _ => wrong.push("the header's ends were not drawn".into()),
        }
        // One zone under the other, the footer ending with the window: with
        // no scrolling in the wide window, and in a game in the narrow one.
        if !compact || game {
            let stacked = bar.bottom() <= sync.top() + 0.5
                && sync.bottom() <= lanes.top() + 0.5
                && lanes.bottom() <= foot.top() + 0.5;
            if !stacked {
                wrong.push(format!("not stacked: {bar:?} {sync:?} {lanes:?} {foot:?}"));
            }
        }
        if (foot.bottom() - size.y).abs() > 0.5 || foot.height() > 160.0 {
            wrong.push(format!("footer {foot:?} in a window of {size:?}"));
        }
        wrong
    }

    /// The two windows S7 names, as the screen gets them under the header.
    fn windows() -> [egui::Vec2; 2] {
        [
            vec2(980.0, 850.0 - style::HEADER_H),
            vec2(420.0, 540.0 - style::HEADER_H),
        ]
    }

    #[test]
    fn every_state_of_the_screen_fits_both_windows_in_every_language() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        let mut wrong = Vec::new();
        for language in Language::ALL {
            for scene in SCENES {
                for size in windows() {
                    let mut app = song_app(folder.path(), language, scene);
                    let mut texts = Vec::new();
                    // The narrow footer takes its height from the frame before.
                    for _ in 0..4 {
                        texts = texts_at(&mut app, &ctx, size, false);
                    }
                    assert!(app.lyrics_screen.open, "{language:?} {scene}");
                    assert_eq!(
                        app.lyrics_screen.game.is_some(),
                        scene.starts_with("-game"),
                        "{language:?} {scene}: the game as staged"
                    );
                    // Settled: the next frames draw the same zones, the
                    // footer neither growing again nor going back to one row.
                    let settled = |app: &NohApp| {
                        let (s, d) = (&app.lyrics_screen, app.lyrics_screen.drawn);
                        (s.foot_wraps, s.foot_h, d.foot, d.foot_text, d.sync, d.lanes)
                    };
                    let now = settled(&app);
                    for _ in 0..3 {
                        texts_at(&mut app, &ctx, size, false);
                        assert_eq!(
                            settled(&app),
                            now,
                            "{language:?} «{scene}» {size:?}: the layout still moves"
                        );
                    }
                    for w in misfits(&app, &texts, size, 0.0) {
                        wrong.push(format!("{language:?} «{scene}» {}×{}: {w}", size.x, size.y));
                    }
                }
            }
        }
        assert!(
            wrong.is_empty(),
            "{} misfits:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }

    #[test]
    fn the_screen_starts_under_the_header_in_the_whole_window() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        for (size, scene) in [
            (vec2(980.0, 850.0), ""),
            (vec2(420.0, 540.0), ""),
            (vec2(420.0, 540.0), "-game-hold"),
        ] {
            let mut app = song_app(folder.path(), Language::Fr, scene);
            let mut texts = Vec::new();
            for _ in 0..4 {
                texts = texts_at(&mut app, &ctx, size, true);
            }
            assert!(app.lyrics_screen.open);
            let d = app.lyrics_screen.drawn;
            // The title is centred in the screen's own bar, right under the header.
            let title = d.title.unwrap();
            assert!(
                (title.center().y - (style::HEADER_H + MODEBAR_H / 2.0)).abs() <= 1.0,
                "{size:?}: {title:?}"
            );
            // The header's own texts are above the screen: left out here.
            let texts: Vec<Painted> = texts
                .into_iter()
                .filter(|t| t.rect.center().y > style::HEADER_H)
                .collect();
            let wrong = misfits(&app, &texts, size, style::HEADER_H);
            assert!(
                wrong.is_empty(),
                "{size:?} «{scene}»:\n{}",
                wrong.join("\n")
            );
        }
    }

    #[test]
    fn the_track_stays_where_it_is_when_a_game_s_footer_gets_shorter() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        let size = windows()[0];
        // French: the first instructions of a game take two rows at 980.
        let mut app = song_app(folder.path(), Language::Fr, "-game-first");
        for _ in 0..4 {
            texts_at(&mut app, &ctx, size, false);
        }
        let first = app.lyrics_screen.drawn;
        let (foot, lanes) = (first.foot.unwrap(), first.lanes.unwrap());
        assert!(foot.height() > FOOT_H + 0.5, "two rows: {foot:?}");
        // The first word is taken: the footer's text is the short one now.
        let guide = app.lyrics_screen.game.as_ref().unwrap().guides[0];
        hold(&mut app, &ctx, guide.start_ms, guide.end_ms);
        for _ in 0..4 {
            texts_at(&mut app, &ctx, size, false);
        }
        let d = app.lyrics_screen.drawn;
        assert_eq!(app.lyrics_screen.game.as_ref().unwrap().holds.len(), 1);
        assert!(d.foot_text.unwrap().height() < first.foot_text.unwrap().height());
        assert_eq!((d.foot.unwrap(), d.lanes.unwrap()), (foot, lanes));
        // The game over, the footer is one row high again.
        app.ls_end_game();
        for _ in 0..4 {
            texts_at(&mut app, &ctx, size, false);
        }
        let d = app.lyrics_screen.drawn;
        assert!(
            (d.foot.unwrap().height() - FOOT_H).abs() <= 0.5,
            "{:?}",
            d.foot
        );
    }

    #[test]
    fn the_speeds_read_from_normal_to_slowest_in_both_windows() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        for size in windows() {
            let mut app = song_app(folder.path(), Language::En, "-game");
            let mut texts = Vec::new();
            for _ in 0..4 {
                texts = texts_at(&mut app, &ctx, size, false);
            }
            let left = |name: &str| {
                texts
                    .iter()
                    .find(|t| t.text == name)
                    .unwrap_or_else(|| panic!("{size:?}: {name} is drawn"))
                    .rect
                    .left()
            };
            assert!(
                left("1×") < left("0.75×") && left("0.75×") < left("0.5×"),
                "{size:?}: {} {} {}",
                left("1×"),
                left("0.75×"),
                left("0.5×")
            );
        }
    }

    #[test]
    fn a_line_longer_than_the_prompt_keeps_the_expected_unit_in_sight() {
        let folder = tempfile::tempdir().unwrap();
        let ctx = context();
        // Thirty words: more than the prompt holds at its smallest size.
        let line = (1..=30)
            .map(|n| format!("word{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        for size in windows() {
            let mut app = app(folder.path());
            app.replace_lyrics(Arc::new(SubtitleTrack {
                language: None,
                duration_ms: 60_000,
                cues: vec![SubtitleCue::new(1000, 31_000, line.clone())],
            }));
            app.stage_lyrics_game(0, Level::Word, 0, "");
            app.lyrics_screen.test_live = Some(true);
            let units = app.lyrics_screen.game.as_ref().unwrap().units.len();
            assert_eq!(units, 30);
            let mut seen = Vec::new();
            for idx in 0..units {
                let g = app.lyrics_screen.game.as_mut().unwrap();
                g.idx = idx;
                app.lyrics_screen.test_clock = Some(Some(g.guides[idx].start_ms));
                let texts = texts_at(&mut app, &ctx, size, false);
                let d = app.lyrics_screen.drawn;
                let (prompt, expected) = (d.prompt.unwrap(), d.expected.unwrap());
                assert!(
                    prompt.expand(0.5).contains_rect(expected),
                    "{size:?} word {idx}: {expected:?} in {prompt:?}"
                );
                // The word drawn there is the expected one, whole.
                let name = format!("word{}", idx + 1);
                let word = texts
                    .iter()
                    .find(|t| t.text == name && prompt.contains(t.rect.center()))
                    .unwrap_or_else(|| panic!("{size:?}: {name} is in the prompt"));
                assert!(prompt.expand(0.5).contains_rect(word.rect), "{word:?}");
                // Where the line's two ends are, in or out of sight.
                let end = |name: &str, right: bool| {
                    let t = texts
                        .iter()
                        .find(|t| t.text == name && t.clip == prompt)
                        .unwrap();
                    if right { t.rect.right() } else { t.rect.left() }
                };
                seen.push((expected.left(), end("word1", false), end("word30", true)));
            }
            // The rule: the line starts at the prompt's left edge; once it
            // slides, the expected word is a quarter of the way in; and the
            // line never ends before the prompt's right edge.
            let prompt = app.lyrics_screen.drawn.prompt.unwrap();
            let quarter = prompt.left() + prompt.width() * 0.25;
            let sliding = seen
                .iter()
                .filter(|(left, _, _)| (left - quarter).abs() <= 0.5)
                .count();
            assert!(sliding >= 5, "{size:?}: {seen:?}");
            for (idx, (left, first, last)) in seen.iter().enumerate() {
                let at_start = (first - prompt.left()).abs() <= 0.5;
                let at_end = (last - prompt.right()).abs() <= 0.5;
                assert!(
                    (left - quarter).abs() <= 0.5 || at_start || at_end,
                    "{size:?} word {idx}: {left} (quarter {quarter}, line {first}..{last})"
                );
                assert!(*last >= prompt.right() - 0.5, "{size:?} word {idx}");
            }
            assert!(
                (seen[0].1 - prompt.left()).abs() <= 0.5,
                "starts at the left edge"
            );
            assert!(
                (seen[29].2 - prompt.right()).abs() <= 0.5,
                "ends at the right edge"
            );
        }
    }

    #[test]
    fn a_long_line_of_syllables_slides_with_its_separators() {
        let folder = tempfile::tempdir().unwrap();
        let packs = folder.path().join("languages");
        let dir = packs.join("en");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("language.json"),
            r#"{"id": "en", "name": {}, "version": "1", "syllables": {"vowels": "aeiouy"}}"#,
        )
        .unwrap();
        let ctx = context();
        let line = ["something", "nothing", "wonderful", "yesterday"]
            .repeat(5)
            .join(" ");
        for size in windows() {
            let mut app = app(folder.path());
            app.set_language_packs(noh::languages::scan(std::slice::from_ref(&packs)));
            app.subtitles.language = "en".into();
            app.replace_lyrics(Arc::new(SubtitleTrack {
                language: None,
                duration_ms: 60_000,
                cues: vec![SubtitleCue::new(1000, 31_000, line.clone())],
            }));
            frame_at(&mut app, &ctx, size, Vec::new());
            game(&mut app, &ctx, Level::Syllable, 0);
            app.lyrics_screen.test_live = Some(true);
            let units = app.lyrics_screen.game.as_ref().unwrap().units.len();
            assert!(units > 20, "the words are cut in syllables: {units}");
            let mut slid = 0;
            for idx in 0..units {
                let g = app.lyrics_screen.game.as_mut().unwrap();
                g.idx = idx;
                app.lyrics_screen.test_clock = Some(Some(g.guides[idx].start_ms));
                let texts = texts_at(&mut app, &ctx, size, false);
                let d = app.lyrics_screen.drawn;
                let (prompt, expected) = (d.prompt.unwrap(), d.expected.unwrap());
                assert!(
                    prompt.expand(0.5).contains_rect(expected),
                    "{size:?} syllable {idx}: {expected:?} in {prompt:?}"
                );
                // The separators are pieces of the line as the syllables are:
                // drawn between them, on the same row.
                let dots: Vec<&Painted> = texts
                    .iter()
                    .filter(|t| t.text == "·" && t.clip == prompt)
                    .collect();
                assert!(!dots.is_empty(), "{size:?} syllable {idx}");
                for dot in dots {
                    assert!((dot.rect.center().y - prompt.center().y).abs() <= 1.0);
                }
                let quarter = prompt.left() + prompt.width() * 0.25;
                slid += usize::from((expected.left() - quarter).abs() <= 0.5);
            }
            assert!(slid >= 5, "{size:?}: {slid}");
        }
    }

    const FRAME_BUDGET_MS: f64 = 2.0;

    /// Opt-in CPU frame cost of the lyrics screen at 980 × 850 (less the
    /// application's header), tessellation included, beside the main screen's
    /// own with the same song: a real waveform and up to 120 lines.
    #[test]
    #[ignore = "opt-in performance measurement; requires NOH_QA_WAV and NOH_FFMPEG"]
    fn lyrics_screen_frame_cost() {
        let wav = std::env::var_os("NOH_QA_WAV").expect("NOH_QA_WAV");
        let ffmpeg = std::env::var_os("NOH_FFMPEG").expect("NOH_FFMPEG");
        let waveform = noh::waveform::extract(
            std::path::Path::new(&wav),
            std::path::Path::new(&ffmpeg),
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap();
        let duration = (waveform.duration * 1000.0) as u64;
        let folder = tempfile::tempdir().unwrap();
        let mut app = app(folder.path());
        let lines = (duration / 1500).min(120);
        let cues = (0..lines)
            .map(|i| {
                let start = i * duration / lines;
                SubtitleCue::new(
                    start,
                    start + 1200,
                    format!("Hold the line number {i} again"),
                )
            })
            .collect();
        app.project.duration_ms = duration;
        app.replace_lyrics(Arc::new(SubtitleTrack {
            language: None,
            duration_ms: duration,
            cues,
        }));
        app.project.waveform = Some(Arc::new(waveform));
        let ctx = context();
        // Beside it, the main screen's own cost in the same build, with the
        // same song and lyrics, the lyrics screen closed.
        // A staged state keeps the song's length and waveform without the
        // song's file being read again at each frame.
        app.wav = Some(std::path::PathBuf::from(&wav));
        app.wav_seconds = Some(duration as f64 / 1000.0);
        app.capture_state = Some("frame-cost".into());
        // Last, a game at the Words level with Espace held all along: the
        // prompt, the guides, the takes and the growing hold are drawn.
        // And the screen read-only, while a job locks the lyrics.
        let cases = [
            (None, false, false),
            (Some(Level::Line), false, false),
            (Some(Level::Syllable), false, false),
            (Some(Level::Word), true, false),
            (Some(Level::Line), false, true),
        ];
        // All of them once unmeasured, then measured: whatever came first in
        // a single pass cost about 0.2 ms more per frame than in any other
        // place (the processor and the caches not yet up to speed), which
        // set the budget too high when that was the main screen.
        for (measured, (level, playing, locked)) in [false, true]
            .into_iter()
            .flat_map(|measured| cases.into_iter().map(move |case| (measured, case)))
        {
            app.cancelling = locked;
            match level {
                Some(level) => {
                    if !app.lyrics_screen.open {
                        app.open_lyrics_screen(Some(0));
                    }
                    app.lyrics_screen.level = level;
                }
                None => app.close_lyrics_screen(),
            }
            if playing {
                app.lyrics_screen.test_clock = Some(Some(0));
                app.ls_start_game(&ctx);
                assert!(app.lyrics_screen.game.is_some());
            }
            let mut samples = Vec::new();
            for n in 0..620u64 {
                // The playhead crosses the song: the track turns its page.
                app.mini_preview.cursor_ms = n * 97 % duration;
                app.lyrics_screen.view_start = (n / 60) * 8000 % duration;
                if let Some(g) = &mut app.lyrics_screen.game {
                    // Held since a little before the playhead, on a unit that
                    // moves on with it; the game never waits.
                    let now = n * 97 % duration;
                    app.lyrics_screen.test_clock = Some(Some(now));
                    g.paused = None;
                    g.waiting = None;
                    g.idx = (n as usize / 6) % g.units.len();
                    g.hold = Some(now.saturating_sub(300));
                }
                let height = if level.is_some() { 802.0 } else { 850.0 };
                let started = Instant::now();
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, height))),
                        events: vec![egui::Event::PointerMoved(pos2(
                            10.0 + (n % 900) as f32,
                            700.0,
                        ))],
                        ..Default::default()
                    },
                    |ui| {
                        if level.is_some() {
                            app_style::apply(ui.ctx());
                            app.lyrics_screen_ui(ui);
                        } else {
                            app.draw(ui);
                        }
                    },
                );
                // No renderer takes the texture updates here.
                output.textures_delta.clear();
                std::hint::black_box(ctx.tessellate(output.shapes, output.pixels_per_point));
                if n >= 20 {
                    samples.push(started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            assert!(app.project.waveform.is_some() && app.project.duration_ms == duration);
            if !measured {
                continue;
            }
            samples.sort_by(f64::total_cmp);
            println!(
                "{} lines={lines} frames={} p50_ms={:.3} p95_ms={:.3} max_ms={:.3}",
                match level {
                    None => "MAIN_SCREEN".to_string(),
                    Some(level) if playing => format!("LYRICS_SCREEN game level={level:?}"),
                    Some(level) if locked => format!("LYRICS_SCREEN read-only level={level:?}"),
                    Some(level) => format!("LYRICS_SCREEN level={level:?}"),
                },
                samples.len(),
                samples[300],
                samples[570],
                samples[599]
            );
            // The budget, in an optimised build: 95 frames in 100 under 2 ms,
            // an eighth of a frame at 60 Hz. (Measured in November 2026: 0.5
            // ms for the main screen, 0.7 to 0.9 ms for the lyrics screen.)
            if !cfg!(debug_assertions) {
                assert!(samples[570] < FRAME_BUDGET_MS, "p95 {:.3} ms", samples[570]);
            }
        }
    }
}
