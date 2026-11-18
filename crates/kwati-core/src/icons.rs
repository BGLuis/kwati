//! Icon source preferences, preset storage (`icons.toml`), and freedesktop/image resolution (#112).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use toml::{Table, Value};

use crate::settings::write_atomic;

pub const CURRENT_ICONS_VERSION: u32 = 1;

/// Logical icon category for group-level overrides (#112, #113).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IconGroup {
    Folders,
    SpecialFolders,
    Images,
    Documents,
    Code,
    Archives,
    AudioVideo,
    Other,
}

impl IconGroup {
    pub const ALL: &'static [IconGroup] = &[
        IconGroup::Folders,
        IconGroup::SpecialFolders,
        IconGroup::Images,
        IconGroup::Documents,
        IconGroup::Code,
        IconGroup::Archives,
        IconGroup::AudioVideo,
        IconGroup::Other,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Folders => "folders",
            Self::SpecialFolders => "special_folders",
            Self::Images => "images",
            Self::Documents => "documents",
            Self::Code => "code",
            Self::Archives => "archives",
            Self::AudioVideo => "audio_video",
            Self::Other => "other",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "folders" => Some(Self::Folders),
            "special_folders" | "special-folders" | "specialfolders" => Some(Self::SpecialFolders),
            "images" => Some(Self::Images),
            "documents" => Some(Self::Documents),
            "code" => Some(Self::Code),
            "archives" => Some(Self::Archives),
            "audio_video" | "audio-video" | "audiovideo" | "audio_and_video" => {
                Some(Self::AudioVideo)
            }
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Folders => "Pastas",
            Self::SpecialFolders => "Pastas especiais",
            Self::Images => "Imagens",
            Self::Documents => "Documentos",
            Self::Code => "Código",
            Self::Archives => "Arquivos compactados",
            Self::AudioVideo => "Áudio e vídeo",
            Self::Other => "Outros",
        }
    }
}

/// Static descriptor for every overridable file/folder icon type from #111.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconTypeSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub group: IconGroup,
}

pub const ICON_TYPE_SPECS: &[IconTypeSpec] = &[
    IconTypeSpec {
        key: "folder",
        label: "Pasta padrão",
        group: IconGroup::Folders,
    },
    IconTypeSpec {
        key: "folder-home",
        label: "Pasta pessoal (Home)",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-desktop",
        label: "Área de trabalho",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-documents",
        label: "Documentos",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-downloads",
        label: "Downloads",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-pictures",
        label: "Imagens",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-music",
        label: "Músicas",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-videos",
        label: "Vídeos",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-public",
        label: "Público",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-projects",
        label: "Projetos",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "folder-network",
        label: "Pasta de rede",
        group: IconGroup::SpecialFolders,
    },
    IconTypeSpec {
        key: "file-image",
        label: "Imagem",
        group: IconGroup::Images,
    },
    IconTypeSpec {
        key: "file-vector",
        label: "Vetor (SVG/EPS/AI)",
        group: IconGroup::Images,
    },
    IconTypeSpec {
        key: "file-text",
        label: "Texto simples",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-markdown",
        label: "Markdown",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-document",
        label: "Documento",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-pdf",
        label: "PDF",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-spreadsheet",
        label: "Planilha",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-presentation",
        label: "Apresentação",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-ebook",
        label: "E-book",
        group: IconGroup::Documents,
    },
    IconTypeSpec {
        key: "file-code",
        label: "Código-fonte",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-script",
        label: "Script",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-web",
        label: "Web",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-data",
        label: "Dados (JSON/YAML/TOML)",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-config",
        label: "Configuração",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-database",
        label: "Banco de dados",
        group: IconGroup::Code,
    },
    IconTypeSpec {
        key: "file-archive",
        label: "Arquivo compactado",
        group: IconGroup::Archives,
    },
    IconTypeSpec {
        key: "file-package",
        label: "Pacote instalador",
        group: IconGroup::Archives,
    },
    IconTypeSpec {
        key: "file-disk-image",
        label: "Imagem de disco",
        group: IconGroup::Archives,
    },
    IconTypeSpec {
        key: "file-audio",
        label: "Áudio",
        group: IconGroup::AudioVideo,
    },
    IconTypeSpec {
        key: "file-video",
        label: "Vídeo",
        group: IconGroup::AudioVideo,
    },
    IconTypeSpec {
        key: "file",
        label: "Arquivo genérico",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-binary",
        label: "Binário / executável",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-font",
        label: "Fonte",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-key",
        label: "Chave / certificado",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-log",
        label: "Log",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-partial",
        label: "Download parcial",
        group: IconGroup::Other,
    },
    IconTypeSpec {
        key: "file-symlink",
        label: "Link simbólico",
        group: IconGroup::Other,
    },
];

/// Returns the [`IconGroup`] that owns `type_key` (normalizing aliases first).
pub fn group_for_type_key(type_key: &str) -> IconGroup {
    let key = normalize_type_key(type_key);
    ICON_TYPE_SPECS
        .iter()
        .find(|spec| spec.key == key)
        .map(|spec| spec.group)
        .unwrap_or_else(|| {
            if key == "folder" {
                IconGroup::Folders
            } else if key.starts_with("folder-") {
                IconGroup::SpecialFolders
            } else {
                IconGroup::Other
            }
        })
}

