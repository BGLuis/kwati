use crate::{Entry, Location};
use std::collections::HashSet;

/// Tracks a multi-item selection of directory entries identified by their [`Location`].
///
/// Keeps track of an anchor for range-based selections and allows selecting, toggling,
/// inverting, and reconciling selections across directory reloads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    anchor: Option<Location>,
    selected: HashSet<Location>,
}

impl Selection {
    /// Creates a new, empty selection with no anchor.
    pub fn new() -> Self {
        Self {
            anchor: None,
            selected: HashSet::new(),
        }
    }

    /// Returns a reference to the current anchor location, if one is set.
    pub fn anchor(&self) -> Option<&Location> {
        self.anchor.as_ref()
    }

    /// Sets or clears the anchor location.
    pub fn set_anchor<T: Into<Location>>(&mut self, anchor: Option<T>) {
        self.anchor = anchor.map(Into::into);
    }

    /// Returns `true` if the given location is currently selected.
    pub fn is_selected(&self, location: &Location) -> bool {
        self.selected.contains(location)
    }

    /// Alias for [`is_selected`].
    pub fn contains(&self, location: &Location) -> bool {
        self.is_selected(location)
    }

    /// Returns the number of selected items.
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Returns `true` if no items are currently selected.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Returns a reference to the underlying set of selected locations.
    pub fn locations(&self) -> &HashSet<Location> {
        &self.selected
    }

