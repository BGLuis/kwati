use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{px, rgb, size, AppContext as _, AssetSource, Hsla, TestAppContext};
use kwati_core::{
    IconColor, IconConfig, IconGroup, IconPreferences, IconResolver, IconSource, Location,
    ResolvedIcon,
};
use kwati_gui::components::{resolve_icon_color, Assets, IconName};
use kwati_gui::state::AppState;
use kwati_gui::theme::Theme;
use kwati_gui::views::MainView;

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "kwati_gui_icon_theme_{}_{}_{}",
            name,
            std::process::id(),
            id
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write_index_theme(theme_dir: &Path, name: &str, inherits: Option<&str>) {
    fs::create_dir_all(theme_dir.join("16x16/places")).unwrap();
    fs::create_dir_all(theme_dir.join("16x16/mimetypes")).unwrap();
    let mut ini = format!("[Icon Theme]\nName={name}\n");
    if let Some(parent) = inherits {
        ini.push_str(&format!("Inherits={parent}\n"));
    }
    ini.push_str(
        "Directories=16x16/places,16x16/mimetypes\n\n\
         [16x16/places]\nSize=16\nContext=Places\nType=Fixed\n\n\
         [16x16/mimetypes]\nSize=16\nContext=MimeTypes\nType=Fixed\n",
    );
    fs::write(theme_dir.join("index.theme"), ini).unwrap();
}

const SAMPLE_SVG: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path d="M2 2h12v12H2z"/></svg>"#;

const MINIMAL_PNG: [u8; 67] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00, 0x01, 0x00, 0x00,
    0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
    0x42, 0x60, 0x82,
];

#[test]
fn icon_theme_inherits_from_parent_and_falls_back_to_spec_and_builtin() {
    let dir = TempDir::new("inherits");
    let themes_root = dir.path.join("icons");
    let parent_dir = themes_root.join("parent-theme");
    let child_dir = themes_root.join("child-theme");

    write_index_theme(&parent_dir, "ParentTheme", None);
    write_index_theme(&child_dir, "ChildTheme", Some("ParentTheme"));

    // Child only provides generic `folder.svg` (no `folder-download`).
    let child_folder = child_dir.join("16x16/places/folder.svg");
    fs::write(&child_folder, SAMPLE_SVG).unwrap();

    // Parent provides `application-pdf.svg`, missing in child.
    let parent_pdf = parent_dir.join("16x16/mimetypes/application-pdf.svg");
    fs::write(&parent_pdf, SAMPLE_SVG).unwrap();

    // Neither theme provides `file-ebook` (`application-epub+zip` / `x-office-document` / `text-x-generic`).
    let resolver = IconResolver::with_base_paths(vec![themes_root]);
    let prefs = IconPreferences {
        base: IconSource::Theme("ChildTheme".into()),
        ..IconPreferences::default()
    };

    // FolderDownloads resolves to `folder` in child theme when `folder-download` is absent.
    assert_eq!(
        resolver.resolve(&prefs, IconName::FolderDownloads.key_name(), 16),
        ResolvedIcon::Image {
            path: child_folder,
            is_symbolic: false,
        }
    );

    // Icon missing in child resolves from parent via `Inherits`.
    assert_eq!(
        resolver.resolve(&prefs, IconName::FilePdf.key_name(), 16),
        ResolvedIcon::Image {
            path: parent_pdf,
            is_symbolic: false,
        }
    );

    // Icon missing in both resolves to BuiltIn.
    assert_eq!(
        resolver.resolve(&prefs, IconName::FileEbook.key_name(), 16),
        ResolvedIcon::BuiltIn
    );
}

#[test]
fn icon_theme_deleted_custom_image_falls_back_to_builtin_and_symbolic_svg_loads() {
    let dir = TempDir::new("deleted_image");
    let custom_png = dir.path.join("custom-pdf.png");
    fs::write(&custom_png, MINIMAL_PNG).unwrap();

    let symbolic_svg = dir.path.join("custom-symbolic.svg");
    fs::write(&symbolic_svg, SAMPLE_SVG).unwrap();

    let resolver = IconResolver::with_base_paths(vec![dir.path.join("no-themes")]);
    let mut prefs = IconPreferences::default();
    prefs.set_type_override("file-pdf", Some(IconSource::Image(custom_png.clone())));
    prefs.set_type_override(
        "file-markdown",
        Some(IconSource::Image(symbolic_svg.clone())),
    );

    assert_eq!(
        resolver.resolve(&prefs, IconName::FilePdf.key_name(), 16),
        ResolvedIcon::Image {
            path: custom_png.clone(),
            is_symbolic: false,
        }
    );

    assert_eq!(
        resolver.resolve(&prefs, IconName::FileMarkdown.key_name(), 16),
        ResolvedIcon::Image {
            path: symbolic_svg.clone(),
            is_symbolic: true,
        }
    );

    // Assets serves `fs-symbolic:` SVGs and rejects non-SVG files.
    let loaded = Assets
        .load(&format!("fs-symbolic:{}", symbolic_svg.display()))
        .unwrap();
    assert!(loaded.is_some());
    assert!(Assets
        .load(&format!("fs-symbolic:{}", custom_png.display()))
        .unwrap()
        .is_none());

    // Deleting the custom PNG makes it resolve cleanly back to BuiltIn.
    fs::remove_file(&custom_png).unwrap();
    assert_eq!(
        resolver.resolve(&prefs, IconName::FilePdf.key_name(), 16),
        ResolvedIcon::BuiltIn
    );
}

