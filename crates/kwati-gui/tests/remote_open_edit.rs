use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use kwati_core::{
    AuthMethod, Backend, BackendError, Confirmed, ConnectionProfile, Entry, EntryKind, EntryMeta,
    Listing, Location, Protocol, ReadSeek, ReconnectingBackend, RetryPolicy,
};
use kwati_gui::actions::{
    open_remote_file, register_keybindings, stream_launcher_from_desktop_id, AppInfo,
    RemoteOpenTarget,
};
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::{AppState, PreviewKind};
use kwati_gui::views::MainView;

#[derive(Clone)]
struct RemoteMemBackend {
    state: Arc<Mutex<RemoteMemState>>,
}

struct RemoteMemState {
    dirs: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    seekable: bool,
}

impl RemoteMemBackend {
    fn new(seekable: bool) -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert(PathBuf::from("/"));
        Self {
            state: Arc::new(Mutex::new(RemoteMemState {
                dirs,
                files: BTreeMap::new(),
                seekable,
            })),
        }
    }

    fn insert_dir(&self, path: impl AsRef<Path>) {
        let mut st = self.state.lock().unwrap();
        let mut cur = Some(path.as_ref());
        while let Some(d) = cur {
            st.dirs.insert(d.to_path_buf());
            cur = d.parent();
        }
    }

    fn insert_file(&self, path: impl AsRef<Path>, data: Vec<u8>) {
        let p = path.as_ref().to_path_buf();
        if let Some(parent) = p.parent() {
            self.insert_dir(parent);
        }
        self.state.lock().unwrap().files.insert(p, data);
    }

    fn read_file(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(path.as_ref()).cloned()
    }
}

struct MemWriter {
    path: PathBuf,
    buf: Vec<u8>,
    state: Arc<Mutex<RemoteMemState>>,
}

impl Write for MemWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state
            .lock()
            .unwrap()
            .files
            .insert(self.path.clone(), self.buf.clone());
        Ok(())
    }
}

impl Drop for MemWriter {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

struct CursorSeek(Cursor<Vec<u8>>);

impl Read for CursorSeek {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.read(buf)
    }
}

impl Seek for CursorSeek {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.0.seek(pos)
    }
}

impl Backend for RemoteMemBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let dir = location.path();
        let st = self.state.lock().unwrap();
        if !st.dirs.contains(dir) {
            return Err(io::Error::from(io::ErrorKind::NotFound).into());
        }
        let mut entries = Vec::new();
        for d in &st.dirs {
            if d.parent() == Some(dir) {
                entries.push(Entry::new(
                    d.file_name().unwrap().to_os_string(),
                    d.clone(),
                    true,
                ));
            }
        }
        for (f, data) in &st.files {
            if f.parent() == Some(dir) {
                entries.push(
                    Entry::new(f.file_name().unwrap().to_os_string(), f.clone(), false)
                        .with_size(data.len() as u64),
                );
            }
        }
        Ok(Listing::new(entries))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        let st = self.state.lock().unwrap();
        if let Some(data) = st.files.get(location.path()) {
            return Ok(EntryMeta {
                kind: EntryKind::File,
                len: data.len() as u64,
            });
        }
        if st.dirs.contains(location.path()) {
            return Ok(EntryMeta {
                kind: EntryKind::Dir,
                len: 0,
            });
        }
        Err(io::Error::from(io::ErrorKind::NotFound).into())
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        let st = self.state.lock().unwrap();
        let data = st
            .files
            .get(location.path())
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok(Box::new(Cursor::new(data)))
    }

    fn open_read_seek(&self, location: &Location) -> Result<Box<dyn ReadSeek>, BackendError> {
        let st = self.state.lock().unwrap();
        if !st.seekable {
            return Err(BackendError::Unsupported);
        }
        let data = st
            .files
            .get(location.path())
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok(Box::new(CursorSeek(Cursor::new(data))))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        Ok(Box::new(MemWriter {
            path: location.path().to_path_buf(),
            buf: Vec::new(),
            state: Arc::clone(&self.state),
        }))
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        let data = st
            .files
            .remove(from.path())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        st.files.insert(to.path().to_path_buf(), data);
        Ok(())
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        st.files.remove(location.path());
        st.dirs.remove(location.path());
        Ok(())
    }
}

