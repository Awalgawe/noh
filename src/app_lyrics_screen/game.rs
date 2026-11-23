//! The lyrics screen's sync game (mockup v5): the song plays, and Espace is
//! held while each line, word or syllable is sung. Key down is its start, key
//! up its end. The takes are written as they come and are one undo step.
use super::*;

/// The track shows this much during a game: more for lines, less for words
/// and syllables; half of it in the narrow window.
const VIEW_LINES_MS: u64 = 14_000;
const VIEW_UNITS_MS: u64 = 9_000;
/// Playback starts this long before the first unit, and resumes this long
/// before a unit that is taken again.
const LEAD_MS: u64 = 2_000;
const RETAKE_MS: u64 = 1_500;
/// A second Retour arrière this soon after the first goes one more unit back.
const TWICE: Duration = Duration::from_millis(700);
/// The shortest take a key press gives.
const MIN_HOLD_MS: u64 = 80;
/// Playback speeds offered during a game (the voice keeps its pitch).
pub(super) const SPEEDS: [f64; 3] = [1.0, 0.75, 0.5];

/// One thing to take: a line, a word, or a syllable.
pub(super) struct GameUnit {
    pub cue: usize,
    pub word: usize,
    pub syllable: usize,
    pub text: String,
}

/// Why playback waits in the middle of a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pause {
    /// The expected unit went by without a press.
    Missed,
    /// The player that could slow the song stopped; the usual one took over.
    Engine,
    /// A question about the lyrics file is open.
    Question,
    /// That question was answered: playback waits for Espace.
    Answered,
}

pub(super) struct Game {
    pub level: Level,
    pub units: Vec<GameUnit>,
    /// Where each unit is expected: the known time, or the letter estimate.
    pub guides: Vec<edit::Guide>,
    /// The unit expected next.
    pub idx: usize,
    /// The takes so far: (unit, the held key).
    pub holds: Vec<(usize, edit::Hold)>,
    /// Espace is down since this song time.
    pub hold: Option<u64>,
    /// The lyrics when the game started.
    before: Arc<SubtitleTrack>,
    /// The same with, at the Syllables level, the language's split of every
    /// word of the lines to play: what the prompt and the guides show. A
    /// line's splits reach the lyrics when that line receives a take.
    split: Arc<SubtitleTrack>,
    /// The lyrics as the takes last left them.
    applied: Option<Arc<SubtitleTrack>>,
    /// The game has an undo step of its own.
    stepped: bool,
    pub paused: Option<Pause>,
    pub redos: u32,
    pub last_back: Option<Instant>,
    /// Playback was asked from this time: the player's clock counts once it
    /// is near it (it may publish an older position once more after a seek).
    pub waiting: Option<u64>,
    started: bool,
    /// The engine playing when the game's playback started could slow down.
    was_live: bool,
    /// The speed the player was last given.
    given_speed: f64,
    /// Why the last press was not kept, or what changed (a locale key).
    pub notice: Option<&'static str>,
    key: u64,
}

/// What the last game did, shown in the footer until something else is done.
#[derive(Clone, Copy)]
pub(super) struct Summary {
    pub count: usize,
    pub level: Level,
    pub lines: usize,
    /// The last line a take was made on.
    pub last: usize,
}

/// The units of the lines from `from` on, at `level`, with their guides.
fn game_units(
    track: &SubtitleTrack,
    level: Level,
    from: usize,
) -> (Vec<GameUnit>, Vec<edit::Guide>) {
    let mut units = Vec::new();
    let mut guides = Vec::new();
    for (i, cue) in track.cues.iter().enumerate().skip(from) {
        if level == Level::Line {
            units.push(GameUnit {
                cue: i,
                word: 0,
                syllable: 0,
                text: cue.text.replace('\n', " "),
            });
            guides.push(edit::Guide {
                cue: i,
                start_ms: cue.start_ms,
                end_ms: cue.end_ms,
            });
            continue;
        }
        // A word that is not cut is one unit at the Syllables level too. The
        // Words and Syllables levels take the main words; « Chœurs » takes the
        // backing vocals, whole.
        let backing = noh::subtitle_track::backing_flags(&cue.text);
        let mut syllable = 0;
        let mut last_word = usize::MAX;
        for u in shown_units(cue, level == Level::Syllable) {
            if backing.get(u.word).copied().unwrap_or(false) != (level == Level::Backing) {
                continue;
            }
            syllable = if u.word == last_word { syllable + 1 } else { 0 };
            last_word = u.word;
            units.push(GameUnit {
                cue: i,
                word: u.word,
                syllable,
                text: u.text,
            });
            guides.push(edit::Guide {
                cue: i,
                start_ms: u.start,
                end_ms: u.end,
            });
        }
    }
    (units, guides)
}