/// Normalizes a type key or hand-written extension alias (e.g. `".pdf"` -> `"file-pdf"`,
/// `"folder-download"` -> `"folder-downloads"`).
pub fn normalize_type_key(raw: &str) -> String {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if ICON_TYPE_SPECS.iter().any(|s| s.key == lower) {
        return lower;
    }
    match lower.as_str() {
        "folder-download" => return "folder-downloads".into(),
        "folder-document" => return "folder-documents".into(),
        "folder-picture" => return "folder-pictures".into(),
        "folder-video" => return "folder-videos".into(),
        "folder-publicshare" => return "folder-public".into(),
        "folder-remote" => return "folder-network".into(),
        _ => {}
    }

    let ext = lower.strip_prefix('.').unwrap_or(&lower);
    let mapped = match ext {
        "txt" | "text" => Some("file-text"),
        "md" | "markdown" | "mdx" => Some("file-markdown"),
        "odt" | "doc" | "docx" | "rtf" | "pages" => Some("file-document"),
        "pdf" => Some("file-pdf"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "avif"
        | "ico" => Some("file-image"),
        "svg" | "eps" | "ai" => Some("file-vector"),
        "rs" | "c" | "h" | "cpp" | "hpp" | "go" | "java" | "kt" | "swift" | "cs" | "zig" => {
            Some("file-code")
        }
        "sh" | "bash" | "zsh" | "fish" | "py" | "rb" | "pl" | "lua" | "ps1" | "bat" => {
            Some("file-script")
        }
        "html" | "htm" | "css" | "scss" | "js" | "mjs" | "ts" | "jsx" | "tsx" | "vue" => {
            Some("file-web")
        }
        "json" | "jsonc" | "yaml" | "yml" | "toml" | "xml" => Some("file-data"),
        "ini" | "conf" | "cfg" | "env" => Some("file-config"),
        "db" | "sqlite" | "sqlite3" | "sql" => Some("file-database"),
        "ods" | "xls" | "xlsx" | "csv" | "tsv" => Some("file-spreadsheet"),
        "odp" | "ppt" | "pptx" => Some("file-presentation"),
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" => Some("file-archive"),
        "mp3" | "flac" | "wav" | "ogg" | "opus" | "m4a" | "aac" => Some("file-audio"),
        "mp4" | "mkv" | "webm" | "mov" | "avi" => Some("file-video"),
        "ttf" | "otf" | "woff" | "woff2" => Some("file-font"),
        "bin" | "so" | "dll" | "o" | "a" | "exe" => Some("file-binary"),
        "deb" | "rpm" | "apk" | "msi" | "flatpak" | "appimage" => Some("file-package"),
        "iso" | "img" | "dmg" | "qcow2" | "vdi" => Some("file-disk-image"),
        "pem" | "key" | "pub" | "crt" | "cer" | "gpg" | "asc" => Some("file-key"),
        "epub" | "mobi" | "azw3" | "djvu" => Some("file-ebook"),
        "log" => Some("file-log"),
        "part" | "crdownload" | "download" => Some("file-partial"),
        _ => None,
    };
    mapped.map(str::to_string).unwrap_or(lower)
}

/// Ordered freedesktop icon names for `type_key`, always ending in a standard spec fallback.
pub fn freedesktop_candidates(type_key: &str) -> &'static [&'static str] {
    match type_key {
        "folder" => &["folder"],
        "folder-desktop" => &["user-desktop", "folder-desktop", "folder"],
        "folder-documents" => &["folder-documents", "folder-document", "folder"],
        "folder-downloads" => &["folder-download", "folder-downloads", "folder"],
        "folder-home" => &["user-home", "folder-home", "folder"],
        "folder-music" => &["folder-music", "folder-sound", "folder"],
        "folder-network" => &[
            "folder-remote",
            "folder-network",
            "network-workgroup",
            "folder",
        ],
        "folder-pictures" => &[
            "folder-pictures",
            "folder-picture",
            "folder-image",
            "folder",
        ],
        "folder-projects" => &[
            "folder-development",
            "folder-git",
            "folder-projects",
            "folder",
        ],
        "folder-public" => &["folder-publicshare", "folder-public", "folder"],
        "folder-videos" => &["folder-videos", "folder-video", "folder"],
        "file-text" => &["text-x-generic", "text-plain"],
        "file-markdown" => &["text-markdown", "text-x-markdown", "text-x-generic"],
        "file-document" => &["x-office-document", "application-msword", "text-x-generic"],
        "file-pdf" => &["application-pdf", "x-office-document", "text-x-generic"],
        "file-image" => &["image-x-generic", "image-png", "image-jpeg"],
        "file-vector" => &["image-svg+xml", "image-x-generic"],
        "file-code" => &[
            "text-x-script",
            "text-x-source",
            "text-x-csrc",
            "text-x-generic",
        ],
        "file-script" => &[
            "text-x-script",
            "application-x-shellscript",
            "text-x-generic",
        ],
        "file-web" => &["text-html", "text-css", "text-x-script", "text-x-generic"],
        "file-data" => &["application-json", "text-xml", "text-x-generic"],
        "file-config" => &["text-x-generic", "preferences-system"],
        "file-database" => &["application-sql", "application-x-sqlite3", "text-x-generic"],
        "file-spreadsheet" => &["x-office-spreadsheet", "text-csv", "text-x-generic"],
        "file-presentation" => &[
            "x-office-presentation",
            "x-office-document",
            "text-x-generic",
        ],
        "file-archive" => &[
            "package-x-generic",
            "application-zip",
            "application-x-archive",
        ],
        "file-audio" => &["audio-x-generic", "audio-mpeg"],
        "file-video" => &["video-x-generic", "video-mp4"],
        "file-font" => &["font-x-generic", "application-x-font-ttf"],
        "file-binary" => &["application-x-executable", "application-octet-stream"],
        "file-package" => &[
            "package-x-generic",
            "application-x-deb",
            "application-x-rpm",
        ],
        "file-disk-image" => &[
            "media-optical",
            "application-x-cd-image",
            "package-x-generic",
        ],
        "file-key" => &[
            "application-pgp-keys",
            "application-x-pem-key",
            "dialog-password",
        ],
        "file-ebook" => &[
            "application-epub+zip",
            "x-office-document",
            "text-x-generic",
        ],
        "file-log" => &["text-x-log", "text-x-generic"],
        "file-partial" => &["folder-download", "text-x-generic"],
        "file-symlink" => &["emblem-symbolic-link", "text-x-generic"],
        _ => &["text-x-generic"],
    }
}

/// Where an icon should be loaded from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IconSource {
    #[default]
    BuiltIn,
    /// Installed freedesktop icon theme name, or a custom theme directory path containing `index.theme`.
    Theme(String),
    /// Custom PNG or SVG image file path.
    Image(PathBuf),
}

impl Serialize for IconSource {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::BuiltIn => serializer.serialize_str("built-in"),
            Self::Theme(name) => {
                let mut t = Table::new();
                t.insert("theme".into(), Value::String(name.clone()));
                t.serialize(serializer)
            }
            Self::Image(path) => {
                let mut t = Table::new();
                t.insert(
                    "image".into(),
                    Value::String(path.to_string_lossy().into_owned()),
                );
                t.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for IconSource {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = Value::deserialize(deserializer)?;
        parse_icon_source_value(&val).map_err(serde::de::Error::custom)
    }
}

fn parse_icon_source_value(val: &Value) -> Result<IconSource, String> {
    match val {
        Value::String(s) => {
            let trimmed = s.trim();
            let lower = trimmed.to_ascii_lowercase();
            if lower == "built-in" || lower == "builtin" {
                Ok(IconSource::BuiltIn)
            } else if let Some(rest) = trimmed.strip_prefix("theme:") {
                let name = rest.trim();
                if name.is_empty() {
                    Err("theme name cannot be empty".into())
                } else {
                    Ok(IconSource::Theme(name.to_string()))
                }
            } else if let Some(rest) = trimmed.strip_prefix("image:") {
                let path = rest.trim();
                if path.is_empty() {
                    Err("image path cannot be empty".into())
                } else {
                    Ok(IconSource::Image(PathBuf::from(path)))
                }
            } else {
                Err(format!(
                    "expected \"built-in\", {{ theme = \"...\" }}, or {{ image = \"...\" }}, got \"{trimmed}\""
                ))
            }
        }
        Value::Table(t) => {
            if let Some(Value::String(theme)) = t.get("theme") {
                if theme.trim().is_empty() {
                    return Err("theme name cannot be empty".into());
                }
                return Ok(IconSource::Theme(theme.trim().to_string()));
            }
            if let Some(Value::String(image)) = t.get("image") {
                if image.trim().is_empty() {
                    return Err("image path cannot be empty".into());
                }
                return Ok(IconSource::Image(PathBuf::from(image.trim())));
            }
            Err("icon source table must contain \"theme\" or \"image\"".into())
        }
        _ => Err("icon source must be a string or table".into()),
    }
}

/// Color specification for built-in and symbolic icons (#112).
///
/// Supports semantic palette tokens (`"text"`, `"muted"`, `"accent"`, `"secondary"`,
/// `"danger"`, `"warning"`), fixed hex RGB values (`"#RRGGBB"`), or light/dark theme pairs
/// (`{ light = "#RRGGBB", dark = "#RRGGBB" }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconColor {
    Text,
    Muted,
    Accent,
    Secondary,
    Danger,
    Warning,
    /// Explicit 24-bit RGB color (`0xRRGGBB`).
    Hex(u32),
    /// Theme-adaptive 24-bit RGB pair (`0xRRGGBB` for light and dark themes).
    Adaptive {
        light: u32,
        dark: u32,
    },
}

