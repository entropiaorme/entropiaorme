//! Taking up the stored session when the app starts.
//!
//! For a player who connected before, the session is read from where it
//! is kept (through [`SessionStore`]) and handed to the connection, which
//! is unconfirmed until the site answers. Accepted, the connection is
//! made; refused, it is expired. With nothing stored the connection stays
//! disconnected and nothing is tried. When the site cannot be reached the
//! connection stays unconfirmed, and the stored session is read and tried
//! again after 30 seconds, then 2 minutes, then every 10 minutes, and at
//! once whenever a tracked session starts. Trying ends on a connection, a
//! refusal, a disconnect, or the app closing.
//!
//! The app keeps the session in memory only, so its store today holds
//! nothing ([`MemoryOnly`]); a store that keeps it between runs (an
//! operating-system keychain) slots in behind the same seam.

use std::sync::Arc;
use std::time::Duration;

use super::recorder::{AccountRecorder, ConnectOutcome, ConnectionState, ConnectionStatus};
use super::site::SessionSecret;

/// Where the signed-in session is kept between launches. The connection
/// never reaches it: only this taking-up and the shell do.
pub trait SessionStore: Send + Sync + 'static {
    /// The stored session, if there is one. May block.
    fn load(&self) -> Option<SessionSecret>;
    /// Forget the stored session. May block.
    fn forget(&self);
}

/// The store of a session held in memory only: it keeps nothing between
/// runs, so there is never a session to take up.
pub struct MemoryOnly;

impl SessionStore for MemoryOnly {
    fn load(&self) -> Option<SessionSecret> {
        None
    }

    fn forget(&self) {}
}

/// How long to wait before trying the stored session again, after
/// `tries` tries that found the site unreachable.
pub fn retry_after(tries: u32) -> Duration {
    match tries {
        0 | 1 => Duration::from_secs(30),
        2 => Duration::from_secs(2 * 60),
        _ => Duration::from_secs(10 * 60),
    }
}

/// Whether trying the stored session goes on: only while the connection
/// is still unconfirmed.
fn still_unconfirmed(status: &ConnectionStatus) -> bool {
    status.state == ConnectionState::Unconfirmed
}

/// Take up the stored session, retrying while the site cannot be reached
/// (see the module doc). Returns once there is nothing more to try.
pub async fn confirm_stored_session(recorder: Arc<AccountRecorder>, store: Arc<dyn SessionStore>) {
    let epoch = recorder.epoch();
    let mut status = recorder.watch_status();
    if !status.borrow().has_connected {
        return;
    }
    let mut tries = 0u32;
    loop {
        let tracked = status.borrow_and_update().tracked_session.clone();
        if tries > 0 && (!still_unconfirmed(&status.borrow()) || recorder.epoch() != epoch) {
            return;
        }
        let loading = store.clone();
        let Ok(loaded) = tokio::task::spawn_blocking(move || loading.load()).await else {
            return;
        };
        let Some(session) = loaded else {
            return;
        };
        if tries == 0 {
            recorder.confirming_stored_session();
        }
        match recorder.connect(epoch, session).await {
            ConnectOutcome::Connected | ConnectOutcome::NotAccepted => return,
            ConnectOutcome::Unreachable(_) | ConnectOutcome::Interrupted => {}
        }
        tries += 1;
        let wait = tokio::time::sleep(retry_after(tries));
        tokio::pin!(wait);
        let prompted = status.wait_for(|current| {
            !still_unconfirmed(current)
                || (current.tracked_session.is_some() && current.tracked_session != tracked)
        });
        tokio::select! {
            () = &mut wait => {}
            changed = prompted => {
                if changed.is_err() {
                    return;
                }
            }
        }
    }
}
