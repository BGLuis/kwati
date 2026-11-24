use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::{Backend, Location, PrefetchReader};

const STREAM_CHUNK_SIZE: usize = 256 * 1024;
const SOCKET_READ_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(30);

const STREAMABLE_MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "avi", "mov", "wmv", "flv", "ts", "m2ts", "mpg", "mpeg", "ogv",
    "3gp", "mp3", "flac", "ogg", "opus", "wav", "m4a", "aac", "wma", "aiff",
];

const BROWSER_NATIVE_MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "webm", "mp3", "ogg", "opus", "wav", "aac", "flac",
];

/// Whether `path`'s extension identifies a video or audio format suitable for streaming.
pub fn is_streamable_media(path: &Path) -> bool {
    extension_lower(path).is_some_and(|ext| STREAMABLE_MEDIA_EXTENSIONS.contains(&ext.as_str()))
}

/// Whether `path` uses a container/codec commonly playable directly inside a web browser.
pub fn is_browser_native_media(path: &Path) -> bool {
    extension_lower(path).is_some_and(|ext| BROWSER_NATIVE_MEDIA_EXTENSIONS.contains(&ext.as_str()))
}

/// MIME type inferred from `path`'s extension for HTTP streaming responses.
pub fn media_mime_type(path: &Path) -> &'static str {
    match extension_lower(path).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mkv") => "video/x-matroska",
        Some("webm") => "video/webm",
        Some("avi") => "video/x-msvideo",
        Some("mov") => "video/quicktime",
        Some("wmv") => "video/x-ms-wmv",
        Some("flv") => "video/x-flv",
        Some("ts") | Some("m2ts") => "video/mp2t",
        Some("mpg") | Some("mpeg") => "video/mpeg",
        Some("ogv") => "video/ogg",
        Some("3gp") => "video/3gpp",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ogg") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("m4a") => "audio/mp4",
        Some("aac") => "audio/aac",
        Some("wma") => "audio/x-ms-wma",
        Some("aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
}

fn extension_lower(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
}

#[derive(Clone)]
struct StreamEntry {
    backend: Arc<dyn Backend>,
    location: Location,
    len: u64,
    content_type: String,
    reader: Arc<Mutex<Option<PrefetchReader>>>,
}

/// Loopback HTTP/1.1 range-request server for streaming remote files directly to media players.
pub struct RemoteStreamServer {
    port: u16,
    entries: Arc<Mutex<HashMap<String, StreamEntry>>>,
    shutdown: Arc<AtomicBool>,
    accept_thread: Option<JoinHandle<()>>,
}

impl RemoteStreamServer {
    /// Starts a new loopback HTTP server bound to `127.0.0.1` on an ephemeral port.
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let entries: Arc<Mutex<HashMap<String, StreamEntry>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let shutdown = Arc::new(AtomicBool::new(false));

        let entries_bg = Arc::clone(&entries);
        let shutdown_bg = Arc::clone(&shutdown);
        let accept_thread = thread::Builder::new()
            .name("kwati-remote-stream".to_string())
            .spawn(move || {
                for incoming in listener.incoming() {
                    if shutdown_bg.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = incoming else {
                        continue;
                    };
                    let entries = Arc::clone(&entries_bg);
                    let shutdown = Arc::clone(&shutdown_bg);
                    thread::spawn(move || handle_client(stream, entries, shutdown));
                }
            })?;

        Ok(Self {
            port,
            entries,
            shutdown,
            accept_thread: Some(accept_thread),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Registers a remote file and returns a secret loopback URL with its filename preserved.
    pub fn register(
        &self,
        backend: Arc<dyn Backend>,
        location: Location,
        len: u64,
        content_type: impl Into<String>,
    ) -> String {
        let token = generate_token();
        let file_name = location
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "media".to_string());
        let encoded_name = percent_encode_segment(&file_name);

        let entry = StreamEntry {
            backend,
            location,
            len,
            content_type: content_type.into(),
            reader: Arc::new(Mutex::new(None)),
        };
        self.entries.lock().unwrap().insert(token.clone(), entry);
        format!("http://127.0.0.1:{}/{}/{}", self.port, token, encoded_name)
    }

    /// Removes all registered streams belonging to `profile_id` (e.g. on disconnect).
    pub fn unregister_profile(&self, profile_id: &str) {
        self.entries.lock().unwrap().retain(|_, entry| {
            !matches!(&entry.location, Location::Remote { profile_id: pid, .. } if pid == profile_id)
        });
    }
}

impl Drop for RemoteStreamServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(handle) = self.accept_thread.take() {
            let _ = handle.join();
        }
    }
}

