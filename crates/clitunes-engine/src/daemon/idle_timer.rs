//! Idle-exit timer for clitunesd.
//!
//! The daemon tracks the number of connected control-socket clients. When
//! that count drops to zero, a countdown begins; if it elapses with no new
//! client, the daemon exits cleanly. A new connection inside the window
//! cancels the countdown.
//!
//! The wire-up between the control socket accept loop and this timer is
//! Unit 10's problem — this module just exposes a pure state machine with
//! an injectable clock so we can unit-test it deterministically and so the
//! future async wire-up can be a thin adapter.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Default idle window before the daemon exits. Chosen to be long enough
/// that a user who `clitunes`-hops between playlists doesn't kill the
/// daemon out from under themselves, and short enough that a forgotten
/// daemon frees its audio device within a minute.
pub const DEFAULT_IDLE_WINDOW: Duration = Duration::from_secs(30);

/// Clock abstraction so tests can drive the timer without sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Real wall clock.
#[derive(Default, Debug, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Outcome of a tick on the idle timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// At least one client is connected; daemon should keep running.
    Busy,
    /// No clients, but the idle window has not yet elapsed. Caller
    /// should tick again after a short sleep.
    Idle { remaining: Duration },
    /// The idle window has elapsed with no new client. Caller should
    /// start graceful shutdown.
    Expired,
}

/// Idle-state snapshot that `IdleTimer::tick` returns unwrapped for
/// callers that don't care about remaining time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleState {
    Busy,
    Waiting,
    ShouldExit,
}

impl From<Tick> for IdleState {
    fn from(t: Tick) -> Self {
        match t {
            Tick::Busy => IdleState::Busy,
            Tick::Idle { .. } => IdleState::Waiting,
            Tick::Expired => IdleState::ShouldExit,
        }
    }
}

/// Shared state of the idle timer. Interior-mutable so the socket
/// accept loop (which owns an `Arc<IdleTimer>`) can increment/decrement
/// from any task without locking-gymnastics at the call site.
pub struct IdleTimer<C: Clock = SystemClock> {
    client_count: AtomicU64,
    idle_since: Mutex<Option<Instant>>,
    window: Duration,
    clock: C,
}

impl IdleTimer<SystemClock> {
    /// New timer with the default idle window and the real wall clock.
    pub fn new() -> Self {
        Self::with_window(DEFAULT_IDLE_WINDOW)
    }

    /// New timer with a custom idle window.
    pub fn with_window(window: Duration) -> Self {
        Self::with_clock(window, SystemClock)
    }
}

impl Default for IdleTimer<SystemClock> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clock> IdleTimer<C> {
    /// New timer with a custom idle window and clock. The timer starts
    /// in the idle state so a daemon that never sees a client still
    /// exits after `window`.
    pub fn with_clock(window: Duration, clock: C) -> Self {
        let now = clock.now();
        Self {
            client_count: AtomicU64::new(0),
            idle_since: Mutex::new(Some(now)),
            window,
            clock,
        }
    }

    /// Record a new client connection. Cancels the idle countdown.
    pub fn on_client_connected(&self) {
        self.client_count.fetch_add(1, Ordering::SeqCst);
        *self.idle_since.lock().unwrap() = None;
    }

    /// Record a client disconnection. If the count drops to zero the
    /// countdown begins at `now`.
    pub fn on_client_disconnected(&self) {
        let prev = self.client_count.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(
            prev > 0,
            "disconnect with zero clients is a programming bug"
        );
        if prev == 1 {
            *self.idle_since.lock().unwrap() = Some(self.clock.now());
        }
    }

    /// Current number of connected clients.
    pub fn client_count(&self) -> u64 {
        self.client_count.load(Ordering::SeqCst)
    }

