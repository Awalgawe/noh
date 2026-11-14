//! One colour matrix per export (the maintainer's rules, 2026-11-08).
//!
//! An export uses a single YUV matrix: the most recent standard among its video
//! clips (BT.2020 > BT.709 > BT.601). A clip without a matrix tag is read the way
//! players read it: BT.709 when at least 1280 wide or more than 576 high, else
//! BT.601. Clips in another matrix, or in full range, are converted; a clip
//! already in the export matrix and in limited range keeps its YUV values.
//!
//! Tags are written with a final `setparams` filter only: on FFmpeg 7.1 the
//! `-colorspace`/`-color_*` output options insert a conversion that treats
//! untagged frames as BT.601, and lose primaries and transfer.

/// A YUV matrix family, ordered from oldest to most recent.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Matrix {
    Bt601,
    Bt709,
    Bt2020,
}

impl Matrix {
    /// Name for swscale's `in_color_matrix`/`out_color_matrix`.
    pub fn swscale(self) -> &'static str {
        match self {
            Self::Bt601 => "bt601",
            Self::Bt709 => "bt709",
            Self::Bt2020 => "bt2020",
        }
    }

    /// Label shown to people.
    pub fn label(self) -> &'static str {
        match self {
            Self::Bt601 => "BT.601",
            Self::Bt709 => "BT.709",
            Self::Bt2020 => "BT.2020",
        }
    }

    /// The matrix players assume for an untagged picture of this size (mpv's rule).
    pub fn guess(width: u32, height: u32) -> Self {
        if width >= 1280 || height > 576 {
            Self::Bt709
        } else {
            Self::Bt601
        }
    }

    fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "bt709" => Some(Self::Bt709),
            "bt470bg" | "smpte170m" => Some(Self::Bt601),
            "bt2020nc" | "bt2020c" => Some(Self::Bt2020),
            _ => None,
        }
    }
}

/// Colour properties read from FFmpeg's `Video:` stream line.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Props {
    /// Matrix tag as FFmpeg names it (`bt709`, `bt470bg`...), when present.
    pub space: Option<String>,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    /// `pc` range, or a `yuvj*` pixel format.
    pub full_range: bool,
    /// The range was stated explicitly (`tv` or `pc`).
    pub range_tagged: bool,
}

impl Props {
    /// Parses the parenthesis after the pixel format, in both FFmpeg forms:
    /// `yuv420p(tv, bt709, progressive)` (all three equal) and
    /// `yuv420p(tv, bt470bg/bt709/bt709, progressive)`. `unknown` is absent.
    pub fn parse(video_line: &str) -> Self {
        let mut props = Self::default();
        let Some(start) = video_line
            .find(", yuv")
            .or_else(|| video_line.find(", gbr"))
        else {
            return props;
        };
        let rest = &video_line[start + 2..];
        let format_end = rest.find(['(', ',', ' ']).unwrap_or(rest.len());
        let format = &rest[..format_end];
        props.full_range = format.starts_with("yuvj");
        let Some(inner) = rest[format_end..]
            .strip_prefix('(')
            .and_then(|s| s.split(')').next())
        else {
            return props;
        };
        for part in inner.split(',').map(str::trim) {
            match part {
                "tv" => props.range_tagged = true,
                "pc" => {
                    props.range_tagged = true;
                    props.full_range = true;
                }
                "progressive" | "" => {}
                p if p.starts_with("top") || p.starts_with("bottom") => {}
                p => {
                    let known = |v: &str| (v != "unknown" && v != "reserved").then(|| v.to_owned());
                    let mut values = p.split('/');
                    let space = values.next().unwrap_or("");
                    match (values.next(), values.next()) {
                        (Some(primaries), Some(transfer)) => {
                            props.space = known(space);
                            props.primaries = known(primaries);
                            props.transfer = known(transfer);
                        }
                        _ => {
                            props.space = known(space);
                            props.primaries = known(space);
                            props.transfer = known(space);
                        }
                    }
                }
            }
        }
        props
    }

