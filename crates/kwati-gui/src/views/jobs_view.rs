//! Background jobs view (owned by guiops-worker for #21, #23).
//!
//! Displays progress of running copy, move, and file operations with cancellation support.

use std::time::Instant;

use gpui::{div, prelude::*, px, relative, Context, Entity, MouseButton, MouseDownEvent, Window};

use crate::actions::operations::cancel_active_job;
use crate::motion::{
    color_lerp, is_reduced_motion, request_frame_if_active, Curve, MotionKind, MotionTracker,
    JOB_COMPLETE_HOLD, QUICK, SETTLE,
};
use crate::state::AppState;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum JobMotionKey {
    Progress(u64),
}

pub struct JobsView {
    pub app_state: Entity<AppState>,
    motion: MotionTracker<JobMotionKey>,
}

impl JobsView {
    pub fn new(app_state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            app_state,
            motion: MotionTracker::new(),
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let theme = Theme::get(cx);
        let (jobs, completed_jobs, reduced) = self.app_state.update(cx, |state, _| {
            state.prune_completed_jobs(now);
            (
                state.active_jobs.clone(),
                state.completed_jobs.clone(),
                is_reduced_motion(state.settings.motion_preference),
            )
        });

        if jobs.is_empty() && completed_jobs.is_empty() {
            self.motion.clear();
            return div();
        }

        let active_rows: Vec<_> = jobs
            .into_iter()
            .map(|job| {
                let job_id = job.id;
                let target = (job.progress_pct as f32 / 100.0).clamp(0.0, 1.0);
                let display = self
                    .motion
                    .step_scalar(
                        JobMotionKey::Progress(job_id),
                        0.0,
                        target,
                        SETTLE,
                        Curve::Spring,
                        MotionKind::MovementOrScale,
                        now,
                        reduced,
                    )
                    .clamp(0.0, 1.0);

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
                                    .w(relative(display))
                                    .rounded_sm()
                                    .bg(theme.accent),
                            ),
                    )
            })
            .collect();

        let completed_rows: Vec<_> = completed_jobs
            .into_iter()
            .map(|job| {
                let elapsed = now.saturating_duration_since(job.completed_at);
                let pulse_t = (elapsed.as_secs_f32() / QUICK.as_secs_f32()).clamp(0.0, 1.0);
                let pulse = if !reduced && pulse_t < 1.0 {
                    (pulse_t * std::f32::consts::PI).sin()
                } else {
                    0.0
                };
                let fade_elapsed = elapsed.saturating_sub(JOB_COMPLETE_HOLD);
                let fade_t = (fade_elapsed.as_secs_f32() / QUICK.as_secs_f32()).clamp(0.0, 1.0);
                let opacity = 1.0 - Curve::EaseOut.sample(fade_t);
                let bar_col = color_lerp(theme.secondary, theme.on_accent, pulse * 0.25);

                div()
                    .flex()
                    .flex_col()
                    .p_2()
                    .rounded_md()
                    .bg(theme.surface_raised)
                    .gap_1()
                    .opacity(opacity)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .items_center()
                            .text_size(px(12.))
                            .text_color(theme.secondary)
                            .child(job.title)
                            .child("100%"),
                    )
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_sm()
                            .bg(theme.border)
                            .child(div().h_full().w_full().rounded_sm().bg(bar_col)),
                    )
            })
            .collect();

        let has_completed = !self.app_state.read(cx).completed_jobs.is_empty();
        let animating = self.motion.has_active(now, reduced) || has_completed;
        request_frame_if_active(animating, window);

        div()
            .flex()
            .flex_col()
            .w_full()
            .p_2()
            .gap_2()
            .bg(theme.surface)
            .border_t_1()
            .border_color(theme.border)
            .children(active_rows)
            .children(completed_rows)
    }
}
