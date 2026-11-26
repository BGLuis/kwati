use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage};
use kwati_core::thumbnail::{
    generate_thumbnail, parse_thumbnail_extensions, wants_thumbnail, SUPPORTED_THUMBNAIL_EXTENSIONS,
};
use kwati_core::Settings;

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("kwati_test_{}_{}_{}", name, std::process::id(), id));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("failed to create test dir");
        Self { path }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

/// A one-page PDF of `width` × `height` points whose left half is filled blue.
fn write_pdf(path: &Path, width: u32, height: u32) {
    let content = format!("0 0 1 rg 0 0 {} {height} re f", width / 2);
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R >>"),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref_at = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        pdf.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    pdf.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    std::fs::write(path, pdf).unwrap();
}

fn assert_close(actual: &Rgba<u8>, expected: Rgba<u8>) {
    let close = actual
        .0
        .iter()
        .zip(expected.0)
        .all(|(a, e)| a.abs_diff(e) <= 8);
    assert!(close, "pixel {actual:?} is not close to {expected:?}");
}

#[test]
fn raster_thumbnail_fits_the_box_and_keeps_aspect_and_content() {
    let dir = TestDir::new("thumb_raster");
    let mut source = RgbaImage::from_pixel(800, 400, BLUE);
    for (x, _, pixel) in source.enumerate_pixels_mut() {
        if x < 400 {
            *pixel = RED;
        }
    }
    let path = dir.file("photo.png");
    source.save(&path).unwrap();

    let thumb = generate_thumbnail(&path, 100).unwrap();
    assert_eq!(thumb.dimensions(), (100, 50));
    assert_close(thumb.get_pixel(10, 25), RED);
    assert_close(thumb.get_pixel(90, 25), BLUE);
}

#[test]
fn raster_format_comes_from_content_not_extension() {
    let dir = TestDir::new("thumb_mislabeled");
    let path = dir.file("actually-png.jpg");
    RgbaImage::from_pixel(20, 20, RED)
        .save_with_format(&path, image::ImageFormat::Png)
        .unwrap();
    assert!(generate_thumbnail(&path, 10).is_ok());
}

#[test]
fn svg_thumbnail_scales_vector_to_the_box() {
    let dir = TestDir::new("thumb_svg");
    let path = dir.file("logo.svg");
    std::fs::write(
        &path,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="10"><rect width="10" height="10" fill="#0000ff"/></svg>"##,
    )
    .unwrap();

    let thumb = generate_thumbnail(&path, 40).unwrap();
    assert_eq!(thumb.dimensions(), (40, 20));
    assert_close(thumb.get_pixel(5, 10), BLUE);
    assert_eq!(
        thumb.get_pixel(35, 10).0[3],
        0,
        "uncovered area stays transparent"
    );
}

#[test]
fn pdf_thumbnail_renders_first_page_on_white() {
    let dir = TestDir::new("thumb_pdf");
    let path = dir.file("report.pdf");
    write_pdf(&path, 200, 100);

    let thumb = generate_thumbnail(&path, 64).unwrap();
    assert_eq!(thumb.dimensions(), (64, 32));
    assert_close(thumb.get_pixel(8, 16), BLUE);
    assert_close(thumb.get_pixel(56, 16), Rgba([255, 255, 255, 255]));
}

#[test]
fn corrupt_files_fail_instead_of_panicking() {
    let dir = TestDir::new("thumb_corrupt");
    for name in ["broken.png", "broken.svg", "broken.pdf"] {
        let path = dir.file(name);
        std::fs::write(&path, b"definitely not an image").unwrap();
        assert!(generate_thumbnail(&path, 64).is_err(), "{name}");
    }
    assert!(generate_thumbnail(&dir.file("missing.png"), 64).is_err());
}

#[test]
fn wants_thumbnail_honours_every_setting() {
    let mut settings = Settings::default();
    let photo = Path::new("/pics/Photo.JPG");
    let small = Some(1024);

    assert!(wants_thumbnail(&settings, photo, small, false));
    assert!(
        !wants_thumbnail(&settings, Path::new("/docs/notes.txt"), small, false),
        "extension not in the list"
    );
    assert!(
        !wants_thumbnail(&settings, Path::new("/pics/noext"), small, false),
        "no extension"
    );
    assert!(
        !wants_thumbnail(&settings, photo, None, false),
        "unknown size cannot be bounded"
    );

    let limit = u64::from(settings.thumbnail_max_file_size_mb) * 1024 * 1024;
    assert!(wants_thumbnail(&settings, photo, Some(limit), false));
    assert!(!wants_thumbnail(&settings, photo, Some(limit + 1), false));

    assert!(
        !wants_thumbnail(&settings, photo, small, true),
        "remote is opt-in"
    );
    settings.thumbnail_remote_enabled = true;
    assert!(wants_thumbnail(&settings, photo, small, true));

    settings.set_thumbnail_extensions("png").unwrap();
    assert!(!wants_thumbnail(&settings, photo, small, false));

    settings.set_thumbnail_extensions("jpg").unwrap();
    settings.thumbnails_enabled = false;
    assert!(!wants_thumbnail(&settings, photo, small, false));
}

#[test]
fn parse_extensions_normalizes_and_reports_unsupported() {
    assert_eq!(
        parse_thumbnail_extensions(" PNG,.jpg  png\tpdf ,, "),
        Ok(vec!["png".into(), "jpg".into(), "pdf".into()])
    );
    assert_eq!(parse_thumbnail_extensions(""), Ok(Vec::new()));
    assert_eq!(
        parse_thumbnail_extensions("png, HEIC, mp4, heic"),
        Err(vec!["heic".into(), "mp4".into()])
    );
    for ext in SUPPORTED_THUMBNAIL_EXTENSIONS {
        assert_eq!(parse_thumbnail_extensions(ext), Ok(vec![ext.to_string()]));
    }
}
