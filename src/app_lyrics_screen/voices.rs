//! Who sings each line, in the lyrics screen (voices mockup): « Voix » in the
//! text's header declares two voices; a badge before each line opens a menu of
//! voices (a single voice, « Ensemble », none, and a « + » per voice to build
//! any set); Ctrl+1 to Ctrl+4 (Cmd on macOS) give the current line that voice
//! alone, Ctrl+Shift+1 to 4 (Ctrl on every system) add or remove it, Ctrl+0
//! none (maintainer, Q2); Shift+click on
//! another badge gives the lines in between the set chosen last. Every change
//! is one undo step.
use super::*;
use crate::app_voices::{self as v, BADGE_H, BADGE_W};
use noh::subtitle_track::{MAX_VOICES, Voice};

/// What the voices part of the screen remembers between frames.
#[derive(Default)]
pub(super) struct Voices {
    /// The line whose badge menu is open, and where its badge was drawn.
    pub menu: Option<(usize, Rect)>,
    /// The line whose voices were set last: Maj+clic on another badge gives
    /// the lines in between the same set.
    pub anchor: Option<usize>,
    /// The set chosen last.
    pub last: Vec<u8>,
    /// A voice being renamed: its number and the text typed.
    pub rename: Option<(u8, String)>,
    /// The rename field takes the keyboard next frame.
    pub focus_rename: bool,
    /// « Retirer Voix n ? » while lines use the voice.
    pub remove_ask: Option<u8>,
    /// The text's header, as last drawn: the question hangs under it.
    pub head: Option<Rect>,
}

/// The width the badge column takes before the text when voices are on.
pub(super) const COLUMN_W: f32 = BADGE_W + 6.0;

/// Ctrl (Cmd on macOS) alone.
fn command_only(m: Modifiers) -> bool {
    m.command && !m.alt && !m.shift
}

/// Ctrl+Maj, on every system: on macOS the Ctrl key itself, since Cmd+Maj+3
/// and Cmd+Maj+4 take screenshots there and never reach NOH.
fn ctrl_shift(m: Modifiers) -> bool {
    m.ctrl && m.shift && !m.alt && !m.mac_cmd
}

/// A digit key, by its place on the keyboard: with Shift a QWERTY « 1 » reads
/// « ! », and an AZERTY digit needs Shift; the place is the same. Events made
/// without a physical key (tests, an agent) are read by their key.
fn take_digit(i: &mut egui::InputState, digit: Key, want: impl Fn(Modifiers) -> bool) -> bool {
    let mut hit = false;
    i.events.retain(|e| match e {
        egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            modifiers,
            ..
        } if !hit && physical_key.unwrap_or(*key) == digit && want(*modifiers) => {
            hit = true;
            false
        }
        _ => true,
    });
    hit
}

impl NohApp {
    /// Give lines `lo..=hi` the set `set`: one undo step, named.
    pub(super) fn ls_set_voices(&mut self, lo: usize, hi: usize, set: Vec<u8>) -> bool {
        self.ls_commit_draft();
        let l = self.locale.language;
        let label = if lo == hi {
            l.format("lyrics_screen.did_voices", &[(lo + 1).to_string()])
        } else {
            l.format(
                "lyrics_screen.did_voices_many",
                &[(hi - lo + 1).to_string()],
            )
        };
        let done = self.edit_track_as(label, |t| edit::set_voices(t, lo, hi, &set));
        // Even unchanged, the set is the one to repeat with Maj+clic.
        self.lyrics_screen.voices.last = set;
        self.lyrics_screen.voices.anchor = Some(hi.max(lo));
        done
    }

    /// Declare `voices` (« Voix » declares two; « + » adds one; « × » takes the
    /// last one away): one undo step.
    fn ls_declare_voices(&mut self, voices: Vec<Voice>) -> bool {
        self.ls_commit_draft();
        let label = self
            .locale
            .language
            .text("lyrics_screen.did_declare")
            .to_string();
        self.edit_track_as(label, |t| edit::declare_voices(t, voices))
    }

