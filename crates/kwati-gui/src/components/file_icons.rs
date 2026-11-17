//! Maps filesystem paths to their semantic [`IconName`] for folders and file types (#111).

use std::path::{Path, PathBuf};

use crate::components::IconName;

/// Standard user directory paths used to recognize special folders without repeating OS lookups
/// on every row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecialFolders {
    pub home: Option<PathBuf>,
    pub desktop: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub pictures: Option<PathBuf>,
    pub music: Option<PathBuf>,
    pub videos: Option<PathBuf>,
    pub public: Option<PathBuf>,
}

impl SpecialFolders {
    /// Resolves standard user directories for the current environment using `dirs`.
    pub fn from_system() -> Self {
        Self {
            home: dirs::home_dir(),
            desktop: dirs::desktop_dir(),
            documents: dirs::document_dir(),
            downloads: dirs::download_dir(),
            pictures: dirs::picture_dir(),
            music: dirs::audio_dir(),
            videos: dirs::video_dir(),
            public: dirs::public_dir(),
        }
    }
}

/// Returns the built-in [`IconName`] for a file or folder at `path`.
pub fn icon_for(path: &Path, is_dir: bool) -> IconName {
    icon_for_with(path, is_dir, &SpecialFolders::from_system())
}

/// Returns the built-in [`IconName`] for `path` using a precomputed [`SpecialFolders`] table.
pub fn icon_for_with(path: &Path, is_dir: bool, specials: &SpecialFolders) -> IconName {
    if is_dir {
        return folder_icon_for(path, specials);
    }
    file_icon_for(path)
}

fn folder_icon_for(path: &Path, specials: &SpecialFolders) -> IconName {
    // Check home before other XDG dirs in case an unconfigured XDG dir aliases $HOME.
    if specials.home.as_deref() == Some(path) {
        return IconName::FolderHome;
    }
    if specials.desktop.as_deref() == Some(path) {
        return IconName::FolderDesktop;
    }
    if specials.documents.as_deref() == Some(path) {
        return IconName::FolderDocuments;
    }
    if specials.downloads.as_deref() == Some(path) {
        return IconName::FolderDownloads;
    }
    if specials.pictures.as_deref() == Some(path) {
        return IconName::FolderPictures;
    }
    if specials.music.as_deref() == Some(path) {
        return IconName::FolderMusic;
    }
    if specials.videos.as_deref() == Some(path) {
        return IconName::FolderVideos;
    }
    if specials.public.as_deref() == Some(path) {
        return IconName::FolderPublic;
    }

    let raw = path.to_string_lossy();
    if raw.starts_with("//") || raw.starts_with(r"\\") {
        return IconName::FolderNetwork;
    }

    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "projects" | "src" | "dev" | "repos"
        ) {
            return IconName::FolderProjects;
        }
    }

    if path.join(".git").symlink_metadata().is_ok() {
        return IconName::FolderProjects;
    }

    IconName::Folder
}

fn file_icon_for(path: &Path) -> IconName {
    let file_name = path.file_name().and_then(|n| n.to_str());
    if let Some(name) = file_name {
        if matches!(name, "id_rsa" | "id_ed25519") {
            return IconName::FileKey;
        }
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let lower = ext.to_ascii_lowercase();
        if let Some(icon) = icon_for_extension(&lower) {
            return icon;
        }
    }

    if let Some(name) = file_name {
        if name.starts_with('.') && name != "." && name != ".." {
            return IconName::FileConfig;
        }
    }

    if path.extension().is_none() && is_executable_file(path) {
        return IconName::FileBinary;
    }

    IconName::File
}

