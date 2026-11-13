use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{AppContext as _, TestAppContext};
use kwati_core::{Entry, Location};
use kwati_gui::actions::register_keybindings;
use kwati_gui::components::register_text_input_keybindings;
use kwati_gui::state::{AppState, PreviewKind};
use kwati_gui::views::preview_view::{
    file_kind_label, format_file_size, generate_preview_for, is_image_path, prepare_preview_lines,
};
use kwati_gui::views::MainView;

#[test]
fn format_file_size_units() {
    assert_eq!(format_file_size(0), "0 B");
    assert_eq!(format_file_size(512), "512 B");
    assert_eq!(format_file_size(1024), "1.0 KB");
    assert_eq!(format_file_size(2048), "2.0 KB");
    assert_eq!(format_file_size(1024 * 1024), "1.0 MB");
    assert_eq!(format_file_size(1024 * 1024 * 1024), "1.00 GB");
}

#[test]
fn is_image_path_extensions() {
    assert!(is_image_path(&PathBuf::from("photo.png")));
    assert!(is_image_path(&PathBuf::from("photo.PNG")));
    assert!(is_image_path(&PathBuf::from("/path/to/img.jpg")));
    assert!(is_image_path(&PathBuf::from("vector.svg")));
    assert!(is_image_path(&PathBuf::from("anim.gif")));
    assert!(is_image_path(&PathBuf::from("picture.webp")));

    assert!(!is_image_path(&PathBuf::from("document.txt")));
    assert!(!is_image_path(&PathBuf::from("code.rs")));
    assert!(!is_image_path(&PathBuf::from("program.bin")));
}

#[test]
fn file_kind_label_detection() {
    assert_eq!(
        file_kind_label(&PathBuf::from("dir"), true, false),
        "Folder"
    );
    assert_eq!(
        file_kind_label(&PathBuf::from("test.png"), false, false),
        "Image"
    );
    assert_eq!(
        file_kind_label(&PathBuf::from("code.rs"), false, false),
        "Rust Source File"
    );
    assert_eq!(
        file_kind_label(&PathBuf::from("README.md"), false, false),
        "Markdown Document"
    );
    assert_eq!(
        file_kind_label(&PathBuf::from("data.bin"), false, true),
        "Binary File"
    );
}

