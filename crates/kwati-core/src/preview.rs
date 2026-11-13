use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// Maximum number of bytes read for a file preview (64 KiB = 65,536 bytes).
pub const PREVIEW_BYTE_LIMIT: usize = 65_536;

/// Classification and bounded content of a file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// The file begins with valid UTF-8 text.
    Text(String),
    /// The file contains binary data (a NUL byte or invalid UTF-8).
    Binary,
}

impl Preview {
    /// Returns `true` if the preview was classified as text.
    pub fn is_text(&self) -> bool {
        matches!(self, Preview::Text(_))
    }

    /// Returns `true` if the preview was classified as binary.
    pub fn is_binary(&self) -> bool {
        matches!(self, Preview::Binary)
    }

    /// Returns a reference to the text if this preview is text, or `None` if binary.
    pub fn text(&self) -> Option<&str> {
        match self {
            Preview::Text(s) => Some(s),
            Preview::Binary => None,
        }
    }
}

/// Classifies a byte slice as a [`Preview`].
///
/// If `bytes` is longer than [`PREVIEW_BYTE_LIMIT`], only the first [`PREVIEW_BYTE_LIMIT`]
/// bytes are inspected. If the slice contains any NUL byte (`0x00`) or invalid UTF-8
/// sequences, it is classified as [`Preview::Binary`]. Otherwise, it is classified as
/// [`Preview::Text`], dropping any incomplete UTF-8 character at the end of the slice.
pub fn classify_preview(bytes: &[u8]) -> Preview {
    let slice = if bytes.len() > PREVIEW_BYTE_LIMIT {
        &bytes[..PREVIEW_BYTE_LIMIT]
    } else {
        bytes
    };

    if slice.contains(&0) {
        return Preview::Binary;
    }

    match std::str::from_utf8(slice) {
        Ok(text) => Preview::Text(text.to_string()),
        Err(err) if err.error_len().is_none() => {
            let valid_len = err.valid_up_to();
            if valid_len == 0 && !slice.is_empty() {
                Preview::Binary
            } else {
                let valid_bytes = &slice[..valid_len];
                match std::str::from_utf8(valid_bytes) {
                    Ok(text) => Preview::Text(text.to_string()),
                    Err(_) => Preview::Binary,
                }
            }
        }
        Err(_) => Preview::Binary,
    }
}

/// Reads at most [`PREVIEW_BYTE_LIMIT`] bytes from a reader and classifies the preview.
pub fn preview_from_reader<R: Read>(reader: R) -> io::Result<Preview> {
    let mut handle = reader.take(PREVIEW_BYTE_LIMIT as u64);
    let mut buf = Vec::with_capacity(PREVIEW_BYTE_LIMIT);
    handle.read_to_end(&mut buf)?;
    Ok(classify_preview(&buf))
}

/// Reads a bounded preview from the file at `path` and classifies it.
///
/// Reads at most the first 65,536 bytes. Classifies them as binary when they contain a NUL
/// byte or are not valid UTF-8, and as text otherwise, dropping an incomplete UTF-8 character
/// at the end.
pub fn read_preview(path: impl AsRef<Path>) -> io::Result<Preview> {
    let path = path.as_ref();
    let meta = std::fs::metadata(path)?;
    if !meta.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let file = File::open(path)?;
    preview_from_reader(file)
}

/// Alias for [`read_preview`].
pub fn preview(path: impl AsRef<Path>) -> io::Result<Preview> {
    read_preview(path)
}

/// Alias for [`read_preview`].
pub fn preview_file(path: impl AsRef<Path>) -> io::Result<Preview> {
    read_preview(path)
}