impl IconColor {
    pub const fn rgb(hex: u32) -> Self {
        Self::Hex(hex & 0x00FF_FFFF)
    }

    pub const fn adaptive(light: u32, dark: u32) -> Self {
        Self::Adaptive {
            light: light & 0x00FF_FFFF,
            dark: dark & 0x00FF_FFFF,
        }
    }

    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        let lower = trimmed.to_ascii_lowercase();
        match lower.as_str() {
            "text" => return Ok(Self::Text),
            "muted" | "text_muted" | "text-muted" => return Ok(Self::Muted),
            "accent" => return Ok(Self::Accent),
            "secondary" => return Ok(Self::Secondary),
            "danger" => return Ok(Self::Danger),
            "warning" => return Ok(Self::Warning),
            _ => {}
        }
        parse_hex_rgb(trimmed).map(Self::Hex)
    }
}

impl std::fmt::Display for IconColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => f.write_str("text"),
            Self::Muted => f.write_str("muted"),
            Self::Accent => f.write_str("accent"),
            Self::Secondary => f.write_str("secondary"),
            Self::Danger => f.write_str("danger"),
            Self::Warning => f.write_str("warning"),
            Self::Hex(rgb) => write!(f, "#{:06X}", rgb & 0x00FF_FFFF),
            Self::Adaptive { light, dark } => write!(
                f,
                "light=#{:06X}, dark=#{:06X}",
                light & 0x00FF_FFFF,
                dark & 0x00FF_FFFF
            ),
        }
    }
}

impl Serialize for IconColor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Text => serializer.serialize_str("text"),
            Self::Muted => serializer.serialize_str("muted"),
            Self::Accent => serializer.serialize_str("accent"),
            Self::Secondary => serializer.serialize_str("secondary"),
            Self::Danger => serializer.serialize_str("danger"),
            Self::Warning => serializer.serialize_str("warning"),
            Self::Hex(rgb) => serializer.serialize_str(&format!("#{:06X}", rgb & 0x00FF_FFFF)),
            Self::Adaptive { light, dark } => {
                let mut t = Table::new();
                t.insert(
                    "light".into(),
                    Value::String(format!("#{:06X}", light & 0x00FF_FFFF)),
                );
                t.insert(
                    "dark".into(),
                    Value::String(format!("#{:06X}", dark & 0x00FF_FFFF)),
                );
                t.serialize(serializer)
            }
        }
    }
}

impl<'de> Deserialize<'de> for IconColor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let val = Value::deserialize(deserializer)?;
        parse_icon_color_value(&val).map_err(serde::de::Error::custom)
    }
}

fn parse_hex_rgb(raw: &str) -> Result<u32, String> {
    let hex = raw.trim().strip_prefix('#').unwrap_or_else(|| raw.trim());
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "expected color token or \"#RRGGBB\" hex color, got \"{raw}\""
        ));
    }
    u32::from_str_radix(hex, 16)
        .map(|v| v & 0x00FF_FFFF)
        .map_err(|e| format!("invalid hex color \"{raw}\": {e}"))
}

fn parse_icon_color_value(val: &Value) -> Result<IconColor, String> {
    match val {
        Value::String(s) => IconColor::parse(s),
        Value::Table(t) => {
            let light_str = t
                .get("light")
                .and_then(Value::as_str)
                .ok_or_else(|| "adaptive icon color must contain string \"light\"".to_string())?;
            let dark_str = t
                .get("dark")
                .and_then(Value::as_str)
                .ok_or_else(|| "adaptive icon color must contain string \"dark\"".to_string())?;
            Ok(IconColor::adaptive(
                parse_hex_rgb(light_str)?,
                parse_hex_rgb(dark_str)?,
            ))
        }
        _ => Err("icon color must be a string or { light, dark } table".into()),
    }
}

/// Returns the default theme-aware [`IconColor`] for `type_key` (normalizing aliases first).
///
/// Every color pair maintains at least 4.5:1 contrast on light and dark primary surfaces.
pub fn default_icon_color(type_key: &str) -> IconColor {
    let key = normalize_type_key(type_key);
    if key == "folder" || key.starts_with("folder-") {
        return IconColor::Accent;
    }
    match key.as_str() {
        "file-pdf" => IconColor::adaptive(0xB3261E, 0xF2746B),
        "file-document" => IconColor::adaptive(0x1E5AA8, 0x7AAEF2),
        "file-spreadsheet" => IconColor::adaptive(0x2D6A3E, 0x74C289),
        "file-presentation" => IconColor::adaptive(0xA64214, 0xF09459),
        "file-markdown" => IconColor::adaptive(0x435399, 0x92A4F0),
        "file-ebook" => IconColor::adaptive(0x7A3E8F, 0xC88CE0),
        "file-image" => IconColor::adaptive(0x0F6E6E, 0x5EC7BD),
        "file-vector" => IconColor::adaptive(0x9C2D6D, 0xE384BB),
        "file-code" => IconColor::adaptive(0x146487, 0x6BBCE0),
        "file-script" => IconColor::Secondary,
        "file-web" => IconColor::adaptive(0x964B00, 0xE8A84C),
        "file-data" | "file-key" => IconColor::Warning,
        "file-config" | "file-log" | "file-partial" => IconColor::Muted,
        "file-database" => IconColor::adaptive(0x5B42A3, 0xA895F0),
        "file-archive" => IconColor::adaptive(0x8F5314, 0xE09C52),
        "file-package" => IconColor::adaptive(0x9E431E, 0xE0824F),
        "file-disk-image" => IconColor::adaptive(0x465C70, 0x92B2CC),
        "file-audio" => IconColor::adaptive(0x763D99, 0xC48EE8),
        "file-video" => IconColor::adaptive(0xA32956, 0xEA7CA4),
        "file-binary" => IconColor::adaptive(0x2B6B5F, 0x6EBFA8),
        "file-font" => IconColor::adaptive(0x824163, 0xD48AB0),
        "file-symlink" => IconColor::adaptive(0x0F6B82, 0x68C3D9),
        _ => IconColor::Text,
    }
}