    /// Whether the line carries any colour tag (matrix, primaries or transfer).
    pub fn tagged(&self) -> bool {
        self.space.is_some() || self.primaries.is_some() || self.transfer.is_some()
    }

    /// The matrix a player uses for this clip: its tag, else the size guess.
    pub fn matrix(&self, width: u32, height: u32) -> Matrix {
        self.space
            .as_deref()
            .and_then(Matrix::from_tag)
            .unwrap_or_else(|| Matrix::guess(width, height))
    }

    /// Range name for swscale/setparams.
    fn range(&self) -> &'static str {
        if self.full_range { "pc" } else { "tv" }
    }
}

/// Coded size from a `Video:` line (`..., 1920x1080 [SAR ...` or `..., 1920x1080,`).
pub fn line_size(video_line: &str) -> Option<(u32, u32)> {
    video_line.split(", ").skip(1).find_map(|part| {
        let size = part.split([' ', ',']).next()?;
        let (w, h) = size.split_once('x')?;
        Some((w.parse().ok()?, h.parse().ok()?))
    })
}

/// The colour tail for a reduced proxy of `source` (decoded by mpv, which would
/// guess BT.601 for its small size): keep the source's values, tag its matrix.
/// One header read (`ffmpeg -i`), no packet scan. None if unreadable.
pub fn proxy_tail(ffmpeg: &std::path::Path, source: &std::path::Path) -> Option<String> {
    Some(Plan::single_source(&header(ffmpeg, source)?))
}

/// The `Video:` line and coded size of `source`, by one `ffmpeg -i` header read
/// (no packet scan), cached per file path, size and modification time. A failed
/// read is not cached: a later call (another FFmpeg, a slow disk) tries again.
fn header(ffmpeg: &std::path::Path, source: &std::path::Path) -> Option<crate::media::MediaInfo> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Key = (std::path::PathBuf, u64, Option<std::time::SystemTime>);
    static CACHE: OnceLock<Mutex<HashMap<Key, (String, u32, u32)>>> = OnceLock::new();
    let meta = std::fs::metadata(source).ok()?;
    let key = (source.to_path_buf(), meta.len(), meta.modified().ok());
    let cache = CACHE.get_or_init(Default::default);
    let cached = cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&key)
        .cloned();
    let found = match cached {
        Some(found) => Some(found),
        None => {
            let mut cmd = crate::command(ffmpeg);
            cmd.args(["-hide_banner", "-nostdin", "-i"]).arg(source);
            let found = crate::process::capture(cmd, std::time::Duration::from_secs(10), false)
                .ok()
                .and_then(|(_, text)| {
                    let line = text.lines().find(|l| l.contains("Video:"))?;
                    let (w, h) = line_size(line)?;
                    Some((line.to_owned(), w, h))
                });
            if let Some(found) = &found {
                cache
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(key, found.clone());
            }
            found
        }
    };
    let (line, width, height) = found?;
    Some(crate::media::MediaInfo {
        width,
        height,
        description: line,
        ..Default::default()
    })
}

/// Tags for decoding `source` to RGB the way players read it: an untagged
/// picture gets the matrix players guess for its size (swscale would assume
/// BT.601). Placed first in the decode chain. None for BT.2020 or unreadable.
pub fn decode_tags(ffmpeg: &std::path::Path, source: &std::path::Path) -> Option<String> {
    let info = header(ffmpeg, source)?;
    let props = Props::parse(&info.description);
    let name = match props.matrix(info.width, info.height) {
        Matrix::Bt709 => "bt709",
        Matrix::Bt601 => "smpte170m",
        Matrix::Bt2020 => return None,
    };
    Some(format!(
        "setparams=colorspace={name}:range={}",
        props.range()
    ))
}

/// How the re-encoded pictures of one export are tagged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tags {
    /// Tag the export matrix (×3) in limited range.
    Norm(Matrix),
    /// Same state as a copied segment: its exact tags and range, or none at all.
    Follow(Props),
    /// BT.2020: keep the decoded tags, as before (no tone mapping).
    Keep,
}

