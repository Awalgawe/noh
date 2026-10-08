//! Conservative text rectangles derived from platform UI templates.
//! Sources, scope and reference dimensions: docs/SHORT_PRESETS.md.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SafeArea {
    #[default]
    None,
    YoutubeShorts,
    Tiktok,
    Reels,
    Universal,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_area_is_the_intersection_at_export_and_preview_sizes() {
        for (w, h) in [(1080, 1920), (360, 640), (302, 540)] {
            let yt = SafeArea::YoutubeShorts.insets(w, h).unwrap();
            let tt = SafeArea::Tiktok.insets(w, h).unwrap();
            let both = SafeArea::Universal.insets(w, h).unwrap();
            for i in 0..4 {
                assert_eq!(both[i], yt[i].max(tt[i]));
            }
        }
        assert!(!SafeArea::YoutubeShorts.exceeds_short_duration(180_000));
        assert!(SafeArea::YoutubeShorts.exceeds_short_duration(180_001));
        assert!(!SafeArea::Tiktok.exceeds_short_duration(600_000));
    }

    #[test]
    fn old_styles_keep_their_geometry_and_unknown_presets_are_rejected() {
        let style: crate::captions::CaptionStyle =
            serde_json::from_str(r#"{"size":"large","placement":"bottom"}"#).unwrap();
        assert_eq!(style.safe_area, SafeArea::None);
        assert!(serde_json::from_str::<SafeArea>(r#""automatic""#).is_err());
    }
}

impl SafeArea {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::YoutubeShorts,
        Self::Tiktok,
        Self::Reels,
        Self::Universal,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::None => "preset.none",
            Self::YoutubeShorts => "preset.youtube",
            Self::Tiktok => "preset.tiktok",
            Self::Reels => "preset.reels",
            Self::Universal => "preset.universal",
        }
    }

    /// Left, top, right, bottom in a 1080×1920 canvas. TikTok's stepped
    /// template is reduced to its inner rectangle, including side crop guards.
    pub fn reference_insets(self) -> Option<[u32; 4]> {
        match self {
            Self::None => None,
            Self::YoutubeShorts => Some([48, 288, 192, 672]),
            Self::Tiktok => Some([120, 240, 300, 660]),
            Self::Universal => Some([120, 288, 300, 672]),
            // Meta specifies 35% at the bottom for Reels ads. Retain our
            // conservative common top/side guards; these are editorial choices.
            Self::Reels => Some([120, 288, 300, 672]),
        }
    }

    /// Round inward so previews and full exports preserve the protected area.
    pub fn insets(self, width: u32, height: u32) -> Option<[u32; 4]> {
        self.reference_insets().map(|[l, t, r, b]| {
            [
                (l * width).div_ceil(1080),
                (t * height).div_ceil(1920),
                (r * width).div_ceil(1080),
                (b * height).div_ceil(1920),
            ]
        })
    }

    pub fn exceeds_short_duration(self, duration_ms: u64) -> bool {
        matches!(self, Self::YoutubeShorts | Self::Universal) && duration_ms > 180_000
    }
}