/// The player's time counts as the game's once it is near where playback was
/// asked from.
fn near(now: u64, target: u64) -> bool {
    now + 250 >= target && now <= target + 1500
}

impl State {
    pub(super) fn game_span(&self, compact: bool) -> Option<u64> {
        let g = self.game.as_ref()?;
        let span = if g.level == Level::Line {
            VIEW_LINES_MS
        } else {
            VIEW_UNITS_MS
        };
        Some(if compact { span / 2 } else { span })
    }
}

impl NohApp {
    /// A sync game runs: the lyrics are not written and not undone meanwhile.
    pub(crate) fn lyrics_game_running(&self) -> bool {
        self.lyrics_screen.game.is_some()
    }

    /// The song time of a key event: the player's own while it plays.
    fn ls_game_clock(&self) -> Option<u64> {
        #[cfg(test)]
        if let Some(clock) = self.lyrics_screen.test_clock {
            return clock;
        }
        self.mini_playing().then(|| self.mini_position())
    }

    /// The engine that can slow the song plays in the player's place.
    fn ls_live(&self) -> bool {
        #[cfg(test)]
        if let Some(live) = self.lyrics_screen.test_live {
            return live;
        }
        self.mini_live()
    }

    /// Give the player the game's speed; false when it cannot slow down.
    fn ls_give_speed(&self, speed: f64) -> bool {
        #[cfg(test)]
        if self.lyrics_screen.test_live == Some(true) {
            return true;
        }
        self.mini_preview
            .player
            .as_ref()
            .is_some_and(|p| p.set_speed(speed))
    }

    fn ls_sync_label(&self, level: Level, count: usize) -> String {
        self.locale.language.plural(
            match level {
                Level::Line => "lyrics_screen.did_sync_lines",
                Level::Word => "lyrics_screen.did_sync_words",
                Level::Syllable => "lyrics_screen.did_sync_syllables",
                Level::Backing => "lyrics_screen.did_sync_backing",
            },
            count,
        )
    }

    /// Play from `ms` for the game, and wait for the player to get there.
    fn ls_game_play(&mut self, ms: u64, ctx: &egui::Context) {
        let (lo, hi) = self.mini_bounds();
        let target = ms.clamp(lo, hi.saturating_sub(1).max(lo));
        if let Some(g) = &mut self.lyrics_screen.game {
            g.waiting = Some(target);
            g.paused = None;
            g.hold = None;
        }
        self.mini_play_from(target, ctx);
    }

    /// « Commencer à la ligne N »: from the current line to the end of the
    /// song, at the chosen level. At the Syllables level every word of those
    /// lines that is not cut yet gets the language's split first.
    pub(super) fn ls_start_game(&mut self, ctx: &egui::Context) {
        if self.lyrics_screen.game.is_some()
            || self.ls_reading()
            || self.lyrics_locked()
            || self.lyrics_question_open()
            || self.lyrics_question_pending()
            || self.project.duration_ms == 0
        {
            return;
        }
        self.ls_leave_row();
        self.end_edit_run();
        if let Some(id) = ctx.memory(|m| m.focused()) {
            ctx.memory_mut(|m| m.surrender_focus(id));
        }
        let s = &mut self.lyrics_screen;
        (s.word, s.syl, s.summary, s.caret, s.drag, s.preview) =
            (None, None, None, None, None, None);
        s.text_focus = false;
        let (level, from) = (s.level, s.cur);
        let key = egui::Id::new(("lyrics-screen-game", Instant::now())).value();
        let Some(track) = self.lyrics_track() else {
            return;
        };
        let mut split = (*track).clone();
        if level == Level::Syllable {
            let language = self.sung_language();
            let packs = self.language_packs();
            let count = split.cues.len();
            edit::propose_syllables(&mut split, from..count, |w| {
                packs.propose(w, language.as_deref()).0
            });
        }
        let (units, guides) = game_units(&split, level, from);
        let Some(first) = guides.first().map(|g| g.start_ms) else {
            return;
        };
        let compact = self.lyrics_screen.compact;
        self.lyrics_screen.game = Some(Game {
            level,
            units,
            guides,
            idx: 0,
            holds: Vec::new(),
            hold: None,
            applied: None,
            before: track,
            split: Arc::new(split),
            stepped: false,
            paused: None,
            redos: 0,
            last_back: None,
            waiting: None,
            started: false,
            was_live: false,
            given_speed: 1.0,
            notice: None,
            key,
        });
        let lead = if compact {
            LEAD_MS * 3 / 4
        } else {
            LEAD_MS * 3 / 2
        };
        self.lyrics_screen.view_start = first.saturating_sub(lead);
        self.ls_game_play(first.saturating_sub(LEAD_MS), ctx);
    }