impl Tags {
    /// The final `setparams` filter, or None when frames keep their tags.
    pub fn setparams(&self) -> Option<String> {
        match self {
            Self::Norm(Matrix::Bt2020) | Self::Keep => None,
            Self::Norm(m) => {
                let name = if *m == Matrix::Bt709 {
                    "bt709"
                } else {
                    "smpte170m"
                };
                Some(format!(
                    "setparams=colorspace={name}:color_primaries={name}:color_trc={name}:range=tv"
                ))
            }
            Self::Follow(p) => {
                let v = |o: &Option<String>| o.clone().unwrap_or_else(|| "unknown".into());
                let range = if p.range_tagged { p.range() } else { "unknown" };
                Some(format!(
                    "setparams=colorspace={}:color_primaries={}:color_trc={}:range={range}",
                    v(&p.space),
                    v(&p.primaries),
                    v(&p.transfer)
                ))
            }
        }
    }

    /// The range the re-encoded pictures must be in.
    pub fn full_range(&self) -> bool {
        matches!(self, Self::Follow(p) if p.full_range)
    }
}

/// The colour decision of one export.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub matrix: Matrix,
    pub tags: Tags,
    /// Per item: the clip's own matrix (None for still images).
    pub clips: Vec<Option<Matrix>>,
}

/// One input of an export: video properties and coded size, or a still image.
#[derive(Clone, Debug)]
pub enum Input {
    Video {
        props: Props,
        width: u32,
        height: u32,
    },
    Image,
}

impl Input {
    pub fn video(info: &crate::media::MediaInfo) -> Self {
        Self::Video {
            props: Props::parse(&info.description),
            width: info.width,
            height: info.height,
        }
    }
}

impl Plan {
    /// The plan of a list of export items, none of them copied.
    pub fn for_items(
        items: &[crate::engine::MediaItem],
        info: &[crate::media::MediaInfo],
        canvas: (u32, u32),
    ) -> Self {
        let inputs: Vec<Input> = items
            .iter()
            .zip(info)
            .map(|(item, meta)| {
                if item.is_image() {
                    Input::Image
                } else {
                    Input::video(meta)
                }
            })
            .collect();
        Self::new(&inputs, canvas, None)
    }

    /// `canvas` decides when there is no video clip. `first_copied` is the first
    /// item whose packets are copied into the output, if any (rule E).
    pub fn new(inputs: &[Input], canvas: (u32, u32), first_copied: Option<usize>) -> Self {
        let clips: Vec<Option<Matrix>> = inputs
            .iter()
            .map(|i| match i {
                Input::Video {
                    props,
                    width,
                    height,
                } => Some(props.matrix(*width, *height)),
                Input::Image => None,
            })
            .collect();
        let matrix = clips
            .iter()
            .flatten()
            .copied()
            .max()
            .unwrap_or_else(|| Matrix::guess(canvas.0, canvas.1));
        let tags = if matrix == Matrix::Bt2020 {
            Tags::Keep
        } else if let Some(Input::Video { props, .. }) = first_copied.and_then(|i| inputs.get(i)) {
            Tags::Follow(props.clone())
        } else {
            Tags::Norm(matrix)
        };
        Self {
            matrix,
            tags,
            clips,
        }
    }

    /// Items whose matrix differs from the export's (they are converted).
    pub fn converted(&self) -> Vec<usize> {
        if self.matrix == Matrix::Bt2020 {
            return Vec::new(); // kept as before; `diagnosis.color_limit` warns
        }
        self.clips
            .iter()
            .enumerate()
            .filter(|(_, m)| m.is_some_and(|m| m != self.matrix))
            .map(|(i, _)| i)
            .collect()
    }

    /// Whether a video item keeps its packets: same matrix, limited range.
    pub fn copyable(&self, input: &Input) -> bool {
        match input {
            Input::Video {
                props,
                width,
                height,
            } => {
                self.matrix == Matrix::Bt2020
                    || (props.matrix(*width, *height) == self.matrix && !props.full_range)
            }
            Input::Image => false,
        }
    }

