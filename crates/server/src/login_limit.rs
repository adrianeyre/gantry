//! Failed logins per client (PRD §9.2): a client that fails too often in a
//! minute waits, whichever accounts it tries. Only failures count, so a
//! household behind one tunnel address can log in as often as it likes; a
//! client guessing passwords is what gets slowed. Per-account slowing and lockout
//! live with the accounts, in SQLite, so they survive a restart; this part is
//! in memory, because a restart already slows an attacker down.
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::store::users::{LoginCheck, LoginLimits};

const WINDOW: Duration = Duration::from_secs(60);

/// Above this many clients, the ones idle for a whole window are forgotten.
const PRUNE_ABOVE: usize = 4096;

#[derive(Debug, Default)]
pub struct ClientLimiter {
    attempts: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl ClientLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `client` may try to log in: not if it has failed `per_minute`
    /// times in the last minute, in which case how long until it may.
    pub fn check(&self, client: &str, per_minute: u32, now: Instant) -> Result<(), Duration> {
        let mut attempts = self.attempts.lock().expect("limiter lock");
        let Some(times) = attempts.get_mut(client) else {
            return Ok(());
        };
        while times.front().is_some_and(|t| now - *t >= WINDOW) {
            times.pop_front();
        }
        if times.len() >= per_minute as usize {
            let oldest = *times.front().expect("at least one failure");
            return Err(WINDOW - (now - oldest));
        }
        Ok(())
    }

    /// Records a failed login by `client`.
    pub fn failed(&self, client: &str, now: Instant) {
        let mut attempts = self.attempts.lock().expect("limiter lock");
        if attempts.len() > PRUNE_ABOVE {
            attempts.retain(|_, times| times.back().is_some_and(|t| now - *t < WINDOW));
        }
        attempts
            .entry(client.to_owned())
            .or_default()
            .push_back(now);
    }
}

/// Failed logins for names that aren't accounts, under the same rules as
/// real accounts (slowed, then locked), so the replies never reveal which
/// names exist. Kept in memory: nothing about a non-account needs to last.
#[derive(Debug, Default)]
pub struct Ghosts {
    names: Mutex<HashMap<String, Ghost>>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Ghost {
    failures: u32,
    last: Option<time::OffsetDateTime>,
    locked_until: Option<time::OffsetDateTime>,
}

impl Ghosts {
    pub fn new() -> Self {
        Self::default()
    }

    /// A failed login for `name`, which isn't an account: refused as a real
    /// account in the same state would be.
    pub fn failed(&self, name: &str, limits: &LoginLimits, at: time::OffsetDateTime) -> LoginCheck {
        let mut names = self.names.lock().expect("ghosts lock");
        if names.len() > PRUNE_ABOVE {
            names.retain(|_, g| {
                g.locked_until.is_some_and(|u| u > at)
                    || g.last.is_some_and(|l| at - l < time::Duration::hours(1))
            });
        }
        let ghost = names.entry(name.trim().to_lowercase()).or_default();
        if let Some(until) = ghost.locked_until
            && until > at
        {
            return LoginCheck::Locked {
                account: None,
                until,
            };
        }
        if let Some(last) = ghost.last {
            let ready = last + limits.wait_after(ghost.failures);
            if ready > at {
                return LoginCheck::TooSoon {
                    account: None,
                    wait: (ready - at).unsigned_abs(),
                };
            }
        }
        ghost.failures += 1;
        if ghost.failures >= limits.max_failures.max(1) {
            *ghost = Ghost {
                locked_until: Some(at + limits.lockout),
                ..Ghost::default()
            };
        } else {
            ghost.last = Some(at);
        }
        LoginCheck::Wrong {
            account: None,
            locked: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_client_waits_after_too_many_failures_in_a_minute() {
        let limiter = ClientLimiter::new();
        let start = Instant::now();
        for i in 0..3 {
            let at = start + Duration::from_secs(i);
            assert!(limiter.check("a", 3, at).is_ok());
            limiter.failed("a", at);
        }
        let wait = limiter
            .check("a", 3, start + Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(wait, Duration::from_secs(50));
        // Another client is unaffected.
        assert!(limiter.check("b", 3, start).is_ok());
        // A minute after the first failure, one more try is allowed.
        assert!(limiter.check("a", 3, start + WINDOW).is_ok());
    }

    #[test]
    fn successes_never_count() {
        let limiter = ClientLimiter::new();
        let now = Instant::now();
        for _ in 0..100 {
            assert!(limiter.check("a", 3, now).is_ok());
        }
    }
}
