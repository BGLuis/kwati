//! File preview side panel view (owned by inspect-worker for #15).
//!
//! Renders textual previews or image previews (via `img`) for the currently selected file,
//! with binary file fallback and metadata summary.

use std::fs;
use std::path::{Path, PathBuf};

use gpui::{div, img, prelude::*, px, Context, Entity, FontWeight, Subscription, Window};
use kwati_core::preview::{read_preview, Preview};

use crate::state::{AppState, PreviewData, PreviewKind};
use crate::theme::Theme;

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
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        let meta = fs::metadata(path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let is_dir = meta.as_ref().map(|m| m.is_dir()).unwrap_or(false);
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
        let summary = MetadataSummary::from_path(path, false);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Binary { name, size },
            },
            summary,
        );
    }

    if is_image_path(path) {
        let summary = MetadataSummary::from_path(path, false);
        return (
            PreviewData {
                path: path.to_path_buf(),
                kind: PreviewKind::Image(path.to_path_buf()),
            },
            summary,
        );
    }

    match read_preview(path) {
        Ok(Preview::Text(text)) => {
            let summary = MetadataSummary::from_path(path, false);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Text(text),
                },
                summary,
            )
        }
        Ok(Preview::Binary) => {
            let summary = MetadataSummary::from_path(path, true);
            (
                PreviewData {
                    path: path.to_path_buf(),
                    kind: PreviewKind::Binary { name, size },
                },
                summary,
            )
        }
        Err(_) => {
            let summary = MetadataSummary::from_path(path, true);
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
    pub theme: Theme,
    pub current_path: Option<PathBuf>,
    pub metadata: Option<MetadataSummary>,
    _subscription: Subscription,
}

impl PreviewView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&app_state, |this, app_state, cx| {
            this.sync_with_selection(app_state, cx);
        });

        Self {
            app_state,
            theme: Theme::dark(),
            current_path: None,
            metadata: None,
            _subscription: subscription,
        }
    }

    /// Synchronizes preview with the currently selected entry in AppState.
    pub fn sync_with_selection(&mut self, app_state: Entity<AppState>, cx: &mut Context<Self>) {
        let (selected_path, needs_clear) = {
            let state = app_state.read(cx);
            if state.selection.len() == 1 {
                let path = state
                    .selection
                    .iter()
                    .next()
                    .map(|loc| loc.path().to_path_buf());
                (path, false)
            } else {
                (None, true)
            }
        };

        if needs_clear {
            if self.current_path.is_some() || self.metadata.is_some() {
                self.current_path = None;
                self.metadata = None;
                app_state.update(cx, |state, _cx| {
                    state.set_preview(None);
                });
                cx.notify();
            }
            return;
        }

        if let Some(path) = selected_path {
            if self.current_path.as_ref() == Some(&path) {
                return;
            }

            let (preview_data, summary) = generate_preview_for(&path);
            self.current_path = Some(path);
            self.metadata = Some(summary);

            app_state.update(cx, |state, _cx| {
                state.set_preview(Some(preview_data));
            });
            cx.notify();
        }
    }
}

impl Render for PreviewView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.app_state.read(cx);
        let preview = state.preview_state.clone();

        let Some(preview_data) = preview else {
            return div()
                .w(px(280.))
                .h_full()
                .p_4()
                .bg(self.theme.bg_panel)
                .border_l_1()
                .border_color(self.theme.border)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .text_size(px(24.))
                        .text_color(self.theme.text_muted)
                        .child("🔍"),
                )
                .child(
                    div()
                        .text_color(self.theme.text_muted)
                        .text_size(px(13.))
                        .child("No file selected"),
                );
        };

        let file_name = preview_data
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| preview_data.path.to_string_lossy().to_string());

        let meta = self.metadata.clone();
        let size_str = meta
            .as_ref()
            .map(|m| format_file_size(m.size))
            .unwrap_or_default();
        let kind_str = meta
            .as_ref()
            .map(|m| m.kind_label.clone())
            .unwrap_or_else(|| "File".to_string());

        div()
            .w(px(280.))
            .h_full()
            .p_3()
            .bg(self.theme.bg_panel)
            .border_l_1()
            .border_color(self.theme.border)
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
                    .border_color(self.theme.border)
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(self.theme.text_primary)
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
                            .text_color(self.theme.text_muted)
                            .child(div().child(kind_str))
                            .child(div().child(size_str)),
                    ),
            )
            // Preview Body
            .child(match preview_data.kind {
                PreviewKind::Text(text) => div()
                    .flex_1()
                    .p_2()
                    .rounded_md()
                    .bg(self.theme.bg_content)
                    .text_color(self.theme.text_secondary)
                    .text_size(px(11.))
                    .overflow_hidden()
                    .child(text),
                PreviewKind::Image(path) => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p_2()
                    .bg(self.theme.bg_content)
                    .rounded_md()
                    .child(img(path).max_w_full().max_h_full()),
                PreviewKind::Binary { name, size } => div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_4()
                    .rounded_md()
                    .bg(self.theme.bg_content)
                    .child(
                        div()
                            .text_size(px(28.))
                            .text_color(self.theme.text_muted)
                            .child("📦"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(self.theme.text_primary)
                            .child(name),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(self.theme.text_muted)
                            .child(format!("Size: {}", format_file_size(size))),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(self.theme.text_muted)
                            .child("Binary file — preview not available"),
                    ),
                PreviewKind::Loading => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(self.theme.text_muted)
                    .text_size(px(12.))
                    .child("Loading preview..."),
            })
    }
}
