use kwati_core::{classify_preview, read_preview, Preview, PREVIEW_BYTE_LIMIT};
use std::path::PathBuf;

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

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[test]
fn preview_truncates() {
    let test_dir = TestDir::new("preview_truncates");
    let base = test_dir.path();

    // Case 1: Pure ASCII file larger than 65,536 bytes.
    let large_ascii_path = base.join("large_ascii.txt");
    let full_ascii_content = vec![b'a'; 70_000];
    std::fs::write(&large_ascii_path, &full_ascii_content).unwrap();

    let preview = read_preview(&large_ascii_path).expect("reading preview succeeds");
    match preview {
        Preview::Text(text) => {
            assert_eq!(text.len(), PREVIEW_BYTE_LIMIT);
            assert_eq!(text.len(), 65_536);
            assert!(text.chars().all(|c| c == 'a'));
        }
        Preview::Binary => panic!("expected Preview::Text, got Preview::Binary"),
    }

    // Case 2: Multi-byte UTF-8 character (3 bytes: '€' = \xE2\x82\xAC) cut at byte 65,535 (1 byte in limit).
    let cut_3byte_path = base.join("cut_3byte.txt");
    let mut cut_3byte_content = vec![b'x'; 65_535];
    cut_3byte_content.extend_from_slice("€".as_bytes()); // 3 bytes: \xE2\x82\xAC
    cut_3byte_content.extend_from_slice(b"trailing content after 65536 limit");
    std::fs::write(&cut_3byte_path, &cut_3byte_content).unwrap();

    let preview = read_preview(&cut_3byte_path).expect("reading preview succeeds");
    match preview {
        Preview::Text(text) => {
            assert_eq!(text.len(), 65_535);
            assert!(text.chars().all(|c| c == 'x'));
        }
        Preview::Binary => panic!("expected Preview::Text, got Preview::Binary"),
    }

    // Case 3: Multi-byte UTF-8 character (3 bytes: '€' = \xE2\x82\xAC) cut at byte 65,534 (2 bytes in limit).
    let cut_3byte_2_path = base.join("cut_3byte_2.txt");
    let mut cut_3byte_2_content = vec![b'y'; 65_534];
    cut_3byte_2_content.extend_from_slice("€".as_bytes());
    cut_3byte_2_content.extend_from_slice(b"more trailing content");
    std::fs::write(&cut_3byte_2_path, &cut_3byte_2_content).unwrap();

    let preview = read_preview(&cut_3byte_2_path).expect("reading preview succeeds");
    match preview {
        Preview::Text(text) => {
            assert_eq!(text.len(), 65_534);
            assert!(text.chars().all(|c| c == 'y'));
        }
        Preview::Binary => panic!("expected Preview::Text, got Preview::Binary"),
    }

    // Case 4: 4-byte UTF-8 character ('🦀' = \xF0\x9F\x96\x80) cut at byte 65,533 (3 bytes in limit).
    let cut_4byte_path = base.join("cut_4byte.txt");
    let mut cut_4byte_content = vec![b'z'; 65_533];
    cut_4byte_content.extend_from_slice("🦀".as_bytes());
    cut_4byte_content.extend_from_slice(b"more trailing content");
    std::fs::write(&cut_4byte_path, &cut_4byte_content).unwrap();

    let preview = read_preview(&cut_4byte_path).expect("reading preview succeeds");
    match preview {
        Preview::Text(text) => {
            assert_eq!(text.len(), 65_533);
            assert!(text.chars().all(|c| c == 'z'));
        }
        Preview::Binary => panic!("expected Preview::Text, got Preview::Binary"),
    }

    // Case 5: 2-byte UTF-8 character ('é' = \xC3\xA9) ending exactly at 65,536 (not cut).
    let exact_boundary_path = base.join("exact_boundary.txt");
    let mut exact_boundary_content = vec![b'w'; 65_534];
    exact_boundary_content.extend_from_slice("é".as_bytes()); // 2 bytes -> total 65,536
    exact_boundary_content.extend_from_slice(b"content beyond limit");
    std::fs::write(&exact_boundary_path, &exact_boundary_content).unwrap();

    let preview = read_preview(&exact_boundary_path).expect("reading preview succeeds");
    match preview {
        Preview::Text(text) => {
            assert_eq!(text.len(), 65_536);
            assert!(text.ends_with('é'));
        }
        Preview::Binary => panic!("expected Preview::Text, got Preview::Binary"),
    }
}

