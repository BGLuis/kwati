//! The Drives section of the sidebar (#60, #61).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use gpui::{px, size, AppContext as _, MouseButton, TestAppContext};
use kwati_core::{
    DriveError, Location, NetworkMount, PlaceEntry, Settings, UnmountedDrive, Volume,
};
use kwati_gui::state::AppState;
use kwati_gui::views::sidebar_view::{system_home_dir, DriveBackend, SidebarView};

const GB: u64 = 1024 * 1024 * 1024;

static COUNTER: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "kwati_gui_drives_{label}_{}_{}",
            std::process::id(),
            id
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct MockDriveState {
    mounted: Vec<Volume>,
    unmounted: Result<Vec<UnmountedDrive>, DriveError>,
    network: Vec<NetworkMount>,
    eject_result: Option<Result<(), DriveError>>,
    mount_result: Option<Result<PathBuf, DriveError>>,
    ejected_paths: Vec<PathBuf>,
    mounted_ids: Vec<String>,
    remove_on_eject: bool,
    mounted_volume_on_mount: Option<Volume>,
}

impl Default for MockDriveState {
    fn default() -> Self {
        Self {
            mounted: Vec::new(),
            unmounted: Ok(Vec::new()),
            network: Vec::new(),
            eject_result: None,
            mount_result: None,
            ejected_paths: Vec::new(),
            mounted_ids: Vec::new(),
            remove_on_eject: false,
            mounted_volume_on_mount: None,
        }
    }
}

struct MockDriveBackend {
    state: Mutex<MockDriveState>,
}

impl MockDriveBackend {
    fn new(state: MockDriveState) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }
}

impl DriveBackend for MockDriveBackend {
    fn list_mounted(&self) -> Vec<Volume> {
        self.state.lock().unwrap().mounted.clone()
    }

    fn list_unmounted(&self) -> Result<Vec<UnmountedDrive>, DriveError> {
        self.state.lock().unwrap().unmounted.clone()
    }

    fn list_network(&self) -> Vec<NetworkMount> {
        self.state.lock().unwrap().network.clone()
    }

    fn mount(&self, drive: &UnmountedDrive) -> Result<PathBuf, DriveError> {
        let mut guard = self.state.lock().unwrap();
        guard.mounted_ids.push(drive.id.clone());
        let res = guard
            .mount_result
            .clone()
            .unwrap_or_else(|| Err(DriveError::Backend("no mock mount configured".into())));
        if let Ok(ref mount_point) = res {
            if let Ok(ref mut unmounted) = guard.unmounted {
                unmounted.retain(|d| d.id != drive.id);
            }
            if let Some(vol) = guard.mounted_volume_on_mount.clone() {
                guard.mounted.push(vol);
            } else {
                guard.mounted.push(Volume {
                    name: drive.name.clone(),
                    mount_point: mount_point.clone(),
                    total_space: drive.total_space,
                    available_space: drive.total_space,
                    free_space: drive.total_space,
                    file_system: drive.file_system.clone(),
                    is_removable: true,
                    is_read_only: false,
                });
            }
        }
        res
    }

    fn eject(&self, mount_point: &Path) -> Result<(), DriveError> {
        let mut guard = self.state.lock().unwrap();
        guard.ejected_paths.push(mount_point.to_path_buf());
        let res = guard.eject_result.clone().unwrap_or(Ok(()));
        if res.is_ok() && guard.remove_on_eject {
            guard.mounted.retain(|v| v.mount_point != mount_point);
        }
        res
    }
}

fn removable_volume(name: &str, mount_point: PathBuf, total_gb: u64, free_gb: u64) -> Volume {
    Volume {
        name: name.into(),
        mount_point,
        total_space: total_gb * GB,
        available_space: free_gb * GB,
        free_space: free_gb * GB,
        file_system: "vfat".into(),
        is_removable: true,
        is_read_only: false,
    }
}

