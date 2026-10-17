// SPDX-License-Identifier: AGPL-3.0-or-later

//! Animation: values that move to a target over a short time, with easing,
//! and the reduced-motion switch that makes every one of them instant
//! (`docs/design/ui-toolkit.md`, "Animation").
//!
//! An animating widget keeps a [`Transition`], reads it when it paints,
//! and asks to be woken for the next frame ([`Transition::wake_at`]); each
//! wake calls [`Transition::advance`] and repaints. Once the frame that
//! shows the target was asked for, nothing redraws. Times come from [`now`], which a
//! test can set ([`set_clock_for_tests`]).

use std::cell::Cell;
use std::time::{Duration, Instant};

/// How long a frame lasts while something animates (120 Hz; a 60 Hz
/// display presents every other wake).
pub const FRAME: Duration = Duration::from_micros(8_333);

/// The interface's usual durations.
pub mod durations {
    use std::time::Duration;

    /// A hover or press color.
    pub const HOVER: Duration = Duration::from_millis(100);
    /// A popover or menu appearing.
    pub const POPOVER: Duration = Duration::from_millis(120);
    /// A dialog and its scrim appearing.
    pub const DIALOG: Duration = Duration::from_millis(160);
    /// A scroll by keyboard (a page, a row brought into view).
    pub const SCROLL: Duration = Duration::from_millis(160);
}

/// How a value accelerates between its start and its target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    /// The same speed throughout.
    Linear,
    /// Fast, then slowing down: things arriving (most of the interface).
    EaseOut,
    /// Slow, fast, slow: things moving from one place to another.
    EaseInOut,
}

impl Easing {
    /// The eased progress for linear progress `t` (0 to 1).
    pub fn apply(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseOut => 1.0 - (1.0 - t).powi(3),
            Easing::EaseInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
                }
            }
        }
    }
}

thread_local! {
    static REDUCE_MOTION: Cell<bool> = const { Cell::new(false) };
    static CLOCK: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Whether motion is reduced: every transition then shows its target,
/// including those already moving. The app will set it from the setting
/// `ui.reduce_motion` (following the system by default; `shell.md` §5) in
/// M3.7; until then it is off unless code turns it on.
pub fn reduce_motion() -> bool {
    REDUCE_MOTION.with(Cell::get)
}

/// Turns reduced motion on or off.
pub fn set_reduce_motion(reduce: bool) {
    REDUCE_MOTION.with(|cell| cell.set(reduce));
    crate::window::request_frame_everywhere();
}

/// The time animations use: the real clock, or the test's.
pub fn now() -> Instant {
    CLOCK.with(Cell::get).unwrap_or_else(Instant::now)
}

/// Fixes [`now`] on this thread, for tests that step through a
/// transition (`None`: the real clock again).
pub fn set_clock_for_tests(time: Option<Instant>) {
    CLOCK.with(|cell| cell.set(time));
}

/// A number moving from where it was to a target.
///
/// ```
/// use std::time::Duration;
/// use imaginarium_ui::animation::{Easing, Transition, now};
///
/// let mut opacity = Transition::new(0.0);
/// opacity.to(1.0, Duration::from_millis(120), Easing::EaseOut);
/// let halfway = opacity.value_at(now() + Duration::from_millis(60));
/// assert!(halfway > 0.5 && halfway < 1.0);
/// assert_eq!(opacity.target(), 1.0);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    from: f64,
    to: f64,
    start: Instant,
    duration: Duration,
    easing: Easing,
    /// Moving, or arrived without the frame that shows the target yet.
    pending: bool,
    /// When the next frame is due. Fixed between wakes: the window asks
    /// for it after reading its clock, so a deadline counted from the
    /// moment of asking would never be due.
    next: Instant,
}

impl Transition {
    /// A value at rest.
    pub fn new(value: f64) -> Self {
        let time = now();
        Self {
            from: value,
            to: value,
            start: time,
            duration: Duration::ZERO,
            easing: Easing::Linear,
            pending: false,
            next: time,
        }
    }

    /// Moves to `target` over `duration`, from wherever it is now (a
    /// change of mind mid-way doesn't jump). Instant with reduced motion.
    pub fn to(&mut self, target: f64, duration: Duration, easing: Easing) {
        if target == self.to {
            return;
        }
        let time = now();
        self.from = self.value_at(time);
        self.to = target;
        self.start = time;
        self.duration = if reduce_motion() {
            Duration::ZERO
        } else {
            duration
        };
        self.easing = easing;
        self.pending = !self.duration.is_zero();
        self.next = (time + FRAME).min(time + self.duration);
    }

    /// Jumps to `value` at once.
    pub fn set(&mut self, value: f64) {
        *self = Self::new(value);
    }