    /// Ctrl+1 to Ctrl+4 (that voice alone), Ctrl+Shift+1 to 4 (that voice
    /// added to the line's voices, or removed) and Ctrl+0 (none) on line `i`. Returns
    /// whether a key was taken (the keys do nothing while no voice is
    /// declared; a voice not declared says so).
    pub(super) fn ls_voice_keys(&mut self, ctx: &egui::Context, i: usize, declared: usize) -> bool {
        if declared == 0 {
            return false;
        }
        const DIGITS: [Key; 5] = [Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4];
        let pressed = ctx.input_mut(|input| {
            DIGITS.iter().enumerate().find_map(|(n, key)| {
                if take_digit(input, *key, command_only) {
                    Some((n as u8, false))
                } else if n > 0 && take_digit(input, *key, ctrl_shift) {
                    Some((n as u8, true))
                } else {
                    None
                }
            })
        });
        let Some((n, toggle)) = pressed else {
            return false;
        };
        if n as usize > declared {
            self.notice = Some(Message::new(
                "lyrics_screen.voice_missing",
                &[self
                    .locale
                    .language
                    .format("voices.numbered", &[n.to_string()])],
            ));
            return true;
        }
        let set = match (n, toggle) {
            (0, _) => vec![],
            (n, false) => vec![n],
            (n, true) => {
                let mut set = self
                    .ls_track()
                    .and_then(|t| t.cues.get(i).map(|c| c.voices.clone()))
                    .unwrap_or_default();
                match set.iter().position(|&x| x == n) {
                    Some(k) => {
                        set.remove(k);
                    }
                    None => set.push(n),
                }
                set.sort_unstable();
                set
            }
        };
        self.ls_set_voices(i, i, set);
        true
    }

