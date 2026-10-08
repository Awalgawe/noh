//! Media strip: picture tiles, the add tile, the song chip and
//! the lyrics chip with its menu.
//! Metadata, thumbnails and file checks stay off the render thread.
use super::*;
use app_icons::Icon;
use app_style::{self as style, space};
use egui::{Align, FontId, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

/// Wide song/lyrics column (tokens `chip`, 296 × 40).
pub const ASSETS_W: f32 = 296.0;
const CHIP_H: f32 = 40.0;
const META_H: f32 = 20.0;
const TILE_GAP: f32 = 12.0;
/// Up to this many tiles every tile is interactive (Tab reaches each one);
/// beyond it only the visible ones are laid out.
const ALL_INTERACTIVE: usize = 256;

#[derive(Default)]
pub struct State {
    /// Image duration being typed: clip id, text, invalid.
    editing: Option<(u64, String, bool)>,
    /// Insertion index while a tile is dragged.
    drop_index: Option<usize>,
    /// The lyrics menu was open last frame (focus on open, Escape back).
    pub(super) lyrics_menu_open: bool,
    /// Lyrics menu content height last frame, to decide whether it scrolls.
    pub(super) lyrics_menu_h: f32,
    /// The menu opened: its first enabled item takes the focus once shown.
    pub(super) lyrics_focus_first: bool,
    #[cfg(test)]
    pub tiles: Vec<(u64, Rect)>,
    #[cfg(test)]
    pub add_tile: Option<Rect>,
}

fn tile_size(compact: bool) -> egui::Vec2 {
    if compact {
        vec2(96.0, 54.0)
    } else {
        vec2(112.0, 63.0)
    }
}
/// `4 s` for whole seconds, `2,5 s` otherwise.
pub(super) fn seconds_label(seconds: f64, l: Language) -> String {
    let precision = if (seconds - seconds.round()).abs() < 0.05 {
        0
    } else {
        1
    };
    format!(
        "{}{}",
        l.decimal(seconds, precision),
        l.text("seconds_suffix")
    )
}
/// Technical meta, shown only in the tooltip (no codec on the main screen).
fn clip_meta(clip: &Clip, l: Language) -> String {
    clip.info
        .as_ref()
        .map(|i| {
            let mut parts = Vec::new();
            if i.width > 0 {
                parts.push(format!("{} × {}", i.width, i.height));
            }
            if !i.codec().is_empty() {
                parts.push(i.codec().to_uppercase());
            }
            if !clip.item.is_image() && i.fps > 0.0 {
                parts.push(format!("{} fps", l.decimal(i.fps, 2)));
            }
            parts.join(" · ")
        })
        .unwrap_or_default()
}
fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}
/// Dashed outline with rounded corners (drop area, add tile, missing chips).
pub(super) fn dashed_rect(painter: &egui::Painter, rect: Rect, radius: f32, stroke: Stroke) {
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [
        (pos2(rect.right() - r, rect.top() + r), -90.0_f32),
        (pos2(rect.right() - r, rect.bottom() - r), 0.0),
        (pos2(rect.left() + r, rect.bottom() - r), 90.0),
        (pos2(rect.left() + r, rect.top() + r), 180.0),
    ];
    let mut path = vec![pos2(rect.left() + r, rect.top())];
    for (center, start) in corners {
        for step in 0..=6 {
            let angle = (start + 15.0 * step as f32).to_radians();
            path.push(center + r * vec2(angle.cos(), angle.sin()));
        }
    }
    path.push(pos2(rect.left() + r, rect.top()));
    painter.extend(egui::Shape::dashed_line(&path, stroke, 5.0, 4.0));
}

