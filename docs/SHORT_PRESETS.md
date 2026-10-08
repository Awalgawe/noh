# Short presets

Verified on 2026-09-30. Platform overlays vary with the device, description,
placement and interactive controls. These are conservative text guides derived
from official advertising templates, not a guarantee for every organic feed UI.

## In NOH

Select a timeline range, then click its times to open the short configuration.
Use the **Short preset** dropdown to choose
**YouTube Shorts**, **TikTok**, **Reels (Instagram/Facebook)**, or
**YouTube + TikTok**. The monitor's Short view
shows the safe rectangle; shading is a guide only and is never exported. The
panel can hide it without changing caption layout or invalidating an export.

The selected preset moves burned captions away from UI overlays, wraps text
inside the safe width, and protects both top and bottom placement. Font size,
fit/fill framing and the selected interval stay under user control. Presets do
not alter the full montage's captions. **Original margins** retains the former
layout. Text that cannot fit reports an error instead of being cut or shrunk.

Every short already exports as 1080 × 1920, 9:16, H.264 MP4 with AAC when audio
is present; source/project cadence is retained. Preview guides and burned text
use the same proportional rectangle. The full-resolution layout is rendered
before a preview is downscaled.

| Preset | Left | Top | Right | Bottom |
| --- | ---: | ---: | ---: | ---: |
| YouTube Shorts | 48 | 288 | 192 | 672 |
| TikTok | 120 | 240 | 300 | 660 |
| YouTube + TikTok | 120 | 288 | 300 | 672 |
| Reels (Instagram/Facebook) | 120 | 288 | 300 | 672 |

Insets are pixels in a 1080 × 1920 frame. An additional small internal allowance
protects glyph overhang and outlines. The common preset is the intersection.
The Reels preset uses that same conservative rectangle: Meta supports the 35%
bottom recommendation; its other margins are NOH's editorial choice, borrowed
from the common profile rather than an official Meta pixel specification.

## Sources and interpretation

- [YouTube's vertical safe-zone guide](https://support.google.com/google-ads/answer/9128498?hl=en)
  links the measured 1080 × 1920 SVG. It covers advertising across placements,
  which deliberately makes this a cautious choice for ordinary Shorts too.
- [TikTok In-Feed specifications](https://ads.tiktok.com/help/article/tiktok-auction-in-feed-ads?lang=en)
  link the Standard LTR ZIP, `In-Feed/Feed.png`: a 720 × 1280 diagram with 80 px
  side guards, 160 px top, 440 px bottom and another 120 px on the lower right.
  We use the inner rectangle and scale by 1.5. The stepped shape's larger upper
  area is not used for captions. Shop/anchor ads and expanded descriptions can
  need a different layout. This preset follows the LTR template.
- [YouTube Shorts eligibility](https://support.google.com/youtube/answer/15424877?hl=fr):
  square or vertical uploads up to three minutes qualify. The short configuration
  warns above 180 seconds; it never silently trims the selection. TikTok ad
  duration limits are not imposed on organic clips.
- [TikTok's creative guide](https://ads.tiktok.com/business/en-US/blog/7-ways-to-make-your-videos-tiktok-friendly)
  recommends concise text overlays and captions. Practical editing: keep cues
  short, use strong contrast, avoid the right action column, and inspect the
  platform's upload preview with the intended description. Extra bitrate cannot
  compensate for text hidden by UI.

## Additional standards

- [Meta's Reels ads guide (PDF)](https://d3m889aznlr23.cloudfront.net/img/events/458925814/assets/e042d2be.reels_ads_guide1.pdf),
  page 4, recommends leaving the bottom 35% free of key text/logos. The
  [Reels ads page](https://www.facebook.com/business/ads/facebook-instagram-reels-ads)
  provides a safe-zone checker. Advertising guidance is not an organic hard limit.
- [YouTube upload encoding](https://support.google.com/youtube/answer/1722171?hl=en)
  recommends MP4, H.264, AAC-LC, original frame rate, 4:2:0 and BT.709 for SDR.
  Its 1080p guidance is 8 Mbps at standard cadence or 12 Mbps at high cadence;
  these are recommendations, not mandatory file sizes. NOH keeps its existing
  quality-based encoder instead of imposing an arbitrary bitrate or 30 fps.
- [Snapchat advertising formats](https://forbusiness.snapchat.com/advertising/ad-formats)
  specify 9:16, 720 × 1280 and 3–180 seconds for Single Image/Video ads. Those
  limits are not used as Spotlight organic limits. No Snapchat-specific preset
  is shipped without a verified caption-safe rectangle.

## CLI and MCP

Add `--caption-safe-area youtube_shorts|tiktok|reels|universal|none` with `--srt` to
caption, short or project exports. MCP caption requests and short/project
`captions` objects accept `safe_area` with the same values. The optional field
defaults to `none`, so existing clients and serialized jobs preserve their
layout. The core shares layout across CLI, GUI, preview, worker and MCP.
