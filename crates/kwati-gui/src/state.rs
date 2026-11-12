use std::path::PathBuf;
use std::sync::Arc;

use kwati_core::{Entry, History, Location, Selection, Settings};

/// Classification of background file operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    Copy,
    Move,
    Delete,
    Trash,
}

/// Status of an active background file operation (owned by guiops-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobStatus {
    pub id: u64,
    pub title: String,
    pub kind: JobKind,
    pub bytes_processed: u64,
    pub bytes_total: u64,
    pub progress_pct: u8,
    pub is_cancelled: bool,
}

impl JobStatus {
    pub fn new(id: u64, title: impl Into<String>, kind: JobKind, bytes_total: u64) -> Self {
        Self {
            id,
            title: title.into(),
            kind,
            bytes_processed: 0,
            bytes_total,
            progress_pct: 0,
            is_cancelled: false,
        }
    }

    pub fn update_progress(&mut self, processed: u64) {
        self.bytes_processed = processed;
        if self.bytes_total > 0 {
            self.progress_pct =
                ((processed as f64 / self.bytes_total as f64) * 100.0).min(100.0) as u8;
        }
    }
}

/// Operations recorded in the undo stack (owned by guiops-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UndoOperation {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    CreateDir {
        path: PathBuf,
    },
    Trash {
        original_paths: Vec<PathBuf>,
    },
    Copy {
        destinations: Vec<PathBuf>,
    },
    Move {
        from: Vec<PathBuf>,
        to: Vec<PathBuf>,
    },
}

/// Undo history stack (owned by guiops-worker).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UndoStack {
    pub operations: Vec<UndoOperation>,
}

impl UndoStack {
    pub fn new() -> Self {
        Self {
            operations: Vec::new(),
        }
    }

    pub fn push(&mut self, op: UndoOperation) {
        self.operations.push(op);
    }

    pub fn pop(&mut self) -> Option<UndoOperation> {
        self.operations.pop()
    }

    pub fn peek(&self) -> Option<&UndoOperation> {
        self.operations.last()
    }

    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }
}

/// Kind of preview content (owned by inspect-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewKind {
    Text(String),
    Image(PathBuf),
    Binary { name: String, size: u64 },
    Loading,
}

/// Preview data for the selected file (owned by inspect-worker).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewData {
    pub path: PathBuf,
    pub kind: PreviewKind,
}

/// Connection and health status for network mounts (owned by guinet-worker).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NetworkMountStatus {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    TimedOut,
    MountBusy,
}

/// Root application state shared across views and background tasks.
pub struct AppState {
    pub current_location: Location,
    pub entries: Arc<Vec<Entry>>,
    pub selection: Selection,
    pub history: History,
    pub settings: Settings,
    pub active_jobs: Vec<JobStatus>,        // Owned by guiops-worker
    pub undo_stack: UndoStack,              // Owned by guiops-worker
    pub preview_state: Option<PreviewData>, // Owned by inspect-worker
    pub search_query: Option<String>,       // Owned by inspect-worker
    pub network_status: NetworkMountStatus, // Owned by guinet-worker
    pub status_message: Option<String>,
}

impl AppState {
    pub fn new(initial_location: Location) -> Self {
        Self {
            history: History::with_initial(initial_location.clone()),
            current_location: initial_location,
            entries: Arc::new(Vec::new()),
            selection: Selection::new(),
            settings: Settings::default(),
            active_jobs: Vec::new(),
            undo_stack: UndoStack::new(),
            preview_state: None,
            search_query: None,
            network_status: NetworkMountStatus::Disconnected,
            status_message: None,
        }
    }

    /// Sets the current browsing location and adds it to history.
    pub fn navigate_to(&mut self, location: Location) {
        self.history.visit(location.clone());
        self.current_location = location;
        self.selection.clear();
        self.preview_state = None;
    }

    /// Replaces the listed entries.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        self.entries = Arc::new(entries);
        self.selection.retain_existing(&self.entries);
    }

    /// Returns the index of the first selected entry in the listed entries, if any.
    pub fn selected_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| self.selection.contains(&e.location()))
    }

    /// Selects the entry at the given index.
    pub fn select_index(&mut self, index: usize) {
        if let Some(entry) = self.entries.get(index) {
            self.selection.select_only(entry.location());
        }
    }

    /// Sets or clears the status message shown in the footer.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }

    pub fn clear_status(&mut self) {
        self.status_message = None;
    }

    /// Appends a new active job.
    pub fn add_job(&mut self, job: JobStatus) {
        self.active_jobs.push(job);
    }

    /// Removes an active job by ID.
    pub fn remove_job(&mut self, id: u64) {
        self.active_jobs.retain(|j| j.id != id);
    }

    /// Updates preview state.
    pub fn set_preview(&mut self, preview: Option<PreviewData>) {
        self.preview_state = preview;
    }

    /// Updates search query filter.
    pub fn set_search_query(&mut self, query: Option<String>) {
        self.search_query = query;
    }

    /// Updates network mount status.
    pub fn set_network_status(&mut self, status: NetworkMountStatus) {
        self.network_status = status;
    }
}
