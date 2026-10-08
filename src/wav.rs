//! Seek-based RIFF/RF64/BW64 duration validation; sample payloads are never read.
//! References: Microsoft RIFF and WAVEFORMATEX/EXTENSIBLE; EBU Tech 3306 (2009)
//! and ITU-R BS.2088-1 for ds64. Tests use synthetic bytes, without a decoder.
use crate::Result;
use std::{
    collections::HashSet,
    io::{Read, Seek, SeekFrom},
};

fn u16le(bytes: &[u8]) -> u16 {
    u16::from_le_bytes(bytes.try_into().unwrap())
}
fn u32le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes(bytes.try_into().unwrap())
}
fn u64le(bytes: &[u8]) -> u64 {
    u64::from_le_bytes(bytes.try_into().unwrap())
}

fn chunk_end(start: u64, size: u64, limit: u64) -> Result<u64> {
    let padded = start
        .checked_add(size)
        .and_then(|end| end.checked_add(size & 1))
        .ok_or("Invalid WAV chunk size")?;
    if padded > limit {
        return Err("WAV chunk or padding exceeds the declared container.".into());
    }
    Ok(padded)
}

struct Ds64 {
    data_size: u64,
    table_start: u64,
    table_len: u32,
    used: HashSet<u32>,
}
impl Ds64 {
    fn table_size(&mut self, file: &mut (impl Read + Seek), id: &[u8; 4]) -> Result<u64> {
        let position = file.stream_position()?;
        file.seek(SeekFrom::Start(self.table_start))?;
        // Read overrides only when required; never allocate the advertised table.
        for index in 0..self.table_len {
            let mut entry = [0; 12];
            file.read_exact(&mut entry)?;
            if &entry[..4] == id && self.used.insert(index) {
                file.seek(SeekFrom::Start(position))?;
                return Ok(u64le(&entry[4..]));
            }
        }
        Err("RF64/BW64 chunk has no matching ds64 size override.".into())
    }
}

fn read_ds64(file: &mut (impl Read + Seek), physical: u64, outer_size: u32) -> Result<(u64, Ds64)> {
    let mut header = [0; 8];
    file.read_exact(&mut header)?;
    let size = u32le(&header[4..]);
    if &header[..4] != b"ds64" || size < 28 || size == u32::MAX {
        return Err("RF64/BW64 requires a complete ds64 chunk first.".into());
    }
    let start = file.stream_position()?;
    let end = chunk_end(start, size.into(), physical)?;
    let mut data = [0; 28];
    file.read_exact(&mut data)?;
    let declared_size = if outer_size == u32::MAX {
        u64le(&data[..8])
    } else {
        outer_size.into()
    };
    let container_end = declared_size
        .checked_add(8)
        .ok_or("Invalid RF64/BW64 container size")?;
    if container_end > physical || container_end < end {
        return Err("RF64/BW64 container extent is incomplete or invalid.".into());
    }
    let table_len = u32le(&data[24..]);
    if 28 + u64::from(table_len) * 12 > u64::from(size) {
        return Err("Incomplete RF64/BW64 ds64 size table.".into());
    }
    // RF64 sampleCount and BW64's compatibility dummy do not replace the
    // authoritative uncompressed data byte count for duration calculation.
    let ds64 = Ds64 {
        data_size: u64le(&data[8..16]),
        table_start: start + 28,
        table_len,
        used: HashSet::new(),
    };
    file.seek(SeekFrom::Start(end))?;
    Ok((container_end, ds64))
}

