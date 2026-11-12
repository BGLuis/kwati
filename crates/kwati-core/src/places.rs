use std::path::{Path, PathBuf};

use crate::Settings;

/// The kind of standard directory recognized by the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
}

impl PlaceKind {
    /// Returns the standard display name for this place kind.
    pub fn default_name(&self) -> &'static str {
        match self {
            PlaceKind::Home => "Home",
            PlaceKind::Desktop => "Desktop",
            PlaceKind::Documents => "Documents",
            PlaceKind::Downloads => "Downloads",
            PlaceKind::Music => "Music",
            PlaceKind::Pictures => "Pictures",
            PlaceKind::Videos => "Videos",
        }
    }
}

/// A standard folder defined by the platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub kind: PlaceKind,
    pub name: String,
    pub path: PathBuf,
}

impl Place {
    pub fn new(kind: PlaceKind, name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            kind,
            name: name.into(),
            path: path.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn kind(&self) -> PlaceKind {
        self.kind
    }
}

/// A user bookmark saved in settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub name: String,
    pub path: PathBuf,
}

impl Bookmark {
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }

    /// Returns `true` if the target folder exists on disk.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Returns `true` if the target folder does not exist on disk.
    pub fn is_missing(&self) -> bool {
        !self.path.exists()
    }

    /// Resolves the current status of the bookmark on disk.
    pub fn resolve(&self) -> ResolvedBookmark {
        ResolvedBookmark {
            name: self.name.clone(),
            path: self.path.clone(),
            is_missing: self.is_missing(),
        }
    }
}

impl<N: Into<String>, P: Into<PathBuf>> From<(N, P)> for Bookmark {
    fn from((name, path): (N, P)) -> Self {
        Self::new(name, path)
    }
}

/// A bookmark with its checked on-disk status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBookmark {
    pub name: String,
    pub path: PathBuf,
    pub is_missing: bool,
}

impl ResolvedBookmark {
    pub fn is_missing(&self) -> bool {
        self.is_missing
    }

    pub fn exists(&self) -> bool {
        !self.is_missing
    }
}

/// Returns the standard user folders defined by the platform that currently exist on disk.
/// Any standard folder that the platform does not define or that does not exist on disk is skipped.
pub fn standard_places() -> Vec<Place> {
    let candidates = [
        (PlaceKind::Home, "Home", dirs::home_dir()),
        (PlaceKind::Desktop, "Desktop", dirs::desktop_dir()),
        (PlaceKind::Documents, "Documents", dirs::document_dir()),
        (PlaceKind::Downloads, "Downloads", dirs::download_dir()),
        (PlaceKind::Music, "Music", dirs::audio_dir()),
        (PlaceKind::Pictures, "Pictures", dirs::picture_dir()),
        (PlaceKind::Videos, "Videos", dirs::video_dir()),
    ];

    standard_places_from(candidates)
}

/// Helper that filters candidate folders: only keeps entries with `Some(path)` where `path.exists()`.
pub fn standard_places_from<I>(candidates: I) -> Vec<Place>
where
    I: IntoIterator<Item = (PlaceKind, &'static str, Option<PathBuf>)>,
{
    candidates
        .into_iter()
        .filter_map(|(kind, default_name, opt_path)| {
            let path = opt_path?;
            if path.exists() {
                Some(Place::new(kind, default_name, path))
            } else {
                None
            }
        })
        .collect()
}

/// Resolves user bookmarks from settings against disk state.
pub fn resolve_bookmarks(settings: &Settings) -> Vec<ResolvedBookmark> {
    settings.bookmarks.iter().map(|b| b.resolve()).collect()
}

/// Container for standard places and user bookmarks for sidebar display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPlaces {
    pub places: Vec<Place>,
    pub bookmarks: Vec<ResolvedBookmark>,
}

/// Returns standard places and user bookmarks configured in settings.
pub fn list_places(settings: &Settings) -> SidebarPlaces {
    SidebarPlaces {
        places: standard_places(),
        bookmarks: resolve_bookmarks(settings),
    }
}