#[gpui::test]
fn icon_theme_renders_custom_and_deleted_images_without_panic(cx: &mut TestAppContext) {
    let dir = TempDir::new("gui_render");
    let folder_dir = dir.path.join("workspace");
    fs::create_dir_all(folder_dir.join("sub")).unwrap();
    let pdf_file = folder_dir.join("invoice.pdf");
    fs::write(&pdf_file, b"%PDF-1.4\n").unwrap();

    let custom_png = dir.path.join("custom.png");
    fs::write(&custom_png, MINIMAL_PNG).unwrap();
    let missing_png = dir.path.join("missing.png");

    let themes_root = dir.path.join("themes");
    write_index_theme(&themes_root.join("fixture"), "Fixture", None);
    fs::write(
        themes_root.join("fixture/16x16/places/folder.svg"),
        SAMPLE_SVG,
    )
    .unwrap();

    let resolver = Arc::new(IconResolver::with_base_paths(vec![themes_root]));
    let (_main, cx) = cx.add_window_view(|window, cx| {
        let state = cx.new(|_| {
            let mut s =
                AppState::new(Location::Local(folder_dir.clone())).with_icon_resolver(resolver);
            s.set_icon_base(IconSource::Theme("Fixture".into()))
                .unwrap();
            s.set_icon_type_override("file-pdf", Some(IconSource::Image(custom_png.clone())))
                .unwrap();
            s.set_icon_type_override("file-text", Some(IconSource::Image(missing_png)))
                .unwrap();
            s.reload_entries();
            s.select_location(Location::Local(pdf_file.clone()));
            s
        });
        MainView::new(state, window, cx)
    });

    cx.simulate_resize(size(px(1024.), px(720.)));
    cx.run_until_parked();
}

#[test]
fn resolve_icon_color_gives_pdf_red_by_default_and_honors_user_overrides() {
    let dir = TempDir::new("color_overrides");
    let icons_path = dir.path.join("icons.toml");

    let light = Theme::light();
    let dark = Theme::dark();
    let defaults = IconPreferences::default();

    // PDF defaults to a red hue in both themes, distinct from generic file/text color.
    let pdf_light = resolve_icon_color(IconName::FilePdf.key_name(), &defaults, &light);
    let pdf_dark = resolve_icon_color(IconName::FilePdf.key_name(), &defaults, &dark);
    let text_light: Hsla = light.text.into();
    let text_dark: Hsla = dark.text.into();
    assert_ne!(pdf_light, text_light);
    assert_ne!(pdf_dark, text_dark);
    assert_eq!(pdf_light, rgb(0xB3261E).into());
    assert_eq!(pdf_dark, rgb(0xF2746B).into());

    // Folders default to theme accent.
    assert_eq!(
        resolve_icon_color(IconName::Folder.key_name(), &defaults, &dark),
        dark.accent.into()
    );

    // AppState setters persist color overrides to icons.toml and follow precedence.
    let mut state =
        AppState::new(Location::Local(dir.path.clone())).with_icons_path(Some(icons_path.clone()));
    state.set_icon_base_color(Some(IconColor::Muted)).unwrap();
    assert_eq!(
        resolve_icon_color(
            IconName::FileCode.key_name(),
            &state.icon_config.active,
            &dark
        ),
        dark.text_muted.into()
    );

    state
        .set_icon_group_color_override(IconGroup::Documents, Some(IconColor::rgb(0x2563EB)))
        .unwrap();
    assert_eq!(
        resolve_icon_color(
            IconName::FilePdf.key_name(),
            &state.icon_config.active,
            &dark
        ),
        rgb(0x2563EB).into()
    );

    state
        .set_icon_type_color_override("file-pdf", Some(IconColor::rgb(0xDC2626)))
        .unwrap();
    assert_eq!(
        resolve_icon_color(
            IconName::FilePdf.key_name(),
            &state.icon_config.active,
            &dark
        ),
        rgb(0xDC2626).into()
    );

    let reloaded = IconConfig::load_from(&icons_path).unwrap();
    assert_eq!(
        resolve_icon_color(IconName::FilePdf.key_name(), &reloaded.active, &dark),
        rgb(0xDC2626).into()
    );
}