    /// Returns an iterator over all selected locations.
    pub fn iter(&self) -> std::collections::hash_set::Iter<'_, Location> {
        self.selected.iter()
    }

    /// Selects the given location and sets the anchor to it.
    pub fn select<T: Into<Location>>(&mut self, location: T) {
        let loc = location.into();
        self.selected.insert(loc.clone());
        self.anchor = Some(loc);
    }

    /// Deselects a location. If it was the anchor, resets the anchor to `None`.
    pub fn deselect(&mut self, location: &Location) {
        self.selected.remove(location);
        if self.anchor.as_ref() == Some(location) {
            self.anchor = None;
        }
    }

    /// Clears previous selection and selects only the given location, moving the anchor to it.
    pub fn select_only<T: Into<Location>>(&mut self, location: T) {
        self.clear();
        self.select(location);
    }

    /// Toggles the selection of an entry: adds it if absent, removes it if present,
    /// and moves the anchor to it.
    pub fn toggle<T: Into<Location>>(&mut self, location: T) {
        let loc = location.into();
        if !self.selected.remove(&loc) {
            self.selected.insert(loc.clone());
        }
        self.anchor = Some(loc);
    }

    /// Selects every entry between the anchor and the target in list order.
    ///
    /// If no anchor is set or the anchor is not present in `entries`, the target becomes
    /// the anchor and only the target is selected.
    ///
    /// Returns the selected locations between the anchor and target, in list order.
    pub fn range_from_anchor<T: Into<Location>>(
        &mut self,
        target: T,
        entries: &[Entry],
    ) -> Vec<Location> {
        let target = target.into();
        let target_idx = entries.iter().position(|e| Location::from(e) == target);
        let Some(target_idx) = target_idx else {
            return Vec::new();
        };

        let anchor_idx = self
            .anchor
            .as_ref()
            .and_then(|a| entries.iter().position(|e| &Location::from(e) == a));

        let (start, end) = match anchor_idx {
            Some(a_idx) => {
                if a_idx <= target_idx {
                    (a_idx, target_idx)
                } else {
                    (target_idx, a_idx)
                }
            }
            None => {
                self.anchor = Some(target.clone());
                (target_idx, target_idx)
            }
        };

        let mut range_locations = Vec::with_capacity(end - start + 1);
        for entry in &entries[start..=end] {
            let loc = Location::from(entry);
            self.selected.insert(loc.clone());
            range_locations.push(loc);
        }

        range_locations
    }

    /// Alias for [`range_from_anchor`].
    pub fn select_range<T: Into<Location>>(
        &mut self,
        target: T,
        entries: &[Entry],
    ) -> Vec<Location> {
        self.range_from_anchor(target, entries)
    }

    /// Alias for [`range_from_anchor`].
    pub fn range<T: Into<Location>>(&mut self, target: T, entries: &[Entry]) -> Vec<Location> {
        self.range_from_anchor(target, entries)
    }

    /// Selects all entries in the given slice.
    pub fn select_all(&mut self, entries: &[Entry]) {
        for entry in entries {
            self.selected.insert(Location::from(entry));
        }
    }

    /// Clears the selection and resets the anchor.
    pub fn clear(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    /// Inverts the selection across the entries in the given slice.
    ///
    /// Entries in the slice that were selected become deselected, and entries that were
    /// unselected become selected. Selected items not present in the slice are dropped.
    /// If the anchor is no longer present in `entries`, it is reset to `None`.
    pub fn invert(&mut self, entries: &[Entry]) {
        let mut new_selected = HashSet::new();
        for entry in entries {
            let loc = Location::from(entry);
            if !self.selected.contains(&loc) {
                new_selected.insert(loc);
            }
        }
        self.selected = new_selected;
        if let Some(ref anchor) = self.anchor {
            if !entries.iter().any(|e| &Location::from(e) == anchor) {
                self.anchor = None;
            }
        }
    }

    /// Reconciles the selection after a reload: keeps selected entries that still exist in
    /// `entries` and drops the rest. If the anchor no longer exists in `entries`, it is
    /// reset to `None`.
    pub fn reload(&mut self, entries: &[Entry]) {
        let valid_locations: HashSet<Location> = entries.iter().map(Location::from).collect();
        self.selected.retain(|loc| valid_locations.contains(loc));
        if let Some(ref anchor) = self.anchor {
            if !valid_locations.contains(anchor) {
                self.anchor = None;
            }
        }
    }

    /// Reconciles the selection against a slice of [`Location`]s directly.
    pub fn reload_locations(&mut self, locations: &[Location]) {
        let valid_locations: HashSet<Location> = locations.iter().cloned().collect();
        self.selected.retain(|loc| valid_locations.contains(loc));
        if let Some(ref anchor) = self.anchor {
            if !valid_locations.contains(anchor) {
                self.anchor = None;
            }
        }
    }

    /// Alias for [`reload`].
    pub fn reconcile(&mut self, entries: &[Entry]) {
        self.reload(entries);
    }

    /// Alias for [`reload`].
    pub fn retain_existing(&mut self, entries: &[Entry]) {
        self.reload(entries);
    }

    /// Returns references to the selected entries from the given slice, in list order.
    pub fn selected_entries<'a>(&self, entries: &'a [Entry]) -> Vec<&'a Entry> {
        entries
            .iter()
            .filter(|e| self.is_selected(&Location::from(*e)))
            .collect()
    }

    /// Alias for [`selected_entries`].
    pub fn selected_in_order<'a>(&self, entries: &'a [Entry]) -> Vec<&'a Entry> {
        self.selected_entries(entries)
    }

    /// Returns the selected locations from the given slice, in list order.
    pub fn selected_locations_in_order(&self, entries: &[Entry]) -> Vec<Location> {
        entries
            .iter()
            .map(Location::from)
            .filter(|loc| self.is_selected(loc))
            .collect()
    }
}

impl<'a> IntoIterator for &'a Selection {
    type Item = &'a Location;
    type IntoIter = std::collections::hash_set::Iter<'a, Location>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for Selection {
    type Item = Location;
    type IntoIter = std::collections::hash_set::IntoIter<Location>;

    fn into_iter(self) -> Self::IntoIter {
        self.selected.into_iter()
    }
}

impl FromIterator<Location> for Selection {
    fn from_iter<I: IntoIterator<Item = Location>>(iter: I) -> Self {
        let selected: HashSet<Location> = iter.into_iter().collect();
        Self {
            anchor: None,
            selected,
        }
    }
}