fn attach_remote(state: &mut AppState, profile_id: &str, backend: RemoteMemBackend) {
    let profile = ConnectionProfile {
        id: profile_id.to_owned(),
        name: "NAS Server".to_owned(),
        protocol: Protocol::Smb,
        host: "nas.local".to_owned(),
        port: 445,
        user: "luis".to_owned(),
        start_path: "/share".to_owned(),
        auth: AuthMethod::Password,
        private_key_path: None,
        allow_insecure: false,
    };
    let _ = state.remote.save_profile(profile, None);
    let reconnecting = ReconnectingBackend::new(
        move || Ok(Box::new(backend.clone())),
        RetryPolicy::new(Duration::from_millis(1), 1),
    )
    .unwrap();
    state.remote.add_connection(profile_id, reconnecting);
}

type OpenedTargets = Arc<Mutex<Vec<RemoteOpenTarget>>>;

fn open_remote_harness<'a>(
    cx: &'a mut TestAppContext,
    initial_location: Location,
    profile_id: &'static str,
    backend: RemoteMemBackend,
) -> (
    Entity<MainView>,
    Entity<AppState>,
    OpenedTargets,
    &'a mut VisualTestContext,
) {
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });
    let opened = Arc::new(Mutex::new(Vec::new()));
    let opened_cb = Arc::clone(&opened);
    let mut app_state_holder: Option<Entity<AppState>> = None;
    let (main, vcx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut st = AppState::new(initial_location);
            st.attach(cx);
            attach_remote(&mut st, profile_id, backend);
            st.set_remote_edit_debounce_for_test(Duration::from_millis(40));
            st.set_remote_opener_for_test(Arc::new(move |target| {
                opened_cb.lock().unwrap().push(target.clone());
                Ok(())
            }));
            st.set_stream_launcher_for_test(Arc::new(|_| {
                Some(AppInfo::new("Test Player", "true %U", true))
            }));
            st.reload_entries();
            st
        });
        app_state_holder = Some(state.clone());
        MainView::new(state, window, cx)
    });
    vcx.run_until_parked();
    (main, app_state_holder.unwrap(), opened, vcx)
}

fn wait_until(
    vcx: &mut VisualTestContext,
    timeout: Duration,
    mut pred: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        vcx.run_until_parked();
        if pred(vcx) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    vcx.run_until_parked();
}