    /// The swscale conversion a re-encoded video item needs, placed before the
    /// final pixel format: matrix and range to the export's. None when its
    /// values already fit (or for BT.2020, kept as before). Accurate rounding:
    /// without it, 601 -> 709 shifted every chart patch by about -3/255 (FFmpeg 7.1.1).
    pub fn convert(&self, input: &Input) -> Option<String> {
        let Input::Video {
            props,
            width,
            height,
        } = input
        else {
            return None;
        };
        if self.matrix == Matrix::Bt2020 {
            return None;
        }
        let from = props.matrix(*width, *height);
        let out_range = if self.tags.full_range() { "pc" } else { "tv" };
        if from == self.matrix && props.range() == out_range {
            return None;
        }
        Some(format!(
            "scale=in_color_matrix={}:out_color_matrix={}:in_range={}:out_range={out_range}:flags=accurate_rnd+full_chroma_int",
            from.swscale(),
            self.matrix.swscale(),
            props.range()
        ))
    }

    /// Output matrix for still images converted from RGB.
    pub fn still_matrix(&self) -> Matrix {
        if self.matrix == Matrix::Bt2020 {
            Matrix::Bt709
        } else {
            self.matrix
        }
    }

    /// The RGB -> YUV tail of a still image: the export matrix, the range of the
    /// re-encoded pictures, and their tags (BT.709 tags for a BT.2020 export).
    pub fn still_tail(&self) -> String {
        let range = if self.tags.full_range() { "pc" } else { "tv" };
        let tags = self
            .tags
            .setparams()
            .or_else(|| Tags::Norm(self.still_matrix()).setparams())
            .expect("BT.601/709 tags");
        format!(
            "scale=out_range={range}:out_color_matrix={},format=yuv420p,{tags}",
            self.still_matrix().swscale()
        )
    }

    /// The colour tail of a re-encoded video piece: its conversion (if any), the
    /// pixel format, then the tags. Appended last in the piece's filter chain.
    pub fn video_tail(&self, input: &Input) -> String {
        let mut tail: Vec<String> = self.convert(input).into_iter().collect();
        tail.push("format=yuv420p".into());
        tail.extend(self.tags.setparams());
        tail.join(",")
    }

    /// The single-source head split for the caption effect export (Task 43):
    /// FFmpeg #1's conversion and pixel format, FFmpeg #2's tags (frames lose
    /// them through the pipe), and the matrix the caption mix converts with. It
    /// is the matrix the tags declare: the source's own tag, else the one players
    /// guess for its size (Matrix::guess), so an untagged source is mixed as it
    /// will be shown. White and black, Classique's colours, are the same in both.
    pub fn single_source_split(info: &crate::media::MediaInfo) -> (String, Option<String>, Matrix) {
        let input = Input::video(info);
        let plan = Self::new(
            std::slice::from_ref(&input),
            (info.width, info.height),
            None,
        );
        let mut head: Vec<String> = plan.convert(&input).into_iter().collect();
        head.push("format=yuv420p".into());
        (head.join(","), plan.tags.setparams(), plan.matrix)
    }

    /// The colour head of a single-source re-encode (caption burn, short): the
    /// source's own matrix, converted from full range if needed, tagged before
    /// libass blends so the captions use that matrix too.
    pub fn single_source(info: &crate::media::MediaInfo) -> String {
        let input = Input::video(info);
        Self::new(
            std::slice::from_ref(&input),
            (info.width, info.height),
            None,
        )
        .video_tail(&input)
    }

    /// The warning for the maintainer's rule 2, when clips do not share one matrix:
    /// `warning.color_matrix|<target>|<count>|<from>`.
    pub fn warning(&self) -> Option<String> {
        let converted = self.converted();
        let first = *converted.first()?;
        let from = self.clips[first]?;
        Some(format!(
            "warning.color_matrix|{}|{}|{}",
            self.matrix.label(),
            converted.len(),
            from.label()
        ))
    }
}