#[derive(Clone, Copy)]
struct Format {
    sample_rate: u32,
    block_align: u16,
}
fn read_format(file: &mut impl Read, size: u64) -> Result<Format> {
    if size < 16 {
        return Err("Incomplete WAV format.".into());
    }
    let mut data = [0; 40];
    file.read_exact(&mut data[..size.min(40) as usize])?;
    let mut codec = u16le(&data[..2]);
    let channels = u16le(&data[2..4]);
    let sample_rate = u32le(&data[4..8]);
    let byte_rate = u32le(&data[8..12]);
    let block_align = u16le(&data[12..14]);
    let bits = u16le(&data[14..16]);
    if codec == 0xfffe {
        let extension = u16le(&data[16..18]);
        if size < 40 || extension < 22 || 18 + u64::from(extension) > size {
            return Err("Incomplete extensible WAV format extension.".into());
        }
        if data[26..40] != [0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113] {
            return Err("Unsupported WAV subformat.".into());
        }
        codec = u16le(&data[24..26]);
        if !bits.is_multiple_of(8) || u16le(&data[18..20]) > bits {
            return Err("Invalid extensible WAV sample container or precision.".into());
        }
        // Zero/partial channel masks and zero valid-bits are tolerated: they do
        // not alter frame size, and the channel-mask count need not equal channels.
    } else if codec == 3 && size > 16 && (size < 18 || 18 + u64::from(u16le(&data[16..18])) > size)
    {
        return Err("Incomplete floating-point WAV format extension.".into());
    }
    if !matches!(codec, 1 | 3) {
        return Err("WAV must contain uncompressed PCM or floating-point samples.".into());
    }
    if channels == 0 || sample_rate == 0 || bits == 0 {
        return Err("Invalid WAV channel count, sample rate or bit depth.".into());
    }
    if codec == 3 && !matches!(bits, 32 | 64) {
        return Err("Floating-point WAV samples must use 32 or 64 bits.".into());
    }
    // PCM may specify a non-byte precision; each channel's sample still
    // occupies a whole number of bytes. Extensible container sizes are byte-aligned.
    let expected_align = u32::from(channels) * u32::from(bits).div_ceil(8);
    if expected_align != u32::from(block_align) {
        return Err("WAV block alignment contradicts its channels and bit depth.".into());
    }
    if sample_rate.checked_mul(expected_align) != Some(byte_rate) {
        return Err("WAV byte rate contradicts its sample rate and frame size.".into());
    }
    // PCM cbSize is implicitly zero and must be ignored (Microsoft WAVEFORMATEX).
    Ok(Format {
        sample_rate,
        block_align,
    })
}

