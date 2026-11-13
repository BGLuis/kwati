//! Search overlay view (#27 local search, #52 remote search).
//!
//! Provides interactive recursive file search for both local and remote locations,
//! background streaming with cancellation, ignore-file toggle (local), depth-limit
//! indicator (remote), and keyboard/mouse result navigation.
//!
//! ## MainView integration (`views/mod.rs`)
//! - Handle global [`ToggleSearch`] (`Ctrl/Cmd+F`) on `MainView`'s root element by
//!   calling `self.search.update(cx, |search, cx| search.toggle(window, cx))` so
//!   pressing `Ctrl/Cmd+F` opens the bar even when collapsed.
//! - Observe `&search` (via `cx.observe_in(&search, window, ...)`) and return focus
//!   to `this.list.read(cx).focus_handle.focus(window)` when `search.read(cx).is_active()`
//!   becomes `false` (unless a dialog is open).
//! - `SearchView` automatically observes `AppState` and calls
//!   [`SearchView::stop_for_location_change`]; `MainView` may also call it explicitly
//!   on location transitions if desired.

use std::path::PathBuf;

use futures::channel::mpsc;
use futures::StreamExt;

use gpui::{
    actions, div, prelude::*, px, transparent_black, App, Context, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, KeyDownEvent, MouseButton, Subscription, Task, Window,
};
use kwati_core::{Location, RemoteSearchOptions, SearchHandle, SearchHit, SearchOptions, Searcher};

use crate::components::TextInput;
use crate::components::{icon, IconName};
use crate::state::AppState;
use crate::theme::Theme;
use crate::tokens::{radius, size, space};

actions!(
    search,
    [
        ToggleSearch,
        CloseSearch,
        NextSearchResult,
        PreviousSearchResult,
        OpenSearchResult,
        ToggleRespectIgnore,
    ]
);

/// Registers search-specific keybindings.
pub fn register_search_keybindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-f", ToggleSearch, None),
        KeyBinding::new("cmd-f", ToggleSearch, None),
        KeyBinding::new("escape", CloseSearch, Some("SearchView")),
        KeyBinding::new("down", NextSearchResult, Some("SearchView")),
        KeyBinding::new("up", PreviousSearchResult, Some("SearchView")),
        KeyBinding::new("enter", OpenSearchResult, Some("SearchView")),
    ]);
}

pub struct SearchView {
    pub app_state: Entity<AppState>,
    pub search_input: Entity<TextInput>,
    pub focus_handle: FocusHandle,
    pub is_active: bool,
    pub results: Vec<SearchHit>,
    pub selected_index: Option<usize>,
    pub respect_ignore: bool,
    pub is_searching: bool,
    pub last_query: String,
    pub current_root: Option<PathBuf>,
    pub current_location: Option<Location>,
    pub search_error: Option<String>,
    searcher: Searcher,
    search_handle: Option<SearchHandle>,
    stream_task: Option<Task<()>>,
    search_generation: u64,
    pending_reveal: Option<(Location, Location)>,
    _input_sub: Subscription,
    _state_sub: Subscription,
}