/// Whether this FFmpeg's libass blends captions in the frame's tagged matrix:
/// one red box drawn on a BT.709-tagged frame, read back. FFmpeg 7.1 gives
/// Y 63 (BT.709); 6.1, Ubuntu 24.04's, gives Y 81 (BT.601, whatever the tag).
/// `None` when the probe itself cannot run (then nothing is claimed).
pub fn libass_blends_bt709(ffmpeg: &std::path::Path) -> Option<bool> {
    let dir = tempfile::tempdir().ok()?;
    std::fs::write(
        dir.path().join("box.ass"),
        concat!(
            "[Script Info]\nScriptType: v4.00+\nPlayResX: 64\nPlayResY: 64\n",
            "[V4+ Styles]\nFormat: Name,Fontname,Fontsize,PrimaryColour,SecondaryColour,",
            "OutlineColour,BackColour,Bold,Italic,Underline,StrikeOut,ScaleX,ScaleY,Spacing,",
            "Angle,BorderStyle,Outline,Shadow,Alignment,MarginL,MarginR,MarginV,Encoding\n",
            "Style: Default,Arial,20,&H000000FF,&H000000FF,&H00000000,&H00000000,",
            "0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\n",
            "[Events]\nFormat: Layer,Start,End,Style,Name,MarginL,MarginR,MarginV,Effect,Text\n",
            "Dialogue: 0,0:00:00.00,0:00:01.00,Default,,0,0,0,,{\\p1}m 0 0 l 64 0 64 64 0 64{\\p0}\n",
        ),
    )
    .ok()?;
    let mut cmd = crate::command(ffmpeg);
    cmd.current_dir(dir.path()).args([
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=64x64:d=0.04,format=yuv420p",
        "-vf",
        "setparams=colorspace=bt709:range=tv,subtitles=filename=box.ass,format=yuv444p",
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-",
    ]);
    let out = cmd.output().ok()?;
    let (plane, at) = (64 * 64, 32 * 64 + 32);
    if !out.status.success() || out.stdout.len() < 3 * plane {
        return None;
    }
    let (y, v) = (
        i32::from(out.stdout[at]),
        i32::from(out.stdout[2 * plane + at]),
    );
    // Pure red in limited range: BT.709 gives Y 63, BT.601 gives Y 81 (V 240 both).
    match (
        (y - 63).abs() <= 2,
        (y - 81).abs() <= 2,
        (v - 240).abs() <= 2,
    ) {
        (true, _, true) => Some(true),
        (_, true, true) => Some(false),
        _ => None,
    }
}