    /// Where it is heading.
    pub fn target(&self) -> f64 {
        self.to
    }

    /// Its value now.
    pub fn value(&self) -> f64 {
        self.value_at(now())
    }

    /// Its value at `time`.
    pub fn value_at(&self, time: Instant) -> f64 {
        if self.duration.is_zero() || reduce_motion() {
            return self.to;
        }
        let t =
            time.saturating_duration_since(self.start).as_secs_f64() / self.duration.as_secs_f64();
        if t >= 1.0 {
            self.to
        } else {
            self.from + (self.to - self.from) * self.easing.apply(t)
        }
    }

    /// Whether it is still moving.
    pub fn is_moving(&self) -> bool {
        !self.duration.is_zero() && !reduce_motion() && now() < self.start + self.duration
    }

    /// When to wake for the next frame (`None`: at rest). After it
    /// arrives it still wants one wake, for the frame that shows the target
    /// exactly, however late the last one came.
    pub fn wake_at(&self) -> Option<Instant> {
        self.pending.then_some(self.next)
    }

    /// Called on each wake: once it has arrived, it stops asking for them.
    /// Returns whether it is still pending (a frame follows).
    pub fn advance(&mut self) -> bool {
        if self.is_moving() {
            let time = now();
            self.next = (time + FRAME).min(self.start + self.duration);
        } else {
            self.pending = false;
        }
        self.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easings_start_at_zero_and_end_at_one() {
        for easing in [Easing::Linear, Easing::EaseOut, Easing::EaseInOut] {
            assert_eq!(easing.apply(0.0), 0.0);
            assert_eq!(easing.apply(1.0), 1.0);
            assert!(easing.apply(0.5) > 0.0 && easing.apply(0.5) < 1.0);
        }
        assert!(Easing::EaseOut.apply(0.5) > 0.5, "ease out is ahead early");
    }

    #[test]
    fn a_transition_moves_and_arrives() {
        let start = Instant::now();
        set_clock_for_tests(Some(start));
        let mut value = Transition::new(0.0);
        value.to(10.0, Duration::from_millis(100), Easing::Linear);
        assert!(value.is_moving());
        assert_eq!(value.value_at(start + Duration::from_millis(50)), 5.0);
        set_clock_for_tests(Some(start + Duration::from_millis(50)));
        // A change of mind mid-way starts from where it is.
        value.to(0.0, Duration::from_millis(100), Easing::Linear);
        assert_eq!(value.value(), 5.0);
        set_clock_for_tests(Some(start + Duration::from_millis(200)));
        assert_eq!(value.value(), 0.0);
        assert!(!value.is_moving());
        // One more wake draws the target, however late it comes.
        assert!(
            value
                .wake_at()
                .is_some_and(|at| at <= start + Duration::from_millis(200))
        );
        assert!(!value.advance());
        assert_eq!(value.wake_at(), None, "at rest, nothing redraws");
        set_clock_for_tests(None);
    }

    #[test]
    fn each_frame_is_due_when_the_window_checks() {
        // The window reads its clock, then asks when to wake: the deadline
        // must not move with the moment of asking.
        let start = Instant::now();
        set_clock_for_tests(Some(start));
        let mut value = Transition::new(0.0);
        value.to(1.0, Duration::from_millis(120), Easing::Linear);
        let mut wakes = 0;
        let mut time = start;
        while let Some(at) = value.wake_at() {
            assert!(
                at > time || wakes == 0 || at == time,
                "deadlines move forward"
            );
            time = at;
            set_clock_for_tests(Some(time));
            value.advance();
            wakes += 1;
            assert!(wakes < 100, "never settles");
        }
        // 120 ms at 8.3 ms a frame, the last one at the arrival.
        assert_eq!(wakes, 15);
        assert_eq!(time, start + Duration::from_millis(120));
        assert_eq!(value.value(), 1.0);
        set_clock_for_tests(None);
    }

    #[test]
    fn reduced_motion_makes_transitions_instant() {
        set_reduce_motion(true);
        let mut value = Transition::new(0.0);
        value.to(1.0, Duration::from_millis(500), Easing::EaseOut);
        assert_eq!(value.value(), 1.0);
        assert!(!value.is_moving());
        assert_eq!(value.wake_at(), None);
        set_reduce_motion(false);
        // One already moving shows its target once motion is reduced.
        value.to(0.0, Duration::from_millis(500), Easing::EaseOut);
        assert!(value.is_moving());
        set_reduce_motion(true);
        assert_eq!(value.value(), 0.0);
        assert!(!value.advance(), "and stops asking for frames");
        set_reduce_motion(false);
    }
}
