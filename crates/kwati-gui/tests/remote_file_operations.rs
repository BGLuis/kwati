use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use kwati_core::{
    Backend, BackendError, Confirmed, ConflictPolicy, Entry, EntryKind, EntryMeta, Listing,
    Location, ReconnectingBackend, RetryPolicy, TrashUnavailable,
};
use kwati_gui::actions::operations::{
    cancel_active_job, check_paste_conflict_at, clear_clipboard, copy_locations_to_clipboard,
    cut_locations_to_clipboard, get_location_clipboard, start_transfer,
    trash_locations_on_backends, Transfer,
};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::AppState;
use kwati_gui::views::dialogs::{confirm_delete_message, DialogKind, DialogsView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrashMode {
    Unavailable,
    GenericError,
    Success,
}

#[derive(Clone)]
struct MemBackend {
    state: Arc<Mutex<MemState>>,
}

type ReadHook = Arc<Mutex<Option<Box<dyn FnOnce() + Send>>>>;

struct MemState {
    dirs: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    trash_mode: TrashMode,
    delete_calls: usize,
    on_second_read: Option<ReadHook>,
}

impl MemBackend {
    fn new(trash_mode: TrashMode) -> Self {
        let mut dirs = BTreeSet::new();
        dirs.insert(PathBuf::from("/"));
        Self {
            state: Arc::new(Mutex::new(MemState {
                dirs,
                files: BTreeMap::new(),
                trash_mode,
                delete_calls: 0,
                on_second_read: None,
            })),
        }
    }

    fn insert_dir(&self, path: impl AsRef<Path>) {
        let mut st = self.state.lock().unwrap();
        let p = path.as_ref().to_path_buf();
        let mut cur = Some(p.as_path());
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

    fn has_file(&self, path: impl AsRef<Path>) -> bool {
        self.state.lock().unwrap().files.contains_key(path.as_ref())
    }

    fn has_dir(&self, path: impl AsRef<Path>) -> bool {
        self.state.lock().unwrap().dirs.contains(path.as_ref())
    }

    fn read_file(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(path.as_ref()).cloned()
    }

    fn delete_calls(&self) -> usize {
        self.state.lock().unwrap().delete_calls
    }
}

struct MemWriter {
    path: PathBuf,
    buf: Vec<u8>,
    state: Arc<Mutex<MemState>>,
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

struct PausedReader {
    inner: Cursor<Vec<u8>>,
    first_chunk_sent: bool,
    on_second_read: Option<Box<dyn FnOnce() + Send>>,
}

impl Read for PausedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if !self.first_chunk_sent {
            self.first_chunk_sent = true;
            let cap = buf.len().min(4096);
            return self.inner.read(&mut buf[..cap]);
        }
        if let Some(cb) = self.on_second_read.take() {
            cb();
        }
        self.inner.read(buf)
    }
}

impl Backend for MemBackend {
    fn list(&self, location: &Location) -> Result<Listing, BackendError> {
        let dir = location.path();
        let st = self.state.lock().unwrap();
        if !st.dirs.contains(dir) {
            return Err(io::Error::new(io::ErrorKind::NotFound, "dir not found").into());
        }
        let mut entries = Vec::new();
        for d in &st.dirs {
            if d != dir && d.parent() == Some(dir) {
                if let Some(name) = d.file_name() {
                    entries.push(Entry::new(OsString::from(name), d.clone(), true));
                }
            }
        }
        for (f, data) in &st.files {
            if f.parent() == Some(dir) {
                if let Some(name) = f.file_name() {
                    entries.push(
                        Entry::new(OsString::from(name), f.clone(), false)
                            .with_size(data.len() as u64),
                    );
                }
            }
        }
        Ok(Listing::new(entries))
    }

    fn stat(&self, location: &Location) -> Result<EntryMeta, BackendError> {
        let path = location.path();
        let st = self.state.lock().unwrap();
        if st.dirs.contains(path) {
            Ok(EntryMeta {
                kind: EntryKind::Dir,
                len: 0,
            })
        } else if let Some(data) = st.files.get(path) {
            Ok(EntryMeta {
                kind: EntryKind::File,
                len: data.len() as u64,
            })
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "not found").into())
        }
    }

    fn open_read(&self, location: &Location) -> Result<Box<dyn Read + Send>, BackendError> {
        let st = self.state.lock().unwrap();
        let data = st
            .files
            .get(location.path())
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "file not found"))?;
        let on_second_read = st
            .on_second_read
            .as_ref()
            .and_then(|slot| slot.lock().unwrap().take());
        Ok(Box::new(PausedReader {
            inner: Cursor::new(data),
            first_chunk_sent: false,
            on_second_read,
        }))
    }

    fn open_write(&self, location: &Location) -> Result<Box<dyn Write + Send>, BackendError> {
        let path = location.path().to_path_buf();
        self.state
            .lock()
            .unwrap()
            .files
            .insert(path.clone(), Vec::new());
        Ok(Box::new(MemWriter {
            path,
            buf: Vec::new(),
            state: self.state.clone(),
        }))
    }

    fn rename(&self, from: &Location, to: &Location) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        let from_p = from.path();
        let to_p = to.path();
        if let Some(data) = st.files.remove(from_p) {
            st.files.insert(to_p.to_path_buf(), data);
            return Ok(());
        }
        if st.dirs.contains(from_p) {
            let moved_dirs: Vec<PathBuf> = st
                .dirs
                .iter()
                .filter(|d| *d == from_p || d.starts_with(from_p))
                .cloned()
                .collect();
            for d in &moved_dirs {
                st.dirs.remove(d);
            }
            for d in moved_dirs {
                let rel = d.strip_prefix(from_p).unwrap();
                st.dirs.insert(to_p.join(rel));
            }
            let moved_files: Vec<(PathBuf, Vec<u8>)> = st
                .files
                .iter()
                .filter(|(f, _)| f.starts_with(from_p))
                .map(|(f, data)| (f.clone(), data.clone()))
                .collect();
            for (f, _) in &moved_files {
                st.files.remove(f);
            }
            for (f, data) in moved_files {
                let rel = f.strip_prefix(from_p).unwrap();
                st.files.insert(to_p.join(rel), data);
            }
            return Ok(());
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "source not found").into())
    }

    fn create_dir(&self, location: &Location) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        let p = location.path().to_path_buf();
        if st.dirs.contains(&p) || st.files.contains_key(&p) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "already exists").into());
        }
        st.dirs.insert(p);
        Ok(())
    }

    fn delete_permanently(
        &self,
        location: &Location,
        _confirmed: Confirmed,
    ) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        st.delete_calls += 1;
        let p = location.path();
        if st.files.remove(p).is_some() {
            return Ok(());
        }
        if st.dirs.contains(p) {
            st.dirs.retain(|d| d != p && !d.starts_with(p));
            st.files.retain(|f, _| !f.starts_with(p));
            return Ok(());
        }
        Err(io::Error::new(io::ErrorKind::NotFound, "not found").into())
    }

    fn trash(&self, location: &Location) -> Result<(), BackendError> {
        let mut st = self.state.lock().unwrap();
        match st.trash_mode {
            TrashMode::Unavailable => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                TrashUnavailable {
                    path: location.path().to_path_buf(),
                },
            )
            .into()),
            TrashMode::GenericError => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "trash permission denied",
            )
            .into()),
            TrashMode::Success => {
                if st.files.remove(location.path()).is_some() {
                    Ok(())
                } else {
                    Err(io::Error::new(io::ErrorKind::NotFound, "not found").into())
                }
            }
        }
    }
}

