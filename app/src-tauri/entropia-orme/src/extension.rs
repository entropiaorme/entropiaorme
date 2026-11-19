//! The shell's extension point: the hooks the shell calls at its lifecycle
//! seams, and (in `extension_commands.rs`) the commands an extension adds.
//! Each hook's default leaves the shell as it is.

use eo_services::account::RecorderParts;

use crate::composition::{Composed, GameWindowSeams};

/// Compose an extension over the native services once they are installed.
/// The default: nothing to compose.
pub fn install(_app: &tauri::AppHandle, _composed: &Composed, _game_window: GameWindowSeams) {}

/// Wind an extension down, ahead of the producer spine it sits on. The
/// default: nothing to wind down.
pub fn exit_teardown(_app: &tauri::AppHandle) {}

/// A step that runs before the native services are composed. The default:
/// nothing to do.
pub fn before_compose() {}

/// The guard every command passes through before it is dispatched. The
/// default: every command runs.
pub fn command_guard<R: tauri::Runtime>(
    inner: impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static,
) -> impl Fn(tauri::ipc::Invoke<R>) -> bool + Send + Sync + 'static {
    inner
}

/// What the account connection is built from, as an extension may adjust
/// it. The default: unchanged.
pub fn account_parts(parts: RecorderParts) -> RecorderParts {
    parts
}