/// Active icon resolution preferences: base source/color + optional per-group and per-type overrides.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IconPreferences {
    pub base: IconSource,
    pub groups: BTreeMap<IconGroup, IconSource>,
    pub types: BTreeMap<String, IconSource>,
    pub base_color: Option<IconColor>,
    pub group_colors: BTreeMap<IconGroup, IconColor>,
    pub type_colors: BTreeMap<String, IconColor>,
}

impl IconPreferences {
    /// Returns the effective [`IconSource`] for `type_key` using precedence:
    /// type override -> group override -> base source (which defaults to [`IconSource::BuiltIn`]).
    pub fn effective_source(&self, type_key: &str) -> &IconSource {
        let canonical = normalize_type_key(type_key);
        if let Some(src) = self.types.get(&canonical) {
            return src;
        }
        if let Some(src) = self.types.get(type_key) {
            return src;
        }
        let group = group_for_type_key(&canonical);
        if let Some(src) = self.groups.get(&group) {
            return src;
        }
        &self.base
    }

    /// Returns the effective [`IconColor`] for `type_key` using precedence:
    /// type color override -> group color override -> base color override -> [`default_icon_color`].
    pub fn effective_color(&self, type_key: &str) -> IconColor {
        let canonical = normalize_type_key(type_key);
        if let Some(&color) = self.type_colors.get(&canonical) {
            return color;
        }
        if let Some(&color) = self.type_colors.get(type_key) {
            return color;
        }
        let group = group_for_type_key(&canonical);
        if let Some(&color) = self.group_colors.get(&group) {
            return color;
        }
        if let Some(color) = self.base_color {
            return color;
        }
        default_icon_color(&canonical)
    }

    pub fn set_group_override(&mut self, group: IconGroup, source: Option<IconSource>) {
        match source {
            Some(src) => {
                self.groups.insert(group, src);
            }
            None => {
                self.groups.remove(&group);
            }
        }
    }

    pub fn set_type_override(&mut self, type_key: &str, source: Option<IconSource>) {
        let canonical = normalize_type_key(type_key);
        match source {
            Some(src) => {
                self.types.insert(canonical, src);
            }
            None => {
                self.types.remove(&canonical);
                self.types.remove(type_key);
            }
        }
    }

    pub fn set_base_color(&mut self, color: Option<IconColor>) {
        self.base_color = color;
    }

    pub fn set_group_color_override(&mut self, group: IconGroup, color: Option<IconColor>) {
        match color {
            Some(c) => {
                self.group_colors.insert(group, c);
            }
            None => {
                self.group_colors.remove(&group);
            }
        }
    }

    pub fn set_type_color_override(&mut self, type_key: &str, color: Option<IconColor>) {
        let canonical = normalize_type_key(type_key);
        match color {
            Some(c) => {
                self.type_colors.insert(canonical, c);
            }
            None => {
                self.type_colors.remove(&canonical);
                self.type_colors.remove(type_key);
            }
        }
    }

    fn to_toml_table(&self) -> Table {
        let mut table = Table::new();
        table.insert(
            "base".into(),
            Value::try_from(&self.base).unwrap_or(Value::String("built-in".into())),
        );
        if let Some(base_color) = &self.base_color {
            if let Ok(val) = Value::try_from(base_color) {
                table.insert("base_color".into(), val);
            }
        }
        insert_group_map(&mut table, "groups", &self.groups);
        insert_type_map(&mut table, "types", &self.types);
        insert_group_map(&mut table, "group_colors", &self.group_colors);
        insert_type_map(&mut table, "type_colors", &self.type_colors);
        table
    }

    fn from_toml_table(table: &Table) -> io::Result<Self> {
        let base = match table.get("base") {
            None => IconSource::BuiltIn,
            Some(val) => parse_icon_source_value(val)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
        };
        let base_color = match table.get("base_color") {
            None => None,
            Some(val) => Some(
                parse_icon_color_value(val)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?,
            ),
        };

        let groups = parse_group_map(table, "groups", parse_icon_source_value)?;
        let types = parse_type_map(table, "types", parse_icon_source_value)?;
        let group_colors = parse_group_map(table, "group_colors", parse_icon_color_value)?;
        let type_colors = parse_type_map(table, "type_colors", parse_icon_color_value)?;

        Ok(Self {
            base,
            groups,
            types,
            base_color,
            group_colors,
            type_colors,
        })
    }
}

fn insert_group_map<V: Serialize>(table: &mut Table, field: &str, map: &BTreeMap<IconGroup, V>) {
    if map.is_empty() {
        return;
    }
    let mut out = Table::new();
    for (group, v) in map {
        if let Ok(val) = Value::try_from(v) {
            out.insert(group.as_str().into(), val);
        }
    }
    table.insert(field.into(), Value::Table(out));
}

fn insert_type_map<V: Serialize>(table: &mut Table, field: &str, map: &BTreeMap<String, V>) {
    if map.is_empty() {
        return;
    }
    let mut out = Table::new();
    for (key, v) in map {
        if let Ok(val) = Value::try_from(v) {
            out.insert(key.clone(), val);
        }
    }
    table.insert(field.into(), Value::Table(out));
}

fn parse_group_map<V>(
    table: &Table,
    field: &str,
    parse_val: fn(&Value) -> Result<V, String>,
) -> io::Result<BTreeMap<IconGroup, V>> {
    let mut out = BTreeMap::new();
    let Some(val) = table.get(field) else {
        return Ok(out);
    };
    let tbl = val.as_table().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} must be a table"),
        )
    })?;
    for (k, v) in tbl {
        let group = IconGroup::parse(k).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unknown icon group '{k}'"),
            )
        })?;
        let parsed = parse_val(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        out.insert(group, parsed);
    }
    Ok(out)
}

fn parse_type_map<V>(
    table: &Table,
    field: &str,
    parse_val: fn(&Value) -> Result<V, String>,
) -> io::Result<BTreeMap<String, V>> {
    let mut out = BTreeMap::new();
    let Some(val) = table.get(field) else {
        return Ok(out);
    };
    let tbl = val.as_table().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{field} must be a table"),
        )
    })?;
    for (k, v) in tbl {
        let parsed = parse_val(v).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        out.insert(normalize_type_key(k), parsed);
    }
    Ok(out)
}