fn attach_remote(state: &mut AppState, profile_id: &str, backend: MemBackend) {
    let reconnecting = ReconnectingBackend::new(
        move || Ok(Box::new(backend.clone()) as Box<dyn Backend>),
        RetryPolicy::new(std::time::Duration::from_millis(1), 1),
    )
    .unwrap();
    state.remote.add_connection(profile_id, reconnecting);
}

fn open_dialogs_harness<'a>(
    cx: &'a mut TestAppContext,
    initial: Location,
    remotes: Vec<(&str, MemBackend)>,
) -> (
    Entity<AppState>,
    Entity<DialogsView>,
    &'a mut VisualTestContext,
) {
    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });
    let mut state_ref = None;
    let (dialogs, vcx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut st = AppState::new(initial);
            for (id, backend) in remotes {
                attach_remote(&mut st, id, backend);
            }
            st.attach(cx);
            st.reload_entries();
            st
        });
        state_ref = Some(state.clone());
        let dialogs = DialogsView::new(state, cx);
        let _ = window;
        dialogs
    });
    vcx.run_until_parked();
    (state_ref.unwrap(), dialogs, vcx)
}

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(1);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "kwati_test_remote_ops_{name}_{}_{id}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[gpui::test]
fn remote_create_folder_and_rename_via_dialogs(cx: &mut TestAppContext) {
    let remote = MemBackend::new(TrashMode::Unavailable);
    remote.insert_dir("/workspace");
    remote.insert_file("/workspace/report.txt", b"hello".to_vec());
    remote.insert_file("/workspace/existing.txt", b"old".to_vec());

    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/workspace"),
    };
    let (state, dialogs, vcx) =
        open_dialogs_harness(cx, root.clone(), vec![("srv", remote.clone())]);

    // 1. Create remote folder
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_new_folder(root.clone(), cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("docs");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert!(remote.has_dir("/workspace/docs"));
    state.read_with(vcx, |st, _| {
        assert!(st.entries.iter().any(|e| e.name == "docs" && e.is_dir));
        assert!(st.undo_stack.is_empty());
    });

    // 2. Rename remote file
    let src = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/workspace/report.txt"),
    };
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_rename(src.clone(), cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("final.txt");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert!(!remote.has_file("/workspace/report.txt"));
    assert!(remote.has_file("/workspace/final.txt"));
    state.read_with(vcx, |st, _| {
        assert!(st.entries.iter().any(|e| e.name == "final.txt"));
        assert!(st.undo_stack.is_empty());
    });

    // 3. Refuses renaming onto an existing remote file
    let final_loc = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/workspace/final.txt"),
    };
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_rename(final_loc, cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("ctrl-a");
    vcx.simulate_input("existing.txt");
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    let DialogKind::Rename { error, .. } = dialogs.read_with(vcx, |d, _| d.current_dialog.clone())
    else {
        panic!("expected rename dialog to stay open with error");
    };
    assert!(error.is_some());
    assert!(remote.has_file("/workspace/final.txt"));
    assert_eq!(remote.read_file("/workspace/existing.txt").unwrap(), b"old");
}

