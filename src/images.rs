//! Still-image timing and transforms. FFmpeg owns pixel decoding and encoding.
use crate::{Result, engine::EngineError, plan::Target};
use std::{ffi::OsString, fs::File, io::BufReader, path::Path};

pub(crate) fn codec(path: &Path) -> std::result::Result<&'static str, EngineError> {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok("png"),
        Some("jpg" | "jpeg") => Ok("mjpeg"),
        _ => Err(EngineError::new(
            "error.image",
            "validate_image",
            Some(path),
            "Images must be PNG or JPEG files.",
        )),
    }
}
pub(crate) fn validate(path: &Path, duration: f64) -> std::result::Result<(), EngineError> {
    codec(path)?;
    if !duration.is_finite() || duration <= 0.0 {
        return Err(duration_error(Some(path)));
    }
    Ok(())
}
fn duration_error(path: Option<&Path>) -> EngineError {
    EngineError::new(
        "error.image_duration",
        "image_timing",
        path,
        "Image duration must be positive, finite and representable on the output frame grid.",
    )
}
pub(crate) fn frames(
    duration: f64,
    target: &Target,
    path: Option<&Path>,
) -> std::result::Result<u64, EngineError> {
    if !duration.is_finite() || duration <= 0.0 || target.rate_num <= 0 || target.rate_den <= 0 {
        return Err(duration_error(path));
    }
    let count = (duration * target.rate_num as f64 / target.rate_den as f64)
        .round()
        .max(1.0);
    let limit = i64::MAX / target.rate_den;
    // The strict floating-point bound avoids a cast rounding 2^63 into i64::MAX.
    if !count.is_finite() || count >= limit as f64 {
        return Err(duration_error(path));
    }
    Ok(count as u64)
}
pub(crate) fn seconds(frames: u64, target: &Target) -> f64 {
    frames as f64 * target.rate_den as f64 / target.rate_num as f64
}
pub(crate) fn preparation_frames(
    frames: u64,
    wav_seconds: f64,
    target: &Target,
    path: &Path,
) -> std::result::Result<u64, EngineError> {
    if target.rate_num <= 0 || target.rate_den <= 0 || frames == 0 {
        return Err(duration_error(Some(path)));
    }
    let cap = (wav_seconds * target.rate_num as f64 / target.rate_den as f64)
        .ceil()
        .max(1.0);
    if !wav_seconds.is_finite()
        || wav_seconds <= 0.0
        || !cap.is_finite()
        || cap >= (i64::MAX / target.rate_den) as f64
    {
        return Err(duration_error(Some(path)));
    }
    // At least the soundtrack's presentation interval survives a clamp, so a
    // long image cannot cause the sequence to restart before the soundtrack ends.
    Ok(frames.min(cap as u64))
}