    /// Once per frame while a game runs: the clock, the player that may have
    /// changed, the line the game is on, and the unit that went by unheld.
    pub(super) fn ls_game_frame(&mut self, ctx: &egui::Context) {
        if self.lyrics_screen.game.is_none() {
            return;
        }
        ctx.request_repaint();
        // A staged capture state has no player: its game stays as staged.
        if self.capture_overlay.is_some() {
            return;
        }
        // A question about the lyrics file (the first edit of an .srt): the
        // game waits for the answer.
        if self.lyrics_question_open() || self.lyrics_question_pending() {
            self.mini_pause();
            if let Some(g) = &mut self.lyrics_screen.game {
                g.paused = Some(Pause::Question);
                g.hold = None;
            }
            return;
        }
        let Some(g) = &self.lyrics_screen.game else {
            return;
        };
        if g.paused == Some(Pause::Question) {
            // Answered. « Annuler » took the game's step back: the game ends
            // with nothing; otherwise Espace resumes.
            let kept = match (&g.applied, self.lyrics_track()) {
                (Some(a), Some(now)) => Arc::ptr_eq(a, &now) || **a == *now,
                (None, _) => true,
                (Some(_), None) => false,
            };
            let (key, stepped) = (g.key, g.stepped);
            if !kept {
                self.ls_drop_game();
            } else {
                // The step the question was asked on is still the game's one
                // step: the next takes grow it instead of starting another.
                if stepped {
                    self.rearm_edit_run(key);
                }
                if let Some(g) = &mut self.lyrics_screen.game {
                    g.paused = Some(Pause::Answered);
                }
            }
            return;
        }
        if g.paused.is_some() {
            return;
        }
        let now = self.ls_game_clock();
        let live = self.ls_live();
        let speed = self.lyrics_screen.speed;
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        if let Some(target) = g.waiting {
            match now {
                Some(now) if near(now, target) => {
                    g.waiting = None;
                    g.started = true;
                    g.was_live = live;
                }
                // Still opening, or a time from before the seek.
                _ => return,
            }
        }
        let Some(now) = now else {
            // Playback stopped by itself: the song ended, or its player closed.
            if g.started {
                self.ls_end_game();
            }
            return;
        };
        if g.was_live && !live {
            // The usual player took over: it plays at normal speed only.
            g.was_live = false;
            g.paused = Some(Pause::Engine);
            g.hold = None;
            g.given_speed = 1.0;
            self.lyrics_screen.speed = 1.0;
            self.mini_pause();
            return;
        }
        if live && g.given_speed != speed {
            let slowed = self.ls_give_speed(speed);
            if let Some(g) = &mut self.lyrics_screen.game {
                g.given_speed = if slowed { speed } else { 1.0 };
            }
            if !slowed {
                self.lyrics_screen.speed = 1.0;
            }
        }
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        if let Some(u) = g.units.get(g.idx) {
            self.lyrics_screen.cur = u.cue;
        }
        if g.hold.is_none() && edit::passed(now, &g.guides, g.idx, g.level, false) {
            g.paused = Some(Pause::Missed);
            self.mini_pause();
        }
    }

