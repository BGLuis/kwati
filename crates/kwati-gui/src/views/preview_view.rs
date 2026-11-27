//! File preview side panel view (owned by inspect-worker for #15).
//!
//! Renders textual previews or image previews (via `img`) for the currently selected file,
//! with binary file fallback and metadata summary.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, img, prelude::*, px, uniform_list, Context, Entity, FontWeight, ObjectFit, Pixels,
    RenderImage, ScrollStrategy, SharedString, Subscription, Task, UniformListScrollHandle, Window,
};
use kwati_core::preview::{preview_from_reader, read_preview, Preview};
use kwati_core::thumbnail::generate_preview_image;
use kwati_core::{
    is_streamable_media, Backend, Entry, EntryKind, IconConfig, Location, RemoteFileCache,
};

use crate::components::thumbnails::{global_thumbnail_pool, render_image_from_rgba};
use crate::components::{
    fast_icon_for_entry, icon, resolve_icon_color, resolved_icon, Ellipsize, IconMotion, IconName,
    SpecialFolders,
};
use crate::state::{AppState, PreviewData, PreviewKind};
use crate::theme::Theme;
use crate::tokens::{size, Fonts, DATA_SIZE};

const MAX_IMAGE_PREVIEW_BYTES: u64 = 50 * 1024 * 1024;
const MAX_PREVIEW_IMAGE_PX: u32 = 1024;
const MAX_PREVIEW_LINE_CHARS: usize = 200;
const PREVIEW_DEBOUNCE: Duration = Duration::from_millis(50);
const PREVIEW_LINE_HEIGHT: Pixels = px(18.);

/// Formats a byte size into a human-readable string.
pub fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

/// Checks if a file path is a supported image extension for `img` rendering.
pub fn is_image_path(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" | "bmp" | "ico" | "tiff" | "tif" | "avif"
    )
}

/// Returns a human-friendly description of the file kind based on path and classification.
pub fn file_kind_label(path: &Path, is_dir: bool, is_binary: bool) -> &'static str {
    if is_dir {
        return "Folder";
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif" | "avif" => {
            "Image"
        }
        "svg" => "SVG Vector Image",
        "rs" => "Rust Source File",
        "toml" => "TOML Document",
        "json" => "JSON Document",
        "md" => "Markdown Document",
        "txt" => "Plain Text Document",
        "html" | "htm" => "HTML Document",
        "css" => "CSS Stylesheet",
        "js" | "mjs" | "cjs" => "JavaScript File",
        "ts" => "TypeScript File",
        "py" => "Python Script",
        "sh" | "bash" | "zsh" => "Shell Script",
        "c" | "h" => "C Source File",
        "cpp" | "hpp" | "cc" => "C++ Source File",
        "pdf" => "PDF Document",
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" => "Archive",
        _ => {
            if is_binary {
                "Binary File"
            } else {
                "Text File"
            }
        }
    }
}

/// Metadata summary for the selected entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataSummary {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    pub is_image: bool,
    pub is_binary: bool,
    pub kind_label: String,
}

impl MetadataSummary {
    pub fn from_path(path: &Path, is_binary: bool) -> Self {
        let meta = fs::metadata(path).ok();
        Self::from_metadata(path, meta.as_ref(), is_binary)
    }

    fn from_metadata(path: &Path, meta: Option<&fs::Metadata>, is_binary: bool) -> Self {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let size = meta.map(|m| m.len()).unwrap_or(0);
        let is_dir = meta.map(|m| m.is_dir()).unwrap_or(false);
        let is_image = is_image_path(path);
        let kind_label = file_kind_label(path, is_dir, is_binary).to_string();

        Self {
            name,
            path: path.to_path_buf(),
            size,
            is_dir,
            is_image,
            is_binary,
            kind_label,
        }
    }

    /// What the listing already says about an entry; the file itself is not read.
    pub fn from_listing(entry: &Entry) -> Self {
        let is_dir = entry.is_dir();
        Self {
            name: entry.display_name(),
            path: entry.path.clone(),
            size: entry.size.unwrap_or(0),
            is_dir,
            is_image: is_image_path(&entry.path),
            is_binary: false,
            kind_label: file_kind_label(&entry.path, is_dir, false).to_string(),
        }
    }
}

