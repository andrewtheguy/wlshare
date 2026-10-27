//! The model `walk-sim` and the daemon's `tiles-walk-sim` share: the captures
//! played one after the other, the browser's link, and the zDesk gateway
//! between the session and the browser. Each includes it as a module of its
//! own, so a picture and its walk are played over the same link.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufReader, Read};
use std::process::{Command, Stdio};
use std::time::Duration;

use wlshare_vp9::Rect;
use wlshare_vp9::capture::Reader;

pub type Error = Box<dyn std::error::Error>;

pub struct Desk {
    pub at: Duration,
    pub size: (u16, u16),
    pub keyframe: bool,
    pub changed: Option<Vec<Rect>>,
    pub pixels: Vec<u8>,
}

/// The captures, one after the other, each's times after the one before.
pub struct Source {
    paths: Vec<String>,
    reader: Option<Reader<BufReader<Box<dyn Read>>>>,
    offset: Duration,
    last: Duration,
    peeked: Option<Desk>,
}

impl Source {
    pub fn new(paths: Vec<String>) -> Self {
        Self { paths, reader: None, offset: Duration::ZERO, last: Duration::ZERO, peeked: None }
    }

    fn open(path: &str) -> Result<Reader<BufReader<Box<dyn Read>>>, Error> {
        let input: Box<dyn Read> = if path.ends_with(".zst") {
            let mut child = Command::new("zstd").arg("-dc").arg(path).stdout(Stdio::piped()).spawn()?;
            Box::new(child.stdout.take().unwrap())
        } else {
            Box::new(File::open(path)?)
        };
        Ok(Reader::new(BufReader::with_capacity(1 << 20, input))?)
    }

    fn peek(&mut self) -> Result<Option<&Desk>, Error> {
        while self.peeked.is_none() {
            if self.reader.is_none() {
                if self.paths.is_empty() {
                    return Ok(None);
                }
                let path = self.paths.remove(0);
                self.reader = Some(Self::open(&path)?);
                // A frame interval after the last capture's end.
                self.offset = self.last + Duration::from_millis(33);
            }
            let reader = self.reader.as_mut().unwrap();
            match reader.next()? {
                Some(f) => {
                    let at = self.offset + f.at;
                    self.last = at;
                    // A capture after the first starts on its own picture: the
                    // whole of it changed, though no keyframe is asked.
                    let first = f.index == 0;
                    self.peeked = Some(Desk {
                        at,
                        size: f.size,
                        keyframe: f.keyframe && !(first && self.offset > Duration::ZERO),
                        changed: if first { None } else { f.changed.map(<[Rect]>::to_vec) },
                        pixels: f.pixels.to_vec(),
                    });
                }
                None => self.reader = None,
            }
        }
        Ok(self.peeked.as_ref())
    }

    pub fn next_at(&mut self) -> Result<Option<Duration>, Error> {
        Ok(self.peek()?.map(|d| d.at))
    }

    pub fn take(&mut self) -> Desk {
        self.peeked.take().unwrap()
    }
}

/// A link of a rate that changes at given times, in bytes a second.
pub struct Link {
    rates: Vec<(Duration, f64)>,
    one_way: Duration,
    /// When the link is next idle.
    free: Duration,
}

impl Link {
    fn rate_at(&self, t: Duration) -> (f64, Option<Duration>) {
        let i = self.rates.iter().rposition(|(from, _)| *from <= t).unwrap_or(0);
        (self.rates[i].1, self.rates.get(i + 1).map(|(from, _)| *from))
    }

    /// When `bytes` written at `at` have all left the bottleneck.
    fn send(&mut self, at: Duration, bytes: usize) -> Duration {
        let mut t = at.max(self.free);
        let mut left = bytes as f64;
        loop {
            let (rate, until) = self.rate_at(t);
            let need = Duration::from_secs_f64(left / rate);
            match until {
                Some(until) if t + need > until => {
                    left -= rate * (until - t).as_secs_f64();
                    t = until;
                }
                _ => {
                    t += need;
                    break;
                }
            }
        }
        self.free = t;
        t
    }
}

/// What the gateway queues for the browser before it reads no more of the
/// session: zDesk's `QUEUE_BUDGET`. Each record takes its body's bytes of it,
/// the whole budget at most, so one larger goes through alone, and gives them
/// back once it is written.
const BUDGET: usize = 512 * 1024;