/// Full persisted icon configuration (`<config dir>/kwati/icons.toml`), holding the active
/// preferences and named presets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconConfig {
    pub version: u32,
    pub active: IconPreferences,
    pub presets: BTreeMap<String, IconPreferences>,
}

impl Default for IconConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_ICONS_VERSION,
            active: IconPreferences::default(),
            presets: BTreeMap::new(),
        }
    }
}

/// `<config dir>/kwati/icons.toml`, beside `settings.toml`, or `None` when no config dir exists.
pub fn default_icons_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("kwati").join("icons.toml"))
}

impl IconConfig {
    /// Reads icon preferences and presets from `path`. A missing file yields built-in defaults.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(err),
        };
        let table: Table = text
            .parse()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;

        let version = match table.get("version") {
            None => CURRENT_ICONS_VERSION,
            Some(v) => v
                .as_integer()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "version must be a positive integer",
                    )
                })?,
        };
        let active = IconPreferences::from_toml_table(&table)?;

        let mut presets = BTreeMap::new();
        if let Some(val) = table.get("presets") {
            let tbl = val.as_table().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "presets must be a table")
            })?;
            for (name, preset_val) in tbl {
                let preset_tbl = preset_val.as_table().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("preset '{name}' must be a table"),
                    )
                })?;
                presets.insert(name.clone(), IconPreferences::from_toml_table(preset_tbl)?);
            }
        }

        Ok(Self {
            version,
            active,
            presets,
        })
    }

    /// Atomically saves `self` to `path`.
    pub fn save_to(&self, path: &Path) -> io::Result<()> {
        let mut table = self.active.to_toml_table();
        table.insert("version".into(), Value::Integer(self.version.into()));
        if !self.presets.is_empty() {
            let mut presets = Table::new();
            for (name, prefs) in &self.presets {
                presets.insert(name.clone(), Value::Table(prefs.to_toml_table()));
            }
            table.insert("presets".into(), Value::Table(presets));
        }
        let text = toml::to_string(&table)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        write_atomic(path, &text, || Ok(()))
    }

    /// Saves the active preferences under `name`.
    pub fn save_preset(&mut self, name: impl Into<String>) -> Result<(), String> {
        let name = name.into().trim().to_string();
        if name.is_empty() {
            return Err("O nome do preset não pode ficar vazio.".into());
        }
        self.presets.insert(name, self.active.clone());
        Ok(())
    }

    /// Applies the saved preset `name` to `self.active`.
    pub fn apply_preset(&mut self, name: &str) -> Result<(), String> {
        let Some(preset) = self.presets.get(name) else {
            return Err(format!("Preset '{name}' não encontrado."));
        };
        self.active = preset.clone();
        Ok(())
    }

    /// Renames preset `old_name` to `new_name`, keeping its preferences intact.
    pub fn rename_preset(
        &mut self,
        old_name: &str,
        new_name: impl Into<String>,
    ) -> Result<(), String> {
        let new_name = new_name.into().trim().to_string();
        if new_name.is_empty() {
            return Err("O nome do preset não pode ficar vazio.".into());
        }
        if !self.presets.contains_key(old_name) {
            return Err(format!("Preset '{old_name}' não encontrado."));
        }
        if old_name == new_name {
            return Ok(());
        }
        if self.presets.contains_key(&new_name) {
            return Err(format!("Já existe um preset chamado '{new_name}'."));
        }
        let prefs = self.presets.remove(old_name).expect("verified above");
        self.presets.insert(new_name, prefs);
        Ok(())
    }

    /// Deletes preset `name` without changing `self.active`.
    pub fn delete_preset(&mut self, name: &str) -> bool {
        self.presets.remove(name).is_some()
    }

    /// Resets `self.active` back to built-in defaults while preserving saved presets.
    pub fn reset_active(&mut self) {
        self.active = IconPreferences::default();
    }
}

/// Output of resolving a type key against active preferences and installed themes/images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedIcon {
    BuiltIn,
    Image { path: PathBuf, is_symbolic: bool },
}

/// True when `path` is an SVG following freedesktop `-symbolic` naming.
pub fn is_symbolic_icon_path(path: &Path) -> bool {
    let is_svg = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
    if !is_svg {
        return false;
    }
    path.file_stem()
        .and_then(|s| s.to_str())
        .is_some_and(|stem| stem.to_ascii_lowercase().ends_with("-symbolic"))
}

