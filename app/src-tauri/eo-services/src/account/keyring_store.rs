//! The signed-in session kept in the operating system's keyring between
//! launches: the Windows Credential Manager, or the Secret Service of a
//! Linux desktop (GNOME Keyring, KWallet). Never a plain file.
//!
//! Each build keeps its own entry, so a development build running beside
//! the installed app never takes up, replaces or forgets the installed
//! app's session. Where no keyring answers (no Secret Service running),
//! nothing is kept: the store logs why once per attempt and the readings
//! wait on a sign-in, as they would with [`super::MemoryOnly`].

use super::adoption::SessionStore;
use super::site::SessionSecret;

/// The service every entry is filed under.
const SERVICE: &str = "com.entropiaorme.app";

/// The entry a build keeps its session under: a development build's is
/// its own.
fn entry_name(development: bool) -> &'static str {
    if development {
        "account-site-session-development"
    } else {
        "account-site-session"
    }
}

/// The session kept in the operating system's keyring under one entry.
pub struct KeyringStore {
    user: &'static str,
}

impl KeyringStore {
    /// The store for this build: the installed app's entry, or the
    /// development build's own.
    pub fn for_this_build() -> KeyringStore {
        KeyringStore {
            user: entry_name(cfg!(debug_assertions)),
        }
    }

    fn entry(&self) -> Option<keyring::Entry> {
        match keyring::Entry::new(SERVICE, self.user) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(target: "eo::account", %error, "the keyring could not be opened; the sign-in is not kept");
                None
            }
        }
    }
}

impl SessionStore for KeyringStore {
    fn load(&self) -> Option<SessionSecret> {
        let entry = self.entry()?;
        match entry.get_password() {
            Ok(value) => match SessionSecret::new(value) {
                Ok(session) => Some(session),
                Err(refused) => {
                    tracing::warn!(target: "eo::account", %refused, "the kept sign-in is not a session value; it is forgotten");
                    let _ = entry.delete_credential();
                    None
                }
            },
            Err(keyring::Error::NoEntry) => None,
            Err(error) => {
                tracing::warn!(target: "eo::account", %error, "the kept sign-in could not be read from the keyring");
                None
            }
        }
    }

    fn save(&self, session: &SessionSecret) {
        let Some(entry) = self.entry() else {
            return;
        };
        if let Err(error) = entry.set_password(session.expose()) {
            tracing::warn!(target: "eo::account", %error, "the sign-in could not be kept in the keyring; the next launch asks for a sign-in");
        }
    }

    fn forget(&self) {
        let Some(entry) = self.entry() else {
            return;
        };
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(error) => {
                tracing::warn!(target: "eo::account", %error, "the kept sign-in could not be removed from the keyring");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::entry_name;

    #[test]
    fn a_development_build_never_shares_the_installed_apps_entry() {
        assert_ne!(entry_name(true), entry_name(false));
    }
}