    /// The game's keys, whatever has the focus: Espace down and up (repeats
    /// ignored), Retour arrière takes again, Échap stops and keeps the takes.
    pub(super) fn ls_game_keys(&mut self, ctx: &egui::Context) {
        let (spaces, back, stop) = ctx.input_mut(|i| {
            let mut spaces = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Key {
                    key: Key::Space,
                    pressed,
                    repeat,
                    ..
                } => {
                    if !*repeat {
                        spaces.push(*pressed);
                    }
                    false
                }
                // The space a held key types belongs to no field either.
                egui::Event::Text(t) if t == " " => false,
                _ => true,
            });
            (
                spaces,
                take_key(i, Key::Backspace, |_| true),
                take_key(i, Key::Escape, |_| true),
            )
        });
        for down in spaces {
            if down {
                self.ls_press_down(ctx);
            } else {
                self.ls_press_up();
            }
        }
        if back {
            self.ls_game_back(ctx);
        }
        if stop {
            self.ls_end_game();
        }
    }

    /// Espace goes down: the expected unit starts now. While the game waits,
    /// the press resumes it just before that unit instead.
    pub(super) fn ls_press_down(&mut self, ctx: &egui::Context) {
        let now = self.ls_game_clock();
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        if g.hold.is_some() || g.idx >= g.units.len() || g.paused == Some(Pause::Question) {
            return;
        }
        if g.paused.is_some() {
            let from = g.guides[g.idx].start_ms.saturating_sub(RETAKE_MS);
            self.ls_game_play(from, ctx);
            return;
        }
        let Some(now) = now.filter(|_| g.waiting.is_none() && g.started) else {
            return;
        };
        g.notice = None;
        // The next line has started and units of this one are left: they
        // were passed. The press is not used, so a slip never spreads.
        if edit::passed(now, &g.guides, g.idx, g.level, true) {
            g.paused = Some(Pause::Missed);
            self.mini_pause();
            return;
        }
        // Playback was taken back before the last take: not a take.
        if g.holds.last().is_some_and(|(_, h)| now <= h.start_ms) {
            return;
        }
        g.hold = Some(now);
    }

    /// Espace comes up: the unit ends now, and the take is written.
    pub(super) fn ls_press_up(&mut self) {
        let now = self.ls_game_clock();
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        let Some(start) = g.hold.take() else {
            return;
        };
        let Some(u) = g.units.get(g.idx) else {
            return;
        };
        let end = now.unwrap_or(start).max(start + MIN_HOLD_MS);
        g.holds.push((
            g.idx,
            edit::Hold {
                cue: u.cue,
                word: u.word,
                syllable: u.syllable,
                start_ms: start,
                end_ms: end,
            },
        ));
        g.idx += 1;
        if !self.ls_game_apply()
            && let Some(g) = &mut self.lyrics_screen.game
        {
            // The take does not fit: it alone is dropped, and said.
            if let Some((idx, _)) = g.holds.pop() {
                g.idx = idx;
            }
            g.notice = Some("lyrics_screen.game_refused");
            return;
        }
        if self
            .lyrics_screen
            .game
            .as_ref()
            .is_some_and(|g| g.idx >= g.units.len())
        {
            self.ls_end_game();
        }
    }

    /// Retour arrière (or « Refaire »): the unit in hand is taken again. With
    /// Espace held, that is the unit being held (its press is dropped); else
    /// the last take is dropped. Pressed again within 0.7 s, one more take
    /// goes. Playback goes back just before the unit to take.
    pub(super) fn ls_game_back(&mut self, ctx: &egui::Context) {
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        if g.paused == Some(Pause::Question) {
            return;
        }
        let now = Instant::now();
        let twice = g.last_back.is_some_and(|t| now.duration_since(t) < TWICE);
        g.last_back = Some(now);
        let held = g.hold.take().is_some();
        g.redos += 1;
        g.notice = None;
        if (!held || twice)
            && let Some((idx, _)) = g.holds.pop()
        {
            g.idx = idx;
        }
        let from = g
            .guides
            .get(g.idx)
            .map_or(0, |u| u.start_ms.saturating_sub(RETAKE_MS));
        self.ls_game_apply();
        self.ls_game_play(from, ctx);
    }

    /// Write the takes into the lyrics, all of them from the lyrics the game
    /// started with: one undo step, named after their count. False when the
    /// last take cannot apply.
    fn ls_game_apply(&mut self) -> bool {
        let Some(g) = &self.lyrics_screen.game else {
            return false;
        };
        let holds: Vec<edit::Hold> = g.holds.iter().map(|h| h.1).collect();
        // From the lyrics the game started with; the lines that have a take
        // come with their proposed splits, the others stay as they were.
        let mut base = (*g.before).clone();
        for h in &holds {
            if let (Some(to), Some(from)) = (base.cues.get_mut(h.cue), g.split.cues.get(h.cue)) {
                *to = from.clone();
            }
        }
        let next = if holds.is_empty() {
            Some(base)
        } else {
            edit::apply_holds(&base, g.level, &holds)
        };
        let Some(next) = next else {
            return false;
        };
        let (key, label) = (g.key, self.ls_sync_label(g.level, holds.len()));
        let changed = self.edit_track_run(key, label, |t| {
            *t = next;
            true
        });
        let now = self.lyrics_track();
        if let Some(g) = &mut self.lyrics_screen.game {
            g.stepped |= changed;
            g.applied = now;
        }
        true
    }

    /// « Arrêter », Échap, or the last unit taken: the takes stay, as one
    /// undo step, and the footer says what was done. A game with no take
    /// leaves the lyrics as they were.
    pub(super) fn ls_end_game(&mut self) {
        if self
            .lyrics_screen
            .game
            .as_ref()
            .is_some_and(|g| g.hold.is_some())
        {
            self.ls_press_up();
        }
        let Some(g) = self.lyrics_screen.game.take() else {
            return;
        };
        self.ls_game_over();
        let count = g.holds.len();
        if count == 0 {
            // Only the proposed splits, if any: taken back with the game.
            // Not while a job locks the lyrics (taking them back would change
            // what the job reads): they stay as the game's undo step.
            if g.stepped && !self.lyrics_locked() {
                self.take_back_edit();
            }
            return;
        }
        let mut lines: Vec<usize> = g.holds.iter().map(|(_, h)| h.cue).collect();
        lines.dedup();
        let last = *lines.last().unwrap_or(&0);
        self.lyrics_screen.cur = last;
        self.lyrics_screen.scroll_to_cur = true;
        self.lyrics_screen.summary = Some((
            Summary {
                count,
                level: g.level,
                lines: lines.len(),
                last,
            },
            self.project.edit_revision,
        ));
    }

    /// A job locks the lyrics during a game: it stops and keeps its takes.
    /// The unit being held is not taken (`ls_end_game` would take it).
    pub(super) fn ls_lock_game(&mut self) {
        let Some(g) = &mut self.lyrics_screen.game else {
            return;
        };
        g.hold = None;
        self.ls_end_game();
    }

    /// The game ends with nothing kept by it (its step was taken back by the
    /// answer to a question).
    fn ls_drop_game(&mut self) {
        self.lyrics_screen.game = None;
        self.ls_game_over();
    }

    /// Playback pauses and the player goes back to normal speed.
    fn ls_game_over(&mut self) {
        self.mini_pause();
        if let Some(p) = &self.mini_preview.player {
            p.set_speed(1.0);
        }
        self.end_edit_run();
        self.lyrics_screen.draft = None;
    }

    /// A capture state: a game on line `i` with `taken` units held, the next
    /// one being held (`hold`) or missed (`missed`); nothing plays.
    pub(super) fn stage_lyrics_game(&mut self, i: usize, level: Level, taken: usize, scene: &str) {
        let Some(track) = self.lyrics_track() else {
            return;
        };
        // The capture shows the game, not the first-edit question of an .srt.
        if let Some(path) = self.project.track.path.clone() {
            self.trust_lyrics(path);
        }
        self.lyrics_screen.level = level;
        let (units, guides) = game_units(&track, level, i);
        if units.is_empty() {
            return;
        }
        let first = guides[0].start_ms;
        let mut g = Game {
            level,
            units,
            guides,
            idx: 0,
            holds: Vec::new(),
            hold: None,
            applied: None,
            split: track.clone(),
            before: track,
            stepped: false,
            paused: None,
            redos: 0,
            last_back: None,
            waiting: None,
            started: true,
            was_live: true,
            given_speed: 1.0,
            notice: None,
            key: 0,
        };
        // Takes a little late and a little short of their guides, as a hand does.
        for n in 0..taken.min(g.units.len().saturating_sub(1)) {
            let (u, guide) = (&g.units[n], g.guides[n]);
            let length = (guide.end_ms - guide.start_ms).max(200);
            g.holds.push((
                n,
                edit::Hold {
                    cue: u.cue,
                    word: u.word,
                    syllable: u.syllable,
                    start_ms: guide.start_ms + 90,
                    end_ms: guide.start_ms + 90 + length * 7 / 10,
                },
            ));
            g.idx = n + 1;
        }
        let at = g.guides[g.idx].start_ms;
        let playhead = match scene {
            "-hold" => {
                g.hold = Some(at + 60);
                at + 60 + (g.guides[g.idx].end_ms - at) / 2
            }
            "-missed" => {
                g.paused = Some(Pause::Missed);
                g.guides[g.idx].end_ms + edit::MISS_AFTER_MS
            }
            _ => at.saturating_sub(400),
        };
        self.lyrics_screen.game = Some(g);
        self.ls_game_apply();
        self.lyrics_screen.cur = i;
        self.lyrics_screen.view_start = first.saturating_sub(LEAD_MS * 3 / 2);
        self.lyrics_screen.speed = 0.75;
        self.mini_seek(playhead, false);
    }

    /// Above the track during a game: the line being sung with the expected
    /// unit marked, the count, the speeds, « Refaire » and « Arrêter »; or,
    /// while the game waits, why, and how to go on.
    pub(super) fn ls_game_syncbar(&mut self, ui: &mut egui::Ui, bar: Rect, compact: bool) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let ctx = ui.ctx().clone();
        let live = self.ls_live()
            || self
                .lyrics_screen
                .game
                .as_ref()
                .is_some_and(|g| g.was_live && !g.started);
        let speed = self.lyrics_screen.speed;
        let prompt_h = self.lyrics_screen.prompt_h;
        let Some(g) = &self.lyrics_screen.game else {
            return;
        };
        // The prompt on its own row in the narrow window, the buttons under it.
        let (prompt_rect, buttons) = if compact {
            (
                Rect::from_min_size(bar.min, vec2(bar.width(), prompt_h)),
                Rect::from_min_max(pos2(bar.left(), bar.top() + prompt_h + 8.0), bar.max),
            )
        } else {
            (bar, bar)
        };
        enum Ask {
            Resume,
            Stop,
            Again,
            Speed(f64),
        }
        let mut ask = None;
        let mut right = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-game-buttons")
                .max_rect(buttons)
                .layout(if compact {
                    Layout::left_to_right(Align::Center)
                } else {
                    Layout::right_to_left(Align::Center)
                }),
        );
        let used = {
            let ui = &mut right;
            let mut parts: Vec<Box<dyn FnMut(&mut egui::Ui, &mut Option<Ask>) + '_>> = Vec::new();
            let stop = |ui: &mut egui::Ui, ask: &mut Option<Ask>| {
                if style::button(ui, l.text("lyrics_screen.game_stop"), true, false, 0.0).clicked()
                {
                    *ask = Some(Ask::Stop);
                }
            };
            if g.paused.is_some() {
                let resume = |ui: &mut egui::Ui, ask: &mut Option<Ask>| {
                    let ready = g.paused != Some(Pause::Question);
                    if style::button(ui, l.text("lyrics_screen.game_resume"), ready, true, 0.0)
                        .clicked()
                    {
                        *ask = Some(Ask::Resume);
                    }
                };
                parts.push(Box::new(resume));
                parts.push(Box::new(stop));
            } else {
                let count = |ui: &mut egui::Ui, _: &mut Option<Ask>| {
                    ui.label(
                        egui::RichText::new(l.format(
                            "words.tap_count",
                            &[g.holds.len().to_string(), g.units.len().to_string()],
                        ))
                        .size(13.0)
                        .color(t.text_2),
                    );
                };
                let speeds = |ui: &mut egui::Ui, ask: &mut Option<Ask>| {
                    // Read from the normal speed to the slowest, left to
                    // right: the wide window lays this bar out from the
                    // right, which would turn them round.
                    let order: Vec<f64> = if compact {
                        SPEEDS.to_vec()
                    } else {
                        SPEEDS.iter().rev().copied().collect()
                    };
                    let names: Vec<String> = order
                        .iter()
                        .map(|s| format!("{}×", l.localize_decimal(format!("{s}"))))
                        .collect();
                    let names: Vec<&str> = names.iter().map(String::as_str).collect();
                    let selected = order.iter().position(|s| *s == speed).unwrap_or(0);
                    if let Some(k) = style::segmented(ui, "lyrics-screen-speed", &names, selected) {
                        *ask = Some(Ask::Speed(order[k]));
                    }
                };
                let again = |ui: &mut egui::Ui, ask: &mut Option<Ask>| {
                    if style::button(ui, l.text("lyrics_screen.game_again"), true, false, 0.0)
                        .on_hover_text(l.text("lyrics_screen.game_again_tip"))
                        .clicked()
                    {
                        *ask = Some(Ask::Again);
                    }
                };
                parts.push(Box::new(count));
                // Only the engine that keeps the voice's pitch can slow down.
                if live {
                    parts.push(Box::new(speeds));
                }
                parts.push(Box::new(again));
                parts.push(Box::new(stop));
            }
            // Right to left in the wide window: the last part goes first.
            if !compact {
                parts.reverse();
            }
            for part in &mut parts {
                part(ui, &mut ask);
            }
            ui.min_rect()
        };
        let prompt_rect = if compact {
            prompt_rect
        } else {
            Rect::from_min_max(
                prompt_rect.min,
                pos2(
                    (used.left() - space::M).max(prompt_rect.left()),
                    prompt_rect.bottom(),
                ),
            )
        };
        // The prompt: the units of the line being sung. Done ones dimmed, the
        // expected one underlined, or filled while Espace is held.
        let painter = ui.painter_at(prompt_rect);
        let y = prompt_rect.center().y;
        // The line being sung, piece by piece: (unit, text), the separators
        // between two units having none.
        let pieces: Vec<(Option<usize>, &str)> = match (g.paused, g.units.get(g.idx)) {
            (None, Some(u)) => {
                let line: Vec<usize> = (0..g.units.len())
                    .filter(|n| g.units[*n].cue == u.cue)
                    .collect();
                let mut pieces = Vec::new();
                for (k, n) in line.iter().enumerate() {
                    let unit = &g.units[*n];
                    if k > 0 {
                        let same_word =
                            g.level == Level::Syllable && g.units[line[k - 1]].word == unit.word;
                        pieces.push((None, if same_word { "·" } else { " " }));
                    }
                    pieces.push((Some(*n), unit.text.as_str()));
                }
                pieces
            }
            _ => Vec::new(),
        };
        // A long line is written smaller, down to a size still read at a
        // glance; longer yet, it slides so that the expected unit and what
        // follows it are in sight.
        let (full, least) = if compact { (19.0, 14.0) } else { (22.0, 16.0) };
        let widths = |size: f32| -> Vec<f32> {
            pieces
                .iter()
                .map(|(_, s)| {
                    painter
                        .layout_no_wrap(s.to_string(), style::semibold(size), t.text)
                        .size()
                        .x
                })
                .collect()
        };
        let room = prompt_rect.width();
        let mut size = full;
        let mut total: f32 = widths(size).iter().sum();
        while total > room && size > least {
            size = (size - 0.5).max(least);
            total = widths(size).iter().sum();
        }
        let slide = if total > room {
            let before: f32 = pieces
                .iter()
                .zip(widths(size))
                .take_while(|((n, _), _)| *n != Some(g.idx))
                .map(|(_, w)| w)
                .sum();
            // The expected unit a quarter of the way in, the line's end at
            // the right edge at most.
            (before - room * 0.25).clamp(0.0, total - room)
        } else {
            0.0
        };
        #[cfg(test)]
        let mut expected = None;
        let mut needed = PROMPT_H;
        let font = style::semibold(size);
        let mut x = prompt_rect.left() - slide;
        let text = |painter: &egui::Painter, x: &mut f32, s: &str, color: Color32, mark: u8| {
            let galley = painter.layout_no_wrap(s.to_string(), font.clone(), color);
            let r = Rect::from_min_size(pos2(*x, y - galley.size().y / 2.0), galley.size());
            match mark {
                // Held: on the accent.
                2 => {
                    painter.rect_filled(r.expand2(vec2(2.0, 0.0)), 4, t.accent);
                }
                // Expected: underlined in the accent.
                1 => {
                    painter.rect_filled(
                        Rect::from_min_max(pos2(r.left(), r.bottom() - 3.0), r.max),
                        0,
                        t.accent,
                    );
                }
                _ => {}
            }
            *x += galley.size().x;
            painter.galley(r.min, galley, color);
        };
        match (g.paused, g.units.get(g.idx)) {
            (Some(pause), Some(u)) => {
                let key = match pause {
                    Pause::Missed => "lyrics_screen.game_missed",
                    Pause::Engine => "lyrics_screen.game_engine",
                    Pause::Question => "lyrics_screen.game_question",
                    Pause::Answered => "lyrics_screen.game_answered",
                };
                // The wide window's bar is one control high: a long reason is
                // written smaller until it holds there, on two rows at most.
                let mut size = if compact { 14.0 } else { 17.0 };
                let layout = |size: f32| {
                    painter.layout(
                        l.format(key, &[u.text.clone()]),
                        style::semibold(size),
                        t.text,
                        prompt_rect.width(),
                    )
                };
                let mut galley = layout(size);
                while !compact && galley.size().y > prompt_rect.height() && size > 13.0 {
                    size -= 1.0;
                    galley = layout(size);
                }
                // Why the game waits may take two rows in the narrow window.
                needed = needed.max(galley.size().y);
                painter.galley(
                    pos2(prompt_rect.left(), y - galley.size().y / 2.0),
                    galley,
                    t.text,
                );
            }
            (None, Some(_)) => {
                for (n, piece) in &pieces {
                    let Some(n) = *n else {
                        text(&painter, &mut x, piece, t.text_3, 0);
                        continue;
                    };
                    let (color, mark) = if n < g.idx {
                        (t.text_2, 0)
                    } else if n == g.idx && g.hold.is_some() {
                        (t.on_accent, 2)
                    } else if n == g.idx {
                        (t.text, 1)
                    } else {
                        (t.text_3, 0)
                    };
                    let from = x;
                    text(&painter, &mut x, piece, color, mark);
                    #[cfg(test)]
                    if n == g.idx {
                        expected = Some(Rect::from_x_y_ranges(from..=x, prompt_rect.y_range()));
                    }
                    #[cfg(not(test))]
                    let _ = from;
                }
            }
            _ => {}
        }
        match ask {
            Some(Ask::Resume) => self.ls_press_down(&ctx),
            Some(Ask::Stop) => self.ls_end_game(),
            Some(Ask::Again) => self.ls_game_back(&ctx),
            Some(Ask::Speed(s)) => self.lyrics_screen.speed = s,
            None => {}
        }
        // The narrow window's prompt is next drawn as high as its text was.
        if (needed - self.lyrics_screen.prompt_h).abs() > 0.5 {
            self.lyrics_screen.prompt_h = needed;
            ctx.request_repaint();
        }
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.prompt = Some(prompt_rect);
            self.lyrics_screen.drawn.expected = expected;
        }
    }

    /// The footer during a game: what Espace, Retour arrière and Échap do,
    /// one short line in the narrow window.
    pub(super) fn ls_game_footer(&self, ui: &mut egui::Ui) {
        let l = self.locale.language;
        let Some(g) = &self.lyrics_screen.game else {
            return;
        };
        if let Some(notice) = g.notice {
            ui.label(
                egui::RichText::new(l.text(notice))
                    .size(13.0)
                    .color(style::tokens(ui).warn),
            );
            row_gap(ui, space::L);
        }
        let compact = self.lyrics_screen.compact;
        let key = match (compact, g.paused) {
            (true, Some(_)) => "lyrics_screen.game_foot_waiting_short",
            (true, None) => "lyrics_screen.game_foot_short",
            (false, Some(_)) => "lyrics_screen.game_foot_waiting",
            (false, None) if g.redos >= 2 && self.lyrics_screen.speed == 1.0 && self.ls_live() => {
                "lyrics_screen.game_foot_slower"
            }
            (false, None) if g.holds.is_empty() && g.hold.is_none() => match g.level {
                Level::Line => "lyrics_screen.game_foot_first_line",
                Level::Word => "lyrics_screen.game_foot_first_word",
                Level::Syllable => "lyrics_screen.game_foot_first_syllable",
                Level::Backing => "lyrics_screen.game_foot_first_backing",
            },
            (false, None) => "lyrics_screen.game_foot",
        };
        key_caps(ui, l.text(key));
    }

    /// After a game: what it did, « Reprendre à la ligne N » and « Tout annuler ».
    pub(super) fn ls_summary_footer(&mut self, ui: &mut egui::Ui, s: Summary, lines: usize) {
        let l = self.locale.language;
        let ctx = ui.ctx().clone();
        let done = l.plural(
            match s.level {
                Level::Line => "lyrics_screen.synced_lines",
                Level::Word => "lyrics_screen.synced_words",
                Level::Syllable => "lyrics_screen.synced_syllables",
                Level::Backing => "lyrics_screen.synced_backing",
            },
            s.count,
        );
        let text = if s.level == Level::Line {
            l.format("lyrics_screen.summary_lines", &[done])
        } else {
            l.format(
                "lyrics_screen.summary",
                &[done, l.plural("lyrics_screen.over_lines", s.lines)],
            )
        };
        key_caps(ui, &text);
        row_gap(ui, space::L);
        let next = (s.last + 1).min(lines.saturating_sub(1));
        let again = small_button(
            ui,
            &l.format("lyrics_screen.resume_at", &[(next + 1).to_string()]),
            true,
            style::Kind::Secondary,
        );
        ui.add_space(space::XS);
        let undo = small_button(
            ui,
            l.text("lyrics_screen.undo_all"),
            true,
            style::Kind::Secondary,
        );
        #[cfg(test)]
        {
            self.lyrics_screen.drawn.again = Some(again.rect);
            self.lyrics_screen.drawn.undo_all = Some(undo.rect);
        }
        if again.clicked() {
            self.lyrics_screen.cur = next;
            self.ls_start_game(&ctx);
        } else if undo.clicked() {
            self.undo();
            self.lyrics_screen.summary = None;
        }
    }

    /// A line's block during a game: the guides of what is still to take
    /// (dashed, the expected one in the accent), the takes drawn solid, and
    /// the key being held growing with the playhead.
    pub(super) fn ls_game_block(
        &self,
        painter: &egui::Painter,
        lane: Rect,
        view: Viewport,
        cue: usize,
        block: Rect,
    ) {
        let Some(g) = &self.lyrics_screen.game else {
            return;
        };
        let t = style::tokens_for(painter.ctx().global_style().visuals.dark_mode);
        let inner = |a: u64, b: u64| {
            Rect::from_min_max(
                pos2(x_at(view, lane, a), block.top() + 4.0),
                pos2(
                    x_at(view, lane, b).max(x_at(view, lane, a) + 2.0),
                    block.bottom() - 4.0,
                ),
            )
        };
        let label = |r: Rect, s: &str, color: Color32| {
            if r.width() > 14.0 {
                let mut job = egui::text::LayoutJob::simple_singleline(
                    s.to_string(),
                    FontId::proportional(13.0),
                    color,
                );
                job.wrap.max_width = r.width() - 8.0;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = painter.layout_job(job);
                painter.with_clip_rect(r.intersect(lane)).galley(
                    pos2(r.left() + 4.0, r.center().y - galley.size().y / 2.0),
                    galley,
                    color,
                );
            }
        };
        for (n, (u, guide)) in g.units.iter().zip(&g.guides).enumerate().skip(g.idx) {
            if u.cue != cue {
                continue;
            }
            let r = inner(guide.start_ms, guide.end_ms).shrink2(vec2(1.0, 0.0));
            let (stroke, ink) = if n == g.idx {
                (t.accent, t.text)
            } else {
                (t.border_strong, t.text_3)
            };
            let c = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            painter
                .with_clip_rect(lane)
                .extend(egui::Shape::dashed_line(
                    &c,
                    Stroke::new(1.0, stroke),
                    4.0,
                    3.0,
                ));
            label(r, &u.text, ink);
        }
        let taken = t.accent.gamma_multiply(0.42);
        for (n, h) in &g.holds {
            if h.cue != cue {
                continue;
            }
            let r = inner(h.start_ms, h.end_ms);
            painter.with_clip_rect(lane).rect_filled(r, 4, taken);
            label(r, &g.units[*n].text, t.text);
        }
        if let (Some(start), Some(u)) = (g.hold, g.units.get(g.idx))
            && u.cue == cue
        {
            let r = inner(start, self.ls_playhead().max(start));
            painter.with_clip_rect(lane).rect_filled(r, 4, t.accent);
            label(r, &u.text, t.on_accent);
        }
    }
}