/// Splits preview text into bounded visual lines so GPUI only shapes visible rows.
pub fn prepare_preview_lines(text: &str) -> Vec<SharedString> {
    if text.is_empty() {
        return Vec::new();
    }

    let trimmed = text.strip_suffix('\n').unwrap_or(text);
    let mut lines = Vec::new();

    for raw_line in trimmed.split('\n') {
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let expanded: std::borrow::Cow<'_, str> = if line.contains('\t') {
            std::borrow::Cow::Owned(line.replace('\t', "    "))
        } else {
            std::borrow::Cow::Borrowed(line)
        };

        if expanded.is_empty() {
            lines.push(SharedString::from(""));
            continue;
        }

        let mut chunk_start = 0;
        let mut char_count = 0;
        for (byte_idx, _) in expanded.char_indices() {
            if char_count == MAX_PREVIEW_LINE_CHARS {
                lines.push(SharedString::from(
                    expanded[chunk_start..byte_idx].to_owned(),
                ));
                chunk_start = byte_idx;
                char_count = 0;
            }
            char_count += 1;
        }
        lines.push(SharedString::from(expanded[chunk_start..].to_owned()));
    }

    lines
}

/// Computes the preview data and metadata summary for a given path.
pub fn generate_preview_for(path: &Path) -> (PreviewData, MetadataSummary) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let meta = fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);

    if is_dir {
        let summary = MetadataSummary::from_metadata(path, meta.as_ref(), false);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Binary { name, size },
            },
            summary,
        );
    }

    let is_regular = meta.as_ref().is_some_and(|m| m.file_type().is_file());
    if let Some(ref m) = meta {
        if !m.file_type().is_file() {
            let summary = MetadataSummary::from_metadata(path, Some(m), true);
            return (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Unavailable("Special file — preview not available".into()),
                },
                summary,
            );
        }
    }

    if is_regular && is_image_path(path) {
        let summary = MetadataSummary::from_metadata(path, meta.as_ref(), false);
        let kind = if size > MAX_IMAGE_PREVIEW_BYTES {
            PreviewKind::Unavailable("Image too large to preview".into())
        } else {
            PreviewKind::Image(path.to_path_buf())
        };
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind,
            },
            summary,
        );
    }

    match read_preview(path) {
        Ok(Preview::Text(text)) => {
            let summary = MetadataSummary::from_metadata(path, meta.as_ref(), false);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Text(text),
                },
                summary,
            )
        }
        Ok(Preview::Binary) | Err(_) => {
            let summary = MetadataSummary::from_metadata(path, meta.as_ref(), true);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Binary { name, size },
                },
                summary,
            )
        }
    }
}

/// Computes preview data and metadata summary for a remote file via its [`Backend`].
pub fn generate_remote_preview_for(
    backend: &dyn Backend,
    location: &Location,
    cache: &RemoteFileCache,
    listed: Option<&Entry>,
) -> (PreviewData, MetadataSummary) {
    let path = location.path();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    let (is_dir, size, is_regular) = match listed.and_then(|e| e.size.map(|s| (e.is_dir(), s))) {
        Some((dir, s)) => (dir, s, !dir),
        None => match backend.stat(location).ok() {
            Some(m) => (m.kind == EntryKind::Dir, m.len, m.kind == EntryKind::File),
            None => match listed {
                Some(entry) => (entry.is_dir(), entry.size.unwrap_or(0), !entry.is_dir()),
                None => {
                    let summary = MetadataSummary {
                        name,
                        path: path.to_path_buf(),
                        size: 0,
                        is_dir: false,
                        is_image: is_image_path(path),
                        is_binary: true,
                        kind_label: file_kind_label(path, false, true).to_string(),
                    };
                    return (
                        PreviewData {
                            path: path.to_path_buf(),
                            kind: PreviewKind::Unavailable("Cannot read remote file".into()),
                        },
                        summary,
                    );
                }
            },
        },
    };

    let build_summary = |is_binary: bool| MetadataSummary {
        name: name.clone(),
        path: path.to_path_buf(),
        size,
        is_dir,
        is_image: is_image_path(path),
        is_binary,
        kind_label: file_kind_label(path, is_dir, is_binary).to_string(),
    };

    if is_dir {
        let summary = build_summary(false);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Binary { name, size: 0 },
            },
            summary,
        );
    }

    if !is_regular {
        let summary = build_summary(true);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Unavailable("Special file — preview not available".into()),
            },
            summary,
        );
    }

    if is_image_path(path) {
        let summary = build_summary(false);
        let kind = if size > MAX_IMAGE_PREVIEW_BYTES {
            PreviewKind::Unavailable("Image too large to preview".into())
        } else {
            match fetch_remote_preview_copy(backend, location, cache, size) {
                Ok(preview_path) => PreviewKind::Image(preview_path),
                Err(_) => PreviewKind::Binary { name, size },
            }
        };
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind,
            },
            summary,
        );
    }

    if is_streamable_media(path) {
        let summary = build_summary(true);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Binary { name, size },
            },
            summary,
        );
    }

    let preview = backend
        .open_read(location)
        .ok()
        .and_then(|reader| preview_from_reader(reader).ok());
    match preview {
        Some(Preview::Text(text)) => {
            let summary = build_summary(false);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Text(text),
                },
                summary,
            )
        }
        _ => {
            let summary = build_summary(true);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Binary { name, size },
                },
                summary,
            )
        }
    }
}

