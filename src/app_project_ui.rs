//! One montage view: timeline and selected short. Lyrics live in the chip (app_lyrics).
use super::*;

pub struct State {
    pub timeline: app_timeline::TimelineState,
    /// Range-row buttons of the last frame, for tests (creation = Tab order).
    #[cfg(test)]
    pub range_buttons: Vec<(&'static str, egui::Rect)>,
    /// The times popover's fields.
    pub times: app_range::Times,
}
impl Default for State {
    fn default() -> Self {
        Self {
            timeline: Default::default(),
            #[cfg(test)]
            range_buttons: Vec::new(),
            times: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use noh::timeline::Range;

    fn clip(id: u64, item: noh::input::MediaItem, seconds: f64) -> Clip {
        let mut info = noh::media::MediaInfo::default();
        info.seconds = seconds;
        Clip {
            id,
            request_id: id,
            item,
            info: Some(info),
            error: None,
        }
    }

    #[test]
    fn unreadable_middle_picture_keeps_original_letters_in_the_painted_lane() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        let mut app = NohApp::default();
        app.project.duration_ms = 20_000;
        app.project.viewport = Range::new(0, 20_000, 20_000).map(noh::timeline::Viewport);
        app.clips = vec![
            clip(1, "a.mp4".into(), 4.0),
            clip(2, "b.mp4".into(), 4.0),
            clip(3, "c.mp4".into(), 4.0),
        ];
        app.clips[1].error = Some("ui.unreadable".into());

        let visuals = app.project_visuals();
        assert_eq!(
            visuals.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
            ["A", "C"]
        );
        assert_eq!(
            visuals.iter().map(|v| v.seconds).collect::<Vec<_>>(),
            [4.0, 4.0]
        );
        assert_eq!(
            visuals.iter().map(|v| v.source_index).collect::<Vec<_>>(),
            [0, 2]
        );

        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(980.0, 400.0),
                )),
                ..Default::default()
            },
            |ui| {
                app_style::apply(ui.ctx());
                app.project_timeline(ui, false);
            },
        );
        output.textures_delta.clear();
        let labels: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text().to_owned())
                } else {
                    None
                }
            })
            .collect();
        assert!(labels.iter().any(|label| label == "A"), "{labels:?}");
        assert!(labels.iter().any(|label| label == "C"), "{labels:?}");
        assert!(!labels.iter().any(|label| label == "B"), "{labels:?}");
        let lane = app.project_ui.timeline.pictures_rect().unwrap();
        let mut cells: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.top() - lane.top()).abs() < 0.1
                        && (rect.rect.bottom() - lane.bottom()).abs() < 0.1
                        // Four-second cells, excluding the lane/fade backgrounds.
                        && (rect.rect.width() - (lane.width() / 5.0 - 1.0)).abs() < 0.1 =>
                {
                    Some((rect.rect.left(), rect.fill))
                }
                _ => None,
            })
            .collect();
        cells.sort_by(|a, b| a.0.total_cmp(&b.0));
        let colors = app_style::tokens_for(ctx.global_style().visuals.dark_mode).seq;
        assert!(cells.len() >= 2, "pictures lane cells: {cells:?}");
        assert_eq!(cells[0].1, colors[0]);
        assert_eq!(cells[1].1, colors[2]);
    }

    #[test]
    fn pending_metadata_and_invalid_image_durations_leave_readable_pictures_visible() {
        let mut app = NohApp::default();
        app.clips = vec![
            clip(1, "a.mp4".into(), 4.0),
            clip(2, "b.mp4".into(), 5.0),
            clip(
                3,
                noh::input::MediaItem::Image {
                    path: "c.png".into(),
                    duration: 0.0,
                },
                0.0,
            ),
            clip(
                4,
                noh::input::MediaItem::Image {
                    path: "d.png".into(),
                    duration: 3.0,
                },
                0.0,
            ),
        ];
        app.clips[1].info = None;
        app.clips[3].info = None;
        assert_eq!(
            app.project_visuals()
                .iter()
                .map(|v| v.label.as_str())
                .collect::<Vec<_>>(),
            ["A"]
        );

        app.clips[2].item = noh::input::MediaItem::Image {
            path: "c.png".into(),
            duration: f64::NAN,
        };
        assert_eq!(
            app.project_visuals()
                .iter()
                .map(|v| v.label.as_str())
                .collect::<Vec<_>>(),
            ["A"]
        );
        app.clips[2].item = noh::input::MediaItem::Image {
            path: "c.png".into(),
            duration: 2.5,
        };
        app.clips[3].info = Some(noh::media::MediaInfo::default());
        let visuals = app.project_visuals();
        assert_eq!(
            visuals.iter().map(|v| v.label.as_str()).collect::<Vec<_>>(),
            ["A", "C", "D"]
        );
        assert_eq!(
            visuals.iter().map(|v| v.seconds).collect::<Vec<_>>(),
            [4.0, 2.5, 3.0]
        );
    }
    /// The painted flag keeps the interface decimal in both preview views.
    #[test]
    fn short_flag_text_follows_the_language_in_video_and_short_views() {
        let ctx = egui::Context::default();
        app_locale::install_fonts(&ctx);
        for (language, expected) in [
            (Language::De, "Kurzclip 5,0 s"),
            (Language::Fr, "Extrait 5,0 s"),
            (Language::Es, "Clip corto 5,0 s"),
            (Language::En, "Short 5.0 s"),
            (Language::Ja, "ショート 5.0 秒"),
        ] {
            for short_view in [false, true] {
                let mut app = NohApp::default();
                app.locale.language = language;
                app.project.duration_ms = 182_400;
                app.project.range = Range::new(13_000, 18_000, 182_400);
                app.project.viewport = Range::new(0, 30_000, 182_400).map(noh::timeline::Viewport);
                app.mini_preview.short = short_view;
                app.mini_preview.cursor_ms = 15_200;
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(980.0, 400.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        app_style::apply(ui.ctx());
                        ui.add_space(40.0);
                        app.project_timeline(ui, false);
                    },
                );
                output.textures_delta.clear();
                let painted = output.shapes.iter().any(|shape| {
                    matches!(&shape.shape, egui::Shape::Text(text) if text.galley.text() == expected)
                });
                assert!(painted, "{expected} (short view {short_view})");
            }
        }
    }
}

