//! Small pictures of image, SVG, and PDF files, drawn in place of their icon in the grid view.

use std::fs;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings};
use image::{DynamicImage, ImageDecoder, ImageReader, Limits, RgbaImage};
use resvg::{tiny_skia, usvg};

use crate::Settings;

/// Extensions a thumbnail can be built for: the raster formats `image` decodes with its default
/// features, SVG through `resvg`, and the first page of a PDF through `hayro`.
pub const SUPPORTED_THUMBNAIL_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "tga", "qoi", "pnm", "svg",
    "pdf",
];

/// Decoding a hostile or huge raster (a "decompression bomb") stops at this allocation.
const MAX_DECODE_ALLOC: u64 = 96 * 1024 * 1024;
/// Maximum raster width or height accepted before decoding pixel buffers.
const MAX_DECODE_DIMENSION: u32 = 16_384;
/// Maximum SVG file size read into memory for thumbnailing or previewing.
const MAX_SVG_BYTES: u64 = 4 * 1024 * 1024;
/// Maximum SVG tree nodes tolerated before aborting CPU rasterization.
const MAX_SVG_NODES: usize = 10_000;
/// Maximum SVG filter primitives/chains tolerated before aborting CPU rasterization.
const MAX_SVG_FILTERS: usize = 32;
/// Hard safety cap on PDF file bytes read into memory for page-1 thumbnailing.
const MAX_PDF_BYTES: u64 = 32 * 1024 * 1024;
/// Maximum PDF page-1 content stream size tolerated by the unoptimized CPU rasterizer.
const MAX_PDF_PAGE_STREAM_BYTES: usize = 4 * 1024 * 1024;

/// Every supported extension, the default for `Settings::thumbnail_extensions`.
pub fn default_thumbnail_extensions() -> Vec<String> {
    SUPPORTED_THUMBNAIL_EXTENSIONS
        .iter()
        .map(|ext| (*ext).to_owned())
        .collect()
}

/// Normalizes extensions (trimmed, lower case, no leading dot, no duplicates, empty entries
/// dropped) and splits them into `(supported, unsupported)`, both in input order.
pub fn split_thumbnail_extensions<'a>(
    items: impl IntoIterator<Item = &'a str>,
) -> (Vec<String>, Vec<String>) {
    let mut supported = Vec::new();
    let mut unsupported = Vec::new();
    for item in items {
        let ext = item.trim().trim_start_matches('.').to_ascii_lowercase();
        if ext.is_empty() || supported.contains(&ext) || unsupported.contains(&ext) {
            continue;
        }
        if SUPPORTED_THUMBNAIL_EXTENSIONS.contains(&ext.as_str()) {
            supported.push(ext);
        } else {
            unsupported.push(ext);
        }
    }
    (supported, unsupported)
}

/// Parses a list typed by the user, separated by commas or whitespace (`"png, .JPG pdf"`).
/// Returns the unsupported entries as the error.
pub fn parse_thumbnail_extensions(raw: &str) -> Result<Vec<String>, Vec<String>> {
    let (supported, unsupported) =
        split_thumbnail_extensions(raw.split(|c: char| c == ',' || c.is_whitespace()));
    if unsupported.is_empty() {
        Ok(supported)
    } else {
        Err(unsupported)
    }
}

/// Whether the file at `path` should show a thumbnail under `settings`. A file of unknown size
/// never does, because the size limit could not be enforced before reading or downloading it.
pub fn wants_thumbnail(
    settings: &Settings,
    path: &Path,
    size: Option<u64>,
    is_remote: bool,
) -> bool {
    if !settings.thumbnails_enabled || (is_remote && !settings.thumbnail_remote_enabled) {
        return false;
    }
    let max_bytes = u64::from(settings.thumbnail_max_file_size_mb) * 1024 * 1024;
    if !size.is_some_and(|s| s <= max_bytes) {
        return false;
    }
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            settings
                .thumbnail_extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
}

/// Renders the local file at `path` into an RGBA picture that fits in `max_px` × `max_px`,
/// keeping its aspect ratio. A PDF yields its first page on a white background.
pub fn generate_thumbnail(path: &Path, max_px: u32) -> io::Result<RgbaImage> {
    let max_px = max_px.max(1);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    // The decoders run on arbitrary files from disk or a server; a panic inside one of them
    // must cost a thumbnail, not the whole file manager.
    panic::catch_unwind(AssertUnwindSafe(|| match ext.as_str() {
        "svg" => svg_thumbnail(path, max_px),
        "pdf" => pdf_thumbnail(path, max_px),
        _ => raster_thumbnail(path, max_px),
    }))
    .unwrap_or_else(|_| Err(invalid_data("thumbnail decoder panicked")))
}

