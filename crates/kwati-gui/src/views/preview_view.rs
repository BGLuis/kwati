//! File preview side panel view (owned by inspect-worker for #15).
//!
//! Renders textual previews or image previews (via `img`) for the currently selected file,
//! with binary file fallback and metadata summary.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui::{
    div, img, prelude::*, px, uniform_list, Context, Entity, FontWeight, Pixels, ScrollStrategy,
    SharedString, Subscription, Task, UniformListScrollHandle, Window,
};
use kwati_core::preview::{read_preview, Preview};
use kwati_core::{Entry, Location};

use crate::components::{icon, IconName};
use crate::state::{AppState, PreviewData, PreviewKind};
use crate::theme::Theme;
use crate::tokens::{Fonts, DATA_SIZE};
use crate::views::list_view::REMOTE_FILE_NOTICE;

const MAX_IMAGE_PREVIEW_BYTES: u64 = 50 * 1024 * 1024;
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

pub struct PreviewView {
    pub app_state: Entity<AppState>,
    pub current_path: Option<PathBuf>,
    pub metadata: Option<MetadataSummary>,
    text_lines: Arc<Vec<SharedString>>,
    text_lines_path: Option<PathBuf>,
    text_scroll_handle: UniformListScrollHandle,
    load_task: Option<Task<()>>,
    request_generation: u64,
    _subscription: Subscription,
}

impl PreviewView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&app_state, |this, app_state, cx| {
            this.sync_with_selection(app_state, cx);
        });

        Self {
            app_state,
            current_path: None,
            metadata: None,
            text_lines: Arc::new(Vec::new()),
            text_lines_path: None,
            text_scroll_handle: UniformListScrollHandle::new(),
            load_task: None,
            request_generation: 0,
            _subscription: subscription,
        }
    }

    fn clear_text_cache(&mut self) {
        if !self.text_lines.is_empty() {
            self.text_lines = Arc::new(Vec::new());
        }
        self.text_lines_path = None;
    }

    fn cancel_pending_load(&mut self) {
        self.load_task = None;
        self.request_generation = self.request_generation.wrapping_add(1);
    }

    /// Synchronizes preview with the currently selected entry in AppState.
    pub fn sync_with_selection(&mut self, app_state: Entity<AppState>, cx: &mut Context<Self>) {
        let (preview_open, selected_location) = {
            let state = app_state.read(cx);
            let selected = if state.selection.len() == 1 {
                state.selection.iter().next().cloned()
            } else {
                None
            };
            (state.panels.preview_open, selected)
        };

        if !preview_open {
            self.cancel_pending_load();
            if self.current_path.is_some()
                || self.metadata.is_some()
                || self.text_lines_path.is_some()
            {
                self.current_path = None;
                self.metadata = None;
                self.clear_text_cache();
            }
            return;
        }

        let Some(location) = selected_location else {
            self.cancel_pending_load();
            let had_preview = app_state.read(cx).preview_state.is_some();
            if self.current_path.is_some() || self.metadata.is_some() || had_preview {
                self.current_path = None;
                self.metadata = None;
                self.clear_text_cache();
                app_state.update(cx, |state, _cx| {
                    state.set_preview(None);
                });
                cx.notify();
            }
            return;
        };

        let path = location.path().to_path_buf();
        if self.current_path.as_ref() == Some(&path) {
            return;
        }

        let listed = {
            let state = app_state.read(cx);
            state
                .entries
                .iter()
                .find(|entry| entry.path == path)
                .cloned()
        };

        match location {
            Location::Remote { .. } => {
                self.cancel_pending_load();
                self.current_path = Some(path.clone());
                self.metadata = Some(listed.as_ref().map_or_else(
                    || MetadataSummary::from_metadata(Path::new(""), None, false),
                    MetadataSummary::from_listing,
                ));
                self.clear_text_cache();

                app_state.update(cx, |state, _cx| {
                    state.set_preview(Some(PreviewData {
                        path,
                        kind: PreviewKind::Unavailable(REMOTE_FILE_NOTICE.into()),
                    }));
                });
                cx.notify();
            }
            Location::Local(_) => {
                self.cancel_pending_load();
                let generation = self.request_generation;
                self.current_path = Some(path.clone());
                self.metadata = listed.as_ref().map(MetadataSummary::from_listing);
                self.clear_text_cache();

                app_state.update(cx, |state, _cx| {
                    state.set_preview(Some(PreviewData {
                        path: path.clone(),
                        kind: PreviewKind::Loading,
                    }));
                });
                cx.notify();

                let target_path = path;
                self.load_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(PREVIEW_DEBOUNCE).await;
                    let still_current = this
                        .read_with(cx, |view, _| view.request_generation == generation)
                        .unwrap_or(false);
                    if !still_current {
                        return;
                    }

                    let path_for_bg = target_path.clone();
                    let (preview_data, summary, lines) = cx
                        .background_executor()
                        .spawn(async move {
                            let (preview_data, summary) = generate_preview_for(&path_for_bg);
                            let lines = match &preview_data.kind {
                                PreviewKind::Text(text) => prepare_preview_lines(text),
                                _ => Vec::new(),
                            };
                            (preview_data, summary, lines)
                        })
                        .await;

                    let _ = this.update(cx, |view, cx| {
                        if view.request_generation != generation {
                            return;
                        }
                        view.load_task = None;
                        view.metadata = Some(summary);
                        view.text_lines = Arc::new(lines);
                        view.text_lines_path = Some(target_path.clone());
                        view.text_scroll_handle
                            .scroll_to_item(0, ScrollStrategy::Top);
                        view.app_state.update(cx, |state, _cx| {
                            state.set_preview(Some(preview_data));
                        });
                        cx.notify();
                    });
                }));
            }
        }
    }
}

enum RenderPreviewBody {
    Text(Arc<Vec<SharedString>>),
    Image(PathBuf),
    Binary { name: String, size: u64 },
    Unavailable(String),
    Loading,
}

impl Render for PreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let fonts = Fonts::get(cx);

        let preview_snapshot = {
            let state = self.app_state.read(cx);
            state.preview_state.as_ref().map(|preview_data| {
                let path = preview_data.path.clone();
                let body = match &preview_data.kind {
                    PreviewKind::Text(text) => {
                        if self.text_lines_path.as_ref() == Some(&path) {
                            Ok(RenderPreviewBody::Text(self.text_lines.clone()))
                        } else {
                            Err((path.clone(), text.clone()))
                        }
                    }
                    PreviewKind::Image(img_path) => Ok(RenderPreviewBody::Image(img_path.clone())),
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
            })
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
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.text)
                            .overflow_hidden()
                            .child(file_name),
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
                RenderPreviewBody::Image(path) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .bg(theme.surface)
                    .rounded_md()
                    .child(img(path).max_w_full().max_h_full()),
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
                    .child(icon(IconName::File).text_color(theme.text_muted))
                    .child(
                        div()
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