pub(crate) fn png_orientation(path: &Path) -> Result<u8> {
    use image::metadata::Orientation;
    let mut decoder = png::Decoder::new_with_limits(
        BufReader::new(File::open(path)?),
        png::Limits {
            bytes: 64 * 1024 * 1024,
        },
    );
    decoder.set_ignore_text_chunk(true);
    let mut reader = decoder.read_info()?;
    // Metadata may follow IDAT. Skip compressed sample data through the library,
    // retaining its chunk/CRC/allocation checks without creating a pixel buffer.
    reader.finish()?;
    if reader.info().animation_control.is_some() {
        return Err("Animated PNG files are not supported as still images".into());
    }
    Ok(reader
        .info()
        .exif_metadata
        .as_deref()
        .and_then(Orientation::from_exif_chunk)
        .unwrap_or(Orientation::NoTransforms)
        .to_exif())
}
pub(crate) fn orientation_filter(orientation: u8) -> &'static str {
    match orientation {
        2 => "hflip,",
        3 => "hflip,vflip,",
        4 => "vflip,",
        5 => "transpose=clock,hflip,",
        6 => "transpose=clock,",
        7 => "transpose=cclock,hflip,",
        8 => "transpose=cclock,",
        _ => "",
    }
}
pub(crate) fn input_args(path: &Path, rate_num: i64, rate_den: i64) -> Result<Vec<OsString>> {
    let codec = codec(path)?;
    let mut args: Vec<OsString> = ["-f", "image2", "-pattern_type", "none", "-framerate"]
        .into_iter()
        .map(Into::into)
        .collect();
    args.push(format!("{rate_num}/{rate_den}").into());
    if codec == "png" {
        args.push("-noautorotate".into());
    }
    args.extend([
        "-c:v".into(),
        codec.into(),
        "-i".into(),
        path.as_os_str().into(),
    ]);
    Ok(args)
}
pub(crate) fn filter(target: &Target, orientation: u8) -> String {
    let (w, h, n, d) = (
        target.width,
        target.height,
        target.rate_num,
        target.rate_den,
    );
    // JPEG's full-range YUV metadata must not survive the RGB composition.
    // Images share a reserved SPS ID, so every encoded image uses the same
    // limited-range BT.601 conversion and matching VUI, regardless of decoder.
    format!(
        "color=c=black:s={w}x{h}:r={n}/{d},format=rgba[bg];[0:v]{}setpts=PTS-STARTPTS,sidedata=mode=delete:type=DISPLAYMATRIX,scale=w='max(2,trunc(min({w},{h}*dar)/2)*2)':h='max(2,trunc(min({h},{w}/dar)/2)*2)',setsar=1,format=rgba[image];[bg][image]overlay=x=(W-w)/2:y=(H-h)/2:format=rgb:alpha=straight,scale=out_range=tv:out_color_matrix=bt601,format=yuv420p,setparams=range=limited:colorspace=bt470bg[v]",
        orientation_filter(orientation)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn target(n: i64, d: i64) -> Target {
        Target {
            width: 96,
            height: 64,
            rate_num: n,
            rate_den: d,
            codec: "h264".into(),
            pixel_format: "yuv420p".into(),
        }
    }
    #[test]
    fn frame_grid_rounding_is_positive_precise_and_bounded() {
        let grid = target(25, 1);
        for (duration, count) in [(0.001, 1), (0.019, 1), (0.06, 2), (0.20, 5), (0.32, 8)] {
            assert_eq!(frames(duration, &grid, None).unwrap(), count);
        }
        let grid = target(30000, 1001);
        assert_eq!(frames(1.0, &grid, None).unwrap(), 30);
        assert!((seconds(30, &grid) - 1.001).abs() < 1e-12);
        for duration in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
            assert_eq!(
                frames(duration, &grid, None).unwrap_err().code,
                "error.image_duration"
            );
        }
    }
    #[test]
    fn preparation_cap_does_not_create_a_premature_loop_or_change_short_items() {
        let grid = target(25, 1);
        let path = Path::new("image.png");
        assert_eq!(preparation_frames(25000000, 1.01, &grid, path).unwrap(), 26);
        assert!(seconds(26, &grid) >= 1.01);
        assert_eq!(preparation_frames(25, 1.01, &grid, path).unwrap(), 25);
        assert_eq!(preparation_frames(1, 0.001, &grid, path).unwrap(), 1);
        assert!(preparation_frames(1, 1.0, &target(25, 0), path).is_err());
        assert!(preparation_frames(0, 1.0, &grid, path).is_err());
    }
    #[test]
    fn image_extensions_and_duration_are_explicit() {
        for name in ["a.PNG", "a.jpg", "a.JPEG"] {
            validate(Path::new(name), 0.1).unwrap();
        }
        assert_eq!(
            validate(Path::new("a.gif"), 1.0).unwrap_err().code,
            "error.image"
        );
        assert_eq!(
            validate(Path::new("a.png"), 0.0).unwrap_err().code,
            "error.image_duration"
        );
        let args = input_args(Path::new("literal%02d.PNG"), 25, 1).unwrap();
        assert!(
            args.windows(2)
                .any(|pair| pair == [OsString::from("-pattern_type"), OsString::from("none")])
        );
        assert_eq!(args.last().unwrap(), "literal%02d.PNG");
    }
    #[test]
    fn png_exif_is_read_before_or_after_idat_for_every_orientation() {
        let folder = tempfile::tempdir().unwrap();
        for trailing in [false, true] {
            for orientation in 1u8..=8 {
                let mut metadata =
                    b"II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00".to_vec();
                metadata.extend_from_slice(&[orientation, 0, 0, 0, 0, 0, 0, 0]);
                let mut bytes = Vec::new();
                {
                    let mut encoder = png::Encoder::new(&mut bytes, 2, 1);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    let mut writer = encoder.write_header().unwrap();
                    if !trailing {
                        writer.write_chunk(png::chunk::eXIf, &metadata).unwrap();
                    }
                    writer
                        .write_image_data(&[255, 0, 0, 0, 255, 0, 0, 255])
                        .unwrap();
                    if trailing {
                        writer.write_chunk(png::chunk::eXIf, &metadata).unwrap();
                    }
                    writer.finish().unwrap();
                }
                let path = folder
                    .path()
                    .join(format!("orientation-{orientation}-{trailing}.png"));
                std::fs::write(&path, bytes).unwrap();
                assert_eq!(png_orientation(&path).unwrap(), orientation);
            }
        }
    }
}
