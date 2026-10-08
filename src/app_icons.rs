//! Stroke icons drawn with the egui painter from the design system's 24-unit grid
//! (18 px, 1.75 stroke, round caps). No SVG loader and no emoji: every glyph is
//! lines, arcs, circles and small filled shapes, defined on a 24-unit grid.
use eframe::egui::{
    Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
    Vec2, pos2,
};
use std::f32::consts::PI;

/// Stroke width on the 24-unit grid, with a 1.75-unit stroke.
const STROKE: f32 = 1.75;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Settings,
    Plus,
    Minus,
    Fit,
    Close,
    Check,
    Song,
    Lyrics,
    More,
    Play,
    Pause,
    Volume,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    ToStart,
    ToEnd,
    FrameBack,
    FrameForward,
    Info,
    Error,
    Ok,
    Warning,
    Folder,
    File,
    Eye,
    Captions,
    Regenerate,
    Replace,
    Remove,
    Restart,
    Drop,
    Picture,
    Video,
    Generate,
    ClipSound,
}

impl Icon {
    pub const ALL: [Icon; 36] = [
        Icon::Settings,
        Icon::Plus,
        Icon::Minus,
        Icon::Fit,
        Icon::Close,
        Icon::Check,
        Icon::Song,
        Icon::Lyrics,
        Icon::More,
        Icon::Play,
        Icon::Pause,
        Icon::Volume,
        Icon::ChevronDown,
        Icon::ChevronRight,
        Icon::ChevronLeft,
        Icon::ToStart,
        Icon::ToEnd,
        Icon::FrameBack,
        Icon::FrameForward,
        Icon::Info,
        Icon::Error,
        Icon::Ok,
        Icon::Warning,
        Icon::Folder,
        Icon::File,
        Icon::Eye,
        Icon::Captions,
        Icon::Regenerate,
        Icon::Replace,
        Icon::Remove,
        Icon::Restart,
        Icon::Drop,
        Icon::Picture,
        Icon::Video,
        Icon::Generate,
        Icon::ClipSound,
    ];
}