impl NohApp {
    /// The media strip: tiles (and the add tile) with the song chip beside them
    /// on the wide layout, below them in compact.
    pub(super) fn strip_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, locked: bool) {
        let opacity = ui.opacity();
        if locked {
            ui.multiply_opacity(0.45);
        }
        let start = ui.cursor().min;
        let compact = ui.available_width() < style::COMPACT_BELOW;
        let width = ui.available_width();
        let tile = tile_size(compact);
        let height = tile.y + 6.0 + META_H;
        if compact {
            self.tiles_ui(ui, ctx, width, height, locked);
            ui.add_space(space::S);
            self.song_chip(ui, ctx, width, CHIP_H, locked);
            if self.lyrics_visible() {
                ui.add_space(space::S);
                self.lyrics_chip(ui, ctx, width, true, locked);
            }
        } else {
            let (row, _) = ui.allocate_exact_size(
                vec2(width, height.max(2.0 * CHIP_H + space::S)),
                Sense::hover(),
            );
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("strip-tiles")
                    .max_rect(Rect::from_min_max(
                        row.min,
                        pos2(row.right() - ASSETS_W - space::XL, row.bottom()),
                    ))
                    .layout(Layout::left_to_right(Align::Min)),
            );
            let tiles_width = left.available_width();
            self.tiles_ui(&mut left, ctx, tiles_width, height, locked);
            let mut right = ui.new_child(
                egui::UiBuilder::new()
                    .id_salt("strip-assets")
                    .max_rect(Rect::from_min_max(
                        pos2(row.right() - ASSETS_W, row.top()),
                        row.max,
                    ))
                    .layout(Layout::top_down(Align::Min)),
            );
            right.spacing_mut().item_spacing.y = space::S;
            // A missing song also stands for the lyrics slot below it.
            let lyrics = self.lyrics_visible();
            let missing_h = if lyrics {
                CHIP_H
            } else {
                2.0 * CHIP_H + space::S
            };
            self.song_chip(&mut right, ctx, ASSETS_W, missing_h, locked);
            if lyrics {
                self.lyrics_chip(&mut right, ctx, ASSETS_W, false, locked);
            }
        }
        ui.set_opacity(opacity);
        if locked {
            ui.interact(
                Rect::from_min_max(start, ui.min_rect().max),
                ui.id().with("strip-locked-reason"),
                Sense::hover(),
            )
            .on_hover_text(self.locale.language.text("bar.locked"));
        }
    }

    fn tiles_ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        width: f32,
        height: f32,
        locked: bool,
    ) {
        let l = self.locale.language;
        let compact = width + ASSETS_W + space::XL < style::COMPACT_BELOW;
        let tile = tile_size(compact);
        let step = tile.x + TILE_GAP;
        // Song only: one wide dashed tile that says what is missing.
        let add_width = if self.clips.is_empty() {
            (2.0 * tile.x + TILE_GAP).min(width)
        } else {
            tile.x
        };
        let total = self.clips.len() as f32 * step + add_width;
        let mut actions: Vec<TileAction> = Vec::new();
        #[cfg(test)]
        self.strip.tiles.clear();
        let overflow = total > width;
        let area = ui.allocate_ui(vec2(width, height), |ui| {
            if overflow && ui.rect_contains_pointer(ui.max_rect()) {
                // The vertical wheel scrolls the strip sideways.
                ui.input_mut(|input| {
                    if input.smooth_scroll_delta.x == 0.0 && input.smooth_scroll_delta.y != 0.0 {
                        input.smooth_scroll_delta.x = input.smooth_scroll_delta.y;
                        input.smooth_scroll_delta.y = 0.0;
                    }
                });
            }
            egui::ScrollArea::horizontal()
                .id_salt("strip")
                .auto_shrink([false, true])
                .show_viewport(ui, |ui, viewport| {
                    let origin = ui.max_rect().min;
                    ui.allocate_rect(
                        Rect::from_min_size(origin, vec2(total, height)),
                        Sense::hover(),
                    );
                    let count = self.clips.len();
                    for index in 0..count {
                        let local = Rect::from_min_size(
                            pos2(index as f32 * step, 0.0),
                            vec2(tile.x, height),
                        );
                        let visible = local.intersects(viewport.expand(step));
                        if !visible && count > ALL_INTERACTIVE {
                            continue;
                        }
                        let rect = local.translate(origin.to_vec2());
                        if let Some(action) =
                            self.tile(ui, ctx, index, rect, tile, visible, locked, l)
                        {
                            actions.push(action);
                        }
                    }
                    let add = Rect::from_min_size(
                        origin + vec2(count as f32 * step, 0.0),
                        vec2(add_width, height),
                    );
                    if self.add_tile(ui, add, tile.y, locked) {
                        actions.push(TileAction::Add);
                    }
                    // Insertion bar while a tile is dragged.
                    if let Some(dragged) = self.dragging
                        && let Some(pointer) = ctx.pointer_latest_pos()
                    {
                        let index = (((pointer.x - origin.x) + step / 2.0) / step)
                            .floor()
                            .clamp(0.0, count as f32) as usize;
                        self.strip.drop_index = Some(index);
                        let x = origin.x + index as f32 * step - TILE_GAP / 2.0;
                        ui.painter().line_segment(
                            [pos2(x, origin.y), pos2(x, origin.y + tile.y)],
                            Stroke::new(2.0, style::accent(ui)),
                        );
                        if ctx.input(|i| i.pointer.any_released()) {
                            if let Some(from) = self.clips.iter().position(|c| c.id == dragged) {
                                let to = if from < index { index - 1 } else { index };
                                actions.push(TileAction::Move(from, to));
                            }
                            self.dragging = None;
                            self.strip.drop_index = None;
                        }
                    }
                });
        });
        let _ = area;
        for action in actions {
            self.apply_tile_action(action, ctx);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tile(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        index: usize,
        rect: Rect,
        tile: egui::Vec2,
        visible: bool,
        locked: bool,
        l: Language,
    ) -> Option<TileAction> {
        let t = style::tokens(ui);
        let clip = &self.clips[index];
        let (id, request_id, item) = (clip.id, clip.request_id, clip.item.clone());
        let name = file_name(item.path());
        let letter = app_timeline::sequence_letter(index);
        let thumb = Rect::from_min_size(rect.min, tile);
        let thumbnail = self.mini_preview.thumbnail(
            request_id,
            &item,
            self.ffmpeg.as_deref().unwrap_or_else(|| Path::new("")),
            ctx,
            visible && self.capture_state.is_none() && !self.project.busy() && self.job.is_none(),
        );
        let clip = &self.clips[index];
        let unreadable = clip.error.is_some();
        let seconds = match &clip.item {
            noh::input::MediaItem::Image { duration, .. } => Some(*duration),
            _ => clip.info.as_ref().map(|i| i.seconds),
        };
        let duration = seconds.map(|s| seconds_label(s, l));
        let meta = clip_meta(clip, l);
        let path = clip.item.path().display().to_string();
        let error = clip.error.as_ref().map(|e| e.render(l));
        let tile_id = ui.id().with(("tile", id));
        // Read before `interact`: Tab hands the focus on inside it, and the ✕
        // must still be registered this frame to receive it.
        let was_focused = ui.memory(|m| m.has_focus(tile_id));
        let response = ui.interact(
            thumb,
            tile_id,
            if locked {
                Sense::hover()
            } else {
                Sense::click_and_drag()
            },
        );
        let label = l.format(
            "strip.item",
            &[
                name.clone(),
                letter.clone(),
                duration
                    .clone()
                    .unwrap_or_else(|| l.text("strip.reading").into()),
            ],
        );
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !locked, &label));
        let painter = ui.painter();
        // Thumbnail, letterboxed on `surface-2`; unreadable media in error.
        if unreadable {
            painter.rect_filled(thumb, style::radius::L, t.error_soft);
            painter.rect_stroke(
                thumb,
                style::radius::L,
                Stroke::new(2.0, t.error),
                egui::StrokeKind::Inside,
            );
            app_icons::paint(
                painter,
                Rect::from_center_size(thumb.center(), vec2(20.0, 20.0)),
                Icon::Warning,
                t.error,
            );
        } else {
            painter.rect_filled(thumb, style::radius::L, t.surface_2);
            if let Some(texture) = &thumbnail {
                egui::Image::new(egui::load::SizedTexture::new(
                    texture.id(),
                    texture.size_vec2(),
                ))
                .corner_radius(style::radius::L)
                .paint_at(ui, thumb);
            } else if seconds.is_none() {
                ui.put(
                    Rect::from_center_size(thumb.center(), vec2(18.0, 18.0)),
                    egui::Spinner::new().size(18.0),
                );
            }
        }
        if self.dragging == Some(id) {
            ui.painter()
                .rect_filled(thumb, style::radius::L, t.bg.gamma_multiply(0.4));
        }
        style::focus(ui, &response);
        // Meta row: letter badge, then the duration (editable for images).
        let meta_top = thumb.bottom() + 6.0;
        let badge = Rect::from_min_size(pos2(rect.left(), meta_top + 1.0), vec2(18.0, 18.0));
        ui.painter().rect_filled(
            badge,
            style::radius::S,
            if unreadable {
                t.error_soft
            } else {
                t.seq[index % 3]
            },
        );
        ui.painter().text(
            badge.center(),
            egui::Align2::CENTER_CENTER,
            &letter,
            style::semibold(style::text::SMALL),
            t.seq_ink,
        );
        let mut action = None;
        let text_left = badge.right() + 6.0;
        let editing = self
            .strip
            .editing
            .as_ref()
            .filter(|(edit, _, _)| *edit == id)
            .cloned();
        if let Some((_, mut text, invalid)) = editing {
            let field = Rect::from_min_size(pos2(text_left, meta_top), vec2(64.0, META_H));
            let edit_id = ui.id().with(("tile-duration", id));
            let edit = ui.put(
                field,
                egui::TextEdit::singleline(&mut text)
                    .id(edit_id)
                    .font(FontId::proportional(style::text::SMALL))
                    .margin(vec2(4.0, 1.0)),
            );
            if invalid {
                ui.painter().rect_stroke(
                    field,
                    style::radius::S,
                    Stroke::new(2.0, t.error),
                    egui::StrokeKind::Inside,
                );
            }
            let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if escape {
                self.strip.editing = None;
            } else if edit.lost_focus() {
                let value = text
                    .trim()
                    .trim_end_matches('s')
                    .trim()
                    .replace(',', ".")
                    .parse::<f64>()
                    .ok();
                match value.filter(|v| v.is_finite() && *v > 0.0) {
                    Some(value) => {
                        self.strip.editing = None;
                        action = Some(TileAction::Duration(id, value));
                    }
                    None => self.strip.editing = Some((id, text, true)),
                }
            } else {
                if !edit.has_focus() && !edit.gained_focus() {
                    edit.request_focus();
                }
                self.strip.editing = Some((id, text, invalid));
            }
            if invalid {
                edit.on_hover_text(l.text("strip.duration_invalid"));
            }
        } else if unreadable {
            ui.painter().text(
                pos2(text_left, meta_top + META_H / 2.0),
                egui::Align2::LEFT_CENTER,
                l.text("strip.unreadable"),
                FontId::proportional(style::text::SMALL),
                t.error,
            );
        } else if let (true, Some(text)) = (item.is_image(), &duration) {
            // Images: the duration is a chip that turns into a field.
            let galley = ui.painter().layout_no_wrap(
                text.clone(),
                FontId::proportional(style::text::SMALL),
                t.text,
            );
            let chip = Rect::from_min_size(
                pos2(text_left, meta_top),
                vec2(galley.size().x + 12.0, META_H),
            );
            let edit = ui.interact(
                chip,
                ui.id().with(("tile-duration-chip", id)),
                if locked {
                    Sense::hover()
                } else {
                    Sense::click()
                },
            );
            let chip_name = l.format("strip.duration", &[name.clone()]);
            edit.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, !locked, &chip_name)
            });
            ui.painter()
                .rect_filled(chip, style::radius::S, t.surface_2);
            ui.painter().galley(
                pos2(chip.left() + 6.0, chip.center().y - galley.size().y / 2.0),
                galley,
                t.text,
            );
            style::focus(ui, &edit);
            if edit.clicked() {
                self.strip.editing = Some((
                    id,
                    text.trim_end_matches(l.text("seconds_suffix")).to_owned(),
                    false,
                ));
            }
        } else {
            ui.painter().text(
                pos2(text_left, meta_top + META_H / 2.0),
                egui::Align2::LEFT_CENTER,
                duration.unwrap_or_else(|| l.text("strip.reading").into()),
                FontId::proportional(style::text::SMALL),
                t.text_2,
            );
        }
        // ✕ on hover or focus.
        let remove_rect = Rect::from_min_size(
            pos2(thumb.right() - 24.0, thumb.top() + 4.0),
            vec2(20.0, 20.0),
        );
        let remove_id = ui.id().with(("tile-remove", id));
        let show_remove = !locked
            && (response.hovered()
                || response.has_focus()
                || was_focused
                || ctx.memory(|m| m.has_focus(remove_id))
                || ui.rect_contains_pointer(remove_rect));
        if show_remove {
            let remove = ui.interact(remove_rect, remove_id, Sense::click());
            let remove_name = l.format("strip.remove", &[name.clone()]);
            remove.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &remove_name)
            });
            ui.painter()
                .rect_filled(remove_rect, style::radius::S, t.surface.gamma_multiply(0.9));
            app_icons::paint(
                ui.painter(),
                remove_rect.shrink(3.0),
                Icon::Close,
                if remove.hovered() { t.text } else { t.text_2 },
            );
            style::focus(ui, &remove);
            if remove.clicked() {
                action = Some(TileAction::Remove(id));
            }
            let _ = remove.on_hover_text(remove_name);
        }
        let response = response.on_hover_text(if locked {
            l.text("bar.locked").to_owned()
        } else {
            match &error {
                Some(error) => format!("{path}\n{error}"),
                None if meta.is_empty() => format!("{path}\n{}", l.text("strip.tile_hint")),
                None => format!("{path}\n{meta}\n{}", l.text("strip.tile_hint")),
            }
        });
        if response.drag_started() && !locked {
            self.dragging = Some(id);
        }
        if response.clicked() {
            // The shown tile keeps focus: Alt+←/→ and Delete then apply to it.
            response.request_focus();
            action = Some(TileAction::Show(id));
        }
        if response.has_focus() && !locked {
            let (left, right, delete, enter) = ui.input(|i| {
                (
                    i.modifiers.alt && i.key_pressed(egui::Key::ArrowLeft),
                    i.modifiers.alt && i.key_pressed(egui::Key::ArrowRight),
                    i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                    i.key_pressed(egui::Key::Enter),
                )
            });
            if left && index > 0 {
                action = Some(TileAction::Move(index, index - 1));
            } else if right && index + 1 < self.clips.len() {
                action = Some(TileAction::Move(index, index + 1));
            } else if delete {
                action = Some(TileAction::Remove(id));
            } else if enter {
                action = Some(TileAction::Show(id));
            }
            if matches!(action, Some(TileAction::Move(..))) {
                // Arrows move the tile, not the keyboard focus.
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        response.id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            ..Default::default()
                        },
                    )
                });
            }
        }
        #[cfg(test)]
        self.strip.tiles.push((id, thumb));
        action
    }

    /// Dashed add tile. With no visuals yet, it is wide and says what is missing.
    fn add_tile(&mut self, ui: &mut egui::Ui, rect: Rect, thumb_h: f32, locked: bool) -> bool {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let thumb = Rect::from_min_size(rect.min, vec2(rect.width(), thumb_h));
        let response = ui.interact(
            thumb,
            ui.id().with("tile-add"),
            if locked {
                Sense::hover()
            } else {
                Sense::click()
            },
        );
        let missing = self.clips.is_empty();
        let name = l.text(if missing {
            "strip.missing_visuals"
        } else {
            "strip.add_help"
        });
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !locked, name));
        if response.hovered() && !locked {
            ui.painter()
                .rect_filled(thumb, style::radius::L, t.surface_2);
        }
        dashed_rect(
            ui.painter(),
            thumb.shrink(0.75),
            f32::from(style::radius::L),
            Stroke::new(1.5, t.border_strong),
        );
        let color = if locked { t.text_3 } else { t.text_2 };
        if missing {
            // Wide tile: picture icon and the missing-visuals line, one centred row.
            let galley = ui.painter().layout(
                l.text("strip.missing_visuals").into(),
                FontId::proportional(style::text::SMALL),
                t.text,
                thumb.width() - 24.0 - 18.0 - space::S,
            );
            let width = 18.0 + space::S + galley.size().x;
            let left = thumb.center().x - width / 2.0;
            app_icons::paint(
                ui.painter(),
                Rect::from_min_size(pos2(left, thumb.center().y - 9.0), vec2(18.0, 18.0)),
                Icon::Picture,
                color,
            );
            ui.painter().galley(
                pos2(
                    left + 18.0 + space::S,
                    thumb.center().y - galley.size().y / 2.0,
                ),
                galley,
                t.text,
            );
        } else {
            app_icons::paint(
                ui.painter(),
                Rect::from_center_size(thumb.center(), vec2(18.0, 18.0)),
                Icon::Plus,
                color,
            );
            ui.painter().text(
                pos2(rect.left(), thumb.bottom() + 6.0 + META_H / 2.0),
                egui::Align2::LEFT_CENTER,
                l.text("strip.add"),
                FontId::proportional(style::text::SMALL),
                t.text_2,
            );
        }
        style::focus(ui, &response);
        #[cfg(test)]
        {
            self.strip.add_tile = Some(thumb);
        }
        response
            .on_hover_text(l.text(if locked {
                "bar.locked"
            } else {
                "strip.add_help"
            }))
            .clicked()
    }

    /// Song chip: name, duration and ⋯ menu; a dashed "Add the song"
    /// chip while there is none.
    pub(super) fn song_chip(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        width: f32,
        missing_h: f32,
        locked: bool,
    ) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let height = if self.wav.is_some() {
            CHIP_H
        } else {
            missing_h
        };
        let (rect, response) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
        let Some(path) = self.wav.clone() else {
            let add = ui.interact(
                rect,
                ui.id().with("song-add"),
                if locked {
                    Sense::hover()
                } else {
                    Sense::click()
                },
            );
            add.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Button, !locked, l.text("song.add"))
            });
            if add.hovered() && !locked {
                ui.painter()
                    .rect_filled(rect, style::radius::L, t.surface_2);
            }
            dashed_rect(
                ui.painter(),
                rect.shrink(0.75),
                f32::from(style::radius::L),
                Stroke::new(1.5, t.border_strong),
            );
            app_icons::paint(
                ui.painter(),
                Rect::from_min_size(
                    pos2(rect.left() + 12.0, rect.center().y - 9.0),
                    vec2(18.0, 18.0),
                ),
                Icon::Song,
                t.text_2,
            );
            ui.painter().text(
                pos2(rect.left() + 40.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                l.text("song.add"),
                FontId::proportional(style::text::BODY),
                t.text,
            );
            style::focus(ui, &add);
            let add = if locked {
                add.on_hover_text(l.text("bar.locked"))
            } else {
                add
            };
            if add.clicked() {
                self.pick_song(ctx);
            }
            return;
        };
        let _ = response;
        let unreadable = self.wav_error.is_some();
        ui.painter().rect_filled(rect, style::radius::L, t.surface);
        if unreadable {
            ui.painter().rect_stroke(
                rect,
                style::radius::L,
                Stroke::new(2.0, t.error),
                egui::StrokeKind::Inside,
            );
        }
        app_icons::paint(
            ui.painter(),
            Rect::from_min_size(
                pos2(rect.left() + 12.0, rect.center().y - 9.0),
                vec2(18.0, 18.0),
            ),
            Icon::Song,
            if unreadable { t.error } else { t.text_2 },
        );
        let menu_rect = Rect::from_min_size(
            pos2(rect.right() - 32.0, rect.center().y - 14.0),
            vec2(28.0, 28.0),
        );
        let (meta, meta_color) = if unreadable {
            (l.text("song.unreadable").to_owned(), t.error)
        } else {
            match self.wav_seconds {
                Some(seconds) => {
                    let whole = seconds.floor() as u64;
                    (format!("{}:{:02}", whole / 60, whole % 60), t.text_2)
                }
                None => (l.text("strip.reading").to_owned(), t.text_2),
            }
        };
        let meta_galley =
            ui.painter()
                .layout_no_wrap(meta, FontId::proportional(style::text::SMALL), meta_color);
        let meta_x = menu_rect.left() - space::S - meta_galley.size().x;
        let name_left = rect.left() + 40.0;
        let name = file_name(&path);
        let mut name_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("song-name")
                .max_rect(Rect::from_min_max(
                    pos2(name_left, rect.top()),
                    pos2(meta_x - space::S, rect.bottom()),
                ))
                .layout(Layout::left_to_right(Align::Center)),
        );
        name_ui
            .add(egui::Label::new(style::strong(&name, style::text::BODY).color(t.text)).truncate())
            .on_hover_text(path.display().to_string());
        ui.painter().galley(
            pos2(meta_x, rect.center().y - meta_galley.size().y / 2.0),
            meta_galley,
            meta_color,
        );
        let mut menu_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("song-menu-button")
                .max_rect(menu_rect),
        );
        let menu = menu_ui
            .add_enabled_ui(!locked, |ui| {
                style::icon_button(ui, Icon::More, l.text("song.menu"), style::ROW_ICON)
            })
            .inner;
        let menu = if locked {
            menu.on_disabled_hover_text(l.text("bar.locked"))
        } else {
            menu
        };
        let mut replace = false;
        let mut remove = false;
        egui::Popup::menu(&menu)
            .id(egui::Id::new("song-menu"))
            .frame(style::popup_frame(ui))
            .width(220.0)
            .show(|ui| {
                replace = style::menu_item(ui, Icon::Replace, l.text("song.replace"), None, true)
                    .clicked();
                remove =
                    style::menu_item(ui, Icon::Remove, l.text("song.remove"), None, true).clicked();
                if replace || remove {
                    ui.close();
                }
            });
        if replace {
            self.pick_song(ctx);
        }
        if remove {
            self.wav = None;
            self.wav_seconds = None;
            self.wav_error = None;
            self.invalidated();
        }
    }

    pub(super) fn pick_song(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        if let Some(path) = rfd::FileDialog::new()
            .set_title(l.text("choose_wav"))
            .add_filter(l.text("wav_filter"), &["wav"])
            .pick_file()
        {
            self.drop_files(vec![path], ctx);
        }
    }
    pub(super) fn pick_visuals(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        if let Some(paths) = rfd::FileDialog::new()
            .set_title(l.text("pick_videos"))
            .add_filter(
                l.text("video_filter"),
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "png", "jpg", "jpeg",
                ],
            )
            .pick_files()
        {
            self.drop_files(paths, ctx);
        }
    }
    /// "Add files…": every accepted type, sorted like a drop.
    pub(super) fn pick_any(&mut self, ctx: &egui::Context) {
        let l = self.locale.language;
        if let Some(paths) = rfd::FileDialog::new()
            .set_title(l.text("drop.pick_title"))
            .add_filter(
                l.text("drop.filter_all"),
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "png", "jpg", "jpeg", "wav", "srt",
                ],
            )
            .pick_files()
        {
            self.drop_files(paths, ctx);
        }
    }
    /// Replaces a clip at the same position (an unreadable one, typically).
    pub(super) fn replace_clip(&mut self, id: u64, ctx: &egui::Context) {
        let l = self.locale.language;
        let Some(index) = self.clips.iter().position(|c| c.id == id) else {
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .set_title(l.text("pick_videos"))
            .add_filter(
                l.text("video_filter"),
                &[
                    "mp4", "mov", "mkv", "webm", "avi", "m4v", "png", "jpg", "jpeg",
                ],
            )
            .pick_file()
        else {
            return;
        };
        let Some(app_media::Kind::Picture | app_media::Kind::Video) = app_media::kind_of(&path)
        else {
            self.drop_error = Some(noh::i18n::Message::new(
                "drop.rejected_unsupported",
                &[file_name(&path)],
            ));
            return;
        };
        let item = app_media::item_for_path(path);
        self.next_id += 1;
        self.metadata
            .request_media(self.next_id, item.clone(), self.ffmpeg.clone(), ctx);
        self.clips[index] = Clip {
            id: self.next_id,
            request_id: self.next_id,
            item,
            info: None,
            error: None,
        };
        self.invalidated();
    }

    fn apply_tile_action(&mut self, action: TileAction, ctx: &egui::Context) {
        match action {
            TileAction::Add => self.pick_visuals(ctx),
            TileAction::Remove(id) => {
                self.clips.retain(|c| c.id != id);
                self.invalidated();
            }
            TileAction::Move(from, to) => {
                if from != to && from < self.clips.len() {
                    let clip = self.clips.remove(from);
                    self.clips.insert(to.min(self.clips.len()), clip);
                    self.invalidated();
                }
            }
            TileAction::Duration(id, value) => {
                self.set_image_duration(id, value);
            }
            TileAction::Show(id) => {
                if let Some(clip) = self.clips.iter().find(|c| c.id == id) {
                    let texture = self.mini_preview.thumbnail(
                        clip.request_id,
                        &clip.item,
                        self.ffmpeg.as_deref().unwrap_or_else(|| Path::new("")),
                        ctx,
                        false,
                    );
                    if texture.is_some() {
                        self.mini_preview.invalidate();
                        self.mini_preview.source = texture;
                    }
                }
            }
        }
    }

    /// Stage panel for a missing song or missing visuals.
    pub(super) fn missing_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, song: bool) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let compact = ui.available_width() < style::COMPACT_BELOW;
        egui::Frame::NONE
            .fill(t.surface)
            .corner_radius(style::radius::L)
            .inner_margin(egui::Margin::symmetric(24, if compact { 16 } else { 0 }))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let (icon, text, button) = if song {
                    (Icon::Song, "song.missing_line", "song.choose")
                } else {
                    (Icon::Picture, "home.drop_body", "strip.choose_visuals")
                };
                let layout = if compact {
                    Layout::top_down(Align::Center)
                } else {
                    Layout::left_to_right(Align::Center)
                };
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), if compact { 0.0 } else { 120.0 }),
                    layout,
                    |ui| {
                        ui.spacing_mut().item_spacing = vec2(space::L, space::M);
                        // Wide: one row centred on the width measured last frame.
                        let measured_id = ui.id().with("missing-row-width");
                        let measured: f32 = ui.data(|d| d.get_temp(measured_id)).unwrap_or(0.0);
                        if !compact {
                            ui.set_min_height(120.0);
                            ui.add_space(((ui.available_width() - measured) / 2.0).max(0.0));
                        }
                        let content_left = ui.cursor().left();
                        let (icon_rect, _) =
                            ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        app_icons::paint(ui.painter(), icon_rect, icon, t.text_2);
                        ui.add(
                            egui::Label::new(RichText::new(l.text(text)).color(t.text_2)).wrap(),
                        );
                        if style::button(ui, l.text(button), true, false, 0.0).clicked() {
                            if song {
                                self.pick_song(ctx);
                            } else {
                                self.pick_visuals(ctx);
                            }
                        }
                        let width = ui.min_rect().right() - content_left;
                        if !compact && (width - measured).abs() > 0.5 {
                            ui.data_mut(|d| d.insert_temp(measured_id, width));
                            ui.ctx().request_discard("missing panel measured");
                        }
                    },
                );
            });
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum TileAction {
    Add,
    Remove(u64),
    Move(usize, usize),
    Duration(u64, f64),
    Show(u64),
}