fn unmounted_usb(id: &str, name: &str, device: &str, total_gb: u64) -> UnmountedDrive {
    UnmountedDrive {
        id: id.into(),
        drive_id: Some("/org/freedesktop/UDisks2/drives/usb0".into()),
        name: name.into(),
        device: device.into(),
        file_system: "exfat".into(),
        total_space: total_gb * GB,
    }
}

#[gpui::test]
fn refreshing_drives_lists_a_newly_mounted_drive_with_its_free_space(cx: &mut TestAppContext) {
    let state = cx.new(|_| AppState::new(Location::Local(std::env::temp_dir())));
    let sidebar = cx.new(|cx| SidebarView::new(state.clone(), cx));
    // Lets the first poll finish; the next one waits on a timer the test clock never reaches.
    cx.run_until_parked();

    let usb = Volume {
        name: "USB".into(),
        mount_point: PathBuf::from("/media/usb"),
        total_space: 16 * GB,
        available_space: 3 * GB,
        free_space: 3 * GB,
        file_system: "vfat".into(),
        is_removable: true,
        is_read_only: false,
    };
    sidebar.update(cx, |sidebar, cx| {
        sidebar.refresh_devices_with(move || (vec![usb], Vec::new()), cx)
    });
    cx.run_until_parked();

    sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.volumes.len(), 1);
        assert_eq!(sidebar.volumes[0].name, "USB");
        assert_eq!(
            sidebar.volumes[0].capacity_summary(),
            "3.0 GB free of 16.0 GB"
        );
    });
}

#[gpui::test]
fn refreshing_drives_lists_mounted_and_unmounted_and_survives_unmounted_errors(
    cx: &mut TestAppContext,
) {
    let usb_mount = PathBuf::from("/media/usb-data");
    let backend = MockDriveBackend::new(MockDriveState {
        mounted: vec![removable_volume("DATA", usb_mount, 16, 10)],
        unmounted: Ok(vec![unmounted_usb("block/sdb1", "BACKUP", "/dev/sdb1", 64)]),
        ..Default::default()
    });

    let state = cx.new(|_| AppState::new(Location::Local(std::env::temp_dir())));
    let sidebar = cx.new(|cx| SidebarView::with_drive_backend(state.clone(), backend.clone(), cx));
    cx.run_until_parked();

    sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.volumes.len(), 1);
        assert_eq!(sidebar.volumes[0].name, "DATA");
        assert_eq!(sidebar.unmounted_drives.len(), 1);
        assert_eq!(sidebar.unmounted_drives[0].name, "BACKUP");
    });

    // If unmounted_drives() errors, refreshing drives must not panic and still lists mounted volumes.
    {
        let mut guard = backend.state.lock().unwrap();
        guard.unmounted = Err(DriveError::Backend("udisks2 unavailable".into()));
    }
    sidebar.update(cx, |sidebar, cx| sidebar.refresh_devices(cx));
    cx.run_until_parked();

    sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.volumes.len(), 1);
        assert_eq!(sidebar.volumes[0].name, "DATA");
        assert!(sidebar.unmounted_drives.is_empty());
    });
}