/// Lists installed freedesktop icon theme names on Linux; returns an empty list on Windows/macOS.
pub fn list_installed_themes() -> Vec<String> {
    #[cfg(target_os = "linux")]
    {
        let mut themes = freedesktop_icons::list_themes();
        themes.sort_by_cached_key(|t| t.to_ascii_lowercase());
        themes.dedup();
        themes
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// Resolves icon sources to built-in or on-disk icon paths with per-(theme, key, size) caching.
#[derive(Debug, Default)]
pub struct IconResolver {
    base_paths: Option<Vec<PathBuf>>,
    cache: Mutex<HashMap<(String, String, u16), Option<PathBuf>>>,
}

impl IconResolver {
    /// Creates a resolver backed by system icon themes on Linux and custom directories everywhere.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a resolver restricted to `base_paths` (useful for tests and custom theme roots).
    pub fn with_base_paths(base_paths: Vec<PathBuf>) -> Self {
        Self {
            base_paths: Some(base_paths),
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Clears the resolved path cache (for instance when themes or preferences change).
    pub fn clear_cache(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            guard.clear();
        }
    }

    /// Resolves `type_key` at `size` px using `prefs`, falling back to [`ResolvedIcon::BuiltIn`]
    /// if a custom image or theme icon cannot be found.
    pub fn resolve(&self, prefs: &IconPreferences, type_key: &str, size: u16) -> ResolvedIcon {
        let canonical = normalize_type_key(type_key);
        match prefs.effective_source(&canonical) {
            IconSource::BuiltIn => ResolvedIcon::BuiltIn,
            IconSource::Image(path) => {
                if path.is_file() {
                    ResolvedIcon::Image {
                        path: path.clone(),
                        is_symbolic: is_symbolic_icon_path(path),
                    }
                } else {
                    ResolvedIcon::BuiltIn
                }
            }
            IconSource::Theme(theme) => match self.resolve_theme(theme, &canonical, size) {
                Some(path) => ResolvedIcon::Image {
                    is_symbolic: is_symbolic_icon_path(&path),
                    path,
                },
                None => ResolvedIcon::BuiltIn,
            },
        }
    }

    fn resolve_theme(&self, theme: &str, type_key: &str, size: u16) -> Option<PathBuf> {
        let cache_key = (theme.to_string(), type_key.to_string(), size);
        if let Ok(guard) = self.cache.lock() {
            if let Some(cached) = guard.get(&cache_key) {
                return cached.clone();
            }
        }

        let resolved = self.lookup_theme_uncached(theme, type_key, size);
        if let Ok(mut guard) = self.cache.lock() {
            guard.insert(cache_key, resolved.clone());
        }
        resolved
    }

    fn lookup_theme_uncached(&self, theme: &str, type_key: &str, size: u16) -> Option<PathBuf> {
        let candidates = freedesktop_candidates(type_key);

        if let Some(base_paths) = &self.base_paths {
            return lookup_in_base_paths(base_paths, theme, candidates, size);
        }

        // Cross-platform support for pointing Theme(...) directly at a theme directory.
        let as_path = Path::new(theme);
        if as_path.is_dir() && as_path.join("index.theme").is_file() {
            let mut bases = Vec::new();
            if let Some(parent) = as_path.parent() {
                bases.push(parent.to_path_buf());
            }
            if let Some(found) = lookup_direct_theme_dir(as_path, &bases, candidates, size) {
                return Some(found);
            }
        }

        #[cfg(target_os = "linux")]
        {
            lookup_system_linux(theme, candidates, size)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (candidates, size);
            None
        }
    }
}

#[cfg(target_os = "linux")]
fn lookup_system_linux(theme: &str, candidates: &[&str], size: u16) -> Option<PathBuf> {
    let dir_name = resolve_linux_theme_dir_name(theme);
    let theme_arg = dir_name.as_deref().unwrap_or(theme);
    for candidate in candidates {
        if let Some(path) = freedesktop_icons::lookup(candidate)
            .with_size(size)
            .with_theme(theme_arg)
            .with_cache()
            .find()
        {
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

/// Maps a theme's display `Name=` (as returned by `freedesktop_icons::list_themes()`, e.g.
/// `"Breeze Dark"`) to its directory name (`"breeze-dark"`), which `lookup().with_theme()` expects.
#[cfg(target_os = "linux")]
fn resolve_linux_theme_dir_name(theme: &str) -> Option<String> {
    for base in linux_icon_base_paths() {
        if base.join(theme).join("index.theme").is_file() {
            return Some(theme.to_string());
        }
    }
    for base in linux_icon_base_paths() {
        let Ok(entries) = fs::read_dir(&base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let index = path.join("index.theme");
            if !index.is_file() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&index) {
                if parse_theme_display_name(&content)
                    .is_some_and(|name| name.eq_ignore_ascii_case(theme))
                {
                    return Some(entry.file_name().to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn linux_icon_base_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".icons"));
    }
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        paths.push(PathBuf::from(data_home).join("icons"));
    } else if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".local/share/icons"));
    }
    let data_dirs =
        std::env::var_os("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    for dir in std::env::split_paths(&data_dirs) {
        paths.push(dir.join("icons"));
    }
    paths.into_iter().filter(|p| p.is_dir()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Debug, Clone)]
struct ThemeDirectorySpec {
    name: String,
    size: i32,
    scale: i32,
    dtype: DirType,
    min_size: i32,
    max_size: i32,
    threshold: i32,
}

impl ThemeDirectorySpec {
    fn matches_exact(&self, target_size: i32, target_scale: i32) -> bool {
        if self.scale != target_scale {
            return false;
        }
        match self.dtype {
            DirType::Fixed => self.size == target_size,
            DirType::Scalable => self.min_size <= target_size && target_size <= self.max_size,
            DirType::Threshold => {
                self.size - self.threshold <= target_size
                    && target_size <= self.size + self.threshold
            }
        }
    }

    fn size_distance(&self, target_size: i32, target_scale: i32) -> i32 {
        let scaled_target = target_size * target_scale;
        let scaled_size = self.size * self.scale;
        let min_scaled = self.min_size * self.scale;
        let max_scaled = self.max_size * self.scale;
        let thresh = self.threshold * self.scale;
        match self.dtype {
            DirType::Fixed => (scaled_size - scaled_target).abs(),
            DirType::Scalable => {
                if scaled_target < min_scaled {
                    min_scaled - scaled_target
                } else if scaled_target > max_scaled {
                    scaled_target - max_scaled
                } else {
                    0
                }
            }
            DirType::Threshold => {
                if scaled_target < scaled_size - thresh {
                    (scaled_size - thresh) - scaled_target
                } else if scaled_target > scaled_size + thresh {
                    scaled_target - (scaled_size + thresh)
                } else {
                    0
                }
            }
        }
    }
}

struct ParsedTheme {
    dir: PathBuf,
    display_name: Option<String>,
    inherits: Vec<String>,
    directories: Vec<ThemeDirectorySpec>,
}

fn parse_theme_display_name(ini: &str) -> Option<String> {
    let mut in_header = false;
    for raw_line in ini.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_header = &line[1..line.len() - 1] == "Icon Theme";
            continue;
        }
        if in_header {
            if let Some(name) = line.strip_prefix("Name=") {
                return Some(name.trim().to_string());
            }
        }
    }
    None
}

fn parse_theme_dir(dir: &Path) -> Option<ParsedTheme> {
    let content = fs::read_to_string(dir.join("index.theme")).ok()?;
    let mut display_name = None;
    let mut inherits = Vec::new();
    let mut directories = Vec::new();

    let mut current_section: Option<String> = None;
    let mut size: Option<i32> = None;
    let mut scale: Option<i32> = None;
    let mut min_size: Option<i32> = None;
    let mut max_size: Option<i32> = None;
    let mut threshold: Option<i32> = None;
    let mut dtype = DirType::Threshold;

    let flush_section = |section: &Option<String>,
                         size: Option<i32>,
                         scale: Option<i32>,
                         min_size: Option<i32>,
                         max_size: Option<i32>,
                         threshold: Option<i32>,
                         dtype: DirType,
                         out: &mut Vec<ThemeDirectorySpec>| {
        if let Some(sec_name) = section {
            if sec_name != "Icon Theme" {
                if let Some(sz) = size {
                    out.push(ThemeDirectorySpec {
                        name: sec_name.clone(),
                        size: sz,
                        scale: scale.unwrap_or(1),
                        dtype,
                        min_size: min_size.unwrap_or(sz),
                        max_size: max_size.unwrap_or(sz),
                        threshold: threshold.unwrap_or(2),
                    });
                }
            }
        }
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            flush_section(
                &current_section,
                size,
                scale,
                min_size,
                max_size,
                threshold,
                dtype,
                &mut directories,
            );
            current_section = Some(line[1..line.len() - 1].to_string());
            size = None;
            scale = None;
            min_size = None;
            max_size = None;
            threshold = None;
            dtype = DirType::Threshold;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match current_section.as_deref() {
            Some("Icon Theme") => match key {
                "Name" => display_name = Some(value.to_string()),
                "Inherits" => {
                    inherits = value
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                }
                _ => {}
            },
            Some(_) => match key {
                "Size" => size = value.parse().ok(),
                "Scale" => scale = value.parse().ok(),
                "MinSize" => min_size = value.parse().ok(),
                "MaxSize" => max_size = value.parse().ok(),
                "Threshold" => threshold = value.parse().ok(),
                "Type" => {
                    dtype = match value {
                        "Fixed" => DirType::Fixed,
                        "Scalable" => DirType::Scalable,
                        _ => DirType::Threshold,
                    };
                }
                _ => {}
            },
            None => {}
        }
    }
    flush_section(
        &current_section,
        size,
        scale,
        min_size,
        max_size,
        threshold,
        dtype,
        &mut directories,
    );

    Some(ParsedTheme {
        dir: dir.to_path_buf(),
        display_name,
        inherits,
        directories,
    })
}

fn find_theme_dirs(base_paths: &[PathBuf], theme_name: &str) -> Vec<ParsedTheme> {
    let mut exact = Vec::new();
    let mut by_display = Vec::new();

    for base in base_paths {
        let candidate = base.join(theme_name);
        if candidate.is_dir() {
            if let Some(parsed) = parse_theme_dir(&candidate) {
                exact.push(parsed);
            }
        }
    }
    if !exact.is_empty() {
        return exact;
    }

    for base in base_paths {
        let Ok(entries) = fs::read_dir(base) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(parsed) = parse_theme_dir(&path) {
                    if parsed
                        .display_name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(theme_name))
                    {
                        by_display.push(parsed);
                    }
                }
            }
        }
    }
    by_display
}