impl NohApp {
    /// Empty project: one dashed drop area filling the body.
    pub(super) fn drop_area(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, height: f32) {
        let l = self.locale.language;
        let t = style::tokens(ui);
        let compact = ui.available_width() < style::COMPACT_BELOW;
        // The body's own margins give the sides and top; keep 24 (16) below.
        let full = Rect::from_min_size(
            ui.cursor().min,
            vec2(ui.available_width(), height.max(320.0)),
        );
        let bottom = if compact { 8.0 } else { 16.0 };
        let area = Rect::from_min_max(full.min, pos2(full.right(), full.bottom() - bottom));
        ui.allocate_rect(full, Sense::hover());
        dashed_rect(
            ui.painter(),
            area.shrink(1.0),
            f32::from(style::radius::XL),
            Stroke::new(2.0, t.border_strong),
        );
        let inner = area.shrink(if compact { 16.0 } else { 24.0 });
        // Centred vertically on the height measured last frame.
        let measured_id = ui.id().with("drop-area-height");
        let measured: f32 = ui.data(|d| d.get_temp(measured_id)).unwrap_or(0.0);
        let top = inner.top() + ((inner.height() - measured) / 2.0).max(0.0);
        let mut inside = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("drop-area")
                .max_rect(Rect::from_min_max(pos2(inner.left(), top), inner.max))
                .layout(Layout::top_down(Align::Center)),
        );
        let ui = &mut inside;
        ui.spacing_mut().item_spacing.y = if compact { space::M } else { space::L };
        let (icon, _) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::hover());
        app_icons::paint(ui.painter(), icon, Icon::Drop, t.text_2);
        let title_size = if compact {
            style::text::HEADING
        } else {
            style::text::TITLE
        };
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width().min(520.0));
            ui.add(
                egui::Label::new(
                    style::strong(l.text("home.drop_title"), title_size).color(t.text),
                )
                .wrap()
                .halign(Align::Center),
            );
        });
        ui.add(
            egui::Label::new(RichText::new(l.text("home.drop_body")).color(t.text_2))
                .wrap()
                .halign(Align::Center),
        );
        let add = style::button_kind(
            ui,
            l.text("home.add_files"),
            true,
            style::Kind::Primary,
            0.0,
            style::CONTROL_H_LG,
        );
        if add.clicked() {
            self.pick_any(ctx);
        }
        // A refused drop or selection explains itself right under the button.
        if let Some(error) = &self.drop_error {
            let galley = ui.painter().layout(
                error.render(l),
                FontId::proportional(style::text::BODY),
                t.error,
                ui.available_width() - 16.0 - space::S,
            );
            let (line, _) = ui.allocate_exact_size(
                vec2(16.0 + space::S + galley.size().x, galley.size().y.max(16.0)),
                Sense::hover(),
            );
            app_icons::paint(
                ui.painter(),
                Rect::from_min_size(pos2(line.left(), line.center().y - 8.0), vec2(16.0, 16.0)),
                Icon::Error,
                t.error,
            );
            ui.painter().galley(
                pos2(
                    line.left() + 16.0 + space::S,
                    line.center().y - galley.size().y / 2.0,
                ),
                galley,
                t.error,
            );
        }
        // Accepted kinds as pills, each line centred.
        let pills: Vec<_> = [
            (Icon::Picture, "home.kind_pictures"),
            (Icon::Video, "home.kind_videos"),
            (Icon::Song, "home.kind_song"),
            (Icon::Lyrics, "home.kind_lyrics"),
        ]
        .into_iter()
        .map(|(icon, key)| {
            let galley = ui.painter().layout_no_wrap(
                l.text(key).into(),
                FontId::proportional(style::text::SMALL),
                t.text_2,
            );
            (icon, 10.0 + 16.0 + 6.0 + galley.size().x + 10.0, galley)
        })
        .collect();
        let mut lines: Vec<Vec<_>> = vec![Vec::new()];
        let mut line_width = 0.0;
        for pill in pills {
            let width = line_width + if line_width > 0.0 { space::S } else { 0.0 } + pill.1;
            if width > ui.available_width() && line_width > 0.0 {
                lines.push(Vec::new());
                line_width = pill.1;
            } else {
                line_width = width;
            }
            lines.last_mut().unwrap().push(pill);
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = space::S;
            for line in lines {
                let width = line.iter().map(|p| p.1).sum::<f32>()
                    + space::S * (line.len().saturating_sub(1)) as f32;
                let (row, _) = ui.allocate_exact_size(vec2(width, 28.0), Sense::hover());
                let mut x = row.left();
                for (icon, pill_width, galley) in line {
                    let pill = Rect::from_min_size(pos2(x, row.top()), vec2(pill_width, 28.0));
                    ui.painter().rect_filled(pill, 14, t.surface);
                    app_icons::paint(
                        ui.painter(),
                        Rect::from_min_size(
                            pos2(pill.left() + 10.0, pill.center().y - 8.0),
                            vec2(16.0, 16.0),
                        ),
                        icon,
                        t.text_2,
                    );
                    ui.painter().galley(
                        pos2(pill.left() + 32.0, pill.center().y - galley.size().y / 2.0),
                        galley,
                        t.text_2,
                    );
                    x += pill_width + space::S;
                }
            }
        });
        ui.label(
            RichText::new(l.text("home.local"))
                .size(style::text::SMALL)
                .color(t.text_3),
        );
        let height = ui.min_rect().height();
        if (height - measured).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(measured_id, height));
            ui.ctx().request_discard("drop area measured");
        }
    }

    /// While files hover the window: an accent frame and what the drop would
    /// add, or an error frame and why it would be refused. Names only.
    pub(super) fn drop_overlay(&self, ctx: &egui::Context, area: Rect) {
        let files = ctx.input(|i| app_media::hovered_paths(&i.raw.hovered_files));
        if files.is_empty() {
            return;
        }
        let l = self.locale.language;
        let dark = ctx.global_style().visuals.dark_mode;
        let t = style::tokens_for(dark);
        let (ok, text) = match app_media::classify(&files, self.wav.is_some(), false) {
            Ok(plan) => {
                let mut parts = Vec::new();
                if plan.pictures > 0 {
                    parts.push(l.plural("drop.pictures", plan.pictures));
                }
                if plan.videos > 0 {
                    parts.push(l.plural("drop.videos", plan.videos));
                }
                if plan.song.is_some() {
                    parts.push(l.text("drop.song").into());
                }
                if plan.lyrics.is_some() {
                    parts.push(l.text("drop.lyrics").into());
                }
                let mut text = l.format("drop.overlay", &[parts.join(", ")]);
                if let (Some(_), Some(current)) = (&plan.song, &self.wav) {
                    text.push_str("\n");
                    text.push_str(&l.format("drop.replaces", &[file_name(current)]));
                }
                (true, text)
            }
            Err(error) => (false, error.render(l)),
        };
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ));
        let color = if ok { t.accent } else { t.error };
        painter.rect_filled(area, 0, if ok { t.accent_soft } else { t.error_soft });
        painter.rect_stroke(
            area.shrink(1.0),
            style::radius::XL,
            Stroke::new(2.0, color),
            egui::StrokeKind::Inside,
        );
        let galley = painter.layout(
            text,
            style::semibold(style::text::BODY),
            if ok { t.text } else { t.error },
            (area.width() - 64.0).max(120.0),
        );
        let pill = Rect::from_center_size(area.center(), galley.size() + vec2(32.0, 20.0));
        painter.rect_filled(pill, style::radius::XL, t.surface);
        painter.rect_stroke(
            pill,
            style::radius::XL,
            Stroke::new(1.0, t.border),
            egui::StrokeKind::Inside,
        );
        painter.galley(pill.min + vec2(16.0, 10.0), galley, t.text);
        ctx.request_repaint_after(std::time::Duration::from_millis(32));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2};

    #[derive(Debug)]
    struct DroppedPath(PathBuf);
    impl egui::DroppedFile for DroppedPath {
        fn path(&self) -> &Path {
            &self.0
        }
        fn bytes(&self) -> Result<Vec<u8>, String> {
            panic!("The UI must not read dropped media")
        }
    }

    fn input(width: f32, events: Vec<Event>) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 850.0))),
            events,
            ..Default::default()
        }
    }
    /// One frame of the whole window; returns the painted texts.
    fn frame(ctx: &egui::Context, app: &mut NohApp, raw: egui::RawInput) -> Vec<String> {
        let mut output = ctx.run_ui(raw, |ui| {
            app.draw(ui);
        });
        // Layout tests have no renderer to consume texture updates.
        output.textures_delta.clear();
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }
    fn drop(ctx: &egui::Context, app: &mut NohApp, width: f32, paths: &[PathBuf]) {
        let mut raw = input(width, vec![]);
        for path in paths {
            raw.dropped_files
                .push(std::sync::Arc::new(DroppedPath(path.clone())));
        }
        frame(ctx, app, raw);
    }
    fn app(language: Language) -> (egui::Context, NohApp) {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        style::apply(&ctx);
        let mut app = NohApp::default();
        app.locale.language = language;
        (ctx, app)
    }
    fn clip(id: u64, name: &str, image: bool) -> Clip {
        let item = if image {
            noh::input::MediaItem::Image {
                path: name.into(),
                duration: 6.0,
            }
        } else {
            name.into()
        };
        let mut info = noh::media::MediaInfo::default();
        info.seconds = 4.0;
        info.width = 1280;
        info.height = 720;
        Clip {
            id,
            request_id: id,
            item,
            info: Some(info),
            error: None,
        }
    }
    fn image_duration(clip: &Clip) -> Option<f64> {
        match clip.item {
            noh::input::MediaItem::Image { duration, .. } => Some(duration),
            _ => None,
        }
    }

    #[test]
    fn locked_strip_hover_explains_why_files_cannot_be_added() {
        let (ctx, mut app) = app(Language::En);
        ctx.all_styles_mut(|style| {
            style.interaction.tooltip_delay = 0.0;
            style.interaction.show_tooltips_only_when_still = false;
        });
        let mut frame = |events: Vec<Event>, time: f64| {
            let mut rendered = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(980.0, 850.0))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.strip_ui(ui, &ctx, true),
            );
            rendered.textures_delta.clear();
            (rendered.shapes, app.strip.add_tile.expect("add tile"))
        };
        let (_, add) = frame(vec![], 0.0);
        let point = add.center();
        frame(vec![Event::PointerMoved(point)], 0.1);
        let (shapes, _) = frame(vec![], 1.0);
        assert!(shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.text() == Language::En.text("bar.locked"),
            _ => false,
        }));
    }

    /// One drop anywhere, with no click and no pointer position: pictures and
    /// videos go to the strip, the WAV to the song, the SRT to the lyrics.
    #[test]
    fn mixed_drop_fills_strip_song_and_lyrics_in_one_frame() {
        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, []).unwrap();
            path
        };
        let paths = [
            file("vagues.mp4"),
            file("ville-nuit.JPG"),
            file("ma-chanson.wav"),
            file("ma-chanson.srt"),
        ];
        for width in [420.0, 980.0] {
            let (ctx, mut app) = app(Language::En);
            for _ in 0..2 {
                frame(&ctx, &mut app, input(width, vec![]));
            }
            drop(&ctx, &mut app, width, &paths);
            assert!(app.drop_error.is_none(), "{:?}", app.drop_error);
            assert_eq!(app.clips.len(), 2);
            assert!(!app.clips[0].item.is_image() && app.clips[1].item.is_image());
            assert_eq!(app.wav.as_ref(), Some(&paths[2]));
            assert_eq!(app.project.track.path.as_ref(), Some(&paths[3]));
        }
    }

    /// A refused drop changes nothing and says why: under "Add files…" in the
    /// empty project, in the bar once there is media.
    #[test]
    fn refused_drops_change_nothing_and_explain_themselves() {
        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, []).unwrap();
            path
        };
        let (video, notes, wav) = (file("clip.mp4"), file("notes.txt"), file("song.wav"));
        let (ctx, mut app) = app(Language::Fr);
        frame(&ctx, &mut app, input(980.0, vec![]));
        drop(&ctx, &mut app, 980.0, &[video.clone(), notes]);
        assert!(app.clips.is_empty() && app.wav.is_none());
        let refusal = Language::Fr.format("drop.rejected_unsupported", &["notes.txt".into()]);
        let texts = frame(&ctx, &mut app, input(980.0, vec![]));
        let button = texts
            .iter()
            .position(|t| t == Language::Fr.text("home.add_files"))
            .expect("empty state");
        let line = texts.iter().position(|t| *t == refusal);
        assert!(
            line.is_some_and(|line| line > button),
            "the reason follows the button: {texts:?}"
        );
        // With media, the bar owns the message; a valid drop clears it.
        drop(&ctx, &mut app, 980.0, std::slice::from_ref(&video));
        assert!(app.drop_error.is_none());
        assert_eq!(app.clips.len(), 1);
        drop(&ctx, &mut app, 980.0, &[wav.clone(), wav]);
        assert_eq!(app.clips.len(), 1);
        assert!(app.wav.is_none());
        let texts = frame(&ctx, &mut app, input(980.0, vec![]));
        assert!(
            texts.iter().any(|t| t == Language::Fr.text("drop.refused")),
            "{texts:?}"
        );
        // Lyrics need a song, in the project or in the same drop.
        drop(&ctx, &mut app, 980.0, &[file("words.srt")]);
        assert_eq!(
            app.drop_error.as_ref().map(|e| e.key.as_str()),
            Some("drop.lyrics_need_song")
        );
        assert!(app.project.track.path.is_none());
    }

    fn button(point: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos: point,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }
    fn key(key: egui::Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    #[test]
    fn tiles_reorder_by_drag_and_keyboard_and_edit_image_durations() {
        let (ctx, mut app) = app(Language::En);
        app.clips = vec![
            clip(1, "a.mp4", false),
            clip(2, "b.mp4", false),
            clip(3, "c.png", true),
        ];
        let order = |app: &NohApp| app.clips.iter().map(|c| c.id).collect::<Vec<_>>();
        frame(&ctx, &mut app, input(980.0, vec![]));
        let rects = app.strip.tiles.clone();
        assert_eq!(rects.len(), 3);
        // Drag A past C: the insertion bar follows the pointer; release moves it.
        let (from, past) = (
            rects[0].1.center(),
            rects[2].1.right_center() + vec2(8.0, 0.0),
        );
        frame(
            &ctx,
            &mut app,
            input(980.0, vec![Event::PointerMoved(from), button(from, true)]),
        );
        for step in 1..=6 {
            let point = from + (past - from) * step as f32 / 6.0;
            frame(
                &ctx,
                &mut app,
                input(980.0, vec![Event::PointerMoved(point)]),
            );
        }
        assert_eq!(app.strip.drop_index, Some(3));
        frame(&ctx, &mut app, input(980.0, vec![button(past, false)]));
        frame(&ctx, &mut app, input(980.0, vec![]));
        assert_eq!(order(&app), [2, 3, 1]);
        assert!(app.dragging.is_none() && app.strip.drop_index.is_none());
        // Focus B, then Alt+→ moves it and Delete removes it.
        let b = app
            .strip
            .tiles
            .iter()
            .find(|(id, _)| *id == 2)
            .unwrap()
            .1
            .center();
        frame(
            &ctx,
            &mut app,
            input(980.0, vec![Event::PointerMoved(b), button(b, true)]),
        );
        frame(&ctx, &mut app, input(980.0, vec![button(b, false)]));
        frame(
            &ctx,
            &mut app,
            input(
                980.0,
                vec![
                    Event::ModifiersChanged(Modifiers::ALT),
                    key(egui::Key::ArrowRight, Modifiers::ALT),
                ],
            ),
        );
        assert_eq!(order(&app), [3, 2, 1]);
        frame(
            &ctx,
            &mut app,
            input(
                980.0,
                vec![
                    Event::ModifiersChanged(Modifiers::NONE),
                    key(egui::Key::Delete, Modifiers::NONE),
                ],
            ),
        );
        assert_eq!(order(&app), [3, 1]);
        // Image duration: Enter commits, Escape discards, 0 stays invalid.
        let type_and = |app: &mut NohApp, text: &str, end: egui::Key| {
            app.strip.editing = Some((3, text.into(), false));
            frame(&ctx, app, input(980.0, vec![]));
            frame(&ctx, app, input(980.0, vec![]));
            frame(&ctx, app, input(980.0, vec![key(end, Modifiers::NONE)]));
            frame(&ctx, app, input(980.0, vec![]));
        };
        type_and(&mut app, "2,5", egui::Key::Enter);
        assert!(app.strip.editing.is_none());
        assert_eq!(image_duration(&app.clips[0]), Some(2.5));
        type_and(&mut app, "9", egui::Key::Escape);
        assert!(app.strip.editing.is_none());
        assert_eq!(image_duration(&app.clips[0]), Some(2.5));
        type_and(&mut app, "0", egui::Key::Enter);
        assert!(
            matches!(app.strip.editing, Some((3, _, true))),
            "{:?}",
            app.strip.editing
        );
        assert_eq!(image_duration(&app.clips[0]), Some(2.5));
    }

    /// 4096 clips (the limit) lay out only the tiles in view.
    #[test]
    fn a_long_strip_lays_out_only_the_visible_tiles() {
        let (ctx, mut app) = app(Language::En);
        app.clips = (1..=4096).map(|id| clip(id, "loop.mp4", false)).collect();
        let started = std::time::Instant::now();
        frame(&ctx, &mut app, input(980.0, vec![]));
        let drawn = app.strip.tiles.len();
        assert!(drawn > 0 && drawn < 20, "{drawn} tiles laid out");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn strip_and_song_chip_fit_every_language_and_width() {
        for language in Language::ALL {
            for width in [420.0, 980.0] {
                let (ctx, mut app) = app(language);
                app.clips = vec![
                    clip(1, "vagues.mp4", false),
                    clip(2, "ville-nuit.jpg", true),
                ];
                app.wav = Some("a very long song name that must be truncated somewhere.wav".into());
                app.wav_seconds = Some(182.4);
                frame(&ctx, &mut app, input(width, vec![]));
                let add = app.strip.add_tile.expect("add tile");
                assert!(add.right() <= width, "{} {width}", language.code());
                assert!(
                    app.strip.tiles.iter().all(|(_, r)| r.right() <= width),
                    "{} {width}",
                    language.code()
                );
            }
        }
    }

    #[test]
    fn strip_paints_a_thumbnail_without_explicit_ffmpeg_or_soundtrack() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("source.png");
        image::RgbaImage::from_pixel(80, 45, image::Rgba([0, 200, 80, 255]))
            .save(&path)
            .unwrap();
        // The quick suite runs without FFmpeg; `cargo dev verify --media` sets
        // NOH_MEDIA_TESTS so this check can never be skipped silently there.
        if let Err(error) = noh::inspection::resolve_ffmpeg(Path::new("")) {
            assert!(
                std::env::var_os("NOH_MEDIA_TESTS").is_none(),
                "This media test needs automatically discoverable FFmpeg: {error}"
            );
            eprintln!("SKIPPED: no discoverable FFmpeg; `cargo dev verify --media` runs this test");
            return;
        }
        let (ctx, mut app) = app(Language::En);
        app.clips.push(Clip {
            id: 1,
            request_id: 1,
            item: app_media::item_for_path(path),
            info: None,
            error: None,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let mut output = ctx.run_ui(input(980.0, vec![]), |ui| {
                app.draw(ui);
            });
            assert!(app.ffmpeg.is_none() && app.wav.is_none());
            let tile = app.strip.tiles.first().map(|(_, rect)| rect.expand(0.5));
            let texture =
                app.mini_preview
                    .thumbnail(1, &app.clips[0].item, Path::new(""), &ctx, false);
            if let (Some(texture), Some(tile)) = (texture, tile) {
                let painted = output.shapes.iter().any(|shape| match &shape.shape {
                    egui::Shape::Mesh(mesh) => {
                        mesh.texture_id == texture.id()
                            && mesh.vertices.iter().all(|v| tile.contains(v.pos))
                    }
                    egui::Shape::Rect(rect) => {
                        rect.fill_texture_id() == texture.id() && tile.contains_rect(rect.rect)
                    }
                    _ => false,
                });
                output.textures_delta.clear();
                assert!(
                    painted,
                    "The decoded thumbnail must be painted inside its tile"
                );
                break;
            }
            output.textures_delta.clear();
            assert!(
                std::time::Instant::now() < deadline,
                "Tile thumbnail never appeared"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Tab walks the strip; each control's accessible name carries the file.
    #[test]
    fn strip_controls_are_named_after_their_files() {
        let (ctx, mut app) = app(Language::En);
        app.clips = vec![
            clip(1, "vagues.mp4", false),
            clip(2, "ville-nuit.jpg", true),
        ];
        app.wav = Some("ma-chanson.wav".into());
        app.wav_seconds = Some(182.4);
        frame(&ctx, &mut app, input(980.0, vec![]));
        let mut names = Vec::new();
        for _ in 0..12 {
            let mut output = ctx.run_ui(
                input(980.0, vec![key(egui::Key::Tab, Modifiers::NONE)]),
                |ui| {
                    app.draw(ui);
                },
            );
            output.textures_delta.clear();
            for event in &output.platform_output.events {
                if let egui::output::OutputEvent::FocusGained(info) = event {
                    names.extend(info.label.clone());
                }
            }
        }
        for expected in [
            "vagues.mp4, picture A, 4 s",
            "Remove vagues.mp4",
            "ville-nuit.jpg, picture B, 6 s",
            "Duration of ville-nuit.jpg",
        ] {
            assert!(names.iter().any(|n| n == expected), "{expected}: {names:?}");
        }
    }

    /// Hovering files shows what the drop would do, by name only, and why a
    /// drop would be refused; nothing changes until the drop.
    #[test]
    fn hovering_files_previews_the_drop_without_touching_the_project() {
        let (ctx, mut app) = app(Language::En);
        app.clips = vec![clip(1, "vagues.mp4", false)];
        app.wav = Some("old-song.wav".into());
        app.wav_seconds = Some(30.0);
        let hover = |names: &[&str]| {
            let mut raw = input(980.0, vec![]);
            raw.hovered_files = names
                .iter()
                .map(|name| egui::HoveredFile {
                    path: Some(PathBuf::from(name)),
                    ..Default::default()
                })
                .collect();
            raw
        };
        let texts = frame(
            &ctx,
            &mut app,
            hover(&["a.png", "b.JPG", "c.mov", "new.wav"]),
        );
        let expected = format!(
            "{}\n{}",
            Language::En.format("drop.overlay", &["2 pictures, 1 video, the song".into()]),
            Language::En.format("drop.replaces", &["old-song.wav".into()])
        );
        assert!(texts.contains(&expected), "{texts:?}");
        let texts = frame(&ctx, &mut app, hover(&["a.png", "notes.txt"]));
        let refusal = Language::En.format("drop.rejected_unsupported", &["notes.txt".into()]);
        assert!(texts.contains(&refusal), "{texts:?}");
        assert_eq!(app.clips.len(), 1);
        assert_eq!(app.wav.as_deref(), Some(Path::new("old-song.wav")));
        assert!(app.drop_error.is_none());
    }

    /// Drops refused as a whole: over the 4096-item limit, during an export,
    /// and a song or lyrics while lyrics are being generated.
    #[test]
    fn drops_respect_the_clip_limit_and_running_work() {
        let folder = tempfile::tempdir().unwrap();
        let file = |name: &str| {
            let path = folder.path().join(name);
            std::fs::write(&path, []).unwrap();
            path
        };
        let (video, wav) = (file("more.mp4"), file("other.wav"));
        let ctx = egui::Context::default();
        let mut app = NohApp::default();
        app.clips = (1..=4096).map(|id| clip(id, "loop.mp4", false)).collect();
        app.drop_files(vec![video.clone()], &ctx);
        assert_eq!(
            app.drop_error.as_ref().map(|e| e.key.as_str()),
            Some("diagnosis.clip_limit")
        );
        assert_eq!(app.clips.len(), 4096);
        app.clips.truncate(1);
        app.project.generating = true;
        app.drop_files(vec![video.clone(), wav.clone()], &ctx);
        assert_eq!(
            app.drop_error.as_ref().map(|e| e.key.as_str()),
            Some("drop.busy")
        );
        assert!(app.wav.is_none() && app.clips.len() == 1);
        // Visuals stay editable while lyrics are generated.
        app.drop_files(vec![video], &ctx);
        assert!(app.drop_error.is_none());
        assert_eq!(app.clips.len(), 2);
        app.project.generating = false;
        app.cancelling = true;
        app.drop_files(vec![wav], &ctx);
        assert_eq!(
            app.drop_error.as_ref().map(|e| e.key.as_str()),
            Some("drop.busy")
        );
        assert!(app.wav.is_none());
    }
}