fn generate_token() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0u128, |d| d.as_nanos());
    let pid = u128::from(std::process::id());
    let a = nanos ^ (pid << 64) ^ u128::from(seq).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let b = read_os_random_u64().unwrap_or_else(|| (seq ^ (nanos as u64)).rotate_left(17));
    format!("{a:032x}{b:016x}")
}

fn read_os_random_u64() -> Option<u64> {
    #[cfg(unix)]
    {
        let mut f = std::fs::File::open("/dev/urandom").ok()?;
        let mut buf = [0u8; 8];
        f.read_exact(&mut buf).ok()?;
        Some(u64::from_le_bytes(buf))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn percent_encode_segment(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

fn handle_client(
    mut stream: TcpStream,
    entries: Arc<Mutex<HashMap<String, StreamEntry>>>,
    shutdown: Arc<AtomicBool>,
) {
    let _ = stream.set_read_timeout(Some(SOCKET_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(SOCKET_WRITE_TIMEOUT));

    while !shutdown.load(Ordering::Relaxed) {
        let Some(req) = read_http_request(&stream) else {
            break;
        };

        let keep_alive = req.keep_alive;
        if !serve_request(&mut stream, &req, &entries) || !keep_alive {
            break;
        }
    }
}

struct HttpRequest {
    method: String,
    path: String,
    range: Option<String>,
    keep_alive: bool,
}

fn read_http_request(stream: &TcpStream) -> Option<HttpRequest> {
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line).ok()?;
    if n == 0 {
        return None;
    }

    let mut parts = request_line
        .trim_end_matches(['\r', '\n'])
        .split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let version = parts.next().unwrap_or("HTTP/1.1");

    let mut range = None;
    let mut connection_header: Option<String> = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).ok()?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, val)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("range") {
                range = Some(val.trim().to_string());
            } else if name.eq_ignore_ascii_case("connection") {
                connection_header = Some(val.trim().to_ascii_lowercase());
            }
        }
    }

    let path = target.split('?').next().unwrap_or(&target).to_string();
    let keep_alive = match connection_header.as_deref() {
        Some(v) if v.contains("close") => false,
        Some(v) if v.contains("keep-alive") => true,
        _ => version == "HTTP/1.1",
    };

    Some(HttpRequest {
        method,
        path,
        range,
        keep_alive,
    })
}

fn serve_request(
    stream: &mut TcpStream,
    req: &HttpRequest,
    entries: &Mutex<HashMap<String, StreamEntry>>,
) -> bool {
    if req.method != "GET" && req.method != "HEAD" {
        return write_status(stream, 405, "Method Not Allowed", req.keep_alive);
    }

    let Some(token) = req.path.trim_start_matches('/').split('/').next() else {
        return write_status(stream, 404, "Not Found", req.keep_alive);
    };
    let entry = entries.lock().unwrap().get(token).cloned();
    let Some(entry) = entry else {
        return write_status(stream, 404, "Not Found", req.keep_alive);
    };

    let total = entry.len;
    let parsed_range = match &req.range {
        Some(hdr) => match parse_byte_range(hdr, total) {
            Some(Ok((start, end))) => Some((start, end)),
            Some(Err(())) => {
                let headers = format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: {}\r\n\r\n",
                    conn_value(req.keep_alive)
                );
                return stream.write_all(headers.as_bytes()).is_ok();
            }
            None => None,
        },
        None => None,
    };

    let (status_line, start, content_len, content_range_hdr) = match parsed_range {
        Some((start, end)) => (
            "HTTP/1.1 206 Partial Content",
            start,
            end - start + 1,
            format!("Content-Range: bytes {start}-{end}/{total}\r\n"),
        ),
        None => ("HTTP/1.1 200 OK", 0, total, String::new()),
    };

    let headers = format!(
        "{status_line}\r\nContent-Type: {}\r\nAccept-Ranges: bytes\r\n{content_range_hdr}Content-Length: {content_len}\r\nConnection: {}\r\n\r\n",
        entry.content_type,
        conn_value(req.keep_alive)
    );
    if stream.write_all(headers.as_bytes()).is_err() {
        return false;
    }
    if req.method == "HEAD" || content_len == 0 {
        return stream.flush().is_ok();
    }

    let mut guard = entry.reader.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        let Ok(source) = entry.backend.open_range_source(&entry.location) else {
            return false;
        };
        *guard = Some(PrefetchReader::new(source));
    }
    let Some(reader) = guard.as_mut() else {
        return false;
    };

    if reader.seek(SeekFrom::Start(start)).is_err() {
        *guard = None;
        return false;
    }

    let mut remaining = content_len;
    let mut buf = vec![0u8; STREAM_CHUNK_SIZE];
    while remaining > 0 {
        let to_read = (remaining as usize).min(buf.len());
        let Ok(n) = reader.read(&mut buf[..to_read]) else {
            *guard = None;
            return false;
        };
        if n == 0 {
            break;
        }
        if stream.write_all(&buf[..n]).is_err() {
            return false;
        }
        remaining -= n as u64;
    }
    stream.flush().is_ok()
}

