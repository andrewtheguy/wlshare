//! A session against a real wlshare, which no unit test can stand in for: the
//! handshake, the ZRLE and VP9 streams, the cursor, the density extension, the
//! clipboard and the sound all only exist between two processes.
//!
//! Ignored by default and pointed at `WLSHARE_TEST_SERVER`, or `127.0.0.1:5900`,
//! as `WLSHARE_TEST_USERNAME` with `WLSHARE_TEST_PASSWORD`, or unauthenticated
//! when there is none. They run from any checkout against any wlshare —
//! typically the one of the session they run in:
//!
//! ```text
//! cargo test -p wlshare-client --test live_session -- --ignored --nocapture --test-threads=1
//! ```
//!
//! They resize the desktop they talk to, so point them at one nobody is using.

use std::time::{Duration, Instant};

use wlshare_client::{BUTTON_LEFT, Client, Config, Encoding, State, Surface, WHEEL_DOWN};

const PATIENCE: Duration = Duration::from_secs(20);

fn server() -> (String, u16) {
    let address = std::env::var("WLSHARE_TEST_SERVER").unwrap_or_else(|_| "127.0.0.1:5900".to_owned());
    let (host, port) = address.rsplit_once(':').expect("WLSHARE_TEST_SERVER is host:port");
    (host.to_owned(), port.parse().expect("a port"))
}

/// Poll until `done` or give up. Polling rather than the wake callback: what is
/// being tested is what the window would see, and the window sees this.
fn until<T>(what: &str, mut done: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = done() {
            return value;
        }
        assert!(Instant::now() < deadline, "waited {PATIENCE:?} for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn connect(surface: Surface) -> Client {
    connect_with(surface, false, Encoding::Zrle)
}

fn connect_with(surface: Surface, audio: bool, encoding: Encoding) -> Client {
    let _ = env_logger::builder().is_test(false).try_init();
    let (host, port) = server();
    let username = std::env::var("WLSHARE_TEST_USERNAME").unwrap_or_default();
    let password = std::env::var("WLSHARE_TEST_PASSWORD").unwrap_or_default();
    let client = Client::connect(Config { host, port, username, password, audio, encoding }, surface);
    until("the handshake", || match client.status() {
        status if status.state == State::Ready => Some(()),
        status if status.state == State::Closed => panic!("the session ended: {:?}", status.error),
        _ => None,
    });
    client
}

#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn a_session_gets_a_desktop_and_paints_it() {
    let client = connect(Surface { width: 1024, height: 768, scale: 1.0 });
    let status = client.status();
    assert!(!status.name.is_empty(), "ServerInit names the desktop");
    println!("connected to {:?}", status.name);

    // The desktop painted at the size that was asked for. A previous test may
    // have left it another size, so this waits for both rather than looking
    // once: the resize and the repaint that follows it are two frames apart.
    let (damage, lit) = until("a painted 1024x768 desktop", || {
        client.with_frame(|fb, damage| {
            if (fb.width(), fb.height()) != (1024, 768) {
                return None;
            }
            let lit = fb.pixels().as_chunks::<4>().0.iter().filter(|p| p[..3] != [0, 0, 0]).count();
            (lit > 0).then_some((damage, lit))
        })
    });
    println!("1024x768, damage {damage:?}, {lit} pixels lit");
    let damage = damage.expect("a frame that painted damages what it painted");
    assert!(damage.width > 0 && damage.height > 0);
    assert!(client.status().frames > 0, "those pixels came from a decoded update, not from a resize");
}

