//! Checking an RSA-AES login against the system: PAM, and the one account it
//! may name.
//!
//! swayrx runs as one user and injects input into that user's desktop, so the
//! only account whose password may open it is that user's — any other account's
//! password would open somebody else's session. The check is here, before PAM is
//! asked anything, and not left to the PAM stack.
//!
//! What PAM is asked is `pam_authenticate` and `pam_acct_mgmt`, under the
//! service name from the configuration (`/etc/pam.d/swayrx` by default): is the
//! password right, and may the account log in. No session is opened and no
//! credentials are set, so a module that does its work in the session phase
//! never runs here; a stack that wants the verified password for something —
//! unlocking a keyring — takes it from the auth phase with `pam_exec
//! expose_authtok`. Three libpam calls and one callback, bound here directly:
//! the crates that wrap them bring bindgen or an unmaintained `users` behind
//! them, for an interface this small.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

use anyhow::Context as _;
use log::{debug, warn};

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

type ConvFn = unsafe extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int;

#[repr(C)]
struct PamConv {
    conv: Option<ConvFn>,
    appdata_ptr: *mut c_void,
}

type PamHandle = c_void;

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(service: *const c_char, user: *const c_char, conv: *const PamConv, pamh: *mut *mut PamHandle) -> c_int;
    fn pam_set_item(pamh: *mut PamHandle, item_type: c_int, item: *const c_void) -> c_int;
    fn pam_authenticate(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(pamh: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_end(pamh: *mut PamHandle, status: c_int) -> c_int;
    fn pam_strerror(pamh: *mut PamHandle, errnum: c_int) -> *const c_char;
}

const PAM_SUCCESS: c_int = 0;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;
/// `PAM_RHOST`: where the login comes from, for the stack's logs.
const PAM_RHOST: c_int = 4;
/// An account with no password does not get in on that account.
const PAM_DISALLOW_NULL_AUTHTOK: c_int = 0x1;

const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

/// What the conversation hands back: the password for a hidden prompt, the
/// username for a visible one.
struct Answers {
    username: CString,
    password: CString,
}

/// Why a login was refused. The text is PAM's own, or this module's for the
/// account check, and is for the log — the client is told only that the login
/// failed.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Refused(String);

/// The account the process runs as, which is the one account a login may name.
pub fn process_user() -> anyhow::Result<String> {
    // SAFETY: getpwuid_r writes the passwd record into `pwd` and its strings
    // into `buf`, both of which outlive the call; `result` is set to `pwd` or
    // null. The buffer is sized as sysconf suggests, retried when too small.
    unsafe {
        let uid = libc::getuid();
        let mut size = match libc::sysconf(libc::_SC_GETPW_R_SIZE_MAX) {
            n if n > 0 => n as usize,
            _ => 1024,
        };
        loop {
            let mut buf = vec![0u8; size];
            let mut pwd: libc::passwd = std::mem::zeroed();
            let mut result: *mut libc::passwd = ptr::null_mut();
            let rc = libc::getpwuid_r(uid, &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut result);
            if rc == libc::ERANGE {
                size *= 2;
                continue;
            }
            anyhow::ensure!(rc == 0, "getpwuid_r({uid}) failed: {}", std::io::Error::from_raw_os_error(rc));
            anyhow::ensure!(!result.is_null(), "uid {uid} has no passwd entry");
            return Ok(CStr::from_ptr(pwd.pw_name).to_str().context("the account name is not UTF-8")?.to_owned());
        }
    }
}

/// Check a login: the username must be `account`, the process's own, and PAM
/// must accept the password for it and let the account in. Blocks for as long
/// as the stack takes — its modules may sleep on failure or run programs — so
/// call it off the runtime.
pub fn check(service: &str, account: &str, username: &str, password: &str, remote: &str) -> Result<(), Refused> {
    if username != account {
        return Err(Refused(format!("the login names {username:?}, and this desktop belongs to {account:?}")));
    }
    let answers = Answers {
        username: CString::new(username).map_err(|_| Refused("the username contains a NUL".to_owned()))?,
        password: CString::new(password).map_err(|_| Refused("the password contains a NUL".to_owned()))?,
    };
    let service = CString::new(service).map_err(|_| Refused("the PAM service name contains a NUL".to_owned()))?;
    let remote = CString::new(remote).map_err(|_| Refused("the remote address contains a NUL".to_owned()))?;
    let conv = PamConv {
        conv: Some(converse),
        appdata_ptr: (&answers as *const Answers).cast_mut().cast(),
    };
    // SAFETY: every pointer handed to libpam outlives the handle — `answers`,
    // `service`, `remote` and `conv` live to the end of this function, and the
    // handle is ended before it returns. libpam copies what it keeps.
    unsafe {
        let mut handle: *mut PamHandle = ptr::null_mut();
        let rc = pam_start(service.as_ptr(), answers.username.as_ptr(), &conv, &mut handle);
        if rc != PAM_SUCCESS {
            return Err(Refused(format!("pam_start failed: {}", strerror(ptr::null_mut(), rc))));
        }
        let rc = pam_set_item(handle, PAM_RHOST, remote.as_ptr().cast());
        if rc != PAM_SUCCESS {
            debug!("pam_set_item(PAM_RHOST) failed: {}", strerror(handle, rc));
        }
        let mut rc = pam_authenticate(handle, PAM_DISALLOW_NULL_AUTHTOK);
        let step = if rc == PAM_SUCCESS {
            rc = pam_acct_mgmt(handle, 0);
            "pam_acct_mgmt"
        } else {
            "pam_authenticate"
        };
        let outcome = if rc == PAM_SUCCESS { Ok(()) } else { Err(Refused(format!("{step}: {}", strerror(handle, rc)))) };
        pam_end(handle, rc);
        outcome
    }
}

unsafe fn strerror(handle: *mut PamHandle, rc: c_int) -> String {
    // SAFETY: pam_strerror returns a static string for any code.
    unsafe {
        let text = pam_strerror(handle, rc);
        if text.is_null() { format!("PAM error {rc}") } else { CStr::from_ptr(text).to_string_lossy().into_owned() }
    }
}

/// The PAM conversation: answer a hidden prompt with the password and a visible
/// one with the username, log what the stack has to say, and refuse anything
/// else. Linux-PAM hands the messages as an array of pointers, and takes the
/// responses as one `calloc`ed array it will `free`, each answer `malloc`ed.
unsafe extern "C" fn converse(num_msg: c_int, msg: *mut *const PamMessage, resp: *mut *mut PamResponse, appdata: *mut c_void) -> c_int {
    if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata.is_null() {
        return PAM_CONV_ERR;
    }
    // SAFETY: `appdata` is the `Answers` `check` registered, alive for the
    // whole PAM transaction; `msg` holds `num_msg` valid pointers, per libpam.
    unsafe {
        let answers = &*appdata.cast::<Answers>();
        let count = num_msg as usize;
        let responses = libc::calloc(count, std::mem::size_of::<PamResponse>()).cast::<PamResponse>();
        if responses.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..count {
            let message = &**msg.add(i);
            let text = if message.msg.is_null() { String::new() } else { CStr::from_ptr(message.msg).to_string_lossy().into_owned() };
            let answer = match message.msg_style {
                PAM_PROMPT_ECHO_OFF => Some(&answers.password),
                PAM_PROMPT_ECHO_ON => Some(&answers.username),
                PAM_ERROR_MSG => {
                    warn!("pam: {}", text.trim_end());
                    None
                }
                PAM_TEXT_INFO => {
                    debug!("pam: {}", text.trim_end());
                    None
                }
                other => {
                    warn!("pam: message style {other} is not understood; refusing the conversation");
                    for j in 0..i {
                        libc::free((*responses.add(j)).resp.cast());
                    }
                    libc::free(responses.cast());
                    return PAM_CONV_ERR;
                }
            };
            if let Some(answer) = answer {
                let copy = libc::strdup(answer.as_ptr());
                if copy.is_null() {
                    for j in 0..i {
                        libc::free((*responses.add(j)).resp.cast());
                    }
                    libc::free(responses.cast());
                    return PAM_BUF_ERR;
                }
                (*responses.add(i)).resp = copy;
            }
        }
        *resp = responses;
        PAM_SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_user_has_a_name() {
        let name = process_user().unwrap();
        assert!(!name.is_empty());
    }

    #[test]
    fn another_account_is_refused_before_pam_is_asked() {
        let account = process_user().unwrap();
        let err = check("swayrx", &account, &format!("not-{account}"), "x", "127.0.0.1").unwrap_err();
        assert!(err.to_string().contains("belongs to"), "{err}");
    }
}
