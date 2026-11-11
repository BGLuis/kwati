use crate::Location;

/// A back/forward navigation history of visited locations.
///
/// Discards forward entries when a new location is visited and bounds navigation
/// to the oldest and newest entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History<T = Location> {
    entries: Vec<T>,
    index: usize,
}

impl<T> History<T> {
    /// Creates a new, empty navigation history.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: 0,
        }
    }

    /// Creates a navigation history initialized with a starting location.
    pub fn with_initial(initial: T) -> Self {
        Self {
            entries: vec![initial],
            index: 0,
        }
    }

    /// Visits a directory, truncating any forward entries.
    pub fn visit(&mut self, entry: T) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(entry);
        self.index = self.entries.len() - 1;
    }

    /// Moves back to the previous entry, returning a reference to it.
    ///
    /// Returns `None` if already at the oldest entry or if history is empty.
    pub fn back(&mut self) -> Option<&T> {
        if self.can_go_back() {
            self.index -= 1;
            self.entries.get(self.index)
        } else {
            None
        }
    }

    /// Moves forward to the next entry, returning a reference to it.
    ///
    /// Returns `None` if already at the newest entry or if history is empty.
    pub fn forward(&mut self) -> Option<&T> {
        if self.can_go_forward() {
            self.index += 1;
            self.entries.get(self.index)
        } else {
            None
        }
    }

    /// Returns a reference to the current entry, or `None` if history is empty.
    pub fn current(&self) -> Option<&T> {
        self.entries.get(self.index)
    }

    /// Returns `true` if there is a previous entry to navigate back to.
    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    /// Returns `true` if there is a next entry to navigate forward to.
    pub fn can_go_forward(&self) -> bool {
        !self.entries.is_empty() && self.index + 1 < self.entries.len()
    }

    /// Returns the total number of entries in the history.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the history contains no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns the current index in the history.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns a slice of all entries in the history.
    pub fn entries(&self) -> &[T] {
        &self.entries
    }

    /// Clears all entries from the history.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.index = 0;
    }
}

impl<T> Default for History<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> From<T> for History<T> {
    fn from(initial: T) -> Self {
        Self::with_initial(initial)
    }
}
