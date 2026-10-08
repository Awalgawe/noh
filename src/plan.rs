//! Pure render decisions. Exact inspection is performed by the media adapter.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Translation keys used by both diagnostic adapters; engine reason IDs stay stable.
pub fn reason_key(reason: &str) -> String {
    if let Some(reason) = reason.strip_prefix("warning.partial_") {
        format!("reason.partial_{reason}")
    } else {
        format!("reason.{reason}")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Treatment {
    Copy,
    Convert,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClipPlan {
    pub path: PathBuf,
    pub treatment: Treatment,
    /// Stable reason IDs, including all incompatible geometry/timing constraints.
    pub reasons: Vec<String>,
    pub timestamp_normalization: bool,
    /// Packet duration for videos; realized frame-grid duration for images.
    pub source_seconds: f64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Target {
    pub width: u32,
    pub height: u32,
    pub rate_num: i64,
    pub rate_den: i64,
    pub codec: String,
    pub pixel_format: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RenderPlan {
    pub exact_inspection: bool,
    pub target: Target,
    pub clips: Vec<ClipPlan>,
    /// Sequence preparation and final rendering are reported separately.
    pub stage: String,
    pub segments: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub treatment: Treatment,
}

/// Complete loops around fades must be encoded; an incomplete B-frame tail
/// must also be encoded. This calculation is shared with command generation.
pub fn fade_segments(
    duration: f64,
    loop_seconds: f64,
    fade_in: f64,
    fade_out: f64,
    reordered: bool,
) -> Vec<Segment> {
    if [fade_in, fade_out]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        || fade_in + fade_out > duration
        || !duration.is_finite()
        || !loop_seconds.is_finite()
        || duration <= 0.0
        || loop_seconds <= 0.0
    {
        return Vec::new();
    }
    let head = if fade_in > 0.0 {
        (fade_in / loop_seconds - 1e-9).ceil() * loop_seconds
    } else {
        0.0
    };
    let tail = ((duration - fade_out) / loop_seconds + 1e-9).floor() * loop_seconds;
    if tail <= head {
        return vec![Segment {
            start: 0.0,
            end: duration,
            treatment: Treatment::Convert,
        }];
    }
    let copy_end = if fade_out > 0.0 || reordered {
        tail.min(duration)
    } else {
        duration
    };
    let mut segments = Vec::with_capacity(3);
    if head > 0.0 {
        segments.push(Segment {
            start: 0.0,
            end: head,
            treatment: Treatment::Convert,
        });
    }
    segments.push(Segment {
        start: head,
        end: copy_end,
        treatment: Treatment::Copy,
    });
    if copy_end < duration {
        segments.push(Segment {
            start: copy_end,
            end: duration,
            treatment: Treatment::Convert,
        });
    }
    segments
}

pub(crate) struct Decision {
    pub public: RenderPlan,
    pub profile: Option<String>,
    pub level: Option<String>,
    pub parameter_sets: u32,
    pub encoded_sps: u32,
    pub decode_delay: i64,
    pub normalize: bool,
    pub heterogeneous: bool,
}

pub(crate) fn target(info: &crate::media::MediaInfo, preview: bool) -> Target {
    let scale = if preview {
        (640.0 / info.display_width)
            .min(360.0 / info.display_height)
            .min(1.0)
    } else {
        1.0
    };
    let dimension = |v: f64| {
        if preview {
            ((v * scale / 2.0).floor() as u32 * 2).max(2)
        } else {
            ((v / 2.0).ceil() as u32 * 2).max(2)
        }
    };
    // A packet duration from an irregular/coarse grid is not a frame rate.
    let (rate_num, rate_den) = if info.fixed && info.timescale > 0 && info.step > 0 {
        (info.timescale, info.step)
    } else {
        ((info.fps * 1000.0).round().max(1.0) as i64, 1000)
    };
    Target {
        width: dimension(info.display_width),
        height: dimension(info.display_height),
        rate_num,
        rate_den,
        codec: "h264".into(),
        pixel_format: "yuv420p".into(),
    }
}

pub(crate) fn item_target(
    items: &[crate::engine::MediaItem],
    info: &[crate::media::MediaInfo],
    preview: bool,
) -> Target {
    let first = items.iter().position(|item| !item.is_image()).unwrap_or(0);
    target(&info[first], preview)
}

pub(crate) fn decide(
    items: &[crate::engine::MediaItem],
    info: &[crate::media::MediaInfo],
    support: Vec<Option<crate::h264::Support>>,
    encode: bool,
    preview: bool,
    for_fades: bool,
) -> Decision {
    let mut target = item_target(items, info, preview);
    let heterogeneous = info
        .iter()
        .skip(1)
        .any(|i| i.signature != info[0].signature);
    let normalize =
        encode || heterogeneous || for_fades || items.iter().any(|item| item.is_image());
    if !normalize {
        target.codec = info[0].codec.clone();
        target.pixel_format = "preserved".into();
        target.width = info[0].width;
        target.height = info[0].height;
    }
    let fps = target.rate_num as f64 / target.rate_den as f64;
    let (mut profile, mut level) = (None, None);
    let (mut parameter_sets, mut decode_delay) = (0, 0);
    let mut clips = Vec::with_capacity(items.len());
    for ((item, meta), support) in items.iter().zip(info).zip(support) {
        let mut reasons = Vec::new();
        if item.is_image() {
            reasons.push("image".into());
        }
        if encode {
            reasons.push(
                if preview {
                    "preview"
                } else {
                    "requested_encoding"
                }
                .into(),
            );
        } else if normalize && !item.is_image() {
            if meta.width != target.width || meta.height != target.height {
                reasons.push("dimensions".into());
            }
            if !meta.square_pixels {
                reasons.push("pixel_aspect".into());
            }
            if meta.rotation.rem_euclid(360.0).abs() > 0.01 {
                reasons.push("rotation".into());
            }
            if !meta.fixed {
                reasons.push("warning.partial_timing".into());
            }
            if (meta.fps - fps).abs() > 1e-6 {
                reasons.push("frame_rate".into());
            }
            match support {
                Some(crate::h264::Support::Convert(reason)) => reasons.push(reason.into()),
                Some(crate::h264::Support::Copy(parameters)) if reasons.is_empty() => {
                    let target_profile = profile.get_or_insert(parameters.profile.clone());
                    let target_level = level.get_or_insert(parameters.level.clone());
                    if parameters.profile != *target_profile {
                        reasons.push("profile".into());
                    }
                    if parameters.level != *target_level {
                        reasons.push("level".into());
                    }
                    if reasons.is_empty() {
                        parameter_sets |= parameters.parameter_sets;
                        decode_delay =
                            decode_delay.max(meta.decode_delay / meta.step * target.rate_den);
                    }
                }
                _ => {}
            }
        }
        clips.push(ClipPlan {
            path: item.path().clone(),
            treatment: if reasons.is_empty() {
                Treatment::Copy
            } else {
                Treatment::Convert
            },
            reasons,
            timestamp_normalization: normalize,
            source_seconds: meta.seconds,
        });
    }
    // Reserve an ID for converted clips AND for fade endpoints. Never silently
    // reuse SPS 0 if all available IDs belong to copied input pictures.
    let needs_id =
        u32::from(for_fades) + u32::from(clips.iter().any(|c| c.treatment == Treatment::Convert));
    let free_id = (0..32).find(|id| parameter_sets & (1u32 << id) == 0);
    if needs_id > parameter_sets.count_zeros() {
        for clip in &mut clips {
            clip.treatment = Treatment::Convert;
            clip.reasons.push("warning.partial_parameters".into());
        }
        parameter_sets = 0;
        decode_delay = 0;
        profile = None;
        level = None;
    }
    let encoded_sps = free_id.unwrap_or(0);
    if clips.iter().any(|c| c.treatment == Treatment::Convert) {
        parameter_sets |= 1 << encoded_sps;
    }
    Decision {
        public: RenderPlan {
            exact_inspection: true,
            target,
            clips,
            stage: "preparation".into(),
            segments: Vec::new(),
        },
        profile,
        level,
        parameter_sets,
        encoded_sps,
        decode_delay,
        normalize,
        heterogeneous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn irregular_grid_uses_average_rate_not_first_packet_duration() {
        let meta = crate::media::MediaInfo {
            fps: 25.0,
            timescale: 1000,
            step: 20,
            fixed: false,
            ..Default::default()
        };
        let t = target(&meta, false);
        assert_eq!(t.rate_num as f64 / t.rate_den as f64, 25.0);
    }
    #[test]
    fn segments_cover_output_and_only_complete_b_frame_loops_are_copied() {
        for duration in [0.04_f64, 0.5, 1.0, 3.44, 7.44, 20.0] {
            for fade in [0.0, duration.min(0.2) / 2.0, duration / 2.0] {
                let segments = fade_segments(duration, 1.0, fade, fade, true);
                assert_eq!(segments.first().unwrap().start, 0.0);
                assert_eq!(segments.last().unwrap().end, duration);
                assert!(segments.windows(2).all(|s| s[0].end == s[1].start));
                for s in segments.iter().filter(|s| s.treatment == Treatment::Copy) {
                    assert!(s.start >= fade && s.end <= duration - fade + 1e-9);
                    assert!((s.end - s.start).fract().abs() < 1e-9);
                }
            }
        }
    }

    fn facts() -> crate::media::MediaInfo {
        crate::media::MediaInfo {
            width: 96,
            height: 64,
            display_width: 96.0,
            display_height: 64.0,
            seconds: 1.0,
            fps: 25.0,
            fixed: true,
            timescale: 25,
            step: 1,
            square_pixels: true,
            ..Default::default()
        }
    }
    fn support(profile: &str, level: &str, ids: u32) -> Option<crate::h264::Support> {
        Some(crate::h264::Support::Copy(crate::h264::Video {
            frames: 25,
            step: 1,
            timescale: 25,
            profile: profile.into(),
            level: level.into(),
            sps: 0,
            parameter_sets: ids,
            decode_delay: 0,
        }))
    }
    #[test]
    fn profile_and_level_differences_convert_only_the_affected_clip() {
        let d = decide(
            &["first".into(), "second".into()],
            &[facts(), facts()],
            vec![support("main", "3.0", 1), support("high", "4.0", 2)],
            false,
            false,
            true,
        );
        assert_eq!(d.public.clips[0].treatment, Treatment::Copy);
        assert_eq!(d.public.clips[1].treatment, Treatment::Convert);
        assert_eq!(d.public.clips[1].reasons, ["profile", "level"]);
        assert!(d.public.clips[0].timestamp_normalization);
    }
    #[test]
    fn exhausted_ids_cannot_be_reused_by_converted_clips_or_fades() {
        for mask in [u32::MAX, u32::MAX >> 1] {
            let d = decide(
                &["first".into(), "second".into()],
                &[facts(), facts()],
                vec![
                    support("main", "3.0", mask),
                    Some(crate::h264::Support::Convert("warning.partial_codec")),
                ],
                false,
                false,
                true,
            );
            assert!(
                d.public
                    .clips
                    .iter()
                    .all(|c| c.treatment == Treatment::Convert)
            );
            assert!(
                d.public
                    .clips
                    .iter()
                    .all(|c| c.reasons.iter().any(|r| r == "warning.partial_parameters"))
            );
            assert_eq!(d.parameter_sets.count_ones(), 1);
        }
    }
    #[test]
    fn invalid_fade_inputs_have_no_executable_segments() {
        for (duration, loop_seconds, fade_in, fade_out) in [
            (f64::NAN, 1.0, 0.0, 0.0),
            (1.0, 0.0, 0.0, 0.0),
            (1.0, 1.0, -0.1, 0.0),
            (1.0, 1.0, 0.6, 0.6),
            (1.0, 1.0, f64::INFINITY, 0.0),
        ] {
            assert!(fade_segments(duration, loop_seconds, fade_in, fade_out, true).is_empty());
        }
    }
    #[test]
    fn all_images_use_first_displayed_geometry_at_twenty_five_fps() {
        use crate::engine::MediaItem;
        let items = [
            MediaItem::Image {
                path: "first.png".into(),
                duration: 0.2,
            },
            MediaItem::Image {
                path: "second.jpg".into(),
                duration: 0.3,
            },
        ];
        let mut portrait = facts();
        portrait.width = 65;
        portrait.height = 97;
        portrait.display_width = 65.0;
        portrait.display_height = 97.0;
        let d = decide(
            &items,
            &[portrait, facts()],
            vec![None, None],
            false,
            false,
            false,
        );
        assert_eq!(
            (
                d.public.target.width,
                d.public.target.height,
                d.public.target.rate_num,
                d.public.target.rate_den
            ),
            (66, 98, 25, 1)
        );
        assert!(d.normalize);
        assert!(
            d.public
                .clips
                .iter()
                .all(|clip| clip.treatment == Treatment::Convert && clip.reasons == ["image"])
        );
    }
}
