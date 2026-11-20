//! The Drives section of the sidebar (#60).

use std::path::PathBuf;

use gpui::{AppContext as _, TestAppContext};
use kwati_core::{Location, Volume};
use kwati_gui::state::AppState;
use kwati_gui::views::sidebar_view::SidebarView;

const GB: u64 = 1024 * 1024 * 1024;

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