/// Decodes and downscales a local image or SVG for the preview pane inside `max_px` × `max_px`,
/// enforcing strict allocation/complexity bounds and without upscaling small raster images.
pub fn generate_preview_image(path: &Path, max_px: u32) -> io::Result<RgbaImage> {
    let max_px = max_px.max(1);
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    panic::catch_unwind(AssertUnwindSafe(|| match ext.as_str() {
        "svg" => svg_thumbnail(path, max_px),
        _ => raster_image_bounded(path, max_px, false),
    }))
    .unwrap_or_else(|_| Err(invalid_data("preview image decoder panicked")))
}

fn raster_thumbnail(path: &Path, max_px: u32) -> io::Result<RgbaImage> {
    raster_image_bounded(path, max_px, true)
}

fn raster_image_bounded(path: &Path, max_px: u32, allow_upscale: bool) -> io::Result<RgbaImage> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits.max_image_width = Some(MAX_DECODE_DIMENSION);
    limits.max_image_height = Some(MAX_DECODE_DIMENSION);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(invalid_data)?;
    let orientation = decoder.orientation().map_err(invalid_data)?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(invalid_data)?;
    image.apply_orientation(orientation);
    if !allow_upscale && image.width() <= max_px && image.height() <= max_px {
        return Ok(image.into_rgba8());
    }
    Ok(image.thumbnail(max_px, max_px).into_rgba8())
}

fn validate_svg_complexity(
    group: &usvg::Group,
    nodes: &mut usize,
    filters: &mut usize,
) -> io::Result<()> {
    *filters = filters.saturating_add(group.filters().len());
    if *filters > MAX_SVG_FILTERS {
        return Err(invalid_data("SVG has too many filters"));
    }
    for child in group.children() {
        *nodes += 1;
        if *nodes > MAX_SVG_NODES {
            return Err(invalid_data("SVG has too many nodes"));
        }
        if let usvg::Node::Group(subgroup) = child {
            validate_svg_complexity(subgroup, nodes, filters)?;
        }
    }
    Ok(())
}

fn svg_thumbnail(path: &Path, max_px: u32) -> io::Result<RgbaImage> {
    let len = fs::metadata(path)?.len();
    if len > MAX_SVG_BYTES {
        return Err(invalid_data("SVG too large"));
    }
    let data = fs::read(path)?;
    let tree = usvg::Tree::from_data(&data, &usvg::Options::default()).map_err(invalid_data)?;
    let size = tree.size();
    if !(size.width().is_finite()
        && size.height().is_finite()
        && size.width() > 0.0
        && size.height() > 0.0)
    {
        return Err(invalid_data("SVG has invalid dimensions"));
    }
    let mut nodes = 0;
    let mut filters = 0;
    validate_svg_complexity(tree.root(), &mut nodes, &mut filters)?;

    let scale = max_px as f32 / size.width().max(size.height());
    let width = ((size.width() * scale).round() as u32).clamp(1, max_px);
    let height = ((size.height() * scale).round() as u32).clamp(1, max_px);
    let mut pixmap =
        tiny_skia::Pixmap::new(width, height).ok_or_else(|| invalid_data("empty SVG canvas"))?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let rgba = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let c = pixel.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect();
    RgbaImage::from_raw(width, height, rgba).ok_or_else(|| invalid_data("SVG buffer size"))
}

fn pdf_thumbnail(path: &Path, max_px: u32) -> io::Result<RgbaImage> {
    let len = fs::metadata(path)?.len();
    if len > MAX_PDF_BYTES {
        return Err(invalid_data("PDF too large"));
    }
    let pdf = Pdf::new(fs::read(path)?).map_err(|err| invalid_data(format!("{err:?}")))?;
    let page = pdf
        .pages()
        .first()
        .ok_or_else(|| invalid_data("PDF has no pages"))?;
    let (width, height) = page.render_dimensions();
    if !(width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0) {
        return Err(invalid_data("PDF page has no area"));
    }
    if page
        .page_stream()
        .is_some_and(|stream| stream.len() > MAX_PDF_PAGE_STREAM_BYTES)
    {
        return Err(invalid_data("PDF page content stream too complex"));
    }
    let scale = max_px as f32 / width.max(height);
    let pixmap = hayro::render(
        page,
        &RenderCache::new(),
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    RgbaImage::from_raw(width, height, pixmap.take_rgba8(ImageAlphaType::Alpha))
        .ok_or_else(|| invalid_data("PDF buffer size"))
}

fn invalid_data(err: impl ToString) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, err.to_string())
}