/// The local copy of a remote file in the preview cache, downloaded unless a copy of the same
/// size is already there. Shared by the preview pane and grid thumbnails.
pub(crate) fn fetch_remote_preview_copy(
    backend: &dyn Backend,
    location: &Location,
    cache: &RemoteFileCache,
    size: u64,
) -> std::io::Result<PathBuf> {
    let preview_path = cache.preview_path_for(location);
    if !fs::metadata(&preview_path).is_ok_and(|m| m.len() == size) {
        download_preview_file(backend, location, &preview_path)?;
    }
    Ok(preview_path)
}

fn download_preview_file(
    backend: &dyn Backend,
    location: &Location,
    dest: &Path,
) -> std::io::Result<()> {
    static DOWNLOAD_ID: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    // The preview pane and a grid thumbnail can fetch the same file at once; separate partial
    // files keep the two downloads from writing into each other.
    let tmp = dest.with_extension(format!(
        "kwati-preview-part-{}",
        DOWNLOAD_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let mut reader = backend
        .open_read(location)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let result = fs::File::create(&tmp)
        .and_then(|mut file| std::io::copy(&mut reader, &mut file))
        .and_then(|_| fs::rename(&tmp, dest));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

pub struct PreviewView {
    pub app_state: Entity<AppState>,
    pub current_path: Option<PathBuf>,
    current_location: Option<Location>,
    pub metadata: Option<MetadataSummary>,
    text_lines: Arc<Vec<SharedString>>,
    text_lines_path: Option<PathBuf>,
    decoded_image: Option<(PathBuf, Arc<RenderImage>)>,
    decoded_image_failed_path: Option<PathBuf>,
    text_scroll_handle: UniformListScrollHandle,
    load_task: Option<Task<()>>,
    request_generation: u64,
    preview_generation: Arc<AtomicU64>,
    last_icon_config: IconConfig,
    special_folders: SpecialFolders,
    _subscription: Subscription,
}

impl PreviewView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let last_icon_config = app_state.read(cx).icon_config.clone();
        let subscription = cx.observe(&app_state, |this, app_state, cx| {
            this.sync_with_selection(app_state, cx);
        });

        Self {
            app_state,
            current_path: None,
            current_location: None,
            metadata: None,
            text_lines: Arc::new(Vec::new()),
            text_lines_path: None,
            decoded_image: None,
            decoded_image_failed_path: None,
            text_scroll_handle: UniformListScrollHandle::new(),
            load_task: None,
            request_generation: 0,
            preview_generation: Arc::new(AtomicU64::new(0)),
            last_icon_config,
            special_folders: SpecialFolders::from_system(),
            _subscription: subscription,
        }
    }

    pub fn has_decoded_image(&self) -> bool {
        self.decoded_image.is_some()
    }

    pub fn decoded_image_dimensions(&self) -> Option<(i32, i32)> {
        self.decoded_image.as_ref().map(|(_, img)| {
            let size = img.size(0);
            (size.width.0, size.height.0)
        })
    }

    fn clear_text_cache(&mut self) {
        if !self.text_lines.is_empty() {
            self.text_lines = Arc::new(Vec::new());
        }
        self.text_lines_path = None;
    }

    fn clear_image_cache(&mut self, cx: &mut Context<Self>) {
        if let Some((_, old)) = self.decoded_image.take() {
            cx.drop_image(old, None);
        }
        self.decoded_image_failed_path = None;
    }

    fn spawn_preview_load(
        &mut self,
        target_path: PathBuf,
        cx: &mut Context<Self>,
        compute: impl FnOnce() -> (PreviewData, MetadataSummary) + Send + 'static,
    ) {
        let generation = self.request_generation;
        let preview_gen = Arc::clone(&self.preview_generation);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
            let still_current = this
                .read_with(cx, |view, _| view.request_generation == generation)
                .unwrap_or(false);
            if !still_current {
                return;
            }

            let (preview_data, summary, lines) = cx
                .background_executor()
                .spawn(async move {
                    let (preview_data, summary) = compute();
                    let lines = match &preview_data.kind {
                        PreviewKind::Text(text) => prepare_preview_lines(text),
                        _ => Vec::new(),
                    };
                    (preview_data, summary, lines)
                })
                .await;

            let decoded_image = if let PreviewKind::Image(ref img_path) = preview_data.kind {
                let decode_path = img_path.clone();
                global_thumbnail_pool()
                    .submit(Some((preview_gen, generation)), move || {
                        generate_preview_image(&decode_path, MAX_PREVIEW_IMAGE_PX)
                            .ok()
                            .map(render_image_from_rgba)
                    })
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };

            let _ = this.update(cx, |view, cx| {
                if view.request_generation != generation {
                    if let Some(unused) = decoded_image {
                        cx.drop_image(unused, None);
                    }
                    return;
                }
                view.load_task = None;
                view.metadata = Some(summary);
                view.text_lines = Arc::new(lines);
                view.text_lines_path = Some(target_path.clone());
                view.clear_image_cache(cx);
                if let PreviewKind::Image(ref img_path) = preview_data.kind {
                    match decoded_image {
                        Some(img) => {
                            view.decoded_image = Some((img_path.clone(), img));
                        }
                        None => {
                            view.decoded_image_failed_path = Some(img_path.clone());
                        }
                    }
                }
                view.text_scroll_handle
                    .scroll_to_item(0, ScrollStrategy::Top);
                view.app_state.update(cx, |state, _cx| {
                    state.set_preview(Some(preview_data));
                });
                cx.notify();
            });
        }));
    }

    fn cancel_pending_load(&mut self) {
        self.load_task = None;
        self.request_generation = self.request_generation.wrapping_add(1);
        self.preview_generation
            .store(self.request_generation, Ordering::Relaxed);
    }

    /// Synchronizes preview with the currently selected entry in AppState.
    pub fn sync_with_selection(&mut self, app_state: Entity<AppState>, cx: &mut Context<Self>) {
        let (preview_open, selected_location, icons_changed) = {
            let state = app_state.read(cx);
            let selected = if state.selection.len() == 1 {
                state.selection.iter().next().cloned()
            } else {
                None
            };
            let changed = state.icon_config != self.last_icon_config;
            if changed {
                self.last_icon_config = state.icon_config.clone();
            }
            (state.panels.preview_open, selected, changed)
        };

        if !preview_open {
            self.cancel_pending_load();
            self.clear_image_cache(cx);
            if self.current_path.is_some()
                || self.current_location.is_some()
                || self.metadata.is_some()
                || self.text_lines_path.is_some()
            {
                self.current_path = None;
                self.current_location = None;
                self.metadata = None;
                self.clear_text_cache();
            }
            return;
        }

        let Some(location) = selected_location else {
            self.cancel_pending_load();
            self.clear_image_cache(cx);
            let had_preview = app_state.read(cx).preview_state.is_some();
            if self.current_path.is_some()
                || self.current_location.is_some()
                || self.metadata.is_some()
                || had_preview
            {
                self.current_path = None;
                self.current_location = None;
                self.metadata = None;
                self.clear_text_cache();
                app_state.update(cx, |state, _cx| {
                    state.set_preview(None);
                });
                cx.notify();
            } else if icons_changed {
                cx.notify();
            }
            return;
        };

        let path = location.path().to_path_buf();
        if self.current_location.as_ref() == Some(&location) {
            if icons_changed {
                cx.notify();
            }
            return;
        }

        let (listed, remote_ctx) = {
            let state = app_state.read(cx);
            let listed = state.entry_by_path(&path).cloned();
            let remote_ctx = if matches!(location, Location::Remote { .. }) {
                Some((
                    state.backend_for(&location),
                    Arc::clone(&state.remote_cache),
                ))
            } else {
                None
            };
            (listed, remote_ctx)
        };

        match location {
            remote_loc @ Location::Remote { .. } => {
                self.cancel_pending_load();
                self.current_path = Some(path.clone());
                self.current_location = Some(remote_loc.clone());
                self.metadata = listed.as_ref().map(MetadataSummary::from_listing);
                self.clear_text_cache();
                self.clear_image_cache(cx);

                if listed.as_ref().is_some_and(|e| e.is_dir()) {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    app_state.update(cx, |state, _cx| {
                        state.set_preview(Some(PreviewData {
                            path,
                            kind: PreviewKind::Binary { name, size: 0 },
                        }));
                    });
                    cx.notify();
                    return;
                }

                let Some((Some(backend), cache)) = remote_ctx else {
                    app_state.update(cx, |state, _cx| {
                        state.set_preview(Some(PreviewData {
                            path,
                            kind: PreviewKind::Unavailable("Server is not connected".into()),
                        }));
                    });
                    cx.notify();
                    return;
                };

                app_state.update(cx, |state, _cx| {
                    state.set_preview(Some(PreviewData {
                        path: path.clone(),
                        kind: PreviewKind::Loading,
                    }));
                });
                cx.notify();

                self.spawn_preview_load(path, cx, move || {
                    generate_remote_preview_for(&*backend, &remote_loc, &cache, listed.as_ref())
                });
            }
            local_loc @ Location::Local(_) => {
                self.cancel_pending_load();
                self.current_path = Some(path.clone());
                self.current_location = Some(local_loc);
                self.metadata = listed.as_ref().map(MetadataSummary::from_listing);
                self.clear_text_cache();
                self.clear_image_cache(cx);

                app_state.update(cx, |state, _cx| {
                    state.set_preview(Some(PreviewData {
                        path: path.clone(),
                        kind: PreviewKind::Loading,
                    }));
                });
                cx.notify();

                let path_for_bg = path.clone();
                self.spawn_preview_load(path, cx, move || generate_preview_for(&path_for_bg));
            }
        }
    }
}