enum Part {
    /// Open polyline.
    Line(&'static [(f32, f32)]),
    /// Closed outline.
    Loop(&'static [(f32, f32)]),
    /// Filled convex polygon.
    Fill(&'static [(f32, f32)]),
    Circle(f32, f32, f32),
    /// Filled dot (also the SVG `v.1` "dot" strokes).
    Dot(f32, f32, f32),
    /// Arc around a centre, from one angle to another in degrees (y down).
    Arc(f32, f32, f32, f32, f32),
    /// Rounded rectangle outline: x, y, width, height, radius.
    Frame(f32, f32, f32, f32, f32),
    /// Filled rounded rectangle.
    Block(f32, f32, f32, f32, f32),
}
use Part::*;

const CIRCLE_9: Part = Circle(12.0, 12.0, 9.0);

fn parts(icon: Icon) -> &'static [Part] {
    match icon {
        Icon::Settings => &[
            Line(&[(4.0, 7.0), (14.0, 7.0)]),
            Line(&[(18.0, 7.0), (20.0, 7.0)]),
            Line(&[(4.0, 17.0), (8.0, 17.0)]),
            Line(&[(12.0, 17.0), (20.0, 17.0)]),
            Circle(16.0, 7.0, 2.0),
            Circle(10.0, 17.0, 2.0),
        ],
        Icon::Plus => &[
            Line(&[(12.0, 5.0), (12.0, 19.0)]),
            Line(&[(5.0, 12.0), (19.0, 12.0)]),
        ],
        Icon::Minus => &[Line(&[(5.0, 12.0), (19.0, 12.0)])],
        Icon::Fit => &[
            Line(&[(4.0, 9.0), (4.0, 5.0), (8.0, 5.0)]),
            Line(&[(20.0, 9.0), (20.0, 5.0), (16.0, 5.0)]),
            Line(&[(4.0, 15.0), (4.0, 19.0), (8.0, 19.0)]),
            Line(&[(20.0, 15.0), (20.0, 19.0), (16.0, 19.0)]),
        ],
        Icon::Close => &[
            Line(&[(6.0, 6.0), (18.0, 18.0)]),
            Line(&[(18.0, 6.0), (6.0, 18.0)]),
        ],
        Icon::Check => &[Line(&[(5.0, 12.5), (9.5, 17.0), (19.0, 7.5)])],
        Icon::Song => &[
            Line(&[(9.0, 18.0), (9.0, 6.0), (19.0, 4.0), (19.0, 16.0)]),
            Circle(6.5, 18.0, 2.5),
            Circle(16.5, 16.0, 2.5),
        ],
        Icon::Lyrics => &[
            Line(&[(4.0, 7.0), (20.0, 7.0)]),
            Line(&[(4.0, 12.0), (16.0, 12.0)]),
            Line(&[(4.0, 17.0), (13.0, 17.0)]),
        ],
        Icon::More => &[
            Dot(6.0, 12.0, 1.6),
            Dot(12.0, 12.0, 1.6),
            Dot(18.0, 12.0, 1.6),
        ],
        Icon::Play => &[Fill(&[(8.0, 5.5), (8.0, 18.5), (19.0, 12.0)])],
        Icon::Pause => &[
            Block(7.0, 5.0, 3.5, 14.0, 1.0),
            Block(13.5, 5.0, 3.5, 14.0, 1.0),
        ],
        // "M4 10v4h4l5 4V6L8 10z" + "M16.5 9a4 4 0 0 1 0 6"
        Icon::Volume => &[
            Loop(&[
                (4.0, 10.0),
                (4.0, 14.0),
                (8.0, 14.0),
                (13.0, 18.0),
                (13.0, 6.0),
                (8.0, 10.0),
            ]),
            Arc(13.85, 12.0, 4.0, -48.6, 48.6),
        ],
        Icon::ChevronDown => &[Line(&[(6.0, 9.0), (12.0, 15.0), (18.0, 9.0)])],
        Icon::ChevronRight => &[Line(&[(9.0, 6.0), (15.0, 12.0), (9.0, 18.0)])],
        Icon::ChevronLeft => &[Line(&[(15.0, 6.0), (9.0, 12.0), (15.0, 18.0)])],
        // Navigation: bar and double chevron; one frame: chevron and bar.
        // "M5.5 6v12M13 7l-5 5 5 5M19 7l-5 5 5 5"
        Icon::ToStart => &[
            Line(&[(5.5, 6.0), (5.5, 18.0)]),
            Line(&[(13.0, 7.0), (8.0, 12.0), (13.0, 17.0)]),
            Line(&[(19.0, 7.0), (14.0, 12.0), (19.0, 17.0)]),
        ],
        Icon::ToEnd => &[
            Line(&[(18.5, 6.0), (18.5, 18.0)]),
            Line(&[(11.0, 7.0), (16.0, 12.0), (11.0, 17.0)]),
            Line(&[(5.0, 7.0), (10.0, 12.0), (5.0, 17.0)]),
        ],
        // "M14 7l-5 5 5 5M17.5 7v10"
        Icon::FrameBack => &[
            Line(&[(14.0, 7.0), (9.0, 12.0), (14.0, 17.0)]),
            Line(&[(17.5, 7.0), (17.5, 17.0)]),
        ],
        Icon::FrameForward => &[
            Line(&[(10.0, 7.0), (15.0, 12.0), (10.0, 17.0)]),
            Line(&[(6.5, 7.0), (6.5, 17.0)]),
        ],
        Icon::Info => &[
            CIRCLE_9,
            Line(&[(12.0, 11.0), (12.0, 16.0)]),
            Dot(12.0, 7.75, 1.0),
        ],
        Icon::Error => &[
            CIRCLE_9,
            Line(&[(12.0, 7.5), (12.0, 13.0)]),
            Dot(12.0, 16.35, 1.0),
        ],
        Icon::Ok => &[CIRCLE_9, Line(&[(8.0, 12.3), (10.7, 15.0), (16.0, 9.5)])],
        Icon::Warning => &[
            Loop(&[(12.0, 4.0), (21.0, 20.0), (3.0, 20.0)]),
            Line(&[(12.0, 10.0), (12.0, 14.0)]),
            Dot(12.0, 17.25, 1.0),
        ],
        Icon::Folder => &[Loop(&[
            (3.5, 7.0),
            (5.0, 5.5),
            (9.0, 5.5),
            (11.0, 7.5),
            (19.0, 7.5),
            (20.5, 9.0),
            (20.5, 17.5),
            (19.0, 19.0),
            (5.0, 19.0),
            (3.5, 17.5),
        ])],
        Icon::File => &[
            Loop(&[
                (7.0, 3.5),
                (14.0, 3.5),
                (18.0, 7.5),
                (18.0, 20.5),
                (7.0, 20.5),
            ]),
            Line(&[(14.0, 3.5), (14.0, 7.5), (18.0, 7.5)]),
        ],
        Icon::Eye => &[
            Loop(&[
                (3.0, 12.0),
                (6.2, 8.1),
                (12.0, 6.0),
                (17.8, 8.1),
                (21.0, 12.0),
                (17.8, 15.9),
                (12.0, 18.0),
                (6.2, 15.9),
            ]),
            Circle(12.0, 12.0, 2.5),
        ],
        Icon::Captions => &[
            Frame(3.5, 5.0, 17.0, 14.0, 2.0),
            Line(&[(7.0, 15.0), (17.0, 15.0)]),
        ],
        // "M19 12a7 7 0 1 1-2-4.9" + "M19 4.5V8h-3.5"
        Icon::Regenerate => &[
            Arc(12.0, 12.0, 7.0, 0.0, 315.6),
            Line(&[(19.0, 4.5), (19.0, 8.0), (15.5, 8.0)]),
        ],
        Icon::Replace => &[
            Line(&[(5.0, 9.0), (17.0, 9.0), (14.0, 6.0)]),
            Line(&[(19.0, 15.0), (7.0, 15.0), (10.0, 18.0)]),
        ],
        Icon::Remove => &[
            Line(&[(5.0, 7.0), (19.0, 7.0)]),
            Line(&[(10.0, 7.0), (10.0, 5.0), (14.0, 5.0), (14.0, 7.0)]),
            Line(&[(7.0, 7.0), (8.0, 19.0), (16.0, 19.0), (17.0, 7.0)]),
        ],
        // "M5 12a7 7 0 1 0 2-4.9" + "M5 4.5V8h3.5": the range's restart mark.
        Icon::Restart => &[
            Arc(12.0, 12.0, 7.0, 180.0, -135.6),
            Line(&[(5.0, 4.5), (5.0, 8.0), (8.5, 8.0)]),
        ],
        Icon::Drop => &[
            Line(&[(12.0, 4.0), (12.0, 14.0)]),
            Line(&[(8.0, 10.0), (12.0, 14.0), (16.0, 10.0)]),
            Line(&[(4.0, 14.0), (4.0, 18.5), (20.0, 18.5), (20.0, 14.0)]),
        ],
        Icon::Picture => &[
            Frame(4.0, 5.0, 16.0, 14.0, 2.0),
            Line(&[
                (4.0, 16.0),
                (8.5, 11.5),
                (12.0, 15.0),
                (14.5, 12.5),
                (20.0, 17.0),
            ]),
        ],
        Icon::Video => &[
            Frame(4.0, 5.0, 16.0, 14.0, 2.0),
            Line(&[(4.0, 9.0), (20.0, 9.0)]),
            Line(&[(4.0, 15.0), (20.0, 15.0)]),
            Line(&[(9.0, 5.0), (9.0, 9.0)]),
            Line(&[(15.0, 5.0), (15.0, 9.0)]),
            Line(&[(9.0, 15.0), (9.0, 19.0)]),
            Line(&[(15.0, 15.0), (15.0, 19.0)]),
        ],
        Icon::Generate => &[
            Line(&[(4.0, 9.0), (4.0, 15.0)]),
            Line(&[(7.5, 6.0), (7.5, 18.0)]),
            Line(&[(11.0, 10.0), (11.0, 14.0)]),
            Line(&[(14.0, 8.0), (20.0, 8.0)]),
            Line(&[(14.0, 12.0), (20.0, 12.0)]),
            Line(&[(14.0, 16.0), (18.0, 16.0)]),
        ],
        // Mark of the Fades button when clip sound is mixed.
        Icon::ClipSound => &[
            Line(&[(4.0, 10.0), (4.0, 14.0)]),
            Line(&[(8.0, 7.0), (8.0, 17.0)]),
            Line(&[(12.0, 4.0), (12.0, 20.0)]),
            Line(&[(16.0, 7.0), (16.0, 17.0)]),
            Line(&[(20.0, 10.0), (20.0, 14.0)]),
        ],
    }
}

/// Shapes for `icon` fitted into the square centred in `rect`.
pub fn shapes(icon: Icon, rect: Rect, color: Color32) -> Vec<Shape> {
    let side = rect.width().min(rect.height());
    let unit = side / 24.0;
    let origin = rect.center() - Pos2::new(side / 2.0, side / 2.0).to_vec2();
    let at = |(x, y): (f32, f32)| pos2(origin.x + x * unit, origin.y + y * unit);
    let stroke = Stroke::new(STROKE * unit, color);
    let square =
        |x: f32, y: f32, w: f32, h: f32| Rect::from_min_max(at((x, y)), at((x + w, y + h)));
    let corner = |r: f32| CornerRadius::same((r * unit).round().clamp(0.0, 255.0) as u8);
    parts(icon)
        .iter()
        .map(|part| match part {
            Line(points) => Shape::line(points.iter().copied().map(at).collect(), stroke),
            Loop(points) => Shape::closed_line(points.iter().copied().map(at).collect(), stroke),
            Fill(points) => Shape::convex_polygon(
                points.iter().copied().map(at).collect(),
                color,
                Stroke::NONE,
            ),
            Circle(x, y, r) => Shape::circle_stroke(at((*x, *y)), r * unit, stroke),
            Dot(x, y, r) => Shape::circle_filled(at((*x, *y)), r * unit, color),
            Arc(x, y, r, from, to) => {
                let steps = (((to - from).abs() / 10.0).ceil() as usize).max(2);
                let points = (0..=steps)
                    .map(|i| {
                        let a = (from + (to - from) * i as f32 / steps as f32) * PI / 180.0;
                        at((x + r * a.cos(), y + r * a.sin()))
                    })
                    .collect();
                Shape::line(points, stroke)
            }
            Frame(x, y, w, h, r) => Shape::rect_stroke(
                square(*x, *y, *w, *h),
                corner(*r),
                stroke,
                StrokeKind::Middle,
            ),
            Block(x, y, w, h, r) => Shape::rect_filled(square(*x, *y, *w, *h), corner(*r), color),
        })
        .collect()
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    painter.extend(shapes(icon, rect, color));
}

/// Size of the wordmark in design units (logo construction board).
pub const WORDMARK: (f32, f32) = (2886.1, 770.0);

/// The NOH wordmark: widely spaced Jost ExtraBold letters (font family "logo"),
/// scaled to fit and centred in `rect`. Design units are font units; the box
/// includes the N apexes, which overshoot the 700-unit cap height by 35.
pub fn wordmark(painter: &Painter, rect: Rect, color: Color32) {
    let (w, h) = WORDMARK;
    let s = (rect.width() / w).min(rect.height() / h);
    let origin = rect.center() - Vec2::new(w, h) * s / 2.0;
    let at = |x: f32, y: f32| origin + Vec2::new(x, y) * s;
    let font = FontId::new(1000.0 * s, FontFamily::Name("logo".into()));
    for (letter, x) in [("N", -71.93), ("O", 1029.92), ("H", 2150.19)] {
        let galley = painter.layout_no_wrap(letter.into(), font.clone(), color);
        // Place the pen position on the design baseline, as SVG text does.
        let pen = galley.rows.first().and_then(|row| {
            row.glyphs
                .first()
                .map(|glyph| row.pos + glyph.pos.to_vec2())
        });
        if let Some(pen) = pen {
            painter.galley(at(x, 735.0) - pen.to_vec2(), galley, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::{self, vec2};

    #[test]
    fn icons_stay_inside_their_box_at_every_size_and_scale() {
        for icon in Icon::ALL {
            for side in [16.0, 18.0, 40.0] {
                let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(side, side));
                let shapes = shapes(icon, rect, Color32::WHITE);
                assert!(!shapes.is_empty());
                for shape in &shapes {
                    assert!(
                        rect.contains_rect(shape.visual_bounding_rect()),
                        "{icon:?} {side}: {:?} outside {rect:?}",
                        shape.visual_bounding_rect()
                    );
                }
                for scale in [1.0, 1.5, 2.0] {
                    let ctx = egui::Context::default();
                    ctx.set_pixels_per_point(scale);
                    let mut output = ctx.run_ui(Default::default(), |ui| {
                        paint(ui.painter(), rect, icon, Color32::WHITE)
                    });
                    output.textures_delta.clear();
                    let meshes = ctx.tessellate(output.shapes, scale);
                    let bounds = meshes
                        .iter()
                        .filter_map(|m| match &m.primitive {
                            egui::epaint::Primitive::Mesh(mesh) => Some(mesh.calc_bounds()),
                            _ => None,
                        })
                        .fold(Rect::NOTHING, |a, b| a.union(b));
                    // Feathering adds at most one physical pixel around the strokes.
                    assert!(
                        rect.expand(1.0 / scale).contains_rect(bounds),
                        "{icon:?} {side} @ {scale}: {bounds:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn arcs_start_and_end_at_the_expected_coordinates() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(24.0, 24.0));
        let ends = |icon| match &shapes(icon, rect, Color32::WHITE)[0] {
            Shape::Path(path) => (path.points[0], *path.points.last().unwrap()),
            other => panic!("{other:?}"),
        };
        let close = |a: Pos2, b: (f32, f32)| (a - pos2(b.0, b.1)).length() < 0.15;
        let (start, end) = ends(Icon::Restart);
        assert!(
            close(start, (5.0, 12.0)) && close(end, (7.0, 7.1)),
            "{start:?} {end:?}"
        );
        let (start, end) = ends(Icon::Regenerate);
        assert!(
            close(start, (19.0, 12.0)) && close(end, (17.0, 7.1)),
            "{start:?} {end:?}"
        );
    }
}
