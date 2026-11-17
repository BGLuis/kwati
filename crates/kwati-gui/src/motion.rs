//! Motion tokens, easing curves, interruption-safe transitions, and reduced-motion support.
//!
//! Values match `DESIGN.md` ("Motion"):
//! - `INSTANT` (`160 ms`): hover, press, selection, focus rings.
//! - `QUICK` (`300 ms`): animated icons, menus, tooltips, toggles, checkboxes.
//! - `SETTLE` (`460 ms`): folder entry/back/forward, sort/filter, inserts, panels, dialogs, progress.
//! - Specific durations: delete throw (`420 ms`), invalid-input shake (`560 ms`), completion hold (`1000 ms`),
//!   and mount indeterminate delay (`300 ms`).

use std::collections::HashMap;
use std::f32::consts::TAU;
use std::hash::Hash;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use gpui::{Hsla, Rgba, Window};
use kwati_core::MotionPreference;

/// Instant micro-transitions (hover, press, selection, focus ring).
pub const INSTANT: Duration = Duration::from_millis(160);
/// Short self-contained state shifts (animated icons, menus, tooltips, toggles, checkboxes).
pub const QUICK: Duration = Duration::from_millis(300);
/// Structural and list motion (navigation, sort, inserts, panels, dialogs, progress bars).
pub const SETTLE: Duration = Duration::from_millis(460);
/// Deleted row exit throw + collapse window.
pub const DELETE_THROW: Duration = Duration::from_millis(420);
/// Rejected input decaying 5-cycle shake.
pub const SHAKE: Duration = Duration::from_millis(560);
/// Hold time after a job hits completion before fading out.
pub const JOB_COMPLETE_HOLD: Duration = Duration::from_millis(1000);
/// Delay before showing an indeterminate progress bar while mounting/connecting.
pub const MOUNT_INDETERMINATE_DELAY: Duration = Duration::from_millis(300);
/// Stagger between list rows entering on folder navigation.
pub const LIST_CASCADE_STEP: Duration = Duration::from_millis(38);
/// Maximum number of visible list rows staggered in one cascade.
pub const LIST_CASCADE_CAP: usize = 12;
/// Stagger between context menu items when opening.
pub const MENU_CASCADE_STEP: Duration = Duration::from_millis(45);

/// Distance (`px`) for list row arrival slide.
pub const LIST_ENTER_OFFSET_PX: f32 = 36.0;
/// Distance (`px`) for deleted row right throw.
pub const DELETE_THROW_PX: f32 = 64.0;
/// Peak horizontal amplitude (`px`) for rejected input shake.
pub const SHAKE_AMPLITUDE_PX: f32 = 11.0;
/// Number of oscillation cycles in a shake.
pub const SHAKE_CYCLES: f32 = 5.0;
/// Vertical rise (`px`) for dialog entrance.
pub const DIALOG_RISE_PX: f32 = 28.0;
/// Starting scale for dialog entrance.
pub const DIALOG_START_SCALE: f32 = 0.72;
/// Starting scale for menu and tooltip entrance.
pub const MENU_START_SCALE: f32 = 0.6;
/// Starting scale for newly created/renamed/pasted row.
pub const ROW_INSERT_START_SCALE: f32 = 0.85;
/// Starting scale for status completion message.
pub const STATUS_POP_START_SCALE: f32 = 0.7;
/// Pressed scale factor on interactive controls.
pub const PRESS_SCALE: f32 = 0.93;

/// Controls whether an animation is omitted under reduced motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKind {
    /// Preserved under reduced motion, clamped to [`INSTANT`].
    OpacityOrColor,
    /// Removed (snaps immediately) under reduced motion.
    MovementOrScale,
}

/// Easing curves defined by `DESIGN.md`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Curve {
    Linear,
    /// `cubic-bezier(0.34, 1.56, 0.64, 1)` — may overshoot past `1.0` for bouncy spring motion.
    Spring,
    /// Smooth deceleration for colour and opacity fades.
    EaseOut,
    /// `cubic-bezier(0.5, 0, 0.75, 0)` — accelerating departure for exit motion.
    Exit,
}