impl Focusable for SearchView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SearchView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| TextInput::with_placeholder("Search directory...", cx));
        let focus_handle = cx.focus_handle();

        let input_sub = cx.observe(&search_input, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.last_query {
                this.last_query = text.clone();
                this.start_search(text, cx);
            }
        });

        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            let current_loc = app_state.read(cx).current_location().clone();
            if let Some((expected_parent, target_loc)) = this.pending_reveal.clone() {
                if current_loc == expected_parent {
                    this.pending_reveal = None;
                    app_state.update(cx, |state, cx| {
                        state.select_location(target_loc);
                        cx.notify();
                    });
                    return;
                }
            }
            this.stop_for_location_change(&current_loc, cx);
        });

        Self {
            app_state,
            search_input,
            focus_handle,
            is_active: false,
            results: Vec::new(),
            selected_index: None,
            respect_ignore: true,
            is_searching: false,
            last_query: String::new(),
            current_root: None,
            current_location: None,
            search_error: None,
            searcher: Searcher::new(),
            search_handle: None,
            stream_task: None,
            search_generation: 0,
            pending_reveal: None,
            _input_sub: input_sub,
            _state_sub: state_sub,
        }
    }

    /// Returns whether the search bar is currently expanded.
    pub fn is_active(&self) -> bool {
        self.is_active
    }

    /// Focuses the search input field in `window`.
    pub fn focus_input(&self, window: &mut Window, cx: &App) {
        self.search_input.read(cx).focus_handle.focus(window);
    }

    /// Toggles the search bar visibility, focusing the input when opened.
    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_active {
            self.close(cx);
        } else {
            self.open(window, cx);
        }
    }

    /// Opens the search bar and focuses `search_input`.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.is_active = true;
        self.search_error = None;
        let loc = self.app_state.read(cx).current_location().clone();
        self.current_root = Some(loc.path().to_path_buf());
        self.current_location = Some(loc);
        self.focus_input(window, cx);
        cx.notify();
    }

    /// Closes the search bar, cancels any in-flight search, and clears active query state.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.stop_search();
        self.is_active = false;
        self.results.clear();
        self.selected_index = None;
        self.search_error = None;
        self.pending_reveal = None;
        self.last_query.clear();
        self.search_input.update(cx, |input, cx| {
            input.set_text("", cx);
        });
        self.app_state.update(cx, |state, _cx| {
            state.set_search_query(None);
        });
        cx.notify();
    }

    /// Cancels an in-flight search while keeping the search bar and any hits received so far.
    pub fn cancel_search(&mut self, cx: &mut Context<Self>) {
        self.stop_search();
        cx.notify();
    }

    /// Stops any running background search and its UI streaming task.
    pub fn stop_search(&mut self) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.stream_task = None;
        if let Some(handle) = self.search_handle.take() {
            handle.stop();
        }
        self.is_searching = false;
    }

    /// Returns the configured remote depth limit when browsing a remote location, or `None` locally.
    pub fn remote_depth_limit(&self, cx: &App) -> Option<u32> {
        let state = self.app_state.read(cx);
        match state.current_location() {
            Location::Remote { .. } => Some(state.settings.remote_search_depth.max(1)),
            Location::Local(_) => None,
        }
    }

    /// Starts a new recursive search below the current browsing location (local or remote).
    pub fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.stop_search();
        self.results.clear();
        self.selected_index = None;
        self.search_error = None;

        if query.trim().is_empty() {
            self.app_state.update(cx, |state, _cx| {
                state.set_search_query(None);
            });
            cx.notify();
            return;
        }

        self.app_state.update(cx, |state, _cx| {
            state.set_search_query(Some(query.clone()));
        });

        let (root_location, remote_depth, remote_parallel, remote_backend) = {
            let state = self.app_state.read(cx);
            (
                state.current_location().clone(),
                state.settings.remote_search_depth.max(1),
                state.settings.remote_search_parallel.max(1) as usize,
                state.current_backend(),
            )
        };
        self.current_root = Some(root_location.path().to_path_buf());
        self.current_location = Some(root_location.clone());

        let (tx, mut rx) = mpsc::unbounded::<SearchHit>();
        let handle = match root_location {
            Location::Local(root_path) => {
                let options = SearchOptions {
                    respect_ignore: self.respect_ignore,
                };
                self.searcher
                    .search(&root_path, &query, options, move |hit| {
                        let _ = tx.unbounded_send(hit);
                    })
            }
            Location::Remote { .. } => {
                let Some(backend) = remote_backend else {
                    self.is_searching = false;
                    self.search_error = Some("Not connected to remote server".to_string());
                    cx.notify();
                    return;
                };
                let options = RemoteSearchOptions {
                    max_depth: remote_depth,
                    parallel_requests: remote_parallel,
                };
                self.searcher
                    .search_remote(backend, root_location, &query, options, move |hit| {
                        let _ = tx.unbounded_send(hit);
                    })
            }
        };

        self.is_searching = true;
        self.search_handle = Some(handle);
        let generation = self.search_generation;

        // Stream hits onto the UI thread as the background walk discovers them.
        self.stream_task = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut hits = vec![first];
                while hits.len() < 50 {
                    match rx.try_recv() {
                        Ok(hit) => hits.push(hit),
                        Err(_) => break,
                    }
                }

                let update_res = this.update(cx, |view, cx| {
                    if view.search_generation != generation {
                        return;
                    }
                    view.add_hits(hits);
                    cx.notify();
                });
                if update_res.is_err() {
                    return;
                }
            }

            let _ = this.update(cx, |view, cx| {
                if view.search_generation != generation {
                    return;
                }
                view.is_searching = false;
                view.search_handle = None;
                view.stream_task = None;
                cx.notify();
            });
        }));

        cx.notify();
    }

    /// Appends and sorts search hits.
    pub fn add_hits(&mut self, hits: Vec<SearchHit>) {
        self.results.extend(hits);
        self.results.sort_by_key(|b| std::cmp::Reverse(b.score));
        if self.selected_index.is_none() && !self.results.is_empty() {
            self.selected_index = Some(0);
        }
    }

    /// Sets the search query programmatically and runs the search.
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        let q = query.into();
        self.search_input.update(cx, |input, cx| {
            input.set_text(q.clone(), cx);
        });
        self.last_query = q.clone();
        self.start_search(q, cx);
    }

    /// Toggles the ignore files setting (local searches).
    pub fn toggle_respect_ignore(&mut self, cx: &mut Context<Self>) {
        self.set_respect_ignore(!self.respect_ignore, cx);
    }

    /// Sets the ignore files setting and re-runs search if active.
    pub fn set_respect_ignore(&mut self, respect: bool, cx: &mut Context<Self>) {
        self.respect_ignore = respect;
        if !self.last_query.is_empty() {
            let q = self.last_query.clone();
            self.start_search(q, cx);
        } else {
            cx.notify();
        }
    }

    /// Stops any running search and clears results when the user navigates to another location.
    pub fn stop_for_location_change(&mut self, new_location: &Location, cx: &mut Context<Self>) {
        let changed = match &self.current_location {
            Some(current) => current != new_location,
            None => match &self.current_root {
                Some(root) => root != new_location.path(),
                None => false,
            },
        };
        if changed {
            self.stop_search();
            self.results.clear();
            self.selected_index = None;
            self.search_error = None;
            self.current_root = Some(new_location.path().to_path_buf());
            self.current_location = Some(new_location.clone());
            cx.notify();
        }
    }

    /// Alias for [`Self::stop_for_location_change`].
    pub fn handle_location_change(&mut self, new_location: &Location, cx: &mut Context<Self>) {
        self.stop_for_location_change(new_location, cx);
    }

    /// Selects the next result in the list.
    pub fn select_next(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let next = match self.selected_index {
            Some(i) if i + 1 < self.results.len() => i + 1,
            _ => 0,
        };
        self.selected_index = Some(next);
        cx.notify();
    }

    /// Selects the previous result in the list.
    pub fn select_previous(&mut self, cx: &mut Context<Self>) {
        if self.results.is_empty() {
            return;
        }
        let prev = match self.selected_index {
            Some(i) if i > 0 => i - 1,
            _ => self.results.len() - 1,
        };
        self.selected_index = Some(prev);
        cx.notify();
    }

    /// Opens the currently selected search result, navigating to its folder and selecting it.
    pub fn open_selected_result(&mut self, cx: &mut Context<Self>) -> Option<PathBuf> {
        let index = self.selected_index.unwrap_or(0);
        self.open_result_at(index, cx)
    }

    /// Opens the search result at `index`, navigating to its parent folder and selecting the item
    /// for both local and remote locations.
    pub fn open_result_at(&mut self, index: usize, cx: &mut Context<Self>) -> Option<PathBuf> {
        let hit = self.results.get(index).cloned()?;
        self.stop_search();

        let target_path = hit.path.clone();
        let base_location = self
            .current_location
            .clone()
            .unwrap_or_else(|| self.app_state.read(cx).current_location().clone());
        let target_loc = location_with_path(&base_location, target_path.clone());
        let parent_loc = target_loc.parent().unwrap_or_else(|| target_loc.clone());

        self.current_root = Some(parent_loc.path().to_path_buf());
        self.current_location = Some(parent_loc.clone());
        self.pending_reveal = None;

        self.app_state.update(cx, |state, cx| {
            state.set_search_query(None);
            if state.current_location() != &parent_loc {
                state.navigate_to(parent_loc.clone());
            }
            state.select_location(target_loc.clone());
            cx.notify();
        });

        // If `navigate_to` ran asynchronously on background executor, `arrive` will clear
        // selection once the load completes; remember `target_loc` to re-apply on arrival.
        if self.app_state.read(cx).current_location() != &parent_loc {
            self.pending_reveal = Some((parent_loc, target_loc));
        }

        self.is_active = false;
        cx.notify();
        Some(target_path)
    }

    /// Returns a slice of current search hits.
    pub fn hits(&self) -> &[SearchHit] {
        &self.results
    }
}

