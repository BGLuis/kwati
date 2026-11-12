//! Search overlay view (owned by inspect-worker for #27).
//!
//! Provides interactive recursive file search, background streaming search with
//! cancellation, ignore files toggle, and result navigation.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

use gpui::{
    actions, div, prelude::*, px, transparent_black, App, Context, Entity, FontWeight, KeyBinding,
    MouseButton, Subscription, Window,
};
use kwati_core::{Location, SearchHandle, SearchHit, SearchOptions, Searcher};

use crate::components::TextInput;
use crate::state::AppState;
use crate::theme::Theme;

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
    pub is_active: bool,
    pub results: Vec<SearchHit>,
    pub selected_index: Option<usize>,
    pub respect_ignore: bool,
    pub is_searching: bool,
    pub last_query: String,
    pub current_root: Option<PathBuf>,
    searcher: Searcher,
    search_handle: Option<SearchHandle>,
    _input_sub: Subscription,
    _state_sub: Subscription,
}

impl SearchView {
    pub fn new(app_state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search_input = cx.new(|cx| TextInput::with_placeholder("Search directory...", cx));

        let input_sub = cx.observe(&search_input, |this, input, cx| {
            let text = input.read(cx).text().to_string();
            if text != this.last_query {
                this.last_query = text.clone();
                this.start_search(text, cx);
            }
        });

        let state_sub = cx.observe(&app_state, |this, app_state, cx| {
            let current_loc = app_state.read(cx).current_location.clone();
            this.handle_location_change(&current_loc, cx);
        });

        Self {
            app_state,
            search_input,
            is_active: false,
            results: Vec::new(),
            selected_index: None,
            respect_ignore: true,
            is_searching: false,
            last_query: String::new(),
            current_root: None,
            searcher: Searcher::new(),
            search_handle: None,
            _input_sub: input_sub,
            _state_sub: state_sub,
        }
    }

    /// Toggles the search bar visibility.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.is_active {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    /// Opens the search bar and focuses the input.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.is_active = true;
        let root = self
            .app_state
            .read(cx)
            .current_location
            .path()
            .to_path_buf();
        self.current_root = Some(root);
        cx.notify();
    }

    /// Closes the search bar and cancels any running search.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.stop_search();
        self.is_active = false;
        self.results.clear();
        self.selected_index = None;
        self.app_state.update(cx, |state, _cx| {
            state.set_search_query(None);
        });
        cx.notify();
    }

    /// Stops any running background search.
    pub fn stop_search(&mut self) {
        if let Some(handle) = self.search_handle.take() {
            handle.stop();
        }
        self.is_searching = false;
    }

    /// Starts a new recursive search below current browsing location.
    pub fn start_search(&mut self, query: String, cx: &mut Context<Self>) {
        self.stop_search();
        self.results.clear();
        self.selected_index = None;

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

        let root = self
            .app_state
            .read(cx)
            .current_location
            .path()
            .to_path_buf();
        self.current_root = Some(root.clone());
        self.is_searching = true;

        let options = SearchOptions {
            respect_ignore: self.respect_ignore,
        };

        let (tx, rx): (Sender<SearchHit>, Receiver<SearchHit>) = channel();
        let handle = self.searcher.search(&root, &query, options, move |hit| {
            let _ = tx.send(hit);
        });
        self.search_handle = Some(handle);

        // Spawn a task to stream results into the view as they arrive
        cx.spawn(async move |this, cx| {
            loop {
                // Read available hits from receiver
                let mut hits = Vec::new();
                while let Ok(hit) = rx.try_recv() {
                    hits.push(hit);
                    if hits.len() >= 50 {
                        break;
                    }
                }

                if !hits.is_empty() {
                    let update_res = this.update(cx, |view, cx| {
                        view.add_hits(hits);
                        cx.notify();
                    });
                    if update_res.is_err() {
                        break;
                    }
                }

                // If sender has disconnected and channel is empty, search has finished
                if let Err(std::sync::mpsc::TryRecvError::Disconnected) = rx.try_recv() {
                    let _ = this.update(cx, |view, cx| {
                        view.is_searching = false;
                        cx.notify();
                    });
                    break;
                }

                // Short sleep between streaming batches
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
            }
        })
        .detach();

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

    /// Sets the search query programmatically.
    pub fn set_query(&mut self, query: impl Into<String>, cx: &mut Context<Self>) {
        let q = query.into();
        self.search_input.update(cx, |input, cx| {
            input.set_text(q.clone(), cx);
        });
        self.last_query = q.clone();
        self.start_search(q, cx);
    }

    /// Toggles the ignore files setting.
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

    /// Handles location changes: navigating to another directory stops the running search.
    pub fn handle_location_change(&mut self, new_location: &Location, cx: &mut Context<Self>) {
        let new_path = new_location.path();
        if let Some(ref current_root) = self.current_root {
            if current_root != new_path {
                self.stop_search();
                self.results.clear();
                self.selected_index = None;
                self.current_root = Some(new_path.to_path_buf());
                cx.notify();
            }
        }
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

    /// Opens the search result at the specified index.
    pub fn open_result_at(&mut self, index: usize, cx: &mut Context<Self>) -> Option<PathBuf> {
        if let Some(hit) = self.results.get(index).cloned() {
            self.stop_search();
            let target_path = hit.path.clone();
            let parent = target_path.parent().unwrap_or(&hit.path).to_path_buf();

            self.app_state.update(cx, |state, _cx| {
                state.navigate_to(Location::Local(parent));
                state
                    .selection
                    .select_only(Location::Local(target_path.clone()));
            });

            self.is_active = false;
            cx.notify();
            Some(target_path)
        } else {
            None
        }
    }

    /// Returns a slice of current search hits.
    pub fn hits(&self) -> &[SearchHit] {
        &self.results
    }
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        if !self.is_active {
            return div();
        }

        let query = self.last_query.clone();
        let hit_count = self.results.len();
        let status_text = if self.is_searching {
            if hit_count == 0 {
                "Searching...".to_string()
            } else {
                format!("Searching... ({} results)", hit_count)
            }
        } else if !query.is_empty() {
            format!("{} results for \"{}\"", hit_count, query)
        } else {
            "Type to search directory...".to_string()
        };

        let selected_idx = self.selected_index;
        let root_path = self.current_root.clone();

        let mut results_elements = Vec::new();
        // Display up to 100 results
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
                    .h(px(26.))
                    .px_3()
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
                            .child("📄")
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

        div()
            .w_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .border_b_1()
            .border_color(theme.border)
            .on_action(cx.listener(|this, _: &ToggleSearch, _, cx| this.toggle(cx)))
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
            .child(
                // Search Input Header Bar
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(theme.text_muted)
                            .child("🔍"),
                    )
                    .child(div().flex_1().child(self.search_input.clone()))
                    .child(
                        // Respect .gitignore toggle
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
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
                            ),
                    )
                    .child(
                        // Close button
                        div()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_size(px(13.))
                            .text_color(theme.text_muted)
                            .hover(|s| s.text_color(theme.text))
                            .cursor_pointer()
                            .child("✕")
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
                    .text_color(theme.text_muted)
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