impl Curve {
    /// Evaluates the curve at normalized time `t` in `[0.0, 1.0]`.
    pub fn sample(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        if t <= 0.0 {
            return 0.0;
        }
        if t >= 1.0 {
            return 1.0;
        }
        match self {
            Self::Linear => t,
            Self::Spring => cubic_bezier(0.34, 1.56, 0.64, 1.0, t),
            Self::EaseOut => cubic_bezier(0.0, 0.0, 0.58, 1.0, t),
            Self::Exit => cubic_bezier(0.5, 0.0, 0.75, 0.0, t),
        }
    }
}

/// Evaluates a 2D CSS-style cubic Bézier timing function (`P0=(0,0)`, `P3=(1,1)`) at `x = t`.
pub fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, target_x: f32) -> f32 {
    let target_x = target_x.clamp(0.0, 1.0);
    let mut lo = 0.0_f32;
    let mut hi = 1.0_f32;
    let mut u = target_x;

    // Newton-Raphson with bracket clamping handles non-monotonic or steep segments reliably.
    for _ in 0..8 {
        let x = bezier_coord(x1, x2, u) - target_x;
        if x.abs() < 1e-5 {
            break;
        }
        if x < 0.0 {
            lo = u;
        } else {
            hi = u;
        }
        let dx = bezier_derivative(x1, x2, u);
        if dx.abs() > 1e-5 {
            let next = u - x / dx;
            u = if next > lo && next < hi {
                next
            } else {
                0.5 * (lo + hi)
            };
        } else {
            u = 0.5 * (lo + hi);
        }
    }

    bezier_coord(y1, y2, u)
}

#[inline]
fn bezier_coord(p1: f32, p2: f32, u: f32) -> f32 {
    let inv = 1.0 - u;
    3.0 * inv * inv * u * p1 + 3.0 * inv * u * u * p2 + u * u * u
}

#[inline]
fn bezier_derivative(p1: f32, p2: f32, u: f32) -> f32 {
    let inv = 1.0 - u;
    3.0 * inv * inv * p1 + 6.0 * inv * u * (p2 - p1) + 3.0 * u * u * (1.0 - p2)
}

/// Linear interpolation between `a` and `b` using factor `t` (supports overshoot).
#[inline]
pub fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Linear RGBA interpolation between two GPUI colors.
pub fn color_lerp(from: impl Into<Hsla>, to: impl Into<Hsla>, t: f32) -> Hsla {
    let from = from.into();
    let to = to.into();
    let t = t.clamp(0.0, 1.0);
    if t <= 0.0 {
        return from;
    }
    if t >= 1.0 {
        return to;
    }
    let a = Rgba::from(from);
    let b = Rgba::from(to);
    Rgba {
        r: lerp_f32(a.r, b.r, t),
        g: lerp_f32(a.g, b.g, t),
        b: lerp_f32(a.b, b.b, t),
        a: lerp_f32(a.a, b.a, t),
    }
    .into()
}

/// Horizontal shake offset (`px`) for normalized progress `t` in `[0.0, 1.0]`.
///
/// Shakes `11 px`, 5 cycles, decaying linearly to rest.
pub fn shake_decay(t: f32) -> f32 {
    if t <= 0.0 || t >= 1.0 {
        return 0.0;
    }
    SHAKE_AMPLITUDE_PX * (1.0 - t) * (t * SHAKE_CYCLES * TAU).sin()
}

/// Returns whether the OS prefers reduced motion, cached once per process.
pub fn os_reduced_motion() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(detect_os_reduced_motion)
}

fn detect_os_reduced_motion() -> bool {
    if let Ok(val) = std::env::var("KWATI_REDUCED_MOTION") {
        let lower = val.trim().to_ascii_lowercase();
        if matches!(lower.as_str(), "1" | "true" | "yes" | "on") {
            return true;
        }
        if matches!(lower.as_str(), "0" | "false" | "no" | "off") {
            return false;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(out) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if s.trim() == "false" {
                    return true;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(out) = std::process::Command::new("defaults")
            .args(["read", "-g", "com.apple.universalaccess", "reduceMotion"])
            .output()
        {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout);
                if s.trim() == "1" {
                    return true;
                }
            }
        }
    }

    false
}

/// Resolves whether reduced motion is currently active given a [`MotionPreference`].
pub fn is_reduced_motion(pref: MotionPreference) -> bool {
    resolve_reduced_motion(pref, os_reduced_motion())
}

