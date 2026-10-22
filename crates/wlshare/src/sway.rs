//! sway's IPC, for the one thing no Wayland protocol a client can bind offers:
//! making an output, and taking one away.
//!
//! A client that asks for a number of virtual displays
//! ([`crate::shared::Command::VirtualDisplays`]) is asking for that many headless
//! outputs, the way an RDP client's monitor layout and a Mac's display
//! configuration ask their hosts, and only the compositor can make them. sway
//! makes one for `create_output` and destroys one for `output NAME unplug`, a
//! startup one included; wlroots names them `HEADLESS-n`, counting up for the
//! life of the compositor, so an output made after one was unplugged is a new
//! one under a new name, not the old one back.
//!
//! The socket is `SWAYSOCK` in the environment, which sway sets for everything
//! it starts and the session's user manager carries to its units. Without it,
//! or with something that does not answer as sway, the daemon makes no outputs
//! and says so in every list ([`wlshare_rfb::outputs::output_list`]): [`MOST`]
//! virtual displays where it can, none where it cannot. Nothing is configured.
//!
//! The protocol is i3's: the magic `i3-ipc`, a native-endian length and a
//! native-endian type, then the payload. A `RUN_COMMAND` is answered with a
//! JSON array holding one `success` per command, and `error` where it failed.
//! Each request is one connection, written and read on the compositor thread:
//! sway answers a command once it has run it, in milliseconds, and the output
//! it made or destroyed is announced on the Wayland socket by the time it does.

use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use log::{info, warn};
use serde::Deserialize;

/// The most virtual displays a client may ask for: one per desk, since an
/// output is shown on one desk and there are two ([`crate::shared::BESIDE`]).
pub const MOST: u8 = 2;

const MAGIC: &[u8; 6] = b"i3-ipc";
const RUN_COMMAND: u32 = 0;
const GET_VERSION: u32 = 7;
/// Longer than any command takes a compositor that is answering at all; a
/// compositor that is not should not hold the thread that captures it.
const TIMEOUT: Duration = Duration::from_secs(5);
/// Longer than any reply read here: a version or a command's outcomes.
const MAX_REPLY: usize = 1 << 20;

/// sway's IPC socket.
pub struct Sway {
    path: PathBuf,
}

/// One command's outcome, as sway's reply lists them.
#[derive(Deserialize)]
struct Outcome {
    success: bool,
    #[serde(default)]
    error: Option<String>,
}

impl Sway {
    /// The compositor's IPC, if `SWAYSOCK` names a socket that answers as sway.
    pub fn from_env() -> Option<Self> {
        let Some(path) = std::env::var_os("SWAYSOCK") else {
            info!("no SWAYSOCK in the environment: a client's virtual displays are the outputs the compositor has");
            return None;
        };
        Self::at(Path::new(&path))
    }

    fn at(path: &Path) -> Option<Self> {
        let sway = Self { path: path.to_path_buf() };
        match sway.version() {
            Ok(version) => {
                info!("sway {version} at {}: a client may ask for up to {MOST} virtual displays", path.display());
                Some(sway)
            }
            Err(e) => {
                warn!("{}: {e:#}; a client's virtual displays are the outputs the compositor has", path.display());
                None
            }
        }
    }

    fn version(&self) -> anyhow::Result<String> {
        #[derive(Deserialize)]
        struct Version {
            human_readable: String,
        }
        let reply = self.request(GET_VERSION, b"")?;
        let version: Version = serde_json::from_slice(&reply).context("reading sway's version")?;
        Ok(version.human_readable)
    }

    /// Make a headless output. Its name is wlroots' and arrives with it.
    pub fn create_output(&self) -> anyhow::Result<()> {
        self.command("create_output")
    }

    /// Destroy a headless output.
    pub fn unplug(&self, name: &str) -> anyhow::Result<()> {
        self.command(&format!("output {name} unplug"))
    }