#[gpui::test]
fn ejecting_idle_removable_drive_navigates_home_first_removes_drive_and_sets_safe_status(
    cx: &mut TestAppContext,
) {
    let temp = TempDir::new("idle_eject");
    let usb_mount = temp.path().join("usb");
    let dcim = usb_mount.join("DCIM");
    std::fs::create_dir_all(&dcim).unwrap();

    let backend = MockDriveBackend::new(MockDriveState {
        mounted: vec![removable_volume("FLASH_DRIVE", usb_mount.clone(), 32, 24)],
        unmounted: Ok(Vec::new()),
        eject_result: Some(Ok(())),
        remove_on_eject: true,
        ..Default::default()
    });

    let state_holder: Arc<Mutex<Option<gpui::Entity<AppState>>>> = Arc::new(Mutex::new(None));
    let state_holder_clone = Arc::clone(&state_holder);
    let backend_clone = Arc::clone(&backend);

    let (sidebar, cx) = cx.add_window_view(|_, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![PlaceEntry::Trash]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(dcim.clone()));
            s.settings = settings;
            s.attach(cx);
            s.reload_entries();
            s
        });
        *state_holder_clone.lock().unwrap() = Some(state.clone());
        SidebarView::with_drive_backend(state, backend_clone, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let state = state_holder.lock().unwrap().clone().unwrap();
    let eject_bounds = cx
        .debug_bounds("drive-eject-0")
        .expect("eject button bounds for removable mounted drive");

    cx.simulate_mouse_down(eject_bounds.center(), MouseButton::Left, Default::default());
    cx.run_until_parked();

    let expected_home = system_home_dir()
        .filter(|p| p.is_dir())
        .unwrap_or_else(std::env::temp_dir);

    assert_eq!(backend.state.lock().unwrap().ejected_paths, vec![usb_mount]);
    sidebar.read_with(cx, |sidebar, _| {
        assert!(
            sidebar.volumes.is_empty(),
            "ejected drive should be removed from sidebar"
        );
    });
    state.read_with(cx, |state, _| {
        assert_eq!(state.current_location, Location::Local(expected_home));
        assert!(state.status_is_completion);
        let msg = state.status_message.as_deref().unwrap_or("");
        assert!(msg.contains("FLASH_DRIVE"), "status: {msg}");
        assert!(
            msg.to_ascii_lowercase().contains("safe to remove"),
            "status should clearly state safe to remove: {msg}"
        );
    });
}

#[gpui::test]
fn ejecting_busy_drive_keeps_drive_listed_and_shows_busy_reason(cx: &mut TestAppContext) {
    let temp = TempDir::new("busy_eject");
    let usb_mount = temp.path().join("backup_disk");
    std::fs::create_dir_all(&usb_mount).unwrap();

    let busy_reason = "target is busy: open handle on archive.tar";
    let backend = MockDriveBackend::new(MockDriveState {
        mounted: vec![removable_volume("BACKUP_DISK", usb_mount.clone(), 128, 64)],
        unmounted: Ok(Vec::new()),
        eject_result: Some(Err(DriveError::Busy(busy_reason.into()))),
        remove_on_eject: true,
        ..Default::default()
    });

    let state_holder: Arc<Mutex<Option<gpui::Entity<AppState>>>> = Arc::new(Mutex::new(None));
    let state_holder_clone = Arc::clone(&state_holder);
    let backend_clone = Arc::clone(&backend);

    let (sidebar, cx) = cx.add_window_view(|_, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![PlaceEntry::Trash]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(std::env::temp_dir()));
            s.settings = settings;
            s.attach(cx);
            s
        });
        *state_holder_clone.lock().unwrap() = Some(state.clone());
        SidebarView::with_drive_backend(state, backend_clone, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let state = state_holder.lock().unwrap().clone().unwrap();
    let eject_bounds = cx
        .debug_bounds("drive-eject-0")
        .expect("eject button bounds for busy drive");

    cx.simulate_mouse_down(eject_bounds.center(), MouseButton::Left, Default::default());
    cx.run_until_parked();

    sidebar.read_with(cx, |sidebar, _| {
        assert_eq!(sidebar.volumes.len(), 1, "busy drive must remain listed");
        assert_eq!(sidebar.volumes[0].name, "BACKUP_DISK");
        assert!(sidebar.volumes[0].is_busy);
        assert_eq!(sidebar.volumes[0].busy_reason.as_deref(), Some(busy_reason));
    });
    state.read_with(cx, |state, _| {
        assert!(!state.status_is_completion);
        let msg = state.status_message.as_deref().unwrap_or("");
        assert!(
            msg.contains(busy_reason),
            "status bar must include busy reason: {msg}"
        );
    });
}

