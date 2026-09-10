//! Who may open the desktop: the two things an RSA-AES credential can be
//! checked against, and the refusal they share.
//!
//! RSA-AES carries a credential; what verifies it is the configuration's
//! choice. `[pam]` is the system login — the client names the account wlshare
//! runs as, and PAM checks that account's password ([`crate::pam`]).
//! `[password]` is one password of the operator's, checked against an Argon2
//! hash and naming no account at all, for a host where the desktop's user has
//! no system password to spend on a VNC client, or no PAM stack to spend it on.
//! The choice also decides what the client is asked for: a username and a
//! password, or a password alone.
//!
//! Neither is classic VncAuth, which is still not offered: the password here is
//! whatever length it is, it crosses an already-encrypted channel, and what the
//! host keeps is a hash and not the password itself.
//!
//! Both checks block — a PAM stack may sleep or run programs, and Argon2 is
//! slow on purpose — so both run off the runtime.

use argon2::{Argon2, PasswordHash, PasswordHasher as _, PasswordVerifier as _};
use wlshare_rfb::rsa_aes::{Credentials, Subtype};

use crate::pam;

/// Why a login was refused. The text is PAM's own, or this crate's, and is for
/// the log — the client is told only that the login failed.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(String);

impl Refused {
    pub fn new(reason: impl Into<String>) -> Self {
        Self(reason.into())
    }
}

/// What a client's credentials are checked against, one table or the other.
#[derive(Clone)]
pub enum Login {
    /// `[pam]`: the system password of `account`, the user wlshare runs as,
    /// under the PAM service `service`.
    Pam { service: String, account: String },
    /// `[password]`: one configured password, held as the Argon2 PHC string it
    /// is stored as and never as the password.
    Password { hash: String },
}

impl Login {
    /// The credentials to ask the client for: PAM needs an account name beside
    /// the password, and a configured password names nobody.
    pub fn subtype(&self) -> Subtype {
        match self {
            Self::Pam { .. } => Subtype::UserPass,
            Self::Password { .. } => Subtype::Password,
        }
    }

    /// Check what the client sent, `peer` being where it sent it from. Blocks
    /// for as long as the check takes, so call it off the runtime.
    pub fn check(&self, credentials: &Credentials, peer: &str) -> Result<(), Refused> {
        match self {
            Self::Pam { service, account } => pam::check(service, account, &credentials.username, &credentials.password, peer),
            Self::Password { hash } => verify(hash, &credentials.password),
        }
    }
}

/// Check a password against the stored hash, in the parameters the hash itself
/// names. The empty password is refused before the hash is asked: it is what a
/// client with nothing to say sends, and no configuration should let it in.
fn verify(hash: &str, password: &str) -> Result<(), Refused> {
    if password.is_empty() {
        return Err(Refused::new("the client sent an empty password"));
    }
    Argon2::default()
        .verify_password(password.as_bytes(), hash)
        .map_err(|e| Refused::new(format!("the password does not match the configured hash: {e}")))
}

/// Hash a password as `[password]` wants it: Argon2id in the crate's default
/// parameters, a fresh random salt, and the PHC string that carries both.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!password.is_empty(), "the password is empty");
    let hash = Argon2::default().hash_password(password.as_bytes()).map_err(|e| anyhow::anyhow!("hashing the password: {e}"))?;
    Ok(hash.to_string())
}

/// Read a configured hash, so one wlshare cannot use is an error at startup and
/// not at the first client to try it.
pub fn check_hash(hash: &str) -> anyhow::Result<()> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("the hash is not a PHC string: {e}"))?;
    anyhow::ensure!(
        parsed.algorithm.as_str().starts_with("argon2"),
        "the hash is {}, and wlshare verifies Argon2 only",
        parsed.algorithm
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_verifies_its_own_password_and_no_other() {
        let hash = hash_password("hunter2").unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        check_hash(&hash).unwrap();
        verify(&hash, "hunter2").unwrap();
        assert!(verify(&hash, "hunter3").is_err());
        assert!(verify(&hash, "").is_err());
        // Two hashes of one password differ: the salt is fresh each time.
        assert_ne!(hash, hash_password("hunter2").unwrap());
    }

    #[test]
    fn a_hash_that_is_not_argon2_is_refused_at_startup() {
        assert!(hash_password("").is_err());
        assert!(check_hash("hunter2").is_err());
        assert!(check_hash("$2b$12$K3JNi5xUNaXNaXNaXNaXNuJ3nQ2mDe2hAaXQ1oX8fJz4T6bA0aXNa").is_err());
    }

    #[test]
    fn the_table_decides_which_credentials_the_client_is_asked_for() {
        let pam = Login::Pam { service: "wlshare".into(), account: "me".into() };
        assert_eq!(pam.subtype(), Subtype::UserPass);
        assert_eq!(Login::Password { hash: String::new() }.subtype(), Subtype::Password);
    }
}