    fn command(&self, command: &str) -> anyhow::Result<()> {
        let reply = self.request(RUN_COMMAND, command.as_bytes())?;
        let outcomes: Vec<Outcome> = serde_json::from_slice(&reply).with_context(|| format!("reading sway's answer to `{command}`"))?;
        for outcome in outcomes {
            if !outcome.success {
                anyhow::bail!("sway refused `{command}`: {}", outcome.error.as_deref().unwrap_or("no reason given"));
            }
        }
        Ok(())
    }

    fn request(&self, kind: u32, payload: &[u8]) -> anyhow::Result<Vec<u8>> {
        let mut stream = UnixStream::connect(&self.path).with_context(|| format!("connecting to sway's IPC at {}", self.path.display()))?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut msg = Vec::with_capacity(14 + payload.len());
        msg.extend_from_slice(MAGIC);
        msg.extend_from_slice(&(payload.len() as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(payload);
        stream.write_all(&msg).context("writing to sway's IPC")?;
        let mut header = [0u8; 14];
        stream.read_exact(&mut header).context("reading sway's reply")?;
        anyhow::ensure!(&header[..6] == MAGIC, "the socket does not answer as sway's IPC");
        let len = u32::from_ne_bytes(header[6..10].try_into().expect("four bytes")) as usize;
        let replied = u32::from_ne_bytes(header[10..14].try_into().expect("four bytes"));
        anyhow::ensure!(replied == kind, "sway answered a message of type {kind} with type {replied}");
        anyhow::ensure!(len <= MAX_REPLY, "a {len}-byte reply from sway");
        let mut reply = vec![0u8; len];
        stream.read_exact(&mut reply).context("reading sway's reply")?;
        Ok(reply)
    }
}

/// How many headless outputs the client on the desktop wants, and the step in
/// flight towards it. One step at a time: a `create_output` is followed by
/// another only once its output has appeared, and an unplug by another only
/// once its output is gone, each of which is a change of the outputs the
/// compositor announces ([`crate::compositor::Compositor::reconcile_virtual_displays`]
/// runs on every one).
#[derive(Default)]
pub struct VirtualDisplays {
    /// The count the client on the desktop asked for, while one has. Cleared
    /// when it leaves or is superseded: the outputs then stand as they are
    /// until the next client says.
    pub wanted: Option<u32>,
    /// A `create_output` sent, with the headless outputs there were by name:
    /// it has landed when one not among them is here, however many there are,
    /// since another may go meanwhile.
    creating: Option<Vec<String>>,
    /// An unplug sent, by the output's name.
    unplugging: Option<String>,
}

/// The one step towards `wanted` headless outputs from `headless`, the ones the
/// compositor has by name.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// There are as many as wanted.
    Settled,
    /// One fewer than wanted, at least.
    Create,
    /// One more than wanted, at least: this one goes.
    Unplug(String),
}

impl VirtualDisplays {
    /// Note what the compositor has now, and say what is to be done: nothing
    /// while a step is in flight, and otherwise the step [`step`] names.
    /// `shared` and `preferred` are as `step` takes them.
    pub fn next(&mut self, headless: &[String], shared: Option<&str>, preferred: Option<&str>) -> Step {
        if self.creating.as_ref().is_some_and(|before| headless.iter().any(|name| !before.contains(name))) {
            self.creating = None;
        }
        if self.unplugging.as_ref().is_some_and(|name| !headless.contains(name)) {
            self.unplugging = None;
        }
        let Some(wanted) = self.wanted else { return Step::Settled };
        if self.creating.is_some() || self.unplugging.is_some() {
            return Step::Settled;
        }
        step(wanted, headless, shared, preferred)
    }

    /// A `create_output` went out, with the headless outputs there were.
    pub fn creating(&mut self, headless: Vec<String>) {
        self.creating = Some(headless);
    }

