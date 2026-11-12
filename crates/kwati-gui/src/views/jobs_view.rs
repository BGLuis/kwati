//! Background jobs view (owned by guiops-worker for #21, #23).
//!
//! Displays progress of running copy, move, and file operations with cancellation support.

use gpui::{div, prelude::*, px, relative, Context, Entity, MouseButton, MouseDownEvent, Window};

use crate::actions::operations::cancel_active_job;
use crate::state::AppState;
use crate::theme::Theme;

pub struct JobsView {
    pub app_state: Entity<AppState>,
}

impl JobsView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self { app_state }
    }

    fn cancel_job(
        &mut self,
        job_id: u64,
        _: &MouseDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cancel_active_job(job_id);
        self.app_state.update(cx, |state, _cx| {
            if let Some(job) = state.active_jobs.iter_mut().find(|j| j.id == job_id) {
                job.is_cancelled = true;
            }
            state.remove_job(job_id);
            state.set_status(format!("Cancelled job #{}", job_id));
        });
        cx.notify();
    }
}

impl Render for JobsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let state = self.app_state.read(cx);
        let jobs = state.active_jobs.clone();

        if jobs.is_empty() {
            return div();
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .children(jobs.into_iter().map(|job| {
                let job_id = job.id;
                let progress = job.progress_pct as f32 / 100.0;

                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .rounded_md()
                    .bg(theme.surface_raised)
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(theme.text)
                            .child(job.title)
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(format!("{}%", job.progress_pct))
                                    .child(
                                        div()
                                            .px_2()
                                            .py(px(2.))
                                            .rounded_sm()
                                            .hover(|s| s.bg(theme.surface_raised))
                                            .text_color(theme.danger)
                                            .cursor_pointer()
                                            .child("Cancel")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, ev, window, cx| {
                                                    this.cancel_job(job_id, ev, window, cx);
                                                }),
                                            ),
                                    ),
                            ),
                    )
                    // Progress bar
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_sm()
                            .bg(theme.border)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(progress))
                                    .rounded_sm()
                                    .bg(theme.accent),
                            ),
                    )
            }))
    }
}