#[gpui::test]
fn opening_and_editing_remote_document_syncs_changes_back(cx: &mut TestAppContext) {
    let backend = RemoteMemBackend::new(true);
    backend.insert_file("/share/notes.txt", b"initial remote text".to_vec());
    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/share"),
    };

    let (main, state, opened, vcx) = open_remote_harness(cx, root.clone(), "srv", backend.clone());

    // Select notes.txt and press Enter to open it.
    state.update(vcx, |st, cx| {
        let ix = st
            .entries
            .iter()
            .position(|e| e.name() == "notes.txt")
            .unwrap();
        st.select_index(ix);
        cx.notify();
    });
    vcx.run_until_parked();

    vcx.simulate_keystrokes("enter");
    wait_until(vcx, Duration::from_secs(3), |_| {
        !opened.lock().unwrap().is_empty()
    });

    let cached_path = match opened.lock().unwrap().as_slice() {
        [RemoteOpenTarget::File(path)] => path.clone(),
        other => panic!("expected cached file launch, got {other:?}"),
    };
    assert_eq!(fs::read(&cached_path).unwrap(), b"initial remote text");

    // Simulate an external editor saving edits atomically (temp file + rename over cached_path).
    let tmp = cached_path.with_file_name(".notes.txt.tmp");
    fs::write(&tmp, b"updated in external editor").unwrap();
    fs::rename(&tmp, &cached_path).unwrap();

    wait_until(vcx, Duration::from_secs(5), |_| {
        backend.read_file("/share/notes.txt").as_deref() == Some(b"updated in external editor")
    });
    assert_eq!(
        backend.read_file("/share/notes.txt").as_deref(),
        Some(b"updated in external editor".as_slice())
    );

    let status = main.read_with(vcx, |m, cx| {
        m.app_state
            .read(cx)
            .status_message
            .clone()
            .unwrap_or_default()
    });
    assert!(
        status.contains("Saved 'notes.txt'"),
        "expected save confirmation in status bar, got: {status}"
    );

    // Concurrent remote edit after the baseline sync should pause upload on conflict
    // rather than silently overwriting the remote file.
    backend.insert_file("/share/notes.txt", b"concurrent remote edit!!".to_vec());
    fs::write(&cached_path, b"conflicting local edit!").unwrap();

    wait_until(vcx, Duration::from_secs(5), |vcx| {
        main.read_with(vcx, |m, cx| {
            m.app_state
                .read(cx)
                .status_message
                .as_deref()
                .is_some_and(|s| s.contains("Conflict saving 'notes.txt'"))
        })
    });
    assert_eq!(
        backend.read_file("/share/notes.txt").as_deref(),
        Some(b"concurrent remote edit!!".as_slice())
    );
    assert_eq!(fs::read(&cached_path).unwrap(), b"conflicting local edit!");
}

#[gpui::test]
fn opening_remote_video_on_seekable_backend_streams_over_http_range(cx: &mut TestAppContext) {
    let backend = RemoteMemBackend::new(true);
    let video_bytes: Vec<u8> = (0..4096u32).map(|i| (i % 253) as u8).collect();
    backend.insert_file("/share/movie.mkv", video_bytes.clone());
    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/share"),
    };

    let (_main, state, opened, vcx) = open_remote_harness(cx, root.clone(), "srv", backend);

    let movie_loc = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/share/movie.mkv"),
    };
    state.update(vcx, |st, _| open_remote_file(st, movie_loc));
    wait_until(vcx, Duration::from_secs(3), |_| {
        !opened.lock().unwrap().is_empty()
    });

    let url = match opened.lock().unwrap().as_slice() {
        [RemoteOpenTarget::StreamUrl { url, .. }] => url.clone(),
        other => panic!("expected StreamUrl launch, got {other:?}"),
    };
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(url.ends_with("/movie.mkv"));

    // Verify media player HTTP Range request receives exact bytes from remote backend.
    let without_scheme = url.strip_prefix("http://").unwrap();
    let (host_port, req_path) = without_scheme.split_once('/').unwrap();
    let mut tcp = TcpStream::connect(host_port).unwrap();
    let req = format!(
        "GET /{req_path} HTTP/1.1\r\nHost: {host_port}\r\nRange: bytes=512-1023\r\nConnection: close\r\n\r\n"
    );
    tcp.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    tcp.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let hdr = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];
    assert!(hdr.starts_with("HTTP/1.1 206 Partial Content"));
    assert_eq!(body, &video_bytes[512..1024]);
}

#[gpui::test]
fn opening_remote_video_on_non_seekable_backend_falls_back_to_cache(cx: &mut TestAppContext) {
    let backend = RemoteMemBackend::new(false);
    let video_bytes = b"ftps-video-payload".to_vec();
    backend.insert_file("/share/clip.mp4", video_bytes.clone());
    let root = Location::Remote {
        profile_id: "ftp".into(),
        path: PathBuf::from("/share"),
    };

    let (_main, state, opened, vcx) = open_remote_harness(cx, root.clone(), "ftp", backend);
    let clip_loc = Location::Remote {
        profile_id: "ftp".into(),
        path: PathBuf::from("/share/clip.mp4"),
    };
    state.update(vcx, |st, _| open_remote_file(st, clip_loc));
    wait_until(vcx, Duration::from_secs(3), |_| {
        !opened.lock().unwrap().is_empty()
    });

    let cached_path = match opened.lock().unwrap().as_slice() {
        [RemoteOpenTarget::File(path)] => path.clone(),
        other => panic!("expected fallback File launch, got {other:?}"),
    };
    assert_eq!(fs::read(&cached_path).unwrap(), video_bytes);
}