    /// The right of the text's header: « Voix » while no voice is declared,
    /// then each voice (its colour, its name to rename it, « × » on the last
    /// one) and « + ». Returns where the drawn part starts, for the hint's clip.
    pub(super) fn ls_voice_header(
        &mut self,
        ui: &mut egui::Ui,
        head: Rect,
        track: &SubtitleTrack,
        open: bool,
    ) -> f32 {
        let l = self.locale.language;
        let t = style::tokens(ui);
        self.lyrics_screen.voices.head = Some(head);
        // Nothing to offer while the lyrics cannot change and no voice is declared.
        if track.voices.is_empty() && !open {
            return head.right();
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("lyrics-screen-voices")
                .max_rect(head.expand2(vec2(0.0, 3.0)))
                .layout(Layout::right_to_left(Align::Center)),
        );
        child.spacing_mut().item_spacing.x = 4.0;
        let declared = track.voices.len();
        let mut declare = None;
        if declared == 0 {
            let b = small_button(
                &mut child,
                l.text("lyrics_screen.voices"),
                open,
                style::Kind::Secondary,
            )
            .on_hover_text(l.text("lyrics_screen.voices_tip"));
            if b.clicked() {
                declare = Some(vec![Voice::default(); 2]);
            }
        } else {
            if declared < MAX_VOICES {
                let add = small_button(&mut child, "+", open, style::Kind::Quiet)
                    .on_hover_text(l.text("lyrics_screen.voice_add"));
                if add.clicked() {
                    let mut more = track.voices.clone();
                    more.push(Voice::default());
                    declare = Some(more);
                }
            }
            for k in (0..declared).rev() {
                let n = k as u8 + 1;
                let name = v::name(track, n, l);
                let last = k + 1 == declared;
                child.scope(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if last {
                        let x = small_button(ui, "×", open, style::Kind::Quiet).on_hover_text(
                            l.format("lyrics_screen.voice_remove", std::slice::from_ref(&name)),
                        );
                        if x.clicked() {
                            if edit::lines_with_voice(track, n) == 0 {
                                let mut fewer = track.voices.clone();
                                fewer.pop();
                                declare = Some(fewer);
                            } else {
                                self.lyrics_screen.voices.remove_ask = Some(n);
                            }
                        }
                    }
                    let renaming = self
                        .lyrics_screen
                        .voices
                        .rename
                        .as_ref()
                        .is_some_and(|r| r.0 == n);
                    if renaming {
                        let id = egui::Id::new(("lyrics-screen-rename", n));
                        let mut done = None;
                        if let Some((_, text)) = &mut self.lyrics_screen.voices.rename {
                            let field = ui.add(
                                egui::TextEdit::singleline(text)
                                    .id(id)
                                    .desired_width(96.0)
                                    .char_limit(noh::subtitle_track::MAX_VOICE_NAME_CHARS)
                                    .font(FontId::proportional(style::text::SMALL)),
                            );
                            if std::mem::take(&mut self.lyrics_screen.voices.focus_rename) {
                                field.request_focus();
                            }
                            if field.lost_focus() {
                                let escape = ui.input(|i| i.key_pressed(Key::Escape));
                                done = Some(!escape);
                            }
                        }
                        if let Some(keep) = done {
                            let (_, text) =
                                self.lyrics_screen.voices.rename.take().unwrap_or_default();
                            if keep {
                                let mut named = track.voices.clone();
                                let text = text.trim().to_string();
                                named[k].name = (!text.is_empty()
                                    && text != l.format("voices.numbered", &[n.to_string()]))
                                .then_some(text);
                                declare = Some(named);
                            }
                        }
                    } else {
                        let label = small_button(ui, &name, open, style::Kind::Quiet)
                            .on_hover_text(
                                l.format("lyrics_screen.voice_rename", std::slice::from_ref(&name)),
                            );
                        if label.clicked() {
                            self.lyrics_screen.voices.rename = Some((n, name.clone()));
                            self.lyrics_screen.voices.focus_rename = true;
                        }
                    }
                    let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter()
                        .circle_filled(dot.center(), 5.0, v::color(t, n));
                });
            }
        }
        let left = child.min_rect().left();
        if let Some(voices) = declare {
            self.ls_declare_voices(voices);
        }
        left
    }

    /// « Retirer Voix n ? N lignes la perdent. » under the header, while asked.
    pub(super) fn ls_voice_remove_ask(&mut self, ctx: &egui::Context, track: &SubtitleTrack) {
        let Some(n) = self.lyrics_screen.voices.remove_ask else {
            return;
        };
        let Some(head) = self.lyrics_screen.voices.head else {
            return;
        };
        if track.voices.len() != n as usize {
            self.lyrics_screen.voices.remove_ask = None;
            return;
        }
        let l = self.locale.language;
        let lost = edit::lines_with_voice(track, n);
        let mut answer = None;
        egui::Area::new(egui::Id::new("lyrics-screen-voice-remove"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(head.right() - 300.0, head.bottom() + 6.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(280.0);
                    ui.label(
                        egui::RichText::new(l.plural_with(
                            "lyrics_screen.voice_remove_ask",
                            lost,
                            &[v::name(track, n, l), lost.to_string()],
                        ))
                        .size(style::text::SMALL),
                    );
                    ui.horizontal(|ui| {
                        if small_button(
                            ui,
                            l.text("lyrics_screen.voice_remove_yes"),
                            true,
                            style::Kind::Secondary,
                        )
                        .clicked()
                        {
                            answer = Some(true);
                        }
                        if small_button(
                            ui,
                            l.text("lyrics_screen.voice_remove_no"),
                            true,
                            style::Kind::Quiet,
                        )
                        .clicked()
                        {
                            answer = Some(false);
                        }
                    });
                });
            });
        if let Some(yes) = answer {
            self.lyrics_screen.voices.remove_ask = None;
            if yes {
                let mut fewer = track.voices.clone();
                fewer.pop();
                self.ls_declare_voices(fewer);
            }
        }
    }

    /// The badge of line `i` in `rect`: painted, and its click. Returns the
    /// click (with whether Maj was held).
    pub(super) fn ls_badge(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        i: usize,
        set: &[u8],
        track: &SubtitleTrack,
        open: bool,
    ) -> Option<bool> {
        let l = self.locale.language;
        let badge = Rect::from_min_size(
            pos2(rect.left(), rect.center().y - BADGE_H / 2.0),
            vec2(BADGE_W, BADGE_H),
        );
        let response = ui
            .interact(
                badge,
                ui.id().with(("voice-badge", i)),
                if open { Sense::click() } else { Sense::hover() },
            )
            .on_hover_text(l.format(
                "lyrics_screen.voice_badge",
                &[(i + 1).to_string(), v::set_name(track, set, l)],
            ));
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, open, v::set_name(track, set, l))
        });
        let t = style::tokens(ui);
        if ui.is_rect_visible(badge) {
            v::paint_badge(ui.painter(), badge, set, t, open && response.hovered());
            style::focus(ui, &response);
        }
        (open && response.clicked()).then(|| ui.input(|i| i.modifiers.shift))
    }

    /// The badge was clicked on line `i` (drawn at `at`): Maj+clic gives the
    /// lines from the anchor to here the set chosen last; a click opens the menu.
    pub(super) fn ls_badge_clicked(&mut self, i: usize, at: Rect, shift: bool) {
        let s = &self.lyrics_screen.voices;
        match s.anchor {
            Some(anchor) if shift && anchor != i => {
                let set = s.last.clone();
                let (lo, hi) = (anchor.min(i), anchor.max(i));
                self.lyrics_screen.voices.menu = None;
                self.ls_set_voices(lo, hi, set);
                self.lyrics_screen.voices.anchor = Some(i);
            }
            _ => self.lyrics_screen.voices.menu = Some((i, at)),
        }
    }

    /// The badge menu, while it is open: one row per voice (with « + » to add
    /// it to the line's set or take it out, the menu staying open), then
    /// « Ensemble » (voices 1 and 2) and « Aucune », and a line on Maj+clic.
    pub(super) fn ls_voice_menu(&mut self, ctx: &egui::Context, track: &SubtitleTrack) {
        let Some((i, at)) = self.lyrics_screen.voices.menu else {
            return;
        };
        let Some(cue) = track.cues.get(i) else {
            self.lyrics_screen.voices.menu = None;
            return;
        };
        let l = self.locale.language;
        let declared = track.voices.len();
        let set = cue.voices.clone();
        let height = 36.0 * (declared + 3) as f32 + 60.0;
        let screen = ctx.content_rect();
        let below = at.bottom() + 4.0 + height <= screen.bottom();
        let pos = if below {
            at.left_bottom() + vec2(0.0, 4.0)
        } else {
            pos2(at.left(), (at.top() - 4.0 - height).max(screen.top()))
        };
        let mut choose = None;
        let mut toggle = None;
        let area = egui::Area::new(egui::Id::new("lyrics-screen-voice-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_width(272.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let t = style::tokens(ui);
                    let mut row = |ui: &mut egui::Ui,
                                   option: Vec<u8>,
                                   label: String,
                                   key: &str,
                                   extra: Option<u8>| {
                        ui.horizontal(|ui| {
                            let width =
                                ui.available_width() - if extra.is_some() { 30.0 } else { 0.0 };
                            let (rect, response) =
                                ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
                            let on = option == set;
                            if on {
                                ui.painter().rect_filled(rect, style::radius::M, t.seg_on);
                            } else if response.hovered() {
                                ui.painter()
                                    .rect_filled(rect, style::radius::M, t.surface_2);
                            }
                            let badge = Rect::from_min_size(
                                pos2(rect.left() + 8.0, rect.center().y - BADGE_H / 2.0),
                                vec2(BADGE_W, BADGE_H),
                            );
                            v::paint_badge(ui.painter(), badge, &option, t, false);
                            let font = if on {
                                style::semibold(style::text::BODY)
                            } else {
                                FontId::proportional(style::text::BODY)
                            };
                            let shortcut = ui.painter().layout_no_wrap(
                                key.to_string(),
                                FontId::proportional(style::text::SMALL),
                                t.text_3,
                            );
                            let key_w = shortcut.size().x;
                            ui.painter().galley(
                                pos2(
                                    rect.right() - 8.0 - key_w,
                                    rect.center().y - shortcut.size().y / 2.0,
                                ),
                                shortcut,
                                t.text_3,
                            );
                            // The label wraps no row: it stops before the shortcut.
                            let mut job = egui::text::LayoutJob::simple_singleline(
                                label.clone(),
                                font,
                                t.text,
                            );
                            job.wrap.max_width =
                                rect.right() - 16.0 - key_w - (badge.right() + 8.0);
                            job.wrap.max_rows = 1;
                            let galley = ui.painter().layout_job(job);
                            ui.painter().galley(
                                pos2(badge.right() + 8.0, rect.center().y - galley.size().y / 2.0),
                                galley,
                                t.text,
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::RadioButton,
                                    true,
                                    on,
                                    &label,
                                )
                            });
                            style::focus(ui, &response);
                            if response.clicked() {
                                choose = Some(option.clone());
                            }
                            if let Some(n) = extra {
                                let inside = set.contains(&n);
                                let (r, plus) =
                                    ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                                let fill = if inside {
                                    t.accent
                                } else if plus.hovered() {
                                    t.surface_3
                                } else {
                                    Color32::TRANSPARENT
                                };
                                ui.painter().rect_filled(r, style::radius::M, fill);
                                ui.painter().rect_stroke(
                                    r,
                                    style::radius::M,
                                    Stroke::new(
                                        1.0,
                                        if inside { t.accent } else { t.border_strong },
                                    ),
                                    egui::StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    r.center(),
                                    Align2::CENTER_CENTER,
                                    "+",
                                    FontId::proportional(style::text::BODY),
                                    if inside { t.on_accent } else { t.text_2 },
                                );
                                let name = v::name(track, n, l);
                                let tip = if inside {
                                    l.format("lyrics_screen.voice_toggle_remove", &[name])
                                } else {
                                    l.format("lyrics_screen.voice_toggle_add", &[name])
                                };
                                let plus = plus.on_hover_text(&tip);
                                plus.widget_info(|| {
                                    egui::WidgetInfo::selected(
                                        egui::WidgetType::Checkbox,
                                        true,
                                        inside,
                                        &tip,
                                    )
                                });
                                style::focus(ui, &plus);
                                if plus.clicked() {
                                    toggle = Some(n);
                                }
                            }
                        });
                    };
                    for n in 1..=declared as u8 {
                        row(ui, vec![n], v::name(track, n, l), &format!("Ctrl+{n}"), Some(n));
                    }
                    ui.separator();
                    if declared >= 2 {
                        // « Ensemble » has no key of its own: Ctrl+1 then Ctrl+Maj+2.
                        row(ui, vec![1, 2], v::set_name(track, &[1, 2], l), "", None);
                    }
                    row(
                        ui,
                        vec![],
                        l.text("voices.none").to_string(),
                        "Ctrl+0",
                        None,
                    );
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(l.text("lyrics_screen.voice_menu_hint"))
                                .size(style::text::SMALL)
                                .color(style::tokens(ui).text_3),
                        )
                        .wrap(),
                    );
                });
            });
        let outside = ctx.input(|input| {
            input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|p| !area.response.rect.contains(p) && !at.contains(p))
        });
        if let Some(option) = choose {
            self.lyrics_screen.voices.menu = None;
            self.ls_set_voices(i, i, option);
            self.lyrics_screen.focus = Some((i, None));
        } else if let Some(n) = toggle {
            let mut next = set.clone();
            match next.iter().position(|&x| x == n) {
                Some(k) => {
                    next.remove(k);
                }
                None => next.push(n),
            }
            next.sort_unstable();
            self.ls_set_voices(i, i, next);
        } else if outside {
            self.lyrics_screen.voices.menu = None;
        }
    }
}
