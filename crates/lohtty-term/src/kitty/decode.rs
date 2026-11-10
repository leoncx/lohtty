//! Turns a transmission (direct, file or temporary file; raw RGB/RGBA or
//! PNG; optionally zlib-compressed) into RGBA pixels.

use std::io::{Read, Seek, SeekFrom};

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

use super::MAX_PIXELS;
use super::command::Command;

pub(super) struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub(super) fn decode(cmd: &Command, payload: &[u8]) -> Result<Decoded, String> {
    let data = base64_decode(payload)?;
    let data = match cmd.medium {
        b'd' => data,
        b'f' | b't' => read_file(cmd, &data)?,
        b's' => return Err("EINVAL:shared memory transmission is not supported".into()),
        _ => return Err("EINVAL:unknown transmission medium".into()),
    };
    let data = match cmd.compression {
        0 => data,
        b'z' => inflate(&data)?,
        _ => return Err("EINVAL:unknown compression".into()),
    };
    match cmd.format {
        100 => decode_png(&data),
        24 | 32 => decode_raw(cmd, data),
        _ => Err("EINVAL:unknown format".into()),
    }
}

fn base64_decode(payload: &[u8]) -> Result<Vec<u8>, String> {
    // Chunks are multiples of 4 characters except the last, so padding can
    // only end the data; tolerate it missing.
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let clean: Vec<u8> = payload
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    engine
        .decode(clean)
        .map_err(|e| format!("EINVAL:bad base64 data: {e}"))
}

fn read_file(cmd: &Command, path: &[u8]) -> Result<Vec<u8>, String> {
    let path = std::str::from_utf8(path).map_err(|_| "EINVAL:bad file name".to_owned())?;
    // Only regular local files: devices, pipes and network paths could block
    // the PTY reader (and so the terminal) indefinitely.
    if path.starts_with(r"\\") || path.starts_with("//") {
        return Err("EBADF:only local files are read".into());
    }
    let path = std::path::Path::new(path);
    let meta = std::fs::metadata(path).map_err(|e| format!("EBADF:{e}"))?;
    if !meta.is_file() {
        return Err("EBADF:not a regular file".into());
    }
    let mut file = std::fs::File::open(path).map_err(|e| format!("EBADF:{e}"))?;
    if cmd.offset > 0 {
        file.seek(SeekFrom::Start(cmd.offset))
            .map_err(|e| format!("EBADF:{e}"))?;
    }
    let mut data = Vec::new();
    let limit = if cmd.size > 0 {
        cmd.size
    } else {
        super::MAX_COMMAND_BYTES as u64
    };
    file.take(limit)
        .read_to_end(&mut data)
        .map_err(|e| format!("EBADF:{e}"))?;
    // Temporary files are deleted after reading, but only ones that look
    // like they were made for this purpose (as kitty requires).
    if cmd.medium == b't'
        && path.to_string_lossy().contains("tty-graphics-protocol")
        && path.starts_with(std::env::temp_dir())
    {
        let _ = std::fs::remove_file(path);
    }
    Ok(data)
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(data)
        .take(MAX_PIXELS * 4 + 1)
        .read_to_end(&mut out)
        .map_err(|e| format!("EINVAL:bad zlib data: {e}"))?;
    Ok(out)
}

fn decode_png(data: &[u8]) -> Result<Decoded, String> {
    let mut reader =
        image::ImageReader::with_format(std::io::Cursor::new(data), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4 * 2);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("EBADPNG:{e}"))?
        .into_rgba8();
    Ok(Decoded {
        width: image.width(),
        height: image.height(),
        rgba: image.into_raw(),
    })
}

fn decode_raw(cmd: &Command, data: Vec<u8>) -> Result<Decoded, String> {
    let (w, h) = (cmd.width, cmd.height);
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err("EINVAL:bad image size".into());
    }
    let pixels = (w * h) as usize;
    let rgba = if cmd.format == 32 {
        if data.len() < pixels * 4 {
            return Err("ENODATA:not enough image data".into());
        }
        let mut data = data;
        data.truncate(pixels * 4);
        data
    } else {
        if data.len() < pixels * 3 {
            return Err("ENODATA:not enough image data".into());
        }
        let mut rgba = Vec::with_capacity(pixels * 4);
        for px in data[..pixels * 3].as_chunks::<3>().0 {
            rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
        }
        rgba
    };
    Ok(Decoded {
        width: w,
        height: h,
        rgba,
    })
}