#[gpui::test]
fn preview_panel_loads_remote_text_and_image_files(cx: &mut TestAppContext) {
    let backend = RemoteMemBackend::new(true);
    backend.insert_file("/share/readme.md", b"# Remote Heading\nLine two".to_vec());
    let minimal_png: Vec<u8> = vec![
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    backend.insert_file("/share/photo.png", minimal_png.clone());

    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/share"),
    };
    let (_main, state, _opened, vcx) = open_remote_harness(cx, root.clone(), "srv", backend);

    state.update(vcx, |st, cx| {
        st.panels.preview_open = true;
        st.select_location(Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/share/readme.md"),
        });
        cx.notify();
    });

    vcx.executor().advance_clock(Duration::from_millis(60));
    wait_until(vcx, Duration::from_secs(3), |vcx| {
        state.read_with(vcx, |st, _| {
            matches!(
                st.preview_state.as_ref().map(|p| &p.kind),
                Some(PreviewKind::Text(_))
            )
        })
    });
    state.read_with(vcx, |st, _| {
        let preview = st.preview_state.as_ref().unwrap();
        assert_eq!(
            preview.kind,
            PreviewKind::Text("# Remote Heading\nLine two".to_string())
        );
    });

    state.update(vcx, |st, cx| {
        st.select_location(Location::Remote {
            profile_id: "srv".into(),
            path: PathBuf::from("/share/photo.png"),
        });
        cx.notify();
    });

    vcx.executor().advance_clock(Duration::from_millis(60));
    wait_until(vcx, Duration::from_secs(3), |vcx| {
        state.read_with(vcx, |st, _| {
            matches!(
                st.preview_state.as_ref().map(|p| &p.kind),
                Some(PreviewKind::Image(_))
            )
        })
    });
    state.read_with(vcx, |st, _| {
        let PreviewKind::Image(img_path) = &st.preview_state.as_ref().unwrap().kind else {
            panic!("expected image preview");
        };
        assert_eq!(fs::read(img_path).unwrap(), minimal_png);
    });
}

#[test]
fn stream_launcher_resolves_url_capable_players_and_skips_browsers_for_mkv() {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kwati_desktop_stream_test_{}_{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // `sh` exists in PATH on Unix; use it as the executable in the test desktop files.
    fs::write(
        dir.join("player.desktop"),
        "[Desktop Entry]\nName=Player\nExec=sh -c -- %U\n",
    )
    .unwrap();
    fs::write(
        dir.join("localonly.desktop"),
        "[Desktop Entry]\nName=LocalOnly\nExec=sh %f\n",
    )
    .unwrap();
    fs::write(
        dir.join("firefox.desktop"),
        "[Desktop Entry]\nName=Firefox\nExec=sh %u\n",
    )
    .unwrap();

    let search = vec![dir.clone()];
    // %U player works for mkv
    let mkv_player =
        stream_launcher_from_desktop_id(Path::new("movie.mkv"), "player.desktop", &search);
    assert!(mkv_player.is_some());

    // %f-only player does not support HTTP URLs
    assert!(
        stream_launcher_from_desktop_id(Path::new("movie.mkv"), "localonly.desktop", &search)
            .is_none()
    );

    // Browser is rejected for .mkv, allowed for .mp4
    assert!(
        stream_launcher_from_desktop_id(Path::new("movie.mkv"), "firefox.desktop", &search)
            .is_none()
    );
    assert!(
        stream_launcher_from_desktop_id(Path::new("clip.mp4"), "firefox.desktop", &search)
            .is_some()
    );

    let _ = fs::remove_dir_all(&dir);
}
