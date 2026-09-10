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

use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher as _, PasswordVerifier as _, Version};
use wlshare_rfb::rsa_aes::{Credentials, MAX_CREDENTIAL_LEN, Subtype};

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
///
/// A password RSA-AES cannot carry is refused here rather than hashed into a
/// configuration no client could ever satisfy.
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!password.is_empty(), "the password is empty");
    anyhow::ensure!(
        password.len() <= MAX_CREDENTIAL_LEN,
        "the password is {} bytes, and RSA-AES carries at most {MAX_CREDENTIAL_LEN}: no client could send it",
        password.len()
    );
    let hash = Argon2::default().hash_password(password.as_bytes()).map_err(|e| anyhow::anyhow!("hashing the password: {e}"))?;
    Ok(hash.to_string())
}

/// Read a configured hash, so one wlshare cannot use is an error at startup and
/// not at the first client to try it. A PHC string parses with most of itself
/// missing — `$argon2id` alone is a well-formed one — and a verification against
/// a header with no salt or no digest is indistinguishable from a wrong
/// password, so every piece [`verify`] will reach for is taken out here, in the
/// order it reaches for them.
pub fn check_hash(hash: &str) -> anyhow::Result<()> {
    let parsed = PasswordHash::new(hash).map_err(|e| anyhow::anyhow!("the hash is not a PHC string: {e}"))?;
    Algorithm::new(parsed.algorithm.as_str())
        .map_err(|e| anyhow::anyhow!("the hash names {}, and wlshare verifies Argon2 only: {e}", parsed.algorithm))?;
    if let Some(version) = parsed.version {
        Version::try_from(version).map_err(|e| anyhow::anyhow!("the hash's version {version} is not one Argon2 has: {e}"))?;
    }
    anyhow::ensure!(parsed.salt.is_some(), "the hash carries no salt");
    anyhow::ensure!(parsed.hash.is_some(), "the hash carries no digest: there would be nothing to compare a password against");
    Params::try_from(&parsed).map_err(|e| anyhow::anyhow!("the hash's parameters are not ones Argon2 takes: {e}"))?;
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
        assert!(hash_password(&"a".repeat(MAX_CREDENTIAL_LEN + 1)).is_err());
        assert!(hash_password(&"a".repeat(MAX_CREDENTIAL_LEN)).is_ok());
        assert!(check_hash("hunter2").is_err());
        assert!(check_hash("$2b$12$K3JNi5xUNaXNaXNaXNaXNuJ3nQ2mDe2hAaXQ1oX8fJz4T6bA0aXNa").is_err());
    }

    /// A PHC string is happy with far less than a verification needs, and each
    /// of these would otherwise start a wlshare that refuses every client.
    #[test]
    fn a_hash_missing_what_a_login_needs_is_refused_at_startup() {
        let hash = hash_password("hunter2").unwrap();
        let (head, _) = hash.rsplit_once('$').expect("a digest to cut off");
        check_hash(&hash).unwrap();

        // A header and nothing else, then one with a salt but no digest.
        assert!(check_hash("$argon2id").is_err());
        assert!(check_hash(head).is_err());
        // A name that only begins like Argon2's.
        assert!(check_hash(&hash.replacen("$argon2id$", "$argon2idfoo$", 1)).is_err());
        // Parameters Argon2 will not build from: t=0, and a name it has no use for.
        assert!(check_hash(&hash.replacen("t=2", "t=0", 1)).is_err());
        assert!(check_hash(&hash.replacen("t=2", "z=2", 1)).is_err());
        // A version Argon2 never had.
        assert!(check_hash(&hash.replacen("v=19", "v=7", 1)).is_err());
    }

    #[test]
    fn the_table_decides_which_credentials_the_client_is_asked_for() {
        let pam = Login::Pam { service: "wlshare".into(), account: "me".into() };
        assert_eq!(pam.subtype(), Subtype::UserPass);
        assert_eq!(Login::Password { hash: String::new() }.subtype(), Subtype::Password);
    }
}