#[test]
fn preview_binary() {
    let test_dir = TestDir::new("preview_binary");
    let base = test_dir.path();

    // Case 1: Short text file containing a NUL byte in the middle.
    let nul_file = base.join("nul_file.bin");
    std::fs::write(&nul_file, b"hello\0world").unwrap();

    let preview = read_preview(&nul_file).expect("reading preview succeeds");
    assert_eq!(preview, Preview::Binary);
    assert!(preview.is_binary());
    assert!(!preview.is_text());
    assert_eq!(preview.text(), None);

    // Case 2: File containing only a NUL byte.
    let single_nul = base.join("single_nul.bin");
    std::fs::write(&single_nul, b"\0").unwrap();

    let preview = read_preview(&single_nul).expect("reading preview succeeds");
    assert_eq!(preview, Preview::Binary);

    // Case 3: Large file with a NUL byte within the first 65,536 bytes.
    let large_nul = base.join("large_nul.bin");
    let mut large_nul_content = vec![b'a'; 70_000];
    large_nul_content[100] = 0;
    std::fs::write(&large_nul, &large_nul_content).unwrap();

    let preview = read_preview(&large_nul).expect("reading preview succeeds");
    assert_eq!(preview, Preview::Binary);

    // Case 4: File containing invalid UTF-8 bytes (not a truncated multi-byte sequence).
    let invalid_utf8 = base.join("invalid_utf8.bin");
    std::fs::write(&invalid_utf8, b"hello\xFFworld").unwrap();

    let preview = read_preview(&invalid_utf8).expect("reading preview succeeds");
    assert_eq!(preview, Preview::Binary);
}

#[test]
fn preview_empty_and_small() {
    let test_dir = TestDir::new("preview_empty_and_small");
    let base = test_dir.path();

    // Empty file
    let empty_file = base.join("empty.txt");
    std::fs::write(&empty_file, b"").unwrap();

    let preview = read_preview(&empty_file).expect("reading preview succeeds");
    assert_eq!(preview, Preview::Text(String::new()));
    assert!(preview.is_text());
    assert_eq!(preview.text(), Some(""));

    // Small normal text file
    let small_file = base.join("small.txt");
    std::fs::write(&small_file, "Kwatí file explorer preview\n").unwrap();

    let preview = read_preview(&small_file).expect("reading preview succeeds");
    assert_eq!(
        preview,
        Preview::Text("Kwatí file explorer preview\n".to_string())
    );
    assert_eq!(preview.text(), Some("Kwatí file explorer preview\n"));
}

#[test]
fn preview_missing_file() {
    let test_dir = TestDir::new("preview_missing_file");
    let missing_path = test_dir.path().join("does_not_exist.txt");

    let err = read_preview(&missing_path).expect_err("nonexistent file must fail");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}

#[test]
fn classify_preview_direct() {
    assert_eq!(classify_preview(b""), Preview::Text(String::new()));
    assert_eq!(
        classify_preview(b"plain ascii"),
        Preview::Text("plain ascii".to_string())
    );
    assert_eq!(classify_preview(b"with\0null"), Preview::Binary);
    assert_eq!(classify_preview(b"invalid\xffutf8"), Preview::Binary);

    // Truncated multi-byte UTF-8 at the end
    let bytes = b"hello \xE2\x82".to_vec();
    assert_eq!(
        classify_preview(&bytes),
        Preview::Text("hello ".to_string())
    );
}