    /// An unplug went out.
    pub fn unplugging(&mut self, name: String) {
        self.unplugging = Some(name);
    }
}

/// The step towards `wanted` headless outputs. `shared`, the output the desktop
/// is on, is never unplugged: it is the one the client is looking at. Of the
/// rest the latest made goes first — the highest number, since wlroots counts
/// up — and `preferred`, the configured output, last.
pub fn step(wanted: u32, headless: &[String], shared: Option<&str>, preferred: Option<&str>) -> Step {
    let wanted = usize::try_from(wanted).unwrap_or(usize::MAX);
    if headless.len() < wanted {
        return Step::Create;
    }
    if headless.len() == wanted {
        return Step::Settled;
    }
    let mut candidates: Vec<&String> = headless.iter().filter(|name| Some(name.as_str()) != shared).collect();
    candidates.sort_by_key(|name| (Some(name.as_str()) == preferred, std::cmp::Reverse(number(name))));
    match candidates.first() {
        Some(name) => Step::Unplug((*name).clone()),
        None => Step::Settled,
    }
}

/// The number wlroots gave a headless output: `HEADLESS-3` is 3.
fn number(name: &str) -> u32 {
    name.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_count_below_what_there_is_makes_one_and_a_count_above_unplugs_the_latest() {
        assert_eq!(step(2, &names(&["HEADLESS-1"]), Some("HEADLESS-1"), Some("HEADLESS-1")), Step::Create);
        assert_eq!(step(1, &names(&["HEADLESS-1"]), Some("HEADLESS-1"), Some("HEADLESS-1")), Step::Settled);
        assert_eq!(step(2, &names(&["HEADLESS-1", "HEADLESS-2"]), Some("HEADLESS-1"), None), Step::Settled);
        // The latest made goes first, whatever order the compositor listed them in.
        assert_eq!(
            step(1, &names(&["HEADLESS-3", "HEADLESS-1", "HEADLESS-10"]), Some("HEADLESS-1"), Some("HEADLESS-1")),
            Step::Unplug("HEADLESS-10".into())
        );
    }

    #[test]
    fn the_shared_output_stays_and_the_configured_one_goes_last() {
        // The client on HEADLESS-2 asking for one keeps HEADLESS-2, its own, and
        // loses the configured HEADLESS-1: there is nothing else to unplug.
        assert_eq!(step(1, &names(&["HEADLESS-1", "HEADLESS-2"]), Some("HEADLESS-2"), Some("HEADLESS-1")), Step::Unplug("HEADLESS-1".into()));
        // With a third, the configured one is kept over it.
        assert_eq!(
            step(1, &names(&["HEADLESS-1", "HEADLESS-2", "HEADLESS-3"]), Some("HEADLESS-2"), Some("HEADLESS-1")),
            Step::Unplug("HEADLESS-3".into())
        );
        // Nothing but the shared one is nothing to unplug.
        assert_eq!(step(0, &names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Settled);
    }

    #[test]
    fn one_step_is_in_flight_at_a_time() {
        let mut v = VirtualDisplays::default();
        assert_eq!(v.next(&names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Settled, "nothing asked");
        v.wanted = Some(2);
        assert_eq!(v.next(&names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Create);
        v.creating(names(&["HEADLESS-1"]));
        assert_eq!(v.next(&names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Settled, "the output is still to appear");
        assert_eq!(v.next(&names(&["HEADLESS-1", "HEADLESS-2"]), Some("HEADLESS-1"), None), Step::Settled, "and now there are two");
        v.wanted = Some(1);
        assert_eq!(v.next(&names(&["HEADLESS-1", "HEADLESS-2"]), Some("HEADLESS-1"), None), Step::Unplug("HEADLESS-2".into()));
        v.unplugging("HEADLESS-2".into());
        assert_eq!(v.next(&names(&["HEADLESS-1", "HEADLESS-2"]), Some("HEADLESS-1"), None), Step::Settled, "still going");
        assert_eq!(v.next(&names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Settled, "gone, and one is what was asked");
        // A client leaving clears the request; a step in flight settles on its own.
        v.wanted = None;
        v.creating(names(&["HEADLESS-1"]));
        assert_eq!(v.next(&names(&["HEADLESS-1", "HEADLESS-3"]), Some("HEADLESS-1"), None), Step::Settled);
        v.wanted = Some(2);
        assert_eq!(v.next(&names(&["HEADLESS-1", "HEADLESS-3"]), Some("HEADLESS-1"), None), Step::Settled, "the earlier step has landed");
    }

    /// An output going while the made one arrives leaves the count as it was:
    /// the step has landed all the same, and the next is taken.
    #[test]
    fn a_made_output_has_landed_whatever_went_meanwhile() {
        let mut v = VirtualDisplays { wanted: Some(2), ..VirtualDisplays::default() };
        assert_eq!(v.next(&names(&["HEADLESS-1"]), Some("HEADLESS-1"), None), Step::Create);
        v.creating(names(&["HEADLESS-1"]));
        // HEADLESS-1 was unplugged by hand as HEADLESS-2 arrived: still one short.
        assert_eq!(v.next(&names(&["HEADLESS-2"]), Some("HEADLESS-2"), None), Step::Create);
    }

    /// A sway written from the i3-ipc layout answers each request on a socket
    /// of its own, and what it was sent is checked against the layout too.
    fn fake_sway(replies: Vec<(u32, Vec<u8>, &'static str)>) -> (PathBuf, std::thread::JoinHandle<()>) {
        let dir = std::env::temp_dir().join(format!("wlshare-sway-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sock");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let served = std::thread::spawn(move || {
            for (kind, payload, reply) in replies {
                let (mut stream, _) = listener.accept().unwrap();
                let mut header = [0u8; 14];
                stream.read_exact(&mut header).unwrap();
                assert_eq!(&header[..6], b"i3-ipc");
                let len = u32::from_ne_bytes(header[6..10].try_into().unwrap()) as usize;
                assert_eq!(u32::from_ne_bytes(header[10..14].try_into().unwrap()), kind);
                let mut got = vec![0u8; len];
                stream.read_exact(&mut got).unwrap();
                assert_eq!(got, payload);
                let mut msg = b"i3-ipc".to_vec();
                msg.extend_from_slice(&(reply.len() as u32).to_ne_bytes());
                msg.extend_from_slice(&kind.to_ne_bytes());
                msg.extend_from_slice(reply.as_bytes());
                stream.write_all(&msg).unwrap();
            }
            let _ = std::fs::remove_dir_all(dir);
        });
        (path, served)
    }

    #[test]
    fn commands_are_framed_as_i3_ipc_and_a_refusal_is_an_error_with_its_reason() {
        let (path, served) = fake_sway(vec![
            (GET_VERSION, b"".to_vec(), r#"{"human_readable":"1.11","major":1,"minor":11,"patch":0}"#),
            (RUN_COMMAND, b"create_output".to_vec(), r#"[{"success":true}]"#),
            (RUN_COMMAND, b"output HEADLESS-2 unplug".to_vec(), r#"[{"success":false,"error":"Only headless outputs can be unplugged"}]"#),
        ]);
        let sway = Sway::at(&path).expect("a sway that answers its version");
        sway.create_output().unwrap();
        let err = sway.unplug("HEADLESS-2").unwrap_err();
        assert!(err.to_string().contains("Only headless outputs can be unplugged"), "{err:#}");
        served.join().unwrap();
    }

    #[test]
    fn a_socket_that_is_not_sway_is_no_sway() {
        let (path, served) = fake_sway(vec![(GET_VERSION, b"".to_vec(), "not json")]);
        assert!(Sway::at(&path).is_none());
        served.join().unwrap();
        assert!(Sway::at(Path::new("/nonexistent/sway.sock")).is_none());
    }
}