pub(crate) fn duration(file: &mut (impl Read + Seek), physical: u64) -> Result<f64> {
    let mut header = [0; 12];
    file.read_exact(&mut header)?;
    let extended = matches!(&header[..4], b"RF64" | b"BW64");
    if (!extended && &header[..4] != b"RIFF") || &header[8..] != b"WAVE" {
        return Err(
            "Expected soundtrack: RIFF/RF64/BW64 WAV with PCM or floating-point samples.".into(),
        );
    }
    let outer_size = u32le(&header[4..8]);
    let (container_end, mut ds64) = if extended {
        let (end, sizes) = read_ds64(file, physical, outer_size)?;
        (end, Some(sizes))
    } else {
        let end = 8 + u64::from(outer_size);
        if end < 12 || end > physical {
            return Err("WAV container extent is incomplete or invalid.".into());
        }
        (end, None)
    };
    let (mut format, mut data_size) = (None, None);
    loop {
        let position = file.stream_position()?;
        if position == container_end {
            break;
        }
        if position > container_end || container_end - position < 8 {
            return Err("Incomplete WAV chunk header inside the container.".into());
        }
        let mut chunk = [0; 8];
        file.read_exact(&mut chunk)?;
        let id: [u8; 4] = chunk[..4].try_into().unwrap();
        let mut size = u64::from(u32le(&chunk[4..]));
        if size == u64::from(u32::MAX)
            && let Some(sizes) = &mut ds64
        {
            size = if &id == b"data" && data_size.is_none() {
                sizes.data_size
            } else {
                sizes.table_size(file, &id)?
            };
        }
        let start = file.stream_position()?;
        let end = chunk_end(start, size, container_end)?;
        match &id {
            b"fmt " => {
                if format.is_some() {
                    return Err("Multiple WAV format chunks are ambiguous.".into());
                }
                format = Some(read_format(file, size)?);
            }
            b"data" => {
                if data_size.replace(size).is_some() {
                    return Err("WAV requires a single contiguous data chunk.".into());
                }
            }
            b"ds64" if extended => return Err("Duplicate RF64/BW64 ds64 chunk.".into()),
            _ => {}
        }
        file.seek(SeekFrom::Start(end))?;
    }
    let (
        Some(Format {
            sample_rate,
            block_align,
        }),
        Some(bytes),
    ) = (format, data_size)
    else {
        return Err("Missing fmt/data chunks in WAV.".into());
    };
    if bytes == 0 || !bytes.is_multiple_of(u64::from(block_align)) {
        return Err("Empty WAV or incomplete audio frame.".into());
    }
    Ok((bytes / u64::from(block_align)) as f64 / f64::from(sample_rate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = id.to_vec();
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if !data.len().is_multiple_of(2) {
            bytes.push(0);
        }
        bytes
    }
    fn riff(chunks: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVE".to_vec();
        for chunk in chunks {
            bytes.extend_from_slice(chunk);
        }
        let size = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&size.to_le_bytes());
        bytes
    }
    fn format(codec: u16, channels: u16, rate: u32, bits: u16) -> Vec<u8> {
        let align = channels * bits.div_ceil(8);
        let mut bytes = codec.to_le_bytes().to_vec();
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(align)).to_le_bytes());
        bytes.extend_from_slice(&align.to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        bytes
    }
    fn extensible(codec: u16, channels: u16, rate: u32, bits: u16, valid: u16) -> Vec<u8> {
        let mut bytes = format(0xfffe, channels, rate, bits);
        bytes.extend_from_slice(&22u16.to_le_bytes());
        bytes.extend_from_slice(&valid.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&codec.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]);
        bytes
    }
    fn parse(bytes: &[u8]) -> Result<f64> {
        duration(&mut Cursor::new(bytes), bytes.len() as u64)
    }
    fn wave(fmt: &[u8], data: &[u8]) -> Vec<u8> {
        riff(&[chunk(b"fmt ", fmt), chunk(b"data", data)])
    }

    #[test]
    fn precise_pcm_and_float_frames_preserve_supported_widths() {
        for (codec, bits) in [
            (1, 8),
            (1, 12),
            (1, 16),
            (1, 24),
            (1, 32),
            (1, 64),
            (3, 32),
            (3, 64),
        ] {
            let fmt = format(codec, 2, 44100, bits);
            let data = vec![0; 17 * 2 * usize::from(bits.div_ceil(8))];
            assert_eq!(parse(&wave(&fmt, &data)).unwrap(), 17.0 / 44100.0);
        }
        let mut pcm = format(1, 1, 8000, 16);
        pcm.extend_from_slice(&u16::MAX.to_le_bytes()); // PCM cbSize is ignored.
        assert_eq!(parse(&wave(&pcm, &[0; 2])).unwrap(), 1.0 / 8000.0);
    }

    #[test]
    fn unknown_odd_chunks_and_data_before_format_are_supported() {
        let bytes = riff(&[
            chunk(b"JUNK", &[1, 2, 3]),
            chunk(b"data", &[0; 3]),
            chunk(b"LIST", b"INFO"),
            chunk(b"fmt ", &format(1, 1, 8000, 8)),
            chunk(b"bext", &[0; 3]),
        ]);
        assert_eq!(parse(&bytes).unwrap(), 3.0 / 8000.0);
    }

    #[test]
    fn declared_extent_is_authoritative_but_external_bytes_are_ignored() {
        let valid = wave(&format(1, 1, 8000, 16), &[0; 2]);
        let mut extra = valid.clone();
        extra.extend_from_slice(b"unrelated trailing bytes");
        assert_eq!(parse(&extra).unwrap(), 1.0 / 8000.0);
        let mut before_data = valid.clone();
        before_data[4..8].copy_from_slice(&28u32.to_le_bytes());
        assert!(parse(&before_data).is_err());
        let mut past_eof = valid.clone();
        past_eof[4..8].copy_from_slice(&(valid.len() as u32).to_le_bytes());
        assert!(parse(&past_eof).is_err());
        for small in 0u32..4 {
            let mut bytes = valid.clone();
            bytes[4..8].copy_from_slice(&small.to_le_bytes());
            assert!(parse(&bytes).is_err());
        }
    }

    #[test]
    fn incomplete_chunks_padding_and_trailing_internal_headers_are_rejected() {
        let good = wave(&format(1, 1, 8000, 8), &[0; 3]);
        let mut no_padding = good.clone();
        no_padding.pop();
        let size = no_padding.len() as u32 - 8;
        no_padding[4..8].copy_from_slice(&size.to_le_bytes());
        assert!(parse(&no_padding).is_err());
        for tail in 1..8 {
            let mut bytes = good.clone();
            bytes.extend_from_slice(&vec![0; tail]);
            let size = bytes.len() as u32 - 8;
            bytes[4..8].copy_from_slice(&size.to_le_bytes());
            assert!(parse(&bytes).is_err());
        }
        let oversized = riff(&[
            chunk(b"fmt ", &format(1, 1, 8000, 16)),
            b"data\xff\xff\xff\xff".to_vec(),
        ]);
        assert!(parse(&oversized).is_err());
        for end in 0..good.len() {
            assert!(parse(&good[..end]).is_err());
        }
    }

    #[test]
    fn contradictory_format_duration_is_rejected() {
        let mut fmt = format(1, 2, 8000, 16);
        // Stereo16 needs four bytes/frame. Previous code accepted two and gave
        // twice the correct duration for the same four audio bytes.
        fmt[12..14].copy_from_slice(&2u16.to_le_bytes());
        assert!(parse(&wave(&fmt, &[0; 4])).is_err());
        for (offset, width) in [(2, 2), (4, 4), (12, 2), (14, 2)] {
            let mut fmt = format(1, 2, 8000, 16);
            fmt[offset..offset + width].fill(0);
            assert!(parse(&wave(&fmt, &[0; 4])).is_err());
        }
        let mut byte_rate = format(1, 2, 8000, 16);
        byte_rate[8..12].copy_from_slice(&1u32.to_le_bytes());
        assert!(parse(&wave(&byte_rate, &[0; 4])).is_err());
        assert!(parse(&wave(&format(3, 1, 8000, 16), &[0; 2])).is_err());
        assert!(parse(&wave(&format(6, 1, 8000, 8), &[0])).is_err());
        assert!(parse(&wave(&format(1, 2, 8000, 16), &[0; 3])).is_err());
        assert!(parse(&wave(&format(1, 1, 8000, 16), &[])).is_err());
    }

    #[test]
    fn extensible_precision_guid_and_declared_extension_are_checked() {
        for (codec, bits, valid) in [(1, 24, 20), (1, 32, 0), (3, 32, 32), (3, 64, 64)] {
            let fmt = extensible(codec, 3, 48000, bits, valid);
            let data = vec![0; 3 * usize::from(bits / 8)];
            assert_eq!(parse(&wave(&fmt, &data)).unwrap(), 1.0 / 48000.0);
        }
        let fmt = extensible(1, 2, 48000, 24, 20);
        for end in 16..40 {
            assert!(parse(&wave(&fmt[..end], &[0; 6])).is_err());
        }
        for extension in [0u16, 21, 23, u16::MAX] {
            let mut bad = fmt.clone();
            bad[16..18].copy_from_slice(&extension.to_le_bytes());
            assert!(parse(&wave(&bad, &[0; 6])).is_err());
        }
        let mut bad = fmt.clone();
        bad[18..20].copy_from_slice(&25u16.to_le_bytes());
        assert!(parse(&wave(&bad, &[0; 6])).is_err());
        for index in 24..40 {
            let mut bad = fmt.clone();
            bad[index] ^= 0x80;
            assert!(parse(&wave(&bad, &[0; 6])).is_err());
        }
        let mut more = fmt;
        more[16..18].copy_from_slice(&24u16.to_le_bytes());
        more.extend_from_slice(&[0; 2]);
        assert!(parse(&wave(&more, &[0; 6])).is_ok());
    }

    fn extended(
        magic: &[u8; 4],
        chunks: &[Vec<u8>],
        data_size: u64,
        overrides: &[([u8; 4], u64)],
    ) -> Vec<u8> {
        let mut ds64 = 0u64.to_le_bytes().to_vec();
        ds64.extend_from_slice(&data_size.to_le_bytes());
        ds64.extend_from_slice(&0u64.to_le_bytes());
        ds64.extend_from_slice(&(overrides.len() as u32).to_le_bytes());
        for (id, size) in overrides {
            ds64.extend_from_slice(id);
            ds64.extend_from_slice(&size.to_le_bytes());
        }
        let mut all = vec![chunk(b"ds64", &ds64)];
        all.extend_from_slice(chunks);
        let mut bytes = riff(&all);
        bytes[..4].copy_from_slice(magic);
        bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        let extent = bytes.len() as u64 - 8;
        bytes[20..28].copy_from_slice(&extent.to_le_bytes());
        bytes
    }
    fn sentinel(mut chunk: Vec<u8>) -> Vec<u8> {
        chunk[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        chunk
    }

    #[test]
    fn rf64_and_bw64_use_actual_sizes_and_allow_finite_container_sizes() {
        for magic in [b"RF64", b"BW64"] {
            let fmt = chunk(b"fmt ", &format(1, 1, 48000, 24));
            let data = chunk(b"data", &[0; 6]);
            let bytes = extended(magic, &[fmt.clone(), sentinel(data.clone())], 6, &[]);
            assert_eq!(parse(&bytes).unwrap(), 2.0 / 48000.0);
            let mut finite = extended(magic, &[fmt, data], u64::MAX, &[]);
            let size = finite.len() as u32 - 8;
            finite[4..8].copy_from_slice(&size.to_le_bytes());
            finite[20..28].copy_from_slice(&u64::MAX.to_le_bytes());
            // Non-sentinel fields supply sizes; unused ds64 values cannot override them.
            assert_eq!(parse(&finite).unwrap(), 2.0 / 48000.0);
        }
    }

    #[test]
    fn rf64_unknown_sentinel_chunks_use_table_and_repeated_ids_in_order() {
        let chunks = [
            sentinel(chunk(b"JUNK", &[1])),
            sentinel(chunk(b"JUNK", &[1, 2, 3])),
            chunk(b"fmt ", &format(1, 1, 8000, 8)),
            sentinel(chunk(b"data", &[0; 3])),
        ];
        let bytes = extended(b"RF64", &chunks, 3, &[(*b"JUNK", 1), (*b"JUNK", 3)]);
        assert_eq!(parse(&bytes).unwrap(), 3.0 / 8000.0);
        assert!(parse(&extended(b"RF64", &chunks, 3, &[])).is_err());
        assert!(parse(&extended(b"RF64", &chunks, 3, &[(*b"JUNK", u64::MAX)])).is_err());
    }

    #[test]
    fn rf64_incomplete_sizes_tables_and_overflows_are_rejected_without_allocation() {
        let fmt = chunk(b"fmt ", &format(1, 1, 8000, 8));
        let data = sentinel(chunk(b"data", &[0; 3]));
        let valid = extended(b"RF64", &[fmt, data], 3, &[]);
        for end in 0..valid.len() {
            assert!(parse(&valid[..end]).is_err());
        }
        let mut missing = valid.clone();
        missing[12..16].copy_from_slice(b"JUNK");
        assert!(parse(&missing).is_err());
        let mut short = valid.clone();
        short[16..20].copy_from_slice(&27u32.to_le_bytes());
        assert!(parse(&short).is_err());
        for table_count in [1u32, u32::MAX] {
            let mut bad = valid.clone();
            bad[44..48].copy_from_slice(&table_count.to_le_bytes());
            assert!(parse(&bad).is_err());
        }
        for riff_size in [0, 27, valid.len() as u64, u64::MAX] {
            let mut bad = valid.clone();
            bad[20..28].copy_from_slice(&riff_size.to_le_bytes());
            assert!(parse(&bad).is_err());
        }
        let mut overflow = valid;
        overflow[28..36].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(parse(&overflow).is_err());
    }

    #[test]
    fn rf64_duration_above_four_gib_seeks_over_unreadable_sample_range() {
        // Virtual sparse file: only metadata is backed by memory. Its advertised
        // sample range exists but cannot be read; duration must seek over it.
        let data_size = u64::from(u32::MAX) + 3;
        let mut header = extended(
            b"RF64",
            &[
                chunk(b"fmt ", &format(1, 1, 48000, 16)),
                sentinel(chunk(b"data", &[])),
            ],
            data_size,
            &[],
        );
        let physical = header.len() as u64 + data_size;
        header[20..28].copy_from_slice(&(physical - 8).to_le_bytes());
        let mut sparse = Cursor::new(header);
        assert_eq!(
            duration(&mut sparse, physical).unwrap(),
            (data_size / 2) as f64 / 48000.0
        );
        assert_eq!(sparse.position(), physical);
    }

    #[test]
    fn missing_and_ambiguous_required_chunks_are_rejected() {
        let fmt = chunk(b"fmt ", &format(1, 1, 8000, 8));
        let data = chunk(b"data", &[0; 2]);
        assert!(parse(&riff(std::slice::from_ref(&fmt))).is_err());
        assert!(parse(&riff(std::slice::from_ref(&data))).is_err());
        assert!(parse(&riff(&[fmt.clone(), data.clone(), fmt.clone()])).is_err());
        assert!(parse(&riff(&[fmt, data.clone(), data])).is_err());
        let mut wrong = wave(&format(1, 1, 8000, 8), &[0; 2]);
        wrong[..4].copy_from_slice(b"RIFX");
        assert!(parse(&wrong).is_err());
        wrong[..4].copy_from_slice(b"RIFF");
        wrong[8..12].copy_from_slice(b"AVI ");
        assert!(parse(&wrong).is_err());
    }

    #[test]
    fn public_file_entry_point_uses_the_same_validation() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&wave(&format(1, 2, 48000, 16), &[0; 20]))
            .unwrap();
        assert_eq!(crate::wav_duration(file.path()).unwrap(), 5.0 / 48000.0);
    }
}