/// Pure resolver helper for deterministic tests.
pub const fn resolve_reduced_motion(pref: MotionPreference, os_prefers_reduced: bool) -> bool {
    match pref {
        MotionPreference::System => os_prefers_reduced,
        MotionPreference::Full => false,
        MotionPreference::Reduced => true,
    }
}

static ACTIVE_MOTION_PREF: AtomicU8 = AtomicU8::new(0);

/// Synchronizes the global GUI motion preference from `AppState::settings`.
pub fn set_active_motion_preference(pref: MotionPreference) {
    let code = match pref {
        MotionPreference::System => 0,
        MotionPreference::Full => 1,
        MotionPreference::Reduced => 2,
    };
    ACTIVE_MOTION_PREF.store(code, Ordering::Relaxed);
}

/// Returns the currently active `MotionPreference` configured by `AppState`.
pub fn active_motion_preference() -> MotionPreference {
    match ACTIVE_MOTION_PREF.load(Ordering::Relaxed) {
        1 => MotionPreference::Full,
        2 => MotionPreference::Reduced,
        _ => MotionPreference::System,
    }
}

/// Returns whether reduced motion is currently enabled for leaf UI components.
pub fn reduced_motion_active() -> bool {
    is_reduced_motion(active_motion_preference())
}

/// Computes effective duration and whether a transition should snap under `reduced`.
#[inline]
pub fn effective_duration(duration: Duration, kind: MotionKind, reduced: bool) -> Duration {
    if !reduced {
        return duration;
    }
    match kind {
        MotionKind::MovementOrScale => Duration::ZERO,
        MotionKind::OpacityOrColor => duration.min(INSTANT),
    }
}

/// An interruption-safe scalar transition.
#[derive(Debug, Clone)]
pub struct ScalarTransition {
    pub from: f32,
    pub to: f32,
    pub started_at: Instant,
    pub duration: Duration,
    pub curve: Curve,
    pub kind: MotionKind,
}

impl ScalarTransition {
    pub fn settled(value: f32, kind: MotionKind) -> Self {
        Self {
            from: value,
            to: value,
            started_at: Instant::now(),
            duration: Duration::ZERO,
            curve: Curve::EaseOut,
            kind,
        }
    }

    /// Evaluates the interpolated scalar at `now`.
    pub fn value_at(&self, now: Instant, reduced: bool) -> f32 {
        let dur = effective_duration(self.duration, self.kind, reduced);
        if dur.is_zero() || now <= self.started_at && (self.from - self.to).abs() <= f32::EPSILON {
            return self.to;
        }
        if now <= self.started_at {
            return self.from;
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= dur {
            return self.to;
        }
        let t = (elapsed.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0);
        let curve = if reduced { Curve::EaseOut } else { self.curve };
        lerp_f32(self.from, self.to, curve.sample(t))
    }

    pub fn is_active_at(&self, now: Instant, reduced: bool) -> bool {
        let dur = effective_duration(self.duration, self.kind, reduced);
        !dur.is_zero() && now.saturating_duration_since(self.started_at) < dur
    }

    /// Redirects the transition smoothly from its current in-flight value toward `target`.
    pub fn retarget(
        &mut self,
        target: f32,
        duration: Duration,
        curve: Curve,
        kind: MotionKind,
        now: Instant,
        reduced: bool,
    ) {
        if (self.to - target).abs() <= 1e-4 && self.kind == kind {
            return;
        }
        let current = self.value_at(now, reduced);
        self.from = current;
        self.to = target;
        self.started_at = now;
        self.duration = duration;
        self.curve = curve;
        self.kind = kind;
    }
}

/// One-shot normalized `[0.0, 1.0]` animation trigger (icons, shakes, flashes).
#[derive(Debug, Clone)]
pub struct PulseAnim {
    pub started_at: Instant,
    pub duration: Duration,
    pub kind: MotionKind,
    pub generation: u64,
}

impl PulseAnim {
    pub fn trigger_at(duration: Duration, kind: MotionKind, now: Instant, generation: u64) -> Self {
        Self {
            started_at: now,
            duration,
            kind,
            generation,
        }
    }

