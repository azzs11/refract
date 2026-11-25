//! Resource budgets. Every analysis takes a [`Budget`] and stops cleanly with an
//! [`ErrorKind::Budget`](crate::error::ErrorKind::Budget) error naming the limit it hit.

use crate::error::{Error, Result};
use std::time::Duration;
use web_time::Instant;

/// Limits for one analysis run.
#[derive(Clone, Debug)]
pub struct Budget {
    /// Maximum DFA states built by any single construction.
    pub max_dfa_states: usize,
    /// Maximum NFA states (Thompson construction).
    pub max_nfa_states: usize,
    /// Maximum distinct derivative terms interned.
    pub max_terms: usize,
    /// Maximum size of a counted repetition after expansion (`a{1000}{1000}` is 10^6).
    pub max_repeat: u32,
    /// Wall-clock limit for the whole analysis, measured from [`Budget::start`].
    pub time_limit: Option<Duration>,
    started: Option<Instant>,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_dfa_states: 20_000,
            max_nfa_states: 200_000,
            max_terms: 200_000,
            max_repeat: 1_000,
            time_limit: Some(Duration::from_secs(10)),
            started: None,
        }
    }
}

impl Budget {
    /// A small budget for property tests and fuzzing.
    pub fn small() -> Self {
        Budget {
            max_dfa_states: 2_000,
            max_nfa_states: 20_000,
            max_terms: 20_000,
            max_repeat: 100,
            time_limit: Some(Duration::from_secs(2)),
            started: None,
        }
    }

    /// Starts the clock. Calling it again restarts the clock.
    pub fn start(mut self) -> Self {
        self.started = Some(Instant::now());
        self
    }

    /// Fails if the time limit has passed.
    pub fn check_time(&self) -> Result<()> {
        if let (Some(limit), Some(started)) = (self.time_limit, self.started)
            && started.elapsed() > limit
        {
            return Err(Error::budget(format!(
                "time limit of {} ms exceeded",
                limit.as_millis()
            )));
        }
        Ok(())
    }

    /// Fails when `used` exceeds `limit`, naming `what` in the message.
    pub fn check(&self, what: &str, used: usize, limit: usize) -> Result<()> {
        if used > limit {
            return Err(Error::budget(format!(
                "{what} exceeded the limit of {limit} (raise it with --max-states or split the regex)"
            )));
        }
        Ok(())
    }
}
