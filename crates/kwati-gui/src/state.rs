use std::path::PathBuf;
use std::sync::Arc;

use kwati_core::{
    list_dir_with_options, sort_entries_by, Entry, History, ListOptions, Location, Selection,
    Settings, SortColumn, SortDirection,
};

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
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub show_hidden: bool,
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
            sort_column: SortColumn::Name,
            sort_direction: SortDirection::Ascending,
            show_hidden: false,
        }
    }

    /// Reloads the entries of the current local directory using current sort and hidden options.
    pub fn reload_entries(&mut self) {
        if let Location::Local(path) = &self.current_location {
            if path.is_dir() {
                let options = ListOptions {
                    show_hidden: self.show_hidden,
                    sort_column: self.sort_column,
                    sort_direction: self.sort_direction,
                };
                if let Ok(listing) = list_dir_with_options(path, &options) {
                    self.set_entries(listing.into_entries());
                    self.clear_status();
                }
            }
        }
    }

    /// Sets the current browsing location and adds it to history.
    pub fn navigate_to(&mut self, location: Location) {
        self.history.visit(location.clone());
        self.current_location = location;
        self.selection.clear();
        self.preview_state = None;
        self.reload_entries();
    }

    /// Navigates back in history, returning true if history moved.
    pub fn go_back(&mut self) -> bool {
        if let Some(prev) = self.history.back().cloned() {
            self.current_location = prev;
            self.selection.clear();
            self.preview_state = None;
            self.reload_entries();
            true
        } else {
            false
        }
    }

    /// Navigates forward in history, returning true if history moved.
    pub fn go_forward(&mut self) -> bool {
        if let Some(next) = self.history.forward().cloned() {
            self.current_location = next;
            self.selection.clear();
            self.preview_state = None;
            self.reload_entries();
            true
        } else {
            false
        }
    }

    /// Toggles sorting on the given column.
    /// Clicking the active column inverts direction; clicking another resets to Ascending.
    pub fn toggle_sort(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_direction = match self.sort_direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
        } else {
            self.sort_column = column;
            self.sort_direction = SortDirection::Ascending;
        }

        let mut entries = (*self.entries).clone();
        sort_entries_by(&mut entries, self.sort_column, self.sort_direction);
        self.set_entries(entries);
    }

    /// Toggles the visibility of hidden files and reloads the directory listing.
    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.reload_entries();
    }

    /// Navigates up to the parent directory, returning true if parent exists.
    pub fn go_up(&mut self) -> bool {
        if let Some(parent) = self.current_location.path().parent() {
            if !parent.as_os_str().is_empty() {
                self.navigate_to(parent.into());
                return true;
            }
        }
        false
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

    /// Moves the selection to the next entry in the list, returning the new index.
    pub fn select_next(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let next_ix = match self.selected_index() {
            Some(ix) => (ix + 1).min(self.entries.len() - 1),
            None => 0,
        };
        self.select_index(next_ix);
        Some(next_ix)
    }

    /// Moves the selection to the previous entry in the list, returning the new index.
    pub fn select_previous(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let prev_ix = match self.selected_index() {
            Some(ix) => ix.saturating_sub(1),
            None => self.entries.len() - 1,
        };
        self.select_index(prev_ix);
        Some(prev_ix)
    }

    /// Moves the selection to the first entry in the list.
    pub fn select_first(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        self.select_index(0);
        Some(0)
    }

    /// Moves the selection to the last entry in the list.
    pub fn select_last(&mut self) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let last_ix = self.entries.len() - 1;
        self.select_index(last_ix);
        Some(last_ix)
    }

    /// Moves the selection up by `page_size` entries.
    pub fn select_page_up(&mut self, page_size: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let ix = self.selected_index().unwrap_or(0);
        let target = ix.saturating_sub(page_size);
        self.select_index(target);
        Some(target)
    }

    /// Moves the selection down by `page_size` entries.
    pub fn select_page_down(&mut self, page_size: usize) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let ix = self.selected_index().unwrap_or(0);
        let target = (ix + page_size).min(self.entries.len() - 1);
        self.select_index(target);
        Some(target)
    }

    /// Opens the currently selected entry: if directory, enters it; returns true if navigated.
    pub fn open_selected_entry(&mut self) -> bool {
        if let Some(ix) = self.selected_index() {
            if let Some(entry) = self.entries.get(ix).cloned() {
                if entry.is_dir() {
                    self.navigate_to(entry.location());
                    return true;
                }
            }
        }
        false
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