fn icon_for_extension(ext: &str) -> Option<IconName> {
    match ext {
        "txt" | "text" => Some(IconName::FileText),
        "md" | "markdown" | "mdx" => Some(IconName::FileMarkdown),
        "odt" | "doc" | "docx" | "rtf" | "pages" => Some(IconName::FileDocument),
        "pdf" => Some(IconName::FilePdf),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "avif"
        | "ico" => Some(IconName::FileImage),
        "svg" | "eps" | "ai" => Some(IconName::FileVector),
        "rs" | "c" | "h" | "cpp" | "hpp" | "go" | "java" | "kt" | "swift" | "cs" | "zig" => {
            Some(IconName::FileCode)
        }
        "sh" | "bash" | "zsh" | "fish" | "py" | "rb" | "pl" | "lua" | "ps1" | "bat" => {
            Some(IconName::FileScript)
        }
        "html" | "htm" | "css" | "scss" | "js" | "mjs" | "ts" | "jsx" | "tsx" | "vue" => {
            Some(IconName::FileWeb)
        }
        "json" | "jsonc" | "yaml" | "yml" | "toml" | "xml" => Some(IconName::FileData),
        "ini" | "conf" | "cfg" | "env" => Some(IconName::FileConfig),
        "db" | "sqlite" | "sqlite3" | "sql" => Some(IconName::FileDatabase),
        "ods" | "xls" | "xlsx" | "csv" | "tsv" => Some(IconName::FileSpreadsheet),
        "odp" | "ppt" | "pptx" => Some(IconName::FilePresentation),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => {
            Some(IconName::FileArchive)
        }
        "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" => Some(IconName::FileAudio),
        "mp4" | "mkv" | "webm" | "mov" | "avi" => Some(IconName::FileVideo),
        "ttf" | "otf" | "woff" | "woff2" => Some(IconName::FileFont),
        "bin" | "so" | "dll" | "o" | "a" | "exe" => Some(IconName::FileBinary),
        "deb" | "rpm" | "apk" | "msi" | "flatpak" | "appimage" => Some(IconName::FilePackage),
        "iso" | "img" | "dmg" | "qcow2" | "vdi" => Some(IconName::FileDiskImage),
        "pem" | "key" | "pub" | "crt" | "cer" | "gpg" | "asc" => Some(IconName::FileKey),
        "epub" | "mobi" | "azw3" | "djvu" => Some(IconName::FileEbook),
        "log" => Some(IconName::FileLog),
        "part" | "crdownload" | "download" => Some(IconName::FilePartial),
        _ => None,
    }
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111) != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn file_icons_extensions_and_fallbacks() {
        assert_eq!(
            icon_for(Path::new("photo.JPEG"), false),
            IconName::FileImage
        );
        assert_eq!(icon_for(Path::new("invoice.pdf"), false), IconName::FilePdf);
        assert_eq!(icon_for(Path::new("main.rs"), false), IconName::FileCode);
        assert_eq!(
            icon_for(Path::new("deploy.sh"), false),
            IconName::FileScript
        );
        assert_eq!(
            icon_for(Path::new("backup.tar.zst"), false),
            IconName::FileArchive
        );
        assert_eq!(icon_for(Path::new("notes.xyz"), false), IconName::File);
        assert_eq!(
            icon_for(Path::new(".gitignore"), false),
            IconName::FileConfig
        );
        assert_eq!(icon_for(Path::new("id_ed25519"), false), IconName::FileKey);
        assert_eq!(
            icon_for(Path::new("README.MD"), false),
            IconName::FileMarkdown
        );
        assert_eq!(
            icon_for(Path::new("Kwati.AppImage"), false),
            IconName::FilePackage
        );
    }

    #[test]
    fn file_icons_special_folders_and_non_special_names() {
        let downloads = dirs::download_dir().unwrap_or_else(|| PathBuf::from("/home/u/Downloads"));
        let specials = SpecialFolders {
            downloads: Some(downloads.clone()),
            ..SpecialFolders::default()
        };
        assert_eq!(
            icon_for_with(&downloads, true, &specials),
            IconName::FolderDownloads
        );
        if let Some(real_downloads) = dirs::download_dir() {
            assert_eq!(icon_for(&real_downloads, true), IconName::FolderDownloads);
        }
        assert_eq!(
            icon_for_with(Path::new("/tmp/Downloads"), true, &specials),
            IconName::Folder
        );

        let root = std::env::temp_dir().join(format!("kwati_file_icons_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let git_repo = root.join("my-repo");
        fs::create_dir_all(git_repo.join(".git")).unwrap();
        assert_eq!(
            icon_for_with(&git_repo, true, &specials),
            IconName::FolderProjects
        );

        let exec_file = root.join("tool");
        fs::write(&exec_file, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&exec_file).unwrap().permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&exec_file, perms).unwrap();
            assert_eq!(icon_for(&exec_file, false), IconName::FileBinary);
        }

        let _ = fs::remove_dir_all(&root);
    }
}
