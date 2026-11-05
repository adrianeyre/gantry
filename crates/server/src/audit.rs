//! Writing the audit log from request handlers (PRD §9.2).
//!
//! Each state-changing handler writes exactly one entry, whatever the result,
//! through [`record`]. A failure to write is logged, never turned into a
//! failure of the action itself: the action has already happened.
use crate::AppState;
use crate::api::ApiError;
use crate::store::audit::{Action, Entry, Outcome};
use crate::store::users::User;

/// An entry for something `user` did.
pub fn by(user: &User, action: Action) -> Entry {
    let mut entry = Entry::new(action);
    entry.user_id = Some(user.id);
    entry.username = Some(user.username.clone());
    entry.api_token = user.api_token.clone();
    entry
}

/// Builder methods, so handlers read as one expression.
pub trait EntryExt: Sized {
    fn printer(self, id: &str) -> Self;
    fn target(self, target: impl Into<String>) -> Self;
    fn detail(self, detail: serde_json::Value) -> Self;
    /// Sets the outcome (and, if it went wrong, the message) from a result.
    fn result<T>(self, result: &Result<T, ApiError>) -> Self;
}

impl EntryExt for Entry {
    fn printer(mut self, id: &str) -> Self {
        self.printer_id = Some(id.to_owned());
        self
    }

    fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    fn detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    fn result<T>(mut self, result: &Result<T, ApiError>) -> Self {
        if let Err(e) = result {
            self.outcome = outcome(e);
            self.message = Some(e.message());
        }
        self
    }
}

/// How an API failure counts in the log.
pub fn outcome(error: &ApiError) -> Outcome {
    match error {
        ApiError::Internal(_) | ApiError::Command(gantry_core::CommandError::TimedOut) => {
            Outcome::Failed
        }
        _ => Outcome::Refused,
    }
}

/// Writes `entry`, filling in the Printer's name if only its id is set.
pub async fn record(state: &AppState, mut entry: Entry) {
    if let (Some(id), None) = (&entry.printer_id, &entry.printer_name) {
        entry.printer_name = match state.fleet().view(id) {
            Ok(view) => Some(view.name.clone()),
            Err(_) => state
                .store()
                .printer(state.key(), id)
                .await
                .ok()
                .flatten()
                .map(|p| p.name),
        };
    }
    if let Err(e) = state.store().audit(&entry).await {
        tracing::error!(error = %e, action = entry.action.as_str(), "could not write the audit log");
    }
}