enum RenderPreviewBody {
    Text(Arc<Vec<SharedString>>),
    Image(Arc<RenderImage>),
    Binary { name: String, size: u64 },
    Unavailable(String),
    Loading,
}

impl Render for PreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);

        let (preview_snapshot, icon_prefs, icon_resolver) = {
            let state = self.app_state.read(cx);
            let snap = state.preview_state.as_ref().map(|preview_data| {
                let path = preview_data.path.clone();
                let body = match &preview_data.kind {
                    PreviewKind::Text(text) => {
                        if self.text_lines_path.as_ref() == Some(&path) {
                            Ok(RenderPreviewBody::Text(self.text_lines.clone()))
                        } else {
                            Err((path.clone(), text.clone()))
                        }
                    }
                    PreviewKind::Image(img_path) => {
                        if let Some((cached_path, image)) = &self.decoded_image {
                            if cached_path == img_path {
                                Ok(RenderPreviewBody::Image(Arc::clone(image)))
                            } else {
                                Ok(RenderPreviewBody::Loading)
                            }
                        } else if self.decoded_image_failed_path.as_ref() == Some(img_path) {
                            Ok(RenderPreviewBody::Unavailable(
                                "Cannot preview image".into(),
                            ))
                        } else {
                            Ok(RenderPreviewBody::Loading)
                        }
                    }
                    PreviewKind::Binary { name, size } => Ok(RenderPreviewBody::Binary {
                        name: name.clone(),
                        size: *size,
                    }),
                    PreviewKind::Unavailable(reason) => {
                        Ok(RenderPreviewBody::Unavailable(reason.clone()))
                    }
                    PreviewKind::Loading => Ok(RenderPreviewBody::Loading),
                };
                (path, body)
            });
            (
                snap,
                state.icon_config.active.clone(),
                state.icon_resolver.clone(),
            )
        };

        let Some((preview_path, body_or_uncached)) = preview_snapshot else {
            return div()
                .size_full()
                .p_4()
                .bg(theme.surface)
                .border_l_1()
                .border_color(theme.border)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(icon(IconName::File).text_color(theme.border_strong))
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .text_size(px(13.))
                        .child("No file selected"),
                );
        };

        let body = match body_or_uncached {
            Ok(body) => body,
            Err((path, text)) => {
                let lines = Arc::new(prepare_preview_lines(&text));
                self.text_lines = lines.clone();
                self.text_lines_path = Some(path);
                RenderPreviewBody::Text(lines)
            }
        };

        let file_name = preview_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| preview_path.to_string_lossy().to_string());

        let size_str = self
            .metadata
            .as_ref()
            .map(|m| format_file_size(m.size))
            .unwrap_or_default();
        let kind_str = self
            .metadata
            .as_ref()
            .map(|m| m.kind_label.clone())
            .unwrap_or_else(|| "File".to_string());
        let is_dir = self.metadata.as_ref().is_some_and(|m| m.is_dir);
        let is_remote = matches!(self.current_location, Some(Location::Remote { .. }));
        let header_icon =
            fast_icon_for_entry(&preview_path, is_dir, is_remote, &self.special_folders).0;
        let header_icon_color = resolve_icon_color(header_icon.key_name(), &icon_prefs, &theme);

        div()
            .size_full()
            .p_3()
            .bg(theme.surface)
            .border_l_1()
            .border_color(theme.border)
            .flex()
            .flex_col()
            .gap_3()
            .overflow_hidden()
            // Header: Name and Metadata Summary
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .pb_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(resolved_icon(
                                header_icon,
                                IconMotion::Rest,
                                size::ICON,
                                header_icon_color,
                                &icon_prefs,
                                &icon_resolver,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .ellipsize()
                                    .text_size(px(14.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(theme.text)
                                    .child(file_name),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child(div().child(kind_str))
                            .child(div().child(size_str)),
                    ),
            )
            // Preview Body
            .child(match body {
                RenderPreviewBody::Text(lines) => {
                    if lines.is_empty() {
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .p_2()
                            .rounded_md()
                            .bg(theme.surface)
                            .font_family(fonts.mono.clone())
                            .text_size(DATA_SIZE)
                            .text_color(theme.text_muted)
                            .child("Empty file")
                    } else {
                        let line_count = lines.len();
                        let line_color = theme.text;
                        let mono_font = fonts.mono.clone();
                        div()
                            .flex_1()
                            .min_h_0()
                            .p_2()
                            .rounded_md()
                            .bg(theme.surface)
                            .overflow_hidden()
                            .child(
                                uniform_list(
                                    "preview_text_lines",
                                    line_count,
                                    cx.processor(
                                        move |_this: &mut PreviewView,
                                              range: std::ops::Range<usize>,
                                              _window,
                                              _cx| {
                                            range
                                                .filter_map(|ix| {
                                                    lines.get(ix).map(|line| {
                                                        div()
                                                            .id(ix)
                                                            .w_full()
                                                            .h(PREVIEW_LINE_HEIGHT)
                                                            .font_family(mono_font.clone())
                                                            .text_size(DATA_SIZE)
                                                            .text_color(line_color)
                                                            .whitespace_nowrap()
                                                            .overflow_hidden()
                                                            .child(line.clone())
                                                            .into_any_element()
                                                    })
                                                })
                                                .collect()
                                        },
                                    ),
                                )
                                .track_scroll(self.text_scroll_handle.clone())
                                .size_full(),
                            )
                    }
                }
                RenderPreviewBody::Image(image) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .bg(theme.surface)
                    .rounded_md()
                    .child(
                        img(image)
                            .max_w_full()
                            .max_h_full()
                            .object_fit(ObjectFit::Contain),
                    ),
                RenderPreviewBody::Binary { name, size } => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_4()
                    .rounded_md()
                    .bg(theme.surface)
                    .child(resolved_icon(
                        header_icon,
                        IconMotion::Rest,
                        size::ICON,
                        header_icon_color,
                        &icon_prefs,
                        &icon_resolver,
                    ))
                    .child(
                        div()
                            .w_full()
                            .ellipsize()
                            .text_center()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_muted)
                            .child(format!("Size: {}", format_file_size(size))),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.text_muted)
                            .child("Binary file — preview not available"),
                    ),
                RenderPreviewBody::Unavailable(reason) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .text_align(gpui::TextAlign::Center)
                    .text_color(theme.text_muted)
                    .text_size(px(12.))
                    .child(reason),
                RenderPreviewBody::Loading => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.text_muted)
                    .text_size(px(12.))
                    .child("Loading preview..."),
            })
    }
}