#[gpui::test]
fn trash_unavailable_falls_back_to_confirm_delete_and_only_button_confirms(
    cx: &mut TestAppContext,
) {
    let remote = MemBackend::new(TrashMode::Unavailable);
    remote.insert_dir("/share");
    remote.insert_file("/share/alpha.txt", b"a".to_vec());
    remote.insert_file("/share/beta.txt", b"b".to_vec());

    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/share"),
    };
    let alpha = root.join(std::ffi::OsStr::new("alpha.txt"));
    let beta = root.join(std::ffi::OsStr::new("beta.txt"));
    let (state, dialogs, vcx) = open_dialogs_harness(cx, root, vec![("srv", remote.clone())]);

    // Step 1: When Trash is attempted on a location whose backend returns TrashUnavailable,
    // confirming Trash opens the second ConfirmDelete dialog naming the items.
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_confirm_trash(vec![alpha.clone(), beta.clone()], cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    let current = dialogs.read_with(vcx, |d, _| d.current_dialog.clone());
    let DialogKind::ConfirmDelete { locations } = current else {
        panic!("expected fallback to ConfirmDelete, got {current:?}");
    };
    assert_eq!(locations, vec![alpha.clone(), beta.clone()]);
    let msg = confirm_delete_message(&locations);
    assert!(msg.contains("alpha.txt") && msg.contains("beta.txt"));
    assert!(msg.contains("cannot be undone"));
    assert_eq!(remote.delete_calls(), 0);

    // Step 2: Pressing Enter must NEVER confirm permanent deletion.
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();
    assert!(matches!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::ConfirmDelete { .. }
    ));
    assert_eq!(remote.delete_calls(), 0);
    assert!(remote.has_file("/share/alpha.txt"));

    // Step 3: Escape cancels without deleting.
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert!(remote.has_file("/share/alpha.txt"));
    assert!(remote.has_file("/share/beta.txt"));

    // Step 4: Remote removal helper opens ConfirmDelete directly; clicking confirmation button
    // constructs Confirmed::user_confirmed() and deletes on background executor.
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_remove_confirmation(vec![alpha.clone(), beta.clone()], cx);
        dialogs.focus(window, cx);
    });
    assert!(matches!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::ConfirmDelete { .. }
    ));

    dialogs.update(vcx, |dialogs, cx| {
        dialogs.confirm_permanent_delete(cx);
    });
    vcx.run_until_parked();

    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert_eq!(remote.delete_calls(), 2);
    assert!(!remote.has_file("/share/alpha.txt"));
    assert!(!remote.has_file("/share/beta.txt"));
    state.read_with(vcx, |st, _| {
        assert!(st.entries.is_empty());
        assert!(st.undo_stack.is_empty());
    });
}