    /// Normalized linear `[0.0, 1.0]` progress while running, or `None` once finished/disabled.
    pub fn progress_at(&self, now: Instant, reduced: bool) -> Option<f32> {
        let dur = effective_duration(self.duration, self.kind, reduced);
        if dur.is_zero() {
            return None;
        }
        let elapsed = now.saturating_duration_since(self.started_at);
        if elapsed >= dur {
            None
        } else {
            Some((elapsed.as_secs_f32() / dur.as_secs_f32()).clamp(0.0, 1.0))
        }
    }
}

/// Per-view keyed animation store for hover, press, focus, selection, icons, and one-shot pulses.
#[derive(Debug, Clone)]
pub struct MotionTracker<K: Eq + Hash + Clone> {
    scalars: HashMap<K, ScalarTransition>,
    pulses: HashMap<K, PulseAnim>,
    next_gen: u64,
}

impl<K: Eq + Hash + Clone> Default for MotionTracker<K> {
    fn default() -> Self {
        Self {
            scalars: HashMap::new(),
            pulses: HashMap::new(),
            next_gen: 1,
        }
    }
}

impl<K: Eq + Hash + Clone> MotionTracker<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Updates or initializes a scalar transition toward `target` and returns its value at `now`.
    ///
    /// First-time initialization snaps to `initial_if_new` and immediately targets `target`.
    #[allow(clippy::too_many_arguments)]
    pub fn step_scalar(
        &mut self,
        key: K,
        initial_if_new: f32,
        target: f32,
        duration: Duration,
        curve: Curve,
        kind: MotionKind,
        now: Instant,
        reduced: bool,
    ) -> f32 {
        let entry = self.scalars.entry(key).or_insert_with(|| {
            let mut tr = ScalarTransition::settled(initial_if_new, kind);
            if (initial_if_new - target).abs() > 1e-4 {
                tr.retarget(target, duration, curve, kind, now, reduced);
            }
            tr
        });
        entry.retarget(target, duration, curve, kind, now, reduced);
        entry.value_at(now, reduced)
    }

    /// Reads the current scalar without creating an entry, defaulting to `default`.
    pub fn scalar_or(&self, key: &K, default: f32, now: Instant, reduced: bool) -> f32 {
        self.scalars
            .get(key)
            .map_or(default, |t| t.value_at(now, reduced))
    }

    /// Returns `true` if the scalar for `key` is currently animating at `now`.
    pub fn is_scalar_active(&self, key: &K, now: Instant, reduced: bool) -> bool {
        self.scalars
            .get(key)
            .is_some_and(|t| t.is_active_at(now, reduced))
    }

    /// Forces a scalar to an explicit value without animating.
    pub fn set_immediate(&mut self, key: K, value: f32, kind: MotionKind) {
        self.scalars
            .insert(key, ScalarTransition::settled(value, kind));
    }

    /// Starts or interrupts a one-shot pulse for `key`.
    pub fn trigger_pulse(&mut self, key: K, duration: Duration, kind: MotionKind, now: Instant) {
        let gen_id = self.next_gen;
        self.next_gen = self.next_gen.wrapping_add(1);
        self.pulses
            .insert(key, PulseAnim::trigger_at(duration, kind, now, gen_id));
    }

    /// Returns normalized `[0.0, 1.0)` progress if the pulse for `key` is active at `now`.
    pub fn pulse_progress(&self, key: &K, now: Instant, reduced: bool) -> Option<f32> {
        self.pulses
            .get(key)
            .and_then(|p| p.progress_at(now, reduced))
    }

    /// Returns `true` if any tracked transition or pulse is still animating at `now`.
    pub fn has_active(&self, now: Instant, reduced: bool) -> bool {
        self.scalars.values().any(|t| t.is_active_at(now, reduced))
            || self
                .pulses
                .values()
                .any(|p| p.progress_at(now, reduced).is_some())
    }

    /// Drops completed pulses and settled zero-valued scalars so memory stays bounded.
    pub fn prune_settled(&mut self, now: Instant, reduced: bool) {
        self.pulses
            .retain(|_, p| p.progress_at(now, reduced).is_some());
        self.scalars
            .retain(|_, t| t.is_active_at(now, reduced) || t.to.abs() > 1e-4);
    }

    pub fn clear(&mut self) {
        self.scalars.clear();
        self.pulses.clear();
    }
}