    /// Inspect the timer against the current clock.
    pub fn tick(&self) -> Tick {
        if self.client_count.load(Ordering::SeqCst) > 0 {
            return Tick::Busy;
        }
        let idle_since = *self.idle_since.lock().unwrap();
        let Some(start) = idle_since else {
            // Defensive: client count is 0 but nobody marked idle_since.
            // This can happen if a future caller forgets to seed
            // `idle_since` on init; fall back to "starting now".
            return Tick::Idle {
                remaining: self.window,
            };
        };
        let elapsed = self.clock.now().saturating_duration_since(start);
        if elapsed >= self.window {
            Tick::Expired
        } else {
            Tick::Idle {
                remaining: self.window - elapsed,
            }
        }
    }

    /// Convenience wrapper for callers that only care about the state.
    pub fn state(&self) -> IdleState {
        self.tick().into()
    }

    /// Idle window (test introspection).
    pub fn window(&self) -> Duration {
        self.window
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Test clock: moves only when the test advances it.
    struct FakeClock {
        start: Instant,
        offset: Mutex<Duration>,
    }

    impl FakeClock {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                start: Instant::now(),
                offset: Mutex::new(Duration::ZERO),
            })
        }
        fn advance(&self, by: Duration) {
            let mut g = self.offset.lock().unwrap();
            *g += by;
        }
    }

    impl Clock for Arc<FakeClock> {
        fn now(&self) -> Instant {
            self.start + *self.offset.lock().unwrap()
        }
    }

    #[test]
    fn starts_idle_and_expires_after_window() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        assert_eq!(
            timer.tick(),
            Tick::Idle {
                remaining: Duration::from_secs(30)
            }
        );
        clock.advance(Duration::from_secs(15));
        assert_eq!(
            timer.tick(),
            Tick::Idle {
                remaining: Duration::from_secs(15)
            }
        );
        clock.advance(Duration::from_secs(15));
        assert_eq!(timer.tick(), Tick::Expired);
    }

    #[test]
    fn busy_while_any_client_connected() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        timer.on_client_connected();
        clock.advance(Duration::from_secs(90));
        assert_eq!(timer.tick(), Tick::Busy);
        assert_eq!(timer.client_count(), 1);
    }

    #[test]
    fn disconnect_starts_countdown() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        timer.on_client_connected();
        clock.advance(Duration::from_secs(5));
        timer.on_client_disconnected();
        assert_eq!(timer.client_count(), 0);
        assert_eq!(
            timer.tick(),
            Tick::Idle {
                remaining: Duration::from_secs(30)
            },
            "countdown should reset to full window at disconnect"
        );
        clock.advance(Duration::from_secs(30));
        assert_eq!(timer.tick(), Tick::Expired);
    }

    #[test]
    fn reconnect_cancels_countdown() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        timer.on_client_connected();
        timer.on_client_disconnected();
        clock.advance(Duration::from_secs(20));
        timer.on_client_connected();
        clock.advance(Duration::from_secs(100));
        assert_eq!(
            timer.tick(),
            Tick::Busy,
            "reconnect within window must cancel countdown"
        );
    }

    #[test]
    fn multiple_clients_tracked_independently() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        timer.on_client_connected();
        timer.on_client_connected();
        timer.on_client_connected();
        assert_eq!(timer.client_count(), 3);
        timer.on_client_disconnected();
        timer.on_client_disconnected();
        clock.advance(Duration::from_secs(100));
        assert_eq!(
            timer.tick(),
            Tick::Busy,
            "one remaining client keeps the timer busy"
        );
        timer.on_client_disconnected();
        assert_eq!(
            timer.tick(),
            Tick::Idle {
                remaining: Duration::from_secs(30)
            }
        );
    }

    #[test]
    fn state_projection_matches_tick() {
        let clock = FakeClock::new();
        let timer = IdleTimer::with_clock(Duration::from_secs(30), clock.clone());
        assert_eq!(timer.state(), IdleState::Waiting);
        timer.on_client_connected();
        assert_eq!(timer.state(), IdleState::Busy);
        timer.on_client_disconnected();
        clock.advance(Duration::from_secs(31));
        assert_eq!(timer.state(), IdleState::ShouldExit);
    }
}