fn lookup_in_theme_obj(theme: &ParsedTheme, icon_name: &str, size: u16) -> Option<PathBuf> {
    let target = i32::from(size);
    for dir in &theme.directories {
        if dir.matches_exact(target, 1) {
            if let Some(found) = try_icon_files(&theme.dir.join(&dir.name), icon_name) {
                return Some(found);
            }
        }
    }

    let mut ordered: Vec<&ThemeDirectorySpec> = theme.directories.iter().collect();
    ordered.sort_by_key(|d| d.size_distance(target, 1));
    for dir in ordered {
        if let Some(found) = try_icon_files(&theme.dir.join(&dir.name), icon_name) {
            return Some(found);
        }
    }
    None
}

fn try_icon_files(dir: &Path, icon_name: &str) -> Option<PathBuf> {
    for ext in ["png", "svg"] {
        let path = dir.join(format!("{icon_name}.{ext}"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn lookup_single_in_chain(
    base_paths: &[PathBuf],
    theme_name: &str,
    icon_name: &str,
    size: u16,
    visited: &mut HashSet<String>,
) -> Option<PathBuf> {
    if !visited.insert(theme_name.to_ascii_lowercase()) {
        return None;
    }
    let themes = find_theme_dirs(base_paths, theme_name);
    for theme in &themes {
        if let Some(found) = lookup_in_theme_obj(theme, icon_name, size) {
            return Some(found);
        }
    }
    for theme in &themes {
        for parent in &theme.inherits {
            if let Some(found) =
                lookup_single_in_chain(base_paths, parent, icon_name, size, visited)
            {
                return Some(found);
            }
        }
    }
    None
}

fn lookup_in_base_paths(
    base_paths: &[PathBuf],
    theme_name: &str,
    candidates: &[&str],
    size: u16,
) -> Option<PathBuf> {
    for candidate in candidates {
        let mut visited = HashSet::new();
        if let Some(found) =
            lookup_single_in_chain(base_paths, theme_name, candidate, size, &mut visited)
        {
            return Some(found);
        }
        if !visited.contains("hicolor") {
            if let Some(found) =
                lookup_single_in_chain(base_paths, "hicolor", candidate, size, &mut visited)
            {
                return Some(found);
            }
        }
    }
    None
}

fn lookup_direct_theme_dir(
    theme_dir: &Path,
    base_paths: &[PathBuf],
    candidates: &[&str],
    size: u16,
) -> Option<PathBuf> {
    let parsed = parse_theme_dir(theme_dir)?;
    for candidate in candidates {
        if let Some(found) = lookup_in_theme_obj(&parsed, candidate, size) {
            return Some(found);
        }
        let mut visited = HashSet::new();
        if let Some(name) = theme_dir.file_name().and_then(|n| n.to_str()) {
            visited.insert(name.to_ascii_lowercase());
        }
        for parent in &parsed.inherits {
            if let Some(found) =
                lookup_single_in_chain(base_paths, parent, candidate, size, &mut visited)
            {
                return Some(found);
            }
        }
        if !visited.contains("hicolor") {
            if let Some(found) =
                lookup_single_in_chain(base_paths, "hicolor", candidate, size, &mut visited)
            {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let id = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "kwati_icons_test_{}_{}_{}",
                name,
                std::process::id(),
                id
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn icon_prefs_precedence_type_beats_group_beats_base() {
        let mut prefs = IconPreferences::default();
        assert_eq!(prefs.effective_source("file-pdf"), &IconSource::BuiltIn);

        prefs.base = IconSource::Theme("Breeze".into());
        assert_eq!(
            prefs.effective_source("file-pdf"),
            &IconSource::Theme("Breeze".into())
        );

        let group_img = PathBuf::from("/tmp/doc-group.png");
        prefs.set_group_override(
            IconGroup::Documents,
            Some(IconSource::Image(group_img.clone())),
        );
        assert_eq!(
            prefs.effective_source("file-pdf"),
            &IconSource::Image(group_img.clone())
        );
        assert_eq!(
            prefs.effective_source("file-markdown"),
            &IconSource::Image(group_img)
        );
        // Unrelated group still uses base
        assert_eq!(
            prefs.effective_source("file-code"),
            &IconSource::Theme("Breeze".into())
        );

        // Type override beats group override
        let pdf_img = PathBuf::from("/tmp/pdf-only.svg");
        prefs.set_type_override("file-pdf", Some(IconSource::Image(pdf_img.clone())));
        assert_eq!(
            prefs.effective_source("file-pdf"),
            &IconSource::Image(pdf_img)
        );

        // Removing type override falls back to group
        prefs.set_type_override("file-pdf", None);
        assert_eq!(
            prefs.effective_source("file-pdf"),
            &IconSource::Image(PathBuf::from("/tmp/doc-group.png"))
        );

        // BuiltIn override on a type overrides Theme base/group
        prefs.set_type_override("file-pdf", Some(IconSource::BuiltIn));
        assert_eq!(prefs.effective_source("file-pdf"), &IconSource::BuiltIn);
    }

    #[test]
    fn icon_presets_round_trip_rename_and_delete() {
        let dir = TestDir::new("presets");
        let path = dir.path.join("icons.toml");

        // Missing file -> everything resolves to built-in
        let empty = IconConfig::load_from(&path).unwrap();
        assert_eq!(empty, IconConfig::default());
        assert_eq!(
            empty.active.effective_source("file-pdf"),
            &IconSource::BuiltIn
        );

        let mut cfg = IconConfig::default();
        cfg.active.base = IconSource::Theme("Breeze".into());
        cfg.active.set_group_override(
            IconGroup::Images,
            Some(IconSource::Image(PathBuf::from("/tmp/img.png"))),
        );
        cfg.active.set_type_override(
            "file-pdf",
            Some(IconSource::Image(PathBuf::from("/tmp/pdf.svg"))),
        );

        cfg.save_preset("Meu tema").unwrap();
        cfg.save_to(&path).unwrap();

        let mut loaded = IconConfig::load_from(&path).unwrap();
        assert_eq!(loaded, cfg);

        // Rename keeps content
        loaded.rename_preset("Meu tema", "Trabalho").unwrap();
        assert!(!loaded.presets.contains_key("Meu tema"));
        assert_eq!(loaded.presets.get("Trabalho"), Some(&cfg.active));

        // Reset active keeps saved presets
        loaded.reset_active();
        assert_eq!(loaded.active, IconPreferences::default());
        assert!(loaded.presets.contains_key("Trabalho"));

        // Apply preset restores it
        loaded.apply_preset("Trabalho").unwrap();
        assert_eq!(loaded.active, cfg.active);

        // Deleting preset leaves active unchanged
        assert!(loaded.delete_preset("Trabalho"));
        assert!(!loaded.presets.contains_key("Trabalho"));
        assert_eq!(loaded.active, cfg.active);
    }

    #[test]
    fn icon_presets_hand_written_toml_supports_aliases() {
        let dir = TestDir::new("hand_written");
        let path = dir.path.join("icons.toml");
        fs::write(
            &path,
            r#"
base = { theme = "Breeze" }

[groups]
images = { image = "/tmp/images.png" }

[types]
".pdf" = { image = "/tmp/pdf.png" }
"#,
        )
        .unwrap();

        let cfg = IconConfig::load_from(&path).unwrap();
        assert_eq!(cfg.active.base, IconSource::Theme("Breeze".into()));
        assert_eq!(
            cfg.active.effective_source("file-image"),
            &IconSource::Image(PathBuf::from("/tmp/images.png"))
        );
        assert_eq!(
            cfg.active.effective_source("file-pdf"),
            &IconSource::Image(PathBuf::from("/tmp/pdf.png"))
        );
    }

    #[test]
    fn icon_color_defaults_precedence_and_toml_roundtrip() {
        let dir = TestDir::new("colors");
        let path = dir.path.join("icons.toml");

        let mut prefs = IconPreferences::default();
        // Built-in defaults give semantic colors (e.g. red for PDF, accent for folders, text for generic file).
        assert_eq!(prefs.effective_color("folder"), IconColor::Accent);
        assert_eq!(prefs.effective_color("folder-downloads"), IconColor::Accent);
        assert_eq!(
            prefs.effective_color("file-pdf"),
            IconColor::adaptive(0xB3261E, 0xF2746B)
        );
        assert_ne!(prefs.effective_color("file-pdf"), IconColor::Text);
        assert_eq!(prefs.effective_color("file"), IconColor::Text);

        // Precedence: type > group > base > default.
        prefs.set_base_color(Some(IconColor::Muted));
        assert_eq!(prefs.effective_color("file-code"), IconColor::Muted);
        assert_eq!(prefs.effective_color("file-pdf"), IconColor::Muted);

        prefs.set_group_color_override(IconGroup::Documents, Some(IconColor::rgb(0x1E88E5)));
        assert_eq!(
            prefs.effective_color("file-markdown"),
            IconColor::rgb(0x1E88E5)
        );
        assert_eq!(prefs.effective_color("file-pdf"), IconColor::rgb(0x1E88E5));
        assert_eq!(prefs.effective_color("file-code"), IconColor::Muted);

        prefs.set_type_color_override("file-pdf", Some(IconColor::rgb(0xE53935)));
        assert_eq!(prefs.effective_color("file-pdf"), IconColor::rgb(0xE53935));

        // Removing type override falls back to group; removing group falls back to base; removing base falls back to default.
        prefs.set_type_color_override("file-pdf", None);
        assert_eq!(prefs.effective_color("file-pdf"), IconColor::rgb(0x1E88E5));

        prefs.set_group_color_override(IconGroup::Documents, None);
        assert_eq!(prefs.effective_color("file-pdf"), IconColor::Muted);

        prefs.set_base_color(None);
        assert_eq!(
            prefs.effective_color("file-pdf"),
            default_icon_color("file-pdf")
        );

        // Save in preset + round-trip via icons.toml, including adaptive and hex colors.
        let mut cfg = IconConfig::default();
        cfg.active.set_base_color(Some(IconColor::Text));
        cfg.active
            .set_group_color_override(IconGroup::Code, Some(IconColor::Secondary));
        cfg.active
            .set_type_color_override(".pdf", Some(IconColor::adaptive(0x991B1B, 0xFCA5A5)));
        cfg.save_preset("Vermelho").unwrap();
        cfg.save_to(&path).unwrap();

        let mut loaded = IconConfig::load_from(&path).unwrap();
        assert_eq!(loaded, cfg);
        assert_eq!(
            loaded.active.effective_color("file-pdf"),
            IconColor::adaptive(0x991B1B, 0xFCA5A5)
        );
        assert_eq!(
            loaded.active.effective_color("file-code"),
            IconColor::Secondary
        );
        assert_eq!(loaded.active.effective_color("file"), IconColor::Text);

        // Reset clears active colors back to defaults while preserving saved presets.
        loaded.reset_active();
        assert_eq!(
            loaded.active.effective_color("file-pdf"),
            default_icon_color("file-pdf")
        );
        loaded.apply_preset("Vermelho").unwrap();
        assert_eq!(
            loaded.active.effective_color("file-pdf"),
            IconColor::adaptive(0x991B1B, 0xFCA5A5)
        );

        // Hand-written TOML color aliases and validation.
        fs::write(
            &path,
            r##"
base_color = "text_muted"

[group_colors]
audio_video = "#8e44ad"

[type_colors]
".pdf" = "#d32f2f"
"##,
        )
        .unwrap();
        let hand = IconConfig::load_from(&path).unwrap();
        assert_eq!(hand.active.base_color, Some(IconColor::Muted));
        assert_eq!(
            hand.active.effective_color("file-audio"),
            IconColor::rgb(0x8E44AD)
        );
        assert_eq!(
            hand.active.effective_color("file-pdf"),
            IconColor::rgb(0xD32F2F)
        );

        assert!(IconColor::parse("#GGGGGG").is_err());
        assert!(IconColor::parse("unknown-token").is_err());
    }
}