/// The export warning when libass draws the captions of a BT.709 export with an
/// FFmpeg that blends them in BT.601: their colours come out slightly off (red
/// 18 luma levels lighter). Nothing is corrected, the export says so.
pub fn libass_colour_warning(ffmpeg: &std::path::Path, matrix: Matrix) -> Option<&'static str> {
    (matrix != Matrix::Bt601 && libass_blends_bt709(ffmpeg) == Some(false))
        .then_some("warning.caption_colors_old_ffmpeg")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(line: &str, w: u32, h: u32) -> Input {
        Input::Video {
            props: Props::parse(line),
            width: w,
            height: h,
        }
    }
    const HD709: &str =
        "  Stream #0:0: Video: h264 (High), yuv420p(tv, bt709, progressive), 1920x1080";
    const UNTAGGED: &str = "  Stream #0:0: Video: h264 (High), yuv420p(progressive), 1920x1080";
    const SD601: &str =
        "  Stream #0:0: Video: h264 (High), yuv420p(tv, smpte170m, progressive), 720x576";
    const SPLIT: &str =
        "  Stream #0:0: Video: h264, yuv420p(tv, bt470bg/bt709/bt709, progressive), 1920x1080";
    const FULLJ: &str = "  Stream #0:0: Video: h264, yuvj420p(pc, bt709, progressive), 1920x1080";
    const HDR: &str =
        "  Stream #0:0: Video: hevc, yuv420p10le(tv, bt2020nc/bt2020/smpte2084), 3840x2160";

    #[test]
    fn parses_both_line_forms_range_and_unknown() {
        let p = Props::parse(HD709);
        assert_eq!(p.space.as_deref(), Some("bt709"));
        assert_eq!(p.primaries.as_deref(), Some("bt709"));
        assert!(p.range_tagged && !p.full_range);
        let p = Props::parse(SPLIT);
        assert_eq!(
            (
                p.space.as_deref(),
                p.primaries.as_deref(),
                p.transfer.as_deref()
            ),
            (Some("bt470bg"), Some("bt709"), Some("bt709"))
        );
        let p = Props::parse(UNTAGGED);
        assert!(!p.tagged() && !p.range_tagged && !p.full_range);
        let p = Props::parse(FULLJ);
        assert!(p.full_range && p.range_tagged);
        let p = Props::parse("Video: h264, yuvj420p(progressive), 640x480");
        assert!(p.full_range && !p.range_tagged);
        let p = Props::parse("Video: h264, yuv420p(tv, bt709/unknown/unknown, progressive), 64x64");
        assert_eq!(
            (p.space.as_deref(), p.primaries.as_deref()),
            (Some("bt709"), None)
        );
        assert_eq!(Props::parse(HDR).space.as_deref(), Some("bt2020nc"));
    }

    #[test]
    fn reads_the_coded_size() {
        assert_eq!(line_size(HD709), Some((1920, 1080)));
        assert_eq!(
            line_size(
                "Stream #0:0: Video: h264 (High) (avc1 / 0x31637661), yuv420p(tv, bt709, progressive), 1916x1080 [SAR 1:1 DAR 479:270], 15722 kb/s"
            ),
            Some((1916, 1080))
        );
        assert_eq!(line_size("Video: h264, yuv420p(progressive)"), None);
    }

    #[test]
    fn untagged_clips_are_read_like_mpv() {
        let p = Props::default();
        assert_eq!(p.matrix(1280, 720), Matrix::Bt709);
        assert_eq!(p.matrix(824, 1464), Matrix::Bt709);
        assert_eq!(p.matrix(720, 576), Matrix::Bt601);
        assert_eq!(p.matrix(640, 480), Matrix::Bt601);
        assert_eq!(Props::parse(SD601).matrix(1920, 1080), Matrix::Bt601);
    }

    #[test]
    fn the_most_recent_matrix_wins_and_others_are_converted() {
        let inputs = [
            video(SD601, 720, 576),
            video(HD709, 1920, 1080),
            Input::Image,
        ];
        let plan = Plan::new(&inputs, (720, 576), None);
        assert_eq!(plan.matrix, Matrix::Bt709);
        assert_eq!(plan.tags, Tags::Norm(Matrix::Bt709));
        assert_eq!(plan.converted(), vec![0]);
        assert_eq!(
            plan.warning().as_deref(),
            Some("warning.color_matrix|BT.709|1|BT.601")
        );
        assert!(!plan.copyable(&inputs[0]) && plan.copyable(&inputs[1]));
        assert_eq!(
            plan.convert(&inputs[0]).as_deref(),
            Some(
                "scale=in_color_matrix=bt601:out_color_matrix=bt709:in_range=tv:out_range=tv:flags=accurate_rnd+full_chroma_int"
            )
        );
        assert_eq!(plan.convert(&inputs[1]), None);
        assert_eq!(plan.still_matrix(), Matrix::Bt709);
    }

    #[test]
    fn one_matrix_has_no_warning_and_images_alone_follow_the_canvas() {
        let plan = Plan::new(
            &[video(UNTAGGED, 1920, 1080), video(HD709, 1920, 1080)],
            (1920, 1080),
            None,
        );
        assert_eq!(plan.warning(), None);
        assert_eq!(
            Plan::new(&[Input::Image], (1080, 1920), None).matrix,
            Matrix::Bt709
        );
        assert_eq!(
            Plan::new(&[Input::Image], (720, 480), None).matrix,
            Matrix::Bt601
        );
    }

    #[test]
    fn full_range_is_converted_and_never_copied() {
        let input = video(FULLJ, 1920, 1080);
        let plan = Plan::new(std::slice::from_ref(&input), (1920, 1080), None);
        assert!(!plan.copyable(&input));
        assert_eq!(
            plan.convert(&input).as_deref(),
            Some(
                "scale=in_color_matrix=bt709:out_color_matrix=bt709:in_range=pc:out_range=tv:flags=accurate_rnd+full_chroma_int"
            )
        );
    }

    #[test]
    fn copied_segments_set_the_tag_state_of_reencoded_pictures() {
        // Untagged copied source: re-encoded pieces are stripped of every tag.
        let plan = Plan::new(
            &[video(UNTAGGED, 1920, 1080), Input::Image],
            (1920, 1080),
            Some(0),
        );
        assert_eq!(
            plan.tags.setparams().as_deref(),
            Some(
                "setparams=colorspace=unknown:color_primaries=unknown:color_trc=unknown:range=unknown"
            )
        );
        // Split tags are copied as they are.
        let plan = Plan::new(&[video(SPLIT, 1920, 1080)], (1920, 1080), Some(0));
        assert_eq!(
            plan.tags.setparams().as_deref(),
            Some("setparams=colorspace=bt470bg:color_primaries=bt709:color_trc=bt709:range=tv")
        );
        // A full-range copied hybrid loop keeps pc seams, without range conversion.
        let input = video(FULLJ, 1920, 1080);
        let plan = Plan::new(std::slice::from_ref(&input), (1920, 1080), Some(0));
        assert!(plan.tags.full_range());
        assert_eq!(plan.convert(&input), None);
        assert_eq!(
            plan.tags.setparams().as_deref(),
            Some("setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=pc")
        );
    }

    /// B1 of the plan review: the output options insert a BT.601 conversion of
    /// untagged frames on FFmpeg 7.1. Tags go through `setparams` only.
    #[test]
    fn no_command_uses_the_colour_output_options() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path.file_name().is_some_and(|n| n != "color.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for option in [
                        "-colorspace",
                        "-color_primaries",
                        "-color_trc",
                        "-color_range",
                    ] {
                        for end in [' ', '"'] {
                            assert!(
                                !text.contains(&format!("{option}{end}")),
                                "{} passes {option}",
                                path.display()
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_matrix_warning_and_reason_exist_in_every_language() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
        let mut count = 0;
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|e| e != "lang") {
                continue;
            }
            count += 1;
            let text = std::fs::read_to_string(&path).unwrap();
            let warning = text
                .lines()
                .find_map(|l| l.strip_prefix("warning.color_matrix="))
                .unwrap_or_else(|| panic!("{} lacks warning.color_matrix", path.display()));
            for arg in ["{0}", "{1}", "{2}"] {
                assert!(warning.contains(arg), "{} {arg}", path.display());
            }
            assert!(
                text.lines().any(|l| l.starts_with("reason.color=")),
                "{}",
                path.display()
            );
        }
        assert_eq!(count, 7);
    }

    /// m1 of the plan review: libass blends in the frames' tagged matrix, which
    /// needs FFmpeg's colour negotiation (7.1+). With an older FFmpeg (6.1 on
    /// Ubuntu 24.04) it blends in BT.601: the maintainer's choice b) keeps that FFmpeg and
    /// warns at export, so the probe must say so rather than fail. Checked with
    /// the resolved FFmpeg: 7.1 or later must blend in BT.709; before 7, in BT.601
    /// with the warning; anything else follows the probe.
    #[test]
    fn libass_blends_in_the_tagged_matrix_with_the_resolved_ffmpeg() {
        let Ok(ffmpeg) = crate::find_ffmpeg(None) else {
            return; // no FFmpeg on this machine: the media tests cover it elsewhere
        };
        let version = crate::command(&ffmpeg).arg("-version").output().unwrap();
        let first = String::from_utf8_lossy(&version.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        // "ffmpeg version 7.1.1-…", "ffmpeg version n7.1…", "ffmpeg version 6.1.1-3ubuntu5".
        let numbers: Vec<u32> = first
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .trim_start_matches('n')
            .split(|c: char| !c.is_ascii_digit())
            .take(2)
            .map_while(|n| n.parse().ok())
            .collect();
        let blends = super::libass_blends_bt709(&ffmpeg);
        let warning = super::libass_colour_warning(&ffmpeg, Matrix::Bt709);
        assert_eq!(super::libass_colour_warning(&ffmpeg, Matrix::Bt601), None);
        // Measured releases only: 6.1 (Ubuntu 24.04) blends in BT.601, 7.1.x in
        // BT.709. 7.0 is unmeasured, and a dated git build ("2024-08-01-git…")
        // parses as a huge major: both just follow the probe.
        match numbers.as_slice() {
            [major, minor] if *major < 100 && (*major, *minor) >= (7, 1) => {
                assert_eq!(blends, Some(true), "{first}: FFmpeg 7.1+ blends in BT.709");
                assert_eq!(warning, None);
            }
            [major, _] if *major < 7 => {
                assert_eq!(
                    blends,
                    Some(false),
                    "{first}: FFmpeg before 7 blends in BT.601"
                );
                assert_eq!(warning, Some("warning.caption_colors_old_ffmpeg"));
            }
            // 7.0, a git build: whatever it does, the probe must answer and the
            // warning must follow it.
            _ => {
                assert!(blends.is_some(), "{first}: the probe answers");
                assert_eq!(warning.is_some(), blends == Some(false));
            }
        }
    }

    /// Thumbnails and the single-file fallback decoder read untagged video the
    /// way players do: BT.709 for HD, BT.601 for SD, the tag when present.
    #[test]
    fn decode_tags_read_untagged_video_like_players() {
        let Ok(ffmpeg) = crate::find_ffmpeg(None) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let make = |name: &str, size: &str, extra: &str| {
            let path = dir.path().join(name);
            let out = crate::command(&ffmpeg)
                .args([
                    "-hide_banner",
                    "-nostdin",
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                ])
                .arg(format!(
                    "color=c=green:s={size}:d=0.2,format=yuv420p{extra}"
                ))
                .args(["-c:v", "libx264", "-preset", "ultrafast"])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            path
        };
        let untagged =
            ",setparams=colorspace=unknown:color_primaries=unknown:color_trc=unknown:range=unknown";
        let hd = make("hd.mp4", "1280x720", untagged);
        let sd = make("sd.mp4", "640x480", untagged);
        let tagged = make(
            "tagged.mp4",
            "640x480",
            ",setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv",
        );
        assert_eq!(
            decode_tags(&ffmpeg, &hd).as_deref(),
            Some("setparams=colorspace=bt709:range=tv")
        );
        assert_eq!(
            decode_tags(&ffmpeg, &sd).as_deref(),
            Some("setparams=colorspace=smpte170m:range=tv")
        );
        assert_eq!(
            decode_tags(&ffmpeg, &tagged).as_deref(),
            Some("setparams=colorspace=bt709:range=tv")
        );
        // Cached: the same answer without another read.
        assert_eq!(
            decode_tags(&ffmpeg, &hd).as_deref(),
            Some("setparams=colorspace=bt709:range=tv")
        );
        assert_eq!(decode_tags(&ffmpeg, &dir.path().join("missing.mp4")), None);
    }

    #[test]
    fn norm_tags_are_complete_and_bt2020_keeps_decoded_tags() {
        assert_eq!(
            Tags::Norm(Matrix::Bt709).setparams().as_deref(),
            Some("setparams=colorspace=bt709:color_primaries=bt709:color_trc=bt709:range=tv")
        );
        assert_eq!(
            Tags::Norm(Matrix::Bt601).setparams().as_deref(),
            Some(
                "setparams=colorspace=smpte170m:color_primaries=smpte170m:color_trc=smpte170m:range=tv"
            )
        );
        let inputs = [video(HDR, 3840, 2160), video(HD709, 1920, 1080)];
        let plan = Plan::new(&inputs, (3840, 2160), None);
        assert_eq!(plan.matrix, Matrix::Bt2020);
        assert_eq!(plan.tags, Tags::Keep);
        assert_eq!(plan.tags.setparams(), None);
        assert_eq!(plan.convert(&inputs[1]), None);
        assert!(plan.copyable(&inputs[1]));
        assert_eq!(plan.warning(), None);
    }
}