fn conn_value(keep_alive: bool) -> &'static str {
    if keep_alive {
        "keep-alive"
    } else {
        "close"
    }
}

fn write_status(stream: &mut TcpStream, code: u16, reason: &str, keep_alive: bool) -> bool {
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: {}\r\n\r\n",
        conn_value(keep_alive)
    );
    stream.write_all(resp.as_bytes()).is_ok() && stream.flush().is_ok()
}

/// Parses a single HTTP `Range: bytes=...` specification against `total_len`.
///
/// Returns `None` when the header is not a `bytes=` range, `Some(Err(()))` when the range is
/// unsatisfiable (`416`), and `Some(Ok((start, end)))` (inclusive) when valid.
fn parse_byte_range(header: &str, total_len: u64) -> Option<Result<(u64, u64), ()>> {
    let spec = header.trim().strip_prefix("bytes=")?;
    if spec.contains(',') {
        // Multi-range requests are uncommon for media players; fall back to full response.
        return None;
    }
    let (start_str, end_str) = spec.split_once('-')?;
    let start_str = start_str.trim();
    let end_str = end_str.trim();

    if total_len == 0 {
        return Some(Err(()));
    }

    if start_str.is_empty() {
        let suffix: u64 = match end_str.parse() {
            Ok(n) if n > 0 => n,
            _ => return Some(Err(())),
        };
        let start = total_len.saturating_sub(suffix);
        return Some(Ok((start, total_len - 1)));
    }

    let start: u64 = match start_str.parse() {
        Ok(n) => n,
        Err(_) => return Some(Err(())),
    };
    if start >= total_len {
        return Some(Err(()));
    }

    let end = if end_str.is_empty() {
        total_len - 1
    } else {
        let parsed: u64 = match end_str.parse() {
            Ok(n) => n,
            Err(_) => return Some(Err(())),
        };
        if parsed < start {
            return Some(Err(()));
        }
        parsed.min(total_len - 1)
    };

    Some(Ok((start, end)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BackendError, EntryMeta, Listing, ReadSeek};
    use std::io::{Cursor, Seek};
    use std::path::PathBuf;

    struct MemSeekBackend {
        data: Vec<u8>,
        open_calls: AtomicU64,
    }

    struct CursorReader(Cursor<Vec<u8>>);

    impl Read for CursorReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.0.read(buf)
        }
    }

    impl Seek for CursorReader {
        fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
            self.0.seek(pos)
        }
    }

    impl Backend for MemSeekBackend {
        fn list(&self, _location: &Location) -> Result<Listing, BackendError> {
            Err(BackendError::Unsupported)
        }

        fn stat(&self, _location: &Location) -> Result<EntryMeta, BackendError> {
            Ok(EntryMeta {
                kind: crate::EntryKind::File,
                len: self.data.len() as u64,
            })
        }

        fn open_read(&self, _location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
            Ok(Box::new(Cursor::new(self.data.clone())))
        }

        fn open_read_seek(&self, _location: &Location) -> Result<Box<dyn ReadSeek>, BackendError> {
            self.open_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(CursorReader(Cursor::new(self.data.clone()))))
        }
    }

    fn send_http(port: u16, req: &str) -> (String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(req.as_bytes()).unwrap();
        stream.flush().unwrap();
        let mut raw = Vec::new();
        stream.read_to_end(&mut raw).unwrap();
        let split = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("HTTP header separator");
        let headers = String::from_utf8_lossy(&raw[..split]).into_owned();
        let body = raw[split + 4..].to_vec();
        (headers, body)
    }

    #[test]
    fn parses_valid_and_invalid_byte_ranges() {
        assert_eq!(parse_byte_range("bytes=0-9", 100), Some(Ok((0, 9))));
        assert_eq!(parse_byte_range("bytes=90-500", 100), Some(Ok((90, 99))));
        assert_eq!(parse_byte_range("bytes=50-", 100), Some(Ok((50, 99))));
        assert_eq!(parse_byte_range("bytes=-10", 100), Some(Ok((90, 99))));
        assert_eq!(parse_byte_range("bytes=100-", 100), Some(Err(())));
        assert_eq!(parse_byte_range("bytes=10-5", 100), Some(Err(())));
        assert_eq!(parse_byte_range("bytes=0-0", 0), Some(Err(())));
    }

    #[test]
    fn streams_head_full_and_byte_ranges_over_loopback_http() {
        let server = RemoteStreamServer::start().unwrap();
        let payload: Vec<u8> = (0..1024u32).map(|i| (i % 251) as u8).collect();
        let backend_impl = Arc::new(MemSeekBackend {
            data: payload.clone(),
            open_calls: AtomicU64::new(0),
        });
        let backend: Arc<dyn Backend> = backend_impl.clone();
        let loc = Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/videos/demo clip.mkv"),
        };
        let url = server.register(backend, loc, payload.len() as u64, "video/x-matroska");
        let path = url
            .strip_prefix(&format!("http://127.0.0.1:{}", server.port()))
            .unwrap();
        assert!(path.ends_with("/demo%20clip.mkv"));

        // HEAD
        let (head_hdrs, head_body) = send_http(
            server.port(),
            &format!("HEAD {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
        );
        assert!(head_hdrs.starts_with("HTTP/1.1 200 OK"));
        assert!(head_hdrs.contains("Accept-Ranges: bytes"));
        assert!(head_hdrs.contains("Content-Length: 1024"));
        assert!(head_hdrs.contains("Content-Type: video/x-matroska"));
        assert!(head_body.is_empty());

        // Multiple Partial Range requests must reuse a single cached PrefetchReader handle.
        let (range_hdrs, range_body) = send_http(
            server.port(),
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=100-199\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(range_hdrs.starts_with("HTTP/1.1 206 Partial Content"));
        assert!(range_hdrs.contains("Content-Range: bytes 100-199/1024"));
        assert!(range_hdrs.contains("Content-Length: 100"));
        assert_eq!(range_body, &payload[100..200]);

        let (range2_hdrs, range2_body) = send_http(
            server.port(),
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=800-899\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(range2_hdrs.starts_with("HTTP/1.1 206 Partial Content"));
        assert_eq!(range2_body, &payload[800..900]);
        assert_eq!(backend_impl.open_calls.load(Ordering::Relaxed), 1);

        // Unsatisfiable Range
        let (bad_hdrs, _) = send_http(
            server.port(),
            &format!(
                "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nRange: bytes=5000-\r\nConnection: close\r\n\r\n"
            ),
        );
        assert!(bad_hdrs.starts_with("HTTP/1.1 416 Range Not Satisfiable"));

        // Unknown token
        let (nf_hdrs, _) = send_http(
            server.port(),
            "GET /wrongtoken/movie.mkv HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
        );
        assert!(nf_hdrs.starts_with("HTTP/1.1 404 Not Found"));
    }

    #[test]
    fn detects_media_extensions_and_mime_types() {
        assert!(is_streamable_media(Path::new("/share/movie.MKV")));
        assert!(is_streamable_media(Path::new("/share/song.flac")));
        assert!(!is_streamable_media(Path::new("/share/notes.txt")));
        assert!(is_browser_native_media(Path::new("clip.mp4")));
        assert!(!is_browser_native_media(Path::new("clip.mkv")));
        assert_eq!(media_mime_type(Path::new("clip.mp4")), "video/mp4");
        assert_eq!(media_mime_type(Path::new("clip.mkv")), "video/x-matroska");
    }
}
