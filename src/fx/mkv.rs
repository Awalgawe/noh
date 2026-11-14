//! Minimal streaming Matroska writer for raw I420 video with exact per-frame
//! timestamps (plan GL v3 §B2): EBML header, a Segment of unknown size, Info with a
//! 1 µs timestamp scale, one V_UNCOMPRESSED track, then one Cluster per frame.
use std::io::Write;

fn id(out: &mut Vec<u8>, id: u32) {
    let b = id.to_be_bytes();
    let skip = b.iter().take_while(|x| **x == 0).count();
    out.extend_from_slice(&b[skip..]);
}

/// Element size as an 8-byte EBML vint.
fn size(out: &mut Vec<u8>, n: u64) {
    out.push(0x01);
    out.extend_from_slice(&n.to_be_bytes()[1..]);
}

fn elem(out: &mut Vec<u8>, eid: u32, body: &[u8]) {
    id(out, eid);
    size(out, body.len() as u64);
    out.extend_from_slice(body);
}

fn uint(out: &mut Vec<u8>, eid: u32, v: u64) {
    let b = v.to_be_bytes();
    let skip = b.iter().take_while(|x| **x == 0).count().min(7);
    elem(out, eid, &b[skip..]);
}

pub struct Writer<W: Write> {
    out: W,
}

impl<W: Write> Writer<W> {
    pub fn new(mut out: W, width: u32, height: u32) -> std::io::Result<Self> {
        let mut head = Vec::new();
        let mut ebml = Vec::new();
        uint(&mut ebml, 0x4286, 1);
        uint(&mut ebml, 0x42F7, 1);
        uint(&mut ebml, 0x42F2, 4);
        uint(&mut ebml, 0x42F3, 8);
        elem(&mut ebml, 0x4282, b"matroska");
        uint(&mut ebml, 0x4287, 4);
        uint(&mut ebml, 0x4285, 2);
        elem(&mut head, 0x1A45_DFA3, &ebml);
        // Segment, unknown size (live stream).
        id(&mut head, 0x1853_8067);
        head.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        let mut info = Vec::new();
        uint(&mut info, 0x2AD7B1, 1000); // TimestampScale: 1 µs
        elem(&mut info, 0x4D80, b"noh");
        elem(&mut info, 0x5741, b"noh");
        elem(&mut head, 0x1549_A966, &info);
        let mut video = Vec::new();
        uint(&mut video, 0xB0, width as u64);
        uint(&mut video, 0xBA, height as u64);
        elem(&mut video, 0x2EB524, b"I420"); // ColourSpace fourcc
        let mut track = Vec::new();
        uint(&mut track, 0xD7, 1);
        uint(&mut track, 0x73C5, 1);
        uint(&mut track, 0x83, 1);
        uint(&mut track, 0x9C, 0);
        elem(&mut track, 0x86, b"V_UNCOMPRESSED");
        // Matroska's default language is "eng"; FFmpeg would carry it into the MP4.
        elem(&mut track, 0x22B59C, b"und");
        elem(&mut track, 0xE0, &video);
        let mut tracks = Vec::new();
        elem(&mut tracks, 0xAE, &track);
        elem(&mut head, 0x1654_AE6B, &tracks);
        out.write_all(&head)?;
        Ok(Self { out })
    }

    /// One frame at `pts_us` microseconds, as its own cluster. With a duration
    /// (the last frame of a source whose end is known), a BlockGroup carries it as
    /// BlockDuration: FFmpeg would otherwise guess the last frame's duration.
    pub fn frame(
        &mut self,
        pts_us: u64,
        duration_us: Option<u64>,
        data: &[u8],
    ) -> std::io::Result<()> {
        let mut ts = Vec::new();
        uint(&mut ts, 0xE7, pts_us);
        let block_len = 4 + data.len() as u64; // track vint, i16 relative time, flags
        let mut hdr = Vec::with_capacity(64);
        id(&mut hdr, 0x1F43_B675);
        match duration_us {
            None => {
                size(&mut hdr, ts.len() as u64 + 1 + 8 + block_len);
                hdr.extend_from_slice(&ts);
                hdr.push(0xA3); // SimpleBlock, keyframe
                size(&mut hdr, block_len);
                hdr.extend_from_slice(&[0x81, 0x00, 0x00, 0x80]);
            }
            Some(d) => {
                let mut dur = Vec::new();
                uint(&mut dur, 0x9B, d); // BlockDuration
                let group_len = 1 + 8 + block_len + dur.len() as u64;
                size(&mut hdr, ts.len() as u64 + 1 + 8 + group_len);
                hdr.extend_from_slice(&ts);
                hdr.push(0xA0); // BlockGroup
                size(&mut hdr, group_len);
                hdr.extend_from_slice(&dur);
                hdr.push(0xA1); // Block
                size(&mut hdr, block_len);
                hdr.extend_from_slice(&[0x81, 0x00, 0x00, 0x00]);
            }
        }
        self.out.write_all(&hdr)?;
        self.out.write_all(data)
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    /// FFmpeg reads the frame times and the last frame's BlockDuration (Task 43).
    #[test]
    fn ffmpeg_reads_times_and_the_last_duration() {
        let Ok(ffmpeg) = crate::find_ffmpeg(std::env::var_os("NOH_FFMPEG")) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.mkv");
        let frame = vec![128u8; 16 * 16 * 3 / 2];
        let mut w = super::Writer::new(std::fs::File::create(&path).unwrap(), 16, 16).unwrap();
        w.frame(25_000, None, &frame).unwrap();
        w.frame(65_000, None, &frame).unwrap();
        w.frame(105_000, Some(15_000), &frame).unwrap();
        w.finish().unwrap();
        let out = crate::command(&ffmpeg)
            .args(["-v", "error", "-copyts", "-i"])
            .arg(&path)
            .args(["-map", "0:v", "-c", "copy", "-f", "framecrc", "-"])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let rows: Vec<Vec<i64>> = text
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                l.split(',')
                    .take(4)
                    .map(|v| v.trim().parse().unwrap())
                    .collect()
            })
            .collect();
        assert_eq!(
            rows.iter().map(|r| r[2]).collect::<Vec<_>>(),
            [25_000, 65_000, 105_000],
            "{text}"
        );
        assert_eq!(rows[2][3], 15_000, "last duration (µs): {text}");
    }
}