#[gpui::test]
fn generic_trash_error_never_offers_permanent_deletion(cx: &mut TestAppContext) {
    let backend = MemBackend::new(TrashMode::GenericError);
    backend.insert_dir("/dir");
    backend.insert_file("/dir/secret.txt", b"keep".to_vec());

    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/dir"),
    };
    let secret = root.join(std::ffi::OsStr::new("secret.txt"));
    let (state, dialogs, vcx) = open_dialogs_harness(cx, root, vec![("srv", backend.clone())]);

    // Also test backend helper directly with a local location on a mock backend
    let local_res = trash_locations_on_backends(&[(
        Location::Local(PathBuf::from("/fake/local.txt")),
        Arc::new(backend.clone()),
    )]);
    assert!(local_res.error.is_some());
    assert!(local_res.unavailable.is_empty());

    // Through DialogsView: ConfirmTrash -> generic error -> closes dialog, reports status,
    // never opens ConfirmDelete, never calls delete_permanently.
    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_confirm_trash(vec![secret], cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert_eq!(backend.delete_calls(), 0);
    assert!(backend.has_file("/dir/secret.txt"));
    state.read_with(vcx, |st, _| {
        let msg = st.status_message.as_deref().unwrap_or("");
        assert!(msg.contains("Failed to move to trash"));
    });
}

#[gpui::test]
fn trash_success_via_dialogs_reloads_and_closes(cx: &mut TestAppContext) {
    let backend = MemBackend::new(TrashMode::Success);
    backend.insert_dir("/dir");
    backend.insert_file("/dir/disposable.txt", b"bye".to_vec());

    let root = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/dir"),
    };
    let disposable = root.join(std::ffi::OsStr::new("disposable.txt"));
    let (state, dialogs, vcx) = open_dialogs_harness(cx, root, vec![("srv", backend.clone())]);

    dialogs.update_in(vcx, |dialogs, window, cx| {
        dialogs.open_confirm_trash(vec![disposable], cx);
        dialogs.focus(window, cx);
    });
    vcx.simulate_keystrokes("enter");
    vcx.run_until_parked();

    assert_eq!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::None
    );
    assert_eq!(backend.delete_calls(), 0);
    assert!(!backend.has_file("/dir/disposable.txt"));
    state.read_with(vcx, |st, _| {
        assert!(st.entries.is_empty());
        assert!(st
            .status_message
            .as_deref()
            .unwrap_or("")
            .contains("Moved 1 item(s) to trash"));
    });
}