/// The same desktop as VP9: the stream arrives, decodes to a painted picture,
/// and follows a change of size and density — each a new encoder on the
/// server, whose first frame is a keyframe the same decoder takes.
#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn a_vp9_session_gets_a_desktop_and_follows_a_resize() {
    let client = connect_with(Surface { width: 1024, height: 768, scale: 1.0 }, false, Encoding::Vp9);
    let lit = until("a painted 1024x768 VP9 desktop", || {
        client.with_frame(|fb, _| {
            if (fb.width(), fb.height()) != (1024, 768) {
                return None;
            }
            let pixels = fb.pixels().as_chunks::<4>().0;
            let lit = pixels.iter().filter(|p| p[..3] != [0, 0, 0]).count();
            (lit > 0).then(|| (lit, pixels[pixels.len() / 2 + 512]))
        })
    });
    // B, G, R: the desktop's background, which is whatever colour the server
    // was given — compare it by eye with what ZRLE shows.
    println!("VP9 at 1024x768, {} pixels lit, the centre is {:?}", lit.0, lit.1);

    client.surface(Surface { width: 1280, height: 800, scale: 2.0 });
    settled(&client, 1280, 800, 2.0);
    let frames = client.status().frames;
    until("a VP9 frame at the new size", || (client.status().frames > frames).then_some(()));
    let status = client.status();
    assert_eq!(status.state, State::Ready, "{:?}", status.error);
}

/// Wait for the desktop to be this many pixels across at this scale, which is
/// what a window of that size at that choice of 1× or 2× asked for.
fn settled(client: &Client, width: u16, height: u16, scale: f64) {
    until(&format!("a {width}x{height} desktop at scale {scale}"), || {
        let status = client.status();
        let size = client.desktop_size();
        ((status.scale - scale).abs() < 0.01 && size == (width, height)).then_some(())
    });
    println!("{width}x{height} at scale {scale}");
}

#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn switching_between_1x_and_2x_redraws_the_desktop_at_that_scale() {
    // A window of 1024x768 device pixels at 2× gets a desktop of 1024x768
    // pixels drawn at scale 2 — 512x384 of desktop, twice as sharp.
    let client = connect(Surface { width: 1024, height: 768, scale: 2.0 });
    // Both, and waited for together: the size is asked for only once the scale
    // has been answered, so the two land one after the other.
    settled(&client, 1024, 768, 2.0);

    // The window's switch to 1×, at the same size: only the scale moves.
    client.surface(Surface { width: 1024, height: 768, scale: 1.0 });
    settled(&client, 1024, 768, 1.0);

    // And a resize at 1×, which is the window being dragged.
    client.surface(Surface { width: 800, height: 600, scale: 1.0 });
    settled(&client, 800, 600, 1.0);
}

#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn the_pointer_has_a_shape_and_input_is_taken() {
    let client = connect(Surface { width: 1024, height: 768, scale: 1.0 });

    // A desktop nobody has touched may have no pointer showing at all, and an
    // output with no pointer on it sends an empty cursor rectangle. Move it
    // first, and the shape follows.
    client.pointer(0, 400, 300);
    client.pointer(0, 512, 384);

    // wlshare never paints the pointer into a frame, so a client that is sent
    // no cursor has no pointer at all: this is the one that must arrive.
    let (generation, width, height, hotspot) = until("the cursor", || {
        client.with_cursor(|generation, image| {
            let image = image?;
            Some((generation, image.width(), image.height(), image.hotspot()))
        })
    });
    println!("cursor {width}x{height}, hotspot {hotspot:?}, shape {generation}");
    assert!(width > 0 && height > 0);
    assert!(hotspot.0 < width && hotspot.1 < height);

    // Nothing here asserts what the desktop did with these — that is the
    // daemon's own e2e — only that a session takes them and stays up.
    client.pointer(BUTTON_LEFT, 512, 384);
    client.pointer(0, 512, 384);
    for _ in 0..3 {
        client.pointer(WHEEL_DOWN, 512, 384);
        client.pointer(0, 512, 384);
    }
    client.key(true, 0xffe1); // Shift_L
    client.key(true, 0x41); // 'A', which the server cases for itself
    client.key(false, 0x41);
    client.key(false, 0xffe1);

    std::thread::sleep(Duration::from_millis(500));
    let status = client.status();
    assert_eq!(status.state, State::Ready, "the session survived the input: {:?}", status.error);
}