fn location_with_path(base: &Location, path: PathBuf) -> Location {
    match base {
        Location::Local(_) => Location::Local(path),
        Location::Remote { profile_id, .. } => Location::Remote {
            profile_id: profile_id.clone(),
            path,
        },
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.is_active {
            return div()
                .key_context("SearchView")
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                    this.toggle(window, cx);
                }));
        }

        let (is_remote, max_depth) = {
            let state = self.app_state.read(cx);
            (
                matches!(state.current_location(), Location::Remote { .. }),
                state.settings.remote_search_depth.max(1),
            )
        };

        let query = self.last_query.clone();
        let hit_count = self.results.len();
        let status_text = if let Some(err) = &self.search_error {
            err.clone()
        } else if self.is_searching {
            if is_remote {
                if hit_count == 0 {
                    format!("Searching remote (max depth: {max_depth})...")
                } else {
                    format!("Searching remote (max depth: {max_depth})... ({hit_count} results)")
                }
            } else if hit_count == 0 {
                "Searching...".to_string()
            } else {
                format!("Searching... ({hit_count} results)")
            }
        } else if !query.is_empty() {
            if is_remote {
                format!("{hit_count} results for \"{query}\" (max depth: {max_depth})")
            } else {
                format!("{hit_count} results for \"{query}\"")
            }
        } else if is_remote {
            format!("Type to search remote folder (max depth: {max_depth})...")
        } else {
            "Type to search directory...".to_string()
        };

        let selected_idx = self.selected_index;
        let root_path = self.current_root.clone();

        let mut results_elements = Vec::new();
        for (ix, hit) in self.results.iter().take(100).enumerate() {
            let is_selected = selected_idx == Some(ix);
            let hit_path = hit.path.clone();
            let display_name = hit_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| hit_path.to_string_lossy().to_string());

            let rel_path = root_path
                .as_ref()
                .and_then(|r| hit_path.strip_prefix(r).ok())
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| hit_path.to_string_lossy().to_string());

            results_elements.push(
                div()
                    .id(ix)
                    .flex()
                    .flex_row()
                    .justify_between()
                    .items_center()
                    .h(size::ROW)
                    .px(space::S8)
                    .cursor_pointer()
                    .bg(if is_selected {
                        theme.selection
                    } else {
                        transparent_black().into()
                    })
                    .hover(|s| s.bg(theme.surface_raised))
                    .text_size(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(IconName::File).text_color(theme.text))
                            .child(
                                div()
                                    .text_color(theme.text)
                                    .font_weight(if is_selected {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .child(display_name),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.text_muted)
                                    .child(format!("({})", rel_path)),
                            ),
                    )
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.text_muted)
                            .child(format!("score {}", hit.score)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.open_result_at(ix, cx);
                        }),
                    ),
            );
        }

        let scope_control = if is_remote {
            div()
                .id("remote-search-depth-badge")
                .px_2()
                .py_1()
                .rounded(radius::CONTROL)
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border)
                .text_color(theme.text_muted)
                .text_size(px(11.))
                .child(format!("Max depth: {max_depth}"))
                .into_any_element()
        } else {
            div()
                .id("toggle-respect-ignore-btn")
                .px_2()
                .py_1()
                .rounded(radius::CONTROL)
                .bg(if self.respect_ignore {
                    theme.surface
                } else {
                    theme.accent
                })
                .text_color(if self.respect_ignore {
                    theme.text_muted
                } else {
                    theme.on_accent
                })
                .text_size(px(11.))
                .cursor_pointer()
                .child(if self.respect_ignore {
                    "Ignore files: ON"
                } else {
                    "Ignore files: OFF"
                })
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.toggle_respect_ignore(cx);
                    }),
                )
                .into_any_element()
        };

        div()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .border_b_1()
            .border_color(theme.border)
            .key_context("SearchView")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &ToggleSearch, window, cx| {
                this.toggle(window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseSearch, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &NextSearchResult, _, cx| this.select_next(cx)))
            .on_action(
                cx.listener(|this, _: &PreviousSearchResult, _, cx| this.select_previous(cx)),
            )
            .on_action(cx.listener(|this, _: &OpenSearchResult, _, cx| {
                this.open_selected_result(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleRespectIgnore, _, cx| {
                this.toggle_respect_ignore(cx);
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    this.close(cx);
                }
            }))
            .child(
                // Search Input Header Bar
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .child(icon(IconName::Search).text_color(theme.text_muted))
                    .child(div().flex_1().child(self.search_input.clone()))
                    .child(scope_control)
                    .when(self.is_searching, |row| {
                        row.child(
                            div()
                                .id("search-cancel-btn")
                                .px_2()
                                .py_1()
                                .rounded(radius::CONTROL)
                                .bg(theme.surface_raised)
                                .border_1()
                                .border_color(theme.border)
                                .text_color(theme.text)
                                .text_size(px(11.))
                                .cursor_pointer()
                                .hover(|s| s.bg(theme.selection))
                                .child("Cancel")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.cancel_search(cx);
                                    }),
                                ),
                        )
                    })
                    .child(
                        // Close button
                        div()
                            .id("search-close-btn")
                            .px_2()
                            .py_1()
                            .rounded(radius::CONTROL)
                            .hover(|s| s.bg(theme.surface_raised))
                            .cursor_pointer()
                            .child(icon(IconName::Close).text_color(theme.text_muted))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close(cx);
                                }),
                            ),
                    ),
            )
            .child(
                // Status Bar
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(if self.search_error.is_some() {
                        theme.danger
                    } else {
                        theme.text_muted
                    })
                    .border_t_1()
                    .border_color(theme.border)
                    .child(status_text),
            )
            .child(if !results_elements.is_empty() {
                div()
                    .flex()
                    .flex_col()
                    .max_h(px(220.))
                    .overflow_hidden()
                    .border_t_1()
                    .border_color(theme.border)
                    .children(results_elements)
            } else {
                div()
            })
    }
}
