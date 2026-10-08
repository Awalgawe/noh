//! Small synthetic image fixtures; all media stays inside the owning tempdir.
use super::Fixture;
use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};
use std::path::{Path, PathBuf};

pub fn exif(orientation: u8) -> Vec<u8> {
    vec![
        b'I',
        b'I',
        42,
        0,
        8,
        0,
        0,
        0,
        1,
        0,
        0x12,
        1,
        3,
        0,
        1,
        0,
        0,
        0,
        orientation,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    ]
}

pub fn png(
    f: &Fixture,
    name: &str,
    width: u32,
    height: u32,
    pixel: impl Fn(u32, u32) -> [u8; 4],
    orientation: Option<u8>,
) -> PathBuf {
    let path = f.path(name);
    let bytes: Vec<u8> = (0..height)
        .flat_map(|y| {
            let pixel = &pixel;
            (0..width).flat_map(move |x| pixel(x, y))
        })
        .collect();
    let mut encoder = PngEncoder::new(std::fs::File::create(&path).unwrap());
    if let Some(orientation) = orientation {
        encoder.set_exif_metadata(exif(orientation)).unwrap();
    }
    encoder
        .write_image(&bytes, width, height, ExtendedColorType::Rgba8)
        .unwrap();
    path
}

pub fn jpeg(f: &Fixture, png: &Path, name: &str, orientation: Option<u8>) -> PathBuf {
    let path = f.path(name);
    f.ff(args![
        "-i",
        png,
        "-frames:v",
        "1",
        "-q:v",
        "1",
        "-update",
        "1",
        &path
    ]);
    if let Some(orientation) = orientation {
        let original = std::fs::read(&path).unwrap();
        assert_eq!(&original[..2], &[0xff, 0xd8]);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend(exif(orientation));
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend(((payload.len() + 2) as u16).to_be_bytes());
        bytes.extend(payload);
        bytes.extend(&original[2..]);
        std::fs::write(&path, bytes).unwrap();
    }
    path
}

pub fn rgb(f: &Fixture, video: &Path) -> Vec<u8> {
    f.ff(args![
        "-i",
        video,
        "-map",
        "0:v:0",
        "-fps_mode",
        "passthrough",
        "-pix_fmt",
        "rgb24",
        "-f",
        "rawvideo",
        "-"
    ])
    .stdout
}

pub fn pixel(
    frames: &[u8],
    width: usize,
    height: usize,
    frame: usize,
    x: usize,
    y: usize,
) -> [u8; 3] {
    let offset = ((frame * height + y) * width + x) * 3;
    frames[offset..offset + 3].try_into().unwrap()
}

pub fn close(actual: [u8; 3], expected: [u8; 3], tolerance: u8) {
    assert!(
        actual
            .iter()
            .zip(expected)
            .all(|(a, b)| a.abs_diff(b) <= tolerance),
        "RGB {actual:?} does not match {expected:?} within {tolerance}"
    );
}