/// The zDesk gateway between the session and the browser, as it passes a
/// picture wlshare codes (zDesk's `VideoSink::fence_hold`), `WLSV`'s or the
/// tiles': each record of an update is handed to the browser's link once its
/// share of the gateway's budget is free, the records ahead of it on the link
/// holding theirs until they are written; and its fence, read once the last
/// record is handed, is echoed at once when nothing handed before it is still
/// unpainted, and otherwise once the oldest such has been owed past the link's
/// ping round trip, held 500 ms at most.
pub struct Gateway {
    link: Link,
    /// The round trip between the session and the gateway.
    hop: Duration,
    /// The records on the link holding a share of the budget: when each is
    /// written, and its share.
    queued: VecDeque<(Duration, usize)>,
    /// Updates handed over and not yet painted: when each was, and when its
    /// paint is acknowledged.
    owed: VecDeque<(Duration, Duration)>,
    /// When the last echo went, which the next cannot overtake.
    echoed: Duration,
    /// When the browser has painted the last update, which it paints one at
    /// a time, in order.
    pub painted: Duration,
}

impl Gateway {
    /// A gateway to a browser on a link of `rates`, each from when it starts,
    /// of a round trip `rtt`, `hop` from the session.
    pub fn new(rates: Vec<(Duration, f64)>, rtt: Duration, hop: Duration) -> Self {
        let link = Link { rates, one_way: rtt / 2, free: Duration::ZERO };
        Self { link, hop, queued: VecDeque::new(), owed: VecDeque::new(), echoed: Duration::ZERO, painted: Duration::ZERO }
    }

    /// An update the session wrote at `sent`, its `records` in order, each its
    /// bytes on the link and those of them the budget is taken for: when the
    /// browser has painted it, and when its fence's echo is back at the session.
    pub fn frame(&mut self, sent: Duration, records: &[(usize, usize)], paint: Duration) -> (Duration, Duration) {
        let (mut handed, mut end) = (sent + self.hop / 2, Duration::ZERO);
        for &(bytes, budgeted) in records {
            let share = budgeted.min(BUDGET);
            loop {
                while self.queued.front().is_some_and(|(written, _)| *written <= handed) {
                    self.queued.pop_front();
                }
                if self.queued.iter().map(|(_, share)| share).sum::<usize>() + share <= BUDGET {
                    break;
                }
                handed = self.queued[0].0;
            }
            end = self.link.send(handed, bytes);
            if share > 0 {
                self.queued.push_back((end, share));
            }
        }
        let painted = (end + self.link.one_way).max(self.painted) + paint;
        self.painted = painted;
        while self.owed.front().is_some_and(|(_, acked)| *acked <= handed) {
            self.owed.pop_front();
        }
        let hold = self.owed.front().map_or(Duration::ZERO, |(at, _)| (handed - *at).saturating_sub(self.link.one_way * 2));
        self.owed.push_back((handed, painted + self.link.one_way));
        self.echoed = (handed + hold.min(Duration::from_millis(500))).max(self.echoed);
        (painted, self.echoed + self.hop / 2)
    }
}

/// `--link`'s value: each rate in Mbit/s and the second it starts at, as
/// bytes a second from when it starts.
pub fn link_steps(value: &str) -> Result<Vec<(Duration, f64)>, Error> {
    let rates = value
        .split(',')
        .map(|step| {
            let (rate, from) = step.split_once('@').unwrap_or((step, "0"));
            let (rate, from) = (rate.parse::<f64>()?, from.parse::<f64>()?);
            if !(rate.is_finite() && rate > 0.0 && from.is_finite() && from >= 0.0) {
                return Err(format!("--link step {step}: a rate above zero and a time of zero or more").into());
            }
            Ok((Duration::from_secs_f64(from), rate * 1e6 / 8.0))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    if rates.windows(2).any(|pair| pair[1].0 < pair[0].0) {
        return Err("--link steps go in order of their times".into());
    }
    Ok(rates)
}

/// The link's steps as JSON: each rate in Mbit/s and the second it starts at.
pub fn rates_json(rates: &[(Duration, f64)]) -> String {
    let steps: Vec<String> = rates.iter().map(|(from, rate)| format!("{{\"mbps\":{},\"from_s\":{}}}", rate * 8.0 / 1e6, from.as_secs_f64())).collect();
    format!("[{}]", steps.join(","))
}

/// A run's name: its captures' file names, less `.wlcap.zst`, joined by `+`.
pub fn run_name(paths: &[String]) -> String {
    paths.iter().map(|p| p.rsplit('/').next().unwrap().trim_end_matches(".wlcap.zst").to_string()).collect::<Vec<_>>().join("+")
}

/// Up to `jitter` ms more of the browser's paint, at random, the same run
/// after run.
pub fn jitter(jitter: f64) -> impl FnMut() -> Duration {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        Duration::from_secs_f64((seed >> 11) as f64 / (1u64 << 53) as f64 * jitter / 1e3)
    }
}

pub fn pct(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// A flag's time in milliseconds, which is zero or more.
pub fn millis(flag: &str, value: String) -> Result<f64, Error> {
    let ms: f64 = value.parse()?;
    if !(ms.is_finite() && ms >= 0.0) {
        return Err(format!("{flag} {value}: a time of zero or more").into());
    }
    Ok(ms)
}

/// A string as JSON.
pub fn json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