/// Schedules another frame redraw only when `active` is `true`, returning whether a frame was requested.
#[inline]
pub fn request_frame_if_active(active: bool, window: &mut Window) -> bool {
    if active {
        window.request_animation_frame();
        true
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;

    #[test]
    fn bezier_curves_hit_endpoints_and_spring_overshoots() {
        assert!((Curve::Linear.sample(0.0)).abs() < 1e-4);
        assert!((Curve::Linear.sample(1.0) - 1.0).abs() < 1e-4);

        assert!((Curve::Spring.sample(0.0)).abs() < 1e-4);
        assert!((Curve::Spring.sample(1.0) - 1.0).abs() < 1e-4);
        let mid_spring = Curve::Spring.sample(0.5);
        assert!(
            mid_spring > 1.0,
            "spring curve should overshoot past 1.0 mid-flight, got {mid_spring}"
        );

        let mid_exit = Curve::Exit.sample(0.5);
        assert!(
            mid_exit < 0.5,
            "exit curve should accelerate late, got {mid_exit}"
        );
    }

    #[test]
    fn shake_decay_oscillates_five_times_and_settles_to_zero() {
        assert_eq!(shake_decay(0.0), 0.0);
        assert_eq!(shake_decay(1.0), 0.0);

        let early_peak = shake_decay(0.05).abs();
        let late_peak = shake_decay(0.85).abs();
        assert!(
            early_peak > 7.0,
            "expected strong initial shake, got {early_peak}"
        );
        assert!(
            late_peak < early_peak,
            "shake amplitude should decay over time"
        );

        // 5 full cycles means 10 zero crossings + endpoints. Verify opposite signs inside cycle 1.
        assert!(shake_decay(0.05) > 0.0);
        assert!(shake_decay(0.15) < 0.0);
    }

    #[test]
    fn interruption_redirects_smoothly_from_current_value() {
        let t0 = Instant::now();
        let mut tracker = MotionTracker::new();
        let v0 = tracker.step_scalar(
            "hover",
            0.0,
            1.0,
            Duration::from_millis(160),
            Curve::Linear,
            MotionKind::OpacityOrColor,
            t0,
            false,
        );
        assert!((v0 - 0.0).abs() < 1e-4);

        let mid = t0 + Duration::from_millis(80);
        let v_mid = tracker.step_scalar(
            "hover",
            0.0,
            0.0,
            Duration::from_millis(160),
            Curve::Linear,
            MotionKind::OpacityOrColor,
            mid,
            false,
        );
        assert!(
            (v_mid - 0.5).abs() < 0.05,
            "interruption at 50% should start reverse transition from ~0.5, got {v_mid}"
        );

        let end = mid + Duration::from_millis(160);
        let v_end = tracker.scalar_or(&"hover", -1.0, end, false);
        assert!((v_end - 0.0).abs() < 1e-4);
    }

    #[test]
    fn reduced_motion_removes_movement_but_keeps_instant_opacity() {
        let t0 = Instant::now();
        let mut tracker = MotionTracker::new();

        // Movement snaps immediately under reduced motion.
        let moved = tracker.step_scalar(
            "slide",
            36.0,
            0.0,
            SETTLE,
            Curve::Spring,
            MotionKind::MovementOrScale,
            t0,
            true,
        );
        assert!((moved - 0.0).abs() < 1e-4);
        assert!(!tracker.has_active(t0, true));

        // Opacity continues to animate, clamped to INSTANT (160ms).
        let fade = tracker.step_scalar(
            "fade",
            0.0,
            1.0,
            SETTLE,
            Curve::EaseOut,
            MotionKind::OpacityOrColor,
            t0,
            true,
        );
        assert!((fade - 0.0).abs() < 1e-4);
        assert!(tracker.has_active(t0 + Duration::from_millis(80), true));
        assert!(!tracker.has_active(t0 + INSTANT, true));
    }

    #[test]
    fn color_lerp_interpolates_rgba_channels() {
        let black: Hsla = rgb(0x000000).into();
        let white: Hsla = rgb(0xffffff).into();
        let mid: Rgba = color_lerp(black, white, 0.5).into();
        assert!((mid.r - 0.5).abs() < 0.02);
        assert!((mid.g - 0.5).abs() < 0.02);
        assert!((mid.b - 0.5).abs() < 0.02);
    }

    #[test]
    fn resolve_reduced_motion_honors_preference() {
        assert!(!resolve_reduced_motion(MotionPreference::System, false));
        assert!(resolve_reduced_motion(MotionPreference::System, true));
        assert!(!resolve_reduced_motion(MotionPreference::Full, true));
        assert!(resolve_reduced_motion(MotionPreference::Reduced, false));
    }
}