#[gpui::test]
fn cross_location_copy_and_move_local_and_remote(cx: &mut TestAppContext) {
    clear_clipboard();
    let local_dir = temp_dir("cross_copy");
    let src_folder = local_dir.join("bundle");
    fs::create_dir_all(&src_folder).unwrap();
    fs::write(src_folder.join("readme.txt"), "local-to-remote").unwrap();

    let remote_a = MemBackend::new(TrashMode::Unavailable);
    remote_a.insert_dir("/a");
    let remote_b = MemBackend::new(TrashMode::Unavailable);
    remote_b.insert_dir("/b");

    let remote_a_dir = Location::Remote {
        profile_id: "a".into(),
        path: PathBuf::from("/a"),
    };
    let remote_b_dir = Location::Remote {
        profile_id: "b".into(),
        path: PathBuf::from("/b"),
    };

    let (state, dialogs, vcx) = open_dialogs_harness(
        cx,
        remote_a_dir.clone(),
        vec![("a", remote_a.clone()), ("b", remote_b.clone())],
    );

    // 1. Local -> Remote folder copy
    copy_locations_to_clipboard(vec![Location::Local(src_folder.clone())]);
    let cb = get_location_clipboard().unwrap();
    assert!(!cb.is_cut);

    state.update(vcx, |st, _| {
        let transfer = Transfer::copy(
            cb.sources.clone(),
            remote_a_dir.clone(),
            ConflictPolicy::Replace,
        );
        start_transfer(st, transfer);
    });
    vcx.run_until_parked();

    assert!(remote_a.has_dir("/a/bundle"));
    assert_eq!(
        remote_a.read_file("/a/bundle/readme.txt").unwrap(),
        b"local-to-remote"
    );
    assert!(src_folder.join("readme.txt").exists());
    state.read_with(vcx, |st, _| {
        assert!(st.active_jobs.is_empty());
        assert!(st.entries.iter().any(|e| e.name == "bundle" && e.is_dir));
    });

    // 2. Conflict check + resolution on Remote -> Remote move (profile a -> profile b)
    remote_b.insert_file("/b/readme.txt", b"existing-on-b".to_vec());
    let src_on_a = Location::Remote {
        profile_id: "a".into(),
        path: PathBuf::from("/a/bundle/readme.txt"),
    };
    let conflict =
        check_paste_conflict_at(std::slice::from_ref(&src_on_a), &remote_b_dir, &remote_b).unwrap();
    assert_eq!(conflict.as_deref(), Some("readme.txt"));

    cut_locations_to_clipboard(vec![src_on_a.clone()]);
    dialogs.update(vcx, |dialogs, cx| {
        dialogs.begin_paste(vec![src_on_a], remote_b_dir.clone(), true, cx);
    });
    vcx.run_until_parked();

    assert!(matches!(
        dialogs.read_with(vcx, |d, _| d.current_dialog.clone()),
        DialogKind::ConflictResolution { .. }
    ));

    dialogs.update(vcx, |dialogs, cx| {
        dialogs.resolve_conflict(ConflictPolicy::KeepBoth, cx);
    });
    vcx.run_until_parked();

    assert!(!remote_a.has_file("/a/bundle/readme.txt"));
    assert_eq!(
        remote_b.read_file("/b/readme.txt").unwrap(),
        b"existing-on-b"
    );
    assert_eq!(
        remote_b.read_file("/b/readme (1).txt").unwrap(),
        b"local-to-remote"
    );
    assert!(get_location_clipboard().is_none());

    let _ = fs::remove_dir_all(&local_dir);
}

#[gpui::test]
fn cancelling_remote_to_local_copy_leaves_no_partial_file(cx: &mut TestAppContext) {
    let local_dest = temp_dir("cancel_r2l");
    let remote = MemBackend::new(TrashMode::Unavailable);
    remote.insert_dir("/srv");
    // Larger than one 4KB first read so transfer enters a second read mid-stream
    remote.insert_file("/srv/big.bin", vec![0xAB; 256 * 1024]);

    let saw_partial = Arc::new(AtomicBool::new(false));
    let job_slot = Arc::new(Mutex::new(None::<u64>));
    {
        let saw_partial = saw_partial.clone();
        let job_slot = job_slot.clone();
        let local_dest_check = local_dest.clone();
        let mut st = remote.state.lock().unwrap();
        st.on_second_read = Some(Arc::new(Mutex::new(Some(Box::new(move || {
            let has_partial = fs::read_dir(&local_dest_check)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|e| e.file_name().to_string_lossy().contains(".kwati-partial"));
            saw_partial.store(has_partial, Ordering::Relaxed);
            if let Some(job_id) = *job_slot.lock().unwrap() {
                cancel_active_job(job_id);
            }
        })))));
    }

    let src = Location::Remote {
        profile_id: "srv".into(),
        path: PathBuf::from("/srv/big.bin"),
    };
    let dst = Location::Local(local_dest.clone());
    let (state, _dialogs, vcx) = open_dialogs_harness(cx, dst.clone(), vec![("srv", remote)]);

    let job_id = state.update(vcx, |st, _| {
        let transfer = Transfer::copy(vec![src], dst.clone(), ConflictPolicy::Replace);
        start_transfer(st, transfer);
        assert_eq!(st.active_jobs.len(), 1);
        st.active_jobs[0].id
    });
    *job_slot.lock().unwrap() = Some(job_id);

    vcx.run_until_parked();

    assert!(
        saw_partial.load(Ordering::Relaxed),
        "transfer should have created a .kwati-partial file before cancellation"
    );

    // Neither the final file nor any .kwati-partial file may remain
    let entries: Vec<String> = fs::read_dir(&local_dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        entries.is_empty(),
        "expected no leftover or .kwati-partial files, found: {entries:?}"
    );
    state.read_with(vcx, |st, _| {
        assert!(st.active_jobs.is_empty());
        assert!(st
            .status_message
            .as_deref()
            .unwrap_or("")
            .contains("cancelled"));
    });

    let _ = fs::remove_dir_all(&local_dest);
}