#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn a_clipboard_given_by_one_window_reaches_another_through_the_desktop() {
    // The server keeps a clipboard a client set out of that client's own
    // notifications, so the far end of the round trip is a second session.
    let giver = connect(Surface { width: 1024, height: 768, scale: 1.0 });
    let taker = connect(Surface { width: 1024, height: 768, scale: 1.0 });
    let seen = taker.with_clipboard(|generation, _| generation);

    // Unique per run, so a desktop clipboard left over from the last one is not
    // mistaken for this.
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("a clock after 1970").as_nanos();
    let text = format!("画面\nnaïve ☕ {nanos}");
    giver.clipboard(text.clone());

    // Exactly as it went: UTF-8, and the line ending the wire carries as CRLF
    // back as LF.
    until("the clipboard on the other session", || {
        taker.with_clipboard(|generation, arrived| (generation != seen && arrived == Some(text.as_str())).then_some(()))
    });
}

/// Asked for, the server's sound is turned on and its frames decode: its
/// capture runs whether the desktop plays anything or not, so a silent desktop
/// still sends a frame every 20 ms.
#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn sound_asked_for_is_turned_on_and_decodes() {
    let client = connect_with(Surface { width: 1024, height: 768, scale: 1.0 }, true, Encoding::Zrle);
    until("the sound turned on", || client.status().audio.then_some(()));
    let sound = until("a second of decoded sound", || {
        let sound = client.status().sound;
        (sound >= 50).then_some(sound)
    });
    println!("{sound} FLAC frames decoded");

    let (mut left, mut right) = (vec![9.0; 480], vec![9.0; 480]);
    client.read_audio(&mut left, &mut right);
    assert!(left.iter().chain(&right).all(|s| (-1.0..1.0).contains(s)), "samples, not what was there before");
}

/// Not asked for, the sound stays off however long the session runs.
#[test]
#[ignore = "needs a wlshare server; see the module comment"]
fn sound_not_asked_for_stays_off() {
    let client = connect(Surface { width: 1024, height: 768, scale: 1.0 });
    std::thread::sleep(Duration::from_secs(1));
    let status = client.status();
    assert!(!status.audio);
    assert_eq!(status.sound, 0);
}

/// What the desktop plays arrives as that sound. Something must be playing on
/// the desktop while this runs — a tone into the default sink, which follows
/// the default to wlshare's speaker when the session turns the sound on. In the
/// e2e image from `../remotex`, which plays nothing of its own:
///
/// ```text
/// podman exec -d wlshare-client-dev sh -c 'export XDG_RUNTIME_DIR=/tmp/xdg;
///   gst-launch-1.0 -q audiotestsrc freq=440 volume=0.5
///     ! audio/x-raw,rate=48000,channels=2,format=S16LE,layout=interleaved ! fdsink
///   | pw-cat -p --raw --format s16 --rate 48000 --channels 2 -'
/// ```
///
/// Through `pw-cat` rather than GStreamer's `pipewiresink`, whose stream never
/// finishes negotiating there and holds the speaker and the capture suspended.
///
/// A silent desktop sends frames of zeros, which is what the test above takes;
/// this is the one that proves the samples are the desktop's and not silence
/// that decoded.
#[test]
#[ignore = "needs a wlshare server playing sound; see the doc comment"]
fn the_sound_the_desktop_plays_arrives() {
    let client = connect_with(Surface { width: 1024, height: 768, scale: 1.0 }, true, Encoding::Zrle);
    until("the sound turned on", || client.status().audio.then_some(()));

    // Taken the way an audio device takes it, in 10 ms reads, until a second
    // of sound louder than a whisper has come through.
    let mut heard = Vec::new();
    until("a second of the desktop's sound", || {
        let (mut left, mut right) = (vec![0.0f32; 480], vec![0.0f32; 480]);
        client.read_audio(&mut left, &mut right);
        if left.iter().any(|s| s.abs() > 0.01) {
            heard.extend(left);
        }
        std::thread::sleep(Duration::from_millis(10));
        (heard.len() >= 48_000).then_some(())
    });
    let peak = heard.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    let crossings = heard.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    let hz = crossings as f64 / 2.0 * 48_000.0 / heard.len() as f64;
    println!("peak {peak:.3}, about {hz:.0} Hz on the left");
    assert!(peak > 0.05, "a peak of {peak} is not the desktop playing anything");
}