#[test]
fn preview_png_image_shows_image() {
    let temp_dir = std::env::temp_dir().join("kwati_preview_test_png");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let png_path = temp_dir.join("sample.png");
    // Minimal valid 1x1 PNG bytes
    let minimal_png: [u8; 67] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    let mut file = File::create(&png_path).unwrap();
    file.write_all(&minimal_png).unwrap();

    let (preview_data, summary) = generate_preview_for(&png_path);
    assert!(matches!(preview_data.kind, PreviewKind::Image(ref p) if p == &png_path));
    assert_eq!(summary.name, "sample.png");
    assert_eq!(summary.size, minimal_png.len() as u64);
    assert!(summary.is_image);
    assert!(!summary.is_binary);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn preview_text_file_bounded() {
    let temp_dir = std::env::temp_dir().join("kwati_preview_test_text");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let text_path = temp_dir.join("sample.txt");
    let content = "Hello Kwati File Explorer\nThis is a bounded preview test.";
    let mut file = File::create(&text_path).unwrap();
    file.write_all(content.as_bytes()).unwrap();

    let (preview_data, summary) = generate_preview_for(&text_path);
    match preview_data.kind {
        PreviewKind::Text(ref text) => {
            assert_eq!(text, content);
        }
        other => panic!("expected PreviewKind::Text, got {:?}", other),
    }
    assert_eq!(summary.name, "sample.txt");
    assert_eq!(summary.size, content.len() as u64);
    assert!(!summary.is_image);
    assert!(!summary.is_binary);

    // Large file test: truncated to 65,536 bytes
    let large_path = temp_dir.join("large.txt");
    let large_content = "A".repeat(100_000);
    let mut large_file = File::create(&large_path).unwrap();
    large_file.write_all(large_content.as_bytes()).unwrap();

    let (large_preview, large_summary) = generate_preview_for(&large_path);
    match large_preview.kind {
        PreviewKind::Text(ref text) => {
            assert_eq!(text.len(), 65_536);
        }
        other => panic!("expected PreviewKind::Text, got {:?}", other),
    }
    assert_eq!(large_summary.size, 100_000);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn preview_binary_file_shows_name_and_size_only() {
    let temp_dir = std::env::temp_dir().join("kwati_preview_test_binary");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let bin_path = temp_dir.join("program.bin");
    let binary_bytes = vec![0x7f, 0x45, 0x4c, 0x46, 0x00, 0x01, 0x02, 0xff];
    let mut file = File::create(&bin_path).unwrap();
    file.write_all(&binary_bytes).unwrap();

    let (preview_data, summary) = generate_preview_for(&bin_path);
    match preview_data.kind {
        PreviewKind::Binary { ref name, size } => {
            assert_eq!(name, "program.bin");
            assert_eq!(size, binary_bytes.len() as u64);
        }
        other => panic!("expected PreviewKind::Binary, got {:?}", other),
    }
    assert_eq!(summary.name, "program.bin");
    assert_eq!(summary.size, binary_bytes.len() as u64);
    assert!(summary.is_binary);
    assert!(!summary.is_image);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn preview_replacement_and_navigation() {
    let temp_dir = std::env::temp_dir().join("kwati_preview_test_replace");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let png_path = temp_dir.join("image.png");
    fs::write(&png_path, b"fake png").unwrap();
    let txt_path = temp_dir.join("notes.txt");
    fs::write(&txt_path, b"hello notes").unwrap();

    let mut state = AppState::new(Location::Local(temp_dir.clone()));
    let entry_png = Entry::new("image.png".into(), png_path.clone(), false);
    let entry_txt = Entry::new("notes.txt".into(), txt_path.clone(), false);
    state.set_entries(vec![entry_png.clone(), entry_txt.clone()]);

    // Select PNG
    state.selection.select_only(entry_png.location());
    let (png_preview, _) = generate_preview_for(&png_path);
    state.set_preview(Some(png_preview));
    assert!(matches!(
        state.preview_state.as_ref().unwrap().kind,
        PreviewKind::Image(_)
    ));

    // Move to text file replaces preview
    state.selection.select_only(entry_txt.location());
    let (txt_preview, _) = generate_preview_for(&txt_path);
    state.set_preview(Some(txt_preview));
    assert!(matches!(
        state.preview_state.as_ref().unwrap().kind,
        PreviewKind::Text(_)
    ));

    // Navigating to new location clears preview
    let other_dir = temp_dir.join("sub");
    fs::create_dir_all(&other_dir).unwrap();
    state.navigate_to(Location::Local(other_dir));
    assert!(state.preview_state.is_none());

    let _ = fs::remove_dir_all(&temp_dir);
}

use kwati_core::{SearchHit, SearchOptions, Searcher};

#[test]
fn search_recursive_ranking() {
    let temp_dir = std::env::temp_dir().join("kwati_search_test_rank");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let readme = temp_dir.join("README.md");
    let random = temp_dir.join("random.md");
    fs::write(&readme, b"docs").unwrap();
    fs::write(&random, b"other").unwrap();

    let mut searcher = Searcher::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = searcher.search(&temp_dir, "rdm", SearchOptions::default(), move |hit| {
        let _ = tx.send(hit);
    });
    handle.join();

    let mut hits = Vec::new();
    while let Ok(hit) = rx.try_recv() {
        hits.push(hit);
    }
    hits.sort_by_key(|b| std::cmp::Reverse(b.score));

    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].path, readme);
    assert!(hits[0].score > hits[1].score);

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn search_ignore_toggle_finds_ignored_file() {
    let temp_dir = std::env::temp_dir().join("kwati_search_test_ignore");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let gitignore = temp_dir.join(".gitignore");
    fs::write(&gitignore, b"ignored.txt\n").unwrap();
    let ignored_file = temp_dir.join("ignored.txt");
    fs::write(&ignored_file, b"secret").unwrap();
    let regular_file = temp_dir.join("regular.txt");
    fs::write(&regular_file, b"public").unwrap();

    // 1. With respect_ignore = true (default)
    let mut searcher = Searcher::new();
    let (tx1, rx1) = std::sync::mpsc::channel();
    let handle1 = searcher.search(
        &temp_dir,
        "txt",
        SearchOptions {
            respect_ignore: true,
        },
        move |hit| {
            let _ = tx1.send(hit);
        },
    );
    handle1.join();

    let mut hits1 = Vec::new();
    while let Ok(hit) = rx1.try_recv() {
        hits1.push(hit);
    }
    assert_eq!(hits1.len(), 1);
    assert_eq!(hits1[0].path, regular_file);

    // 2. With respect_ignore = false (toggle OFF)
    let (tx2, rx2) = std::sync::mpsc::channel();
    let handle2 = searcher.search(
        &temp_dir,
        "txt",
        SearchOptions {
            respect_ignore: false,
        },
        move |hit| {
            let _ = tx2.send(hit);
        },
    );
    handle2.join();

    let mut hits2 = Vec::new();
    while let Ok(hit) = rx2.try_recv() {
        hits2.push(hit);
    }
    assert!(hits2.len() >= 2);
    let paths: Vec<PathBuf> = hits2.into_iter().map(|h| h.path).collect();
    assert!(paths.contains(&ignored_file));
    assert!(paths.contains(&regular_file));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn search_navigation_and_jump_to_result() {
    let temp_dir = std::env::temp_dir().join("kwati_search_test_jump");
    let _ = fs::remove_dir_all(&temp_dir);
    let nested_dir = temp_dir.join("sub").join("nested");
    fs::create_dir_all(&nested_dir).unwrap();

    let target_file = nested_dir.join("target.rs");
    fs::write(&target_file, b"fn main() {}").unwrap();

    let mut state = AppState::new(Location::Local(temp_dir.clone()));

    // Simulate search hit
    let hit = SearchHit {
        path: target_file.clone(),
        score: 100,
    };

    // Jumping to hit opens its folder and selects the file
    let parent = hit.path.parent().unwrap().to_path_buf();
    state.navigate_to(Location::Local(parent.clone()));
    state
        .selection
        .select_only(Location::Local(hit.path.clone()));

    assert_eq!(state.current_location, Location::Local(nested_dir.clone()));
    assert_eq!(state.selection.len(), 1);
    assert!(state
        .selection
        .contains(&Location::Local(target_file.clone())));

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn search_cancellation_on_directory_change() {
    let temp_dir = std::env::temp_dir().join("kwati_search_test_cancel");
    let _ = fs::remove_dir_all(&temp_dir);
    let dir_a = temp_dir.join("dir_a");
    let dir_b = temp_dir.join("dir_b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();

    let file_a = dir_a.join("file_a.txt");
    fs::write(&file_a, b"content").unwrap();

    let mut searcher = Searcher::new();
    let handle = searcher.search(&dir_a, "file", SearchOptions::default(), |_| {});

    // Navigating to dir_b stops the running search
    handle.stop();
    handle.join();

    let _ = fs::remove_dir_all(&temp_dir);
}

use kwati_gui::actions::open_with::{
    applications_for, launch_application, launch_custom_command, which_in_path, AppInfo,
};
use kwati_gui::views::dialogs::DialogKind;

#[test]
fn open_with_dialog_kind_state() {
    let target = PathBuf::from("document.pdf");
    let apps = applications_for(&target);
    let dialog = DialogKind::OpenWith {
        target_path: target.clone(),
        available_apps: apps.clone(),
        selected_index: Some(0),
        error: None,
    };

    if let DialogKind::OpenWith {
        target_path,
        available_apps,
        selected_index,
        error,
    } = dialog
    {
        assert_eq!(target_path, target);
        assert_eq!(available_apps.len(), apps.len());
        assert_eq!(selected_index, Some(0));
        assert!(error.is_none());
    } else {
        panic!("expected DialogKind::OpenWith");
    }
}

#[test]
fn open_with_png_lists_image_applications_default_first() {
    let png_path = PathBuf::from("picture.png");
    let apps = applications_for(&png_path);

    assert!(!apps.is_empty());
    // Default application must be first
    assert!(apps[0].is_default);
    // Should include image-specific application
    let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
    assert!(names
        .iter()
        .any(|n| n.contains("Image") || n.contains("Viewer")));

    // At least one non-default application is present
    let non_default = apps.iter().find(|a| !a.is_default);
    assert!(non_default.is_some());
}

#[test]
fn open_with_missing_app_returns_error() {
    let missing_app = AppInfo::new(
        "Ghost Viewer",
        "ghost_viewer_binary_that_does_not_exist",
        false,
    );
    let target = PathBuf::from("test.png");

    let result = launch_application(&missing_app, &target);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn open_with_custom_command_missing_executable_returns_error() {
    let target = PathBuf::from("test.txt");
    let result = launch_custom_command("nonexistent_executable_12345 %f", &target);
    assert!(result.is_err());
    assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn open_with_custom_command_launches_executable() {
    let temp_dir = std::env::temp_dir().join("kwati_openwith_custom_test");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let file_path = temp_dir.join("input.txt");
    fs::write(&file_path, b"data").unwrap();

    // "echo" or "true" or "cat" is always installed on Linux/Unix systems
    #[cfg(unix)]
    {
        let result = launch_custom_command("true", &file_path);
        assert!(result.is_ok());
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn which_in_path_finds_standard_utilities() {
    #[cfg(unix)]
    {
        assert!(which_in_path("ls") || which_in_path("sh"));
        assert!(!which_in_path("nonexistent_binary_xyz_123456789"));
    }
}

#[test]
fn open_with_desktop_file_discovery_in_xdg_data_home() {
    let temp_dir = std::env::temp_dir().join("kwati_xdg_test");
    let _ = fs::remove_dir_all(&temp_dir);
    let apps_dir = temp_dir.join("applications");
    fs::create_dir_all(&apps_dir).unwrap();

    let desktop_content = "[Desktop Entry]\nType=Application\nName=CustomPhotoEditor\nExec=custom-editor %f\nMimeType=image/png;\n";
    let desktop_path = apps_dir.join("custom-photo.desktop");
    fs::write(&desktop_path, desktop_content).unwrap();

    // Temporarily point XDG_DATA_HOME to temp_dir
    let old_xdg = std::env::var_os("XDG_DATA_HOME");
    std::env::set_var("XDG_DATA_HOME", &temp_dir);

    let apps = applications_for(&PathBuf::from("sample.png"));
    let found = apps.iter().any(|a| a.name == "CustomPhotoEditor");

    // Restore env var
    if let Some(old) = old_xdg {
        std::env::set_var("XDG_DATA_HOME", old);
    } else {
        std::env::remove_var("XDG_DATA_HOME");
    }

    assert!(
        found,
        "Should have discovered CustomPhotoEditor in XDG_DATA_HOME"
    );
    let _ = fs::remove_dir_all(&temp_dir);
}

#[test]
fn prepare_preview_lines_wraps_long_lines_and_handles_tabs_and_crlf() {
    assert!(prepare_preview_lines("").is_empty());

    let lines = prepare_preview_lines("first\r\n\tsecond\n");
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].as_ref(), "first");
    assert_eq!(lines[1].as_ref(), "    second");

    // Single 600-char UTF-8 line (multibyte characters) splits cleanly at 200 chars per visual line
    let long_line = "á".repeat(450);
    let wrapped = prepare_preview_lines(&long_line);
    assert_eq!(wrapped.len(), 3);
    assert_eq!(wrapped[0].chars().count(), 200);
    assert_eq!(wrapped[1].chars().count(), 200);
    assert_eq!(wrapped[2].chars().count(), 50);
}

#[gpui::test]
fn preview_loads_asynchronously_cancels_stale_and_skips_when_closed(cx: &mut TestAppContext) {
    let temp_dir = std::env::temp_dir().join(format!("kwati_preview_async_{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).unwrap();

    let first_path = temp_dir.join("alpha.txt");
    let second_path = temp_dir.join("beta.txt");
    let third_path = temp_dir.join("gamma.txt");
    fs::write(&first_path, "alpha content").unwrap();
    fs::write(&second_path, "beta content").unwrap();
    fs::write(&third_path, "gamma content").unwrap();

    cx.update(|cx| {
        register_keybindings(cx);
        register_text_input_keybindings(cx);
    });

    let location = Location::Local(temp_dir.clone());
    let (main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|cx| {
            let mut state = AppState::new(location);
            state.attach(cx);
            state.reload_entries();
            state
        });
        MainView::new(state, window, cx)
    });
    cx.run_until_parked();

    // Selecting alpha then immediately switching to beta cancels alpha's pending load.
    main.update(cx, |main, cx| {
        main.list.update(cx, |list, cx| {
            list.select_next(cx); // alpha.txt
            list.select_next(cx); // beta.txt
        });
    });

    // Immediately after selection, preview enters Loading state without blocking the UI thread.
    let loading_path = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .preview_state
            .as_ref()
            .map(|p| (p.path.clone(), matches!(p.kind, PreviewKind::Loading)))
    });
    assert_eq!(loading_path, Some((second_path.clone(), true)));

    // Advance past debounce and let background preview finish.
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();

    let loaded_text = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .preview_state
            .as_ref()
            .and_then(|p| {
                if let PreviewKind::Text(ref t) = p.kind {
                    Some((p.path.clone(), t.clone()))
                } else {
                    None
                }
            })
    });
    assert_eq!(
        loaded_text,
        Some((second_path.clone(), "beta content".into()))
    );

    // Closing the preview panel stops loading new selections until reopened.
    cx.simulate_keystrokes("alt-p");
    cx.run_until_parked();

    main.update(cx, |main, cx| {
        main.list.update(cx, |list, cx| {
            list.select_next(cx); // gamma.txt
        });
    });
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();

    let while_closed_path = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .preview_state
            .as_ref()
            .map(|p| p.path.clone())
    });
    assert_eq!(while_closed_path, Some(second_path));

    // Reopening the preview loads the currently selected gamma.txt.
    cx.simulate_keystrokes("alt-p");
    cx.executor().advance_clock(Duration::from_millis(60));
    cx.run_until_parked();

    let reopened_text = main.read_with(cx, |main, cx| {
        main.app_state
            .read(cx)
            .preview_state
            .as_ref()
            .and_then(|p| {
                if let PreviewKind::Text(ref t) = p.kind {
                    Some((p.path.clone(), t.clone()))
                } else {
                    None
                }
            })
    });
    assert_eq!(reopened_text, Some((third_path, "gamma content".into())));

    let _ = fs::remove_dir_all(&temp_dir);
}