#[gpui::test]
fn mounting_unmounted_drive_refreshes_free_space_and_navigates_to_contents(
    cx: &mut TestAppContext,
) {
    let temp = TempDir::new("mount_drive");
    let usb_mount = temp.path().join("mounted_usb");
    std::fs::create_dir_all(usb_mount.join("photos")).unwrap();
    std::fs::write(usb_mount.join("readme.txt"), "hello usb").unwrap();

    let unmounted = unmounted_usb(
        "/org/freedesktop/UDisks2/block_devices/sdb1",
        "TRAVEL_USB",
        "/dev/sdb1",
        32,
    );
    let mounted_vol = removable_volume("TRAVEL_USB", usb_mount.clone(), 32, 20);

    let backend = MockDriveBackend::new(MockDriveState {
        mounted: Vec::new(),
        unmounted: Ok(vec![unmounted.clone()]),
        mount_result: Some(Ok(usb_mount.clone())),
        mounted_volume_on_mount: Some(mounted_vol),
        ..Default::default()
    });

    let state_holder: Arc<Mutex<Option<gpui::Entity<AppState>>>> = Arc::new(Mutex::new(None));
    let state_holder_clone = Arc::clone(&state_holder);
    let backend_clone = Arc::clone(&backend);

    let (sidebar, cx) = cx.add_window_view(|_, cx| {
        let mut settings = Settings::default();
        settings.places = Some(vec![PlaceEntry::Trash]);
        let state = cx.new(|cx| {
            let mut s = AppState::new(Location::Local(std::env::temp_dir()));
            s.settings = settings;
            s.attach(cx);
            s
        });
        *state_holder_clone.lock().unwrap() = Some(state.clone());
        SidebarView::with_drive_backend(state, backend_clone, cx)
    });
    cx.simulate_resize(size(px(1200.), px(720.)));
    cx.run_until_parked();

    let state = state_holder.lock().unwrap().clone().unwrap();
    let mount_bounds = cx
        .debug_bounds("drive-mount-0")
        .expect("mount button bounds for unmounted drive");

    cx.simulate_mouse_down(mount_bounds.center(), MouseButton::Left, Default::default());
    cx.run_until_parked();

    assert_eq!(
        backend.state.lock().unwrap().mounted_ids,
        vec![unmounted.id]
    );
    sidebar.read_with(cx, |sidebar, _| {
        assert!(sidebar.unmounted_drives.is_empty());
        assert_eq!(sidebar.volumes.len(), 1);
        assert_eq!(sidebar.volumes[0].name, "TRAVEL_USB");
        assert_eq!(
            sidebar.volumes[0].capacity_summary(),
            "20.0 GB free of 32.0 GB"
        );
    });
    state.read_with(cx, |state, _| {
        assert_eq!(state.current_location, Location::Local(usb_mount.clone()));
        let names: Vec<String> = state.entries.iter().map(|e| e.display_name()).collect();
        assert!(names.contains(&"photos".to_string()), "entries: {names:?}");
        assert!(
            names.contains(&"readme.txt".to_string()),
            "entries: {names:?}"
        );
    });
}

#[gpui::test]
fn mounting_drive_error_keeps_unmounted_drive_and_reports_status(cx: &mut TestAppContext) {
    let unmounted = unmounted_usb(
        "/org/freedesktop/UDisks2/block_devices/sdc1",
        "LOCKED_USB",
        "/dev/sdc1",
        16,
    );
    let backend = MockDriveBackend::new(MockDriveState {
        mounted: Vec::new(),
        unmounted: Ok(vec![unmounted.clone()]),
        mount_result: Some(Err(DriveError::PermissionDenied(
            "Polkit authorization dismissed".into(),
        ))),
        ..Default::default()
    });

    let state = cx.new(|cx| {
        let mut s = AppState::new(Location::Local(std::env::temp_dir()));
        s.attach(cx);
        s
    });
    let sidebar = cx.new(|cx| SidebarView::with_drive_backend(state.clone(), backend, cx));
    cx.run_until_parked();

    sidebar.update(cx, |sidebar, cx| {
        sidebar.mount_drive(unmounted, cx);
    });
    cx.run_until_parked();

    sidebar.read_with(cx, |sidebar, _| {
        assert!(sidebar.volumes.is_empty());
        assert_eq!(sidebar.unmounted_drives.len(), 1);
    });
    state.read_with(cx, |state, _| {
        let msg = state.status_message.as_deref().unwrap_or("");
        assert!(
            msg.contains("Polkit authorization dismissed"),
            "status: {msg}"
        );
    });
}