impl NohApp {
    pub(super) fn project_timeline(&mut self, ui: &mut egui::Ui, locked: bool) {
        let visuals = self.project_visuals();
        let output = app_timeline::show(
            ui,
            &self.locale,
            &mut self.project_ui.timeline,
            app_timeline::Input {
                cursor_ms: Some(self.mini_preview.cursor_ms),
                duration_ms: self.project.duration_ms,
                range: self.project.range,
                viewport: self.project.viewport,
                waveform: self.project.waveform.as_deref(),
                track: self.project.track.loaded.as_ref().map(|l| l.track.as_ref()),
                visuals: &visuals,
                waveform_loading: self.project.waveform_loading,
                waveform_error: self.project.waveform_error.as_deref(),
                enabled: !locked && ui.is_enabled(),
                fade_in_seconds: if self.in_enabled { self.fade_in } else { 0.0 },
                fade_out_seconds: if self.out_enabled { self.fade_out } else { 0.0 },
                restart: self.project.restart_loops,
            },
        );
        if self.mini_preview.short && self.project.range != output.range {
            self.mini_preview.invalidate();
        }
        self.project.range = output.range;
        self.project.viewport = output.viewport;
        if let Some(ms) = output.seek_ms {
            self.mini_hover(None, ui.ctx());
            self.mini_seek(ms, true);
        } else if self.capture_overlay.is_none() {
            // Capture states keep their configured cursor position.
            self.mini_hover(output.hover_ms, ui.ctx());
        }
        if output.commit_changed {
            self.project.short_revision = self.project.short_revision.wrapping_add(1);
            self.project.range_moved = false;
            self.project.failure = None;
            self.project.outcome = None;
        }
        if output.retry {
            self.project.retry_waveform(ui.ctx());
        }
    }

    /// The montage of the pictures that can be read, each with its letter: an
    /// unreadable file (or one still being read) leaves the lane showing the
    /// others instead of emptying it.
    pub(super) fn project_visuals(&self) -> Vec<app_timeline::Visual> {
        self.clips
            .iter()
            .enumerate()
            .filter(|(_, c)| c.error.is_none() && c.info.is_some())
            .map(|(i, c)| app_timeline::Visual {
                label: app_timeline::sequence_letter(i),
                source_index: i,
                seconds: self
                    .diagnosis
                    .as_ref()
                    .filter(|_| self.diagnosis_current())
                    .and_then(|d| d.plan.as_ref())
                    .and_then(|p| p.clips.get(i))
                    .map(|p| p.source_seconds)
                    .unwrap_or_else(|| match c.item {
                        noh::input::MediaItem::Image { duration, .. } => duration,
                        _ => c.info.as_ref().map_or(0.0, |m| m.seconds),
                    }),
            })
            // Not measured yet (still reading), or an invalid picture duration.
            .filter(|v| v.seconds.is_finite() && v.seconds > 0.0)
            .collect()
    }
}
