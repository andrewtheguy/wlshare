//! Scratch: the lossless tiles' planar coder alone over a capture, one
//! thread. `planar-bench CAPTURE [FRAMES]`: every run of tiles a frame
//! changed, a row of them at a time, coded as a planar bitmap, and then
//! coded and framed as a body; prints the time of each and a hash of every byte coded.

#![allow(clippy::zombie_processes)]

use std::hash::Hasher as _;
use std::io::{BufReader, Read};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[allow(unused)]
#[path = "../src/rfb/mod.rs"]
mod rfb;
#[allow(dead_code)]
#[path = "../src/wlcap.rs"]
mod wlcap;

use rfb::planar::{encode, pack, Rows};
use rfb::progressive::TILE;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let most: u64 = args.get(2).map_or(u64::MAX, |n| n.parse().unwrap());
    let input: Box<dyn Read> = if args[1].ends_with(".zst") {
        let mut child = Command::new("zstd").arg("-dc").arg(&args[1]).stdout(Stdio::piped()).spawn().unwrap();
        Box::new(child.stdout.take().unwrap())
    } else {
        Box::new(std::fs::File::open(&args[1]).unwrap())
    };
    let mut reader = wlcap::Reader::new(BufReader::with_capacity(1 << 20, input)).unwrap();
    let mut hash = xxhash_rust::xxh3::Xxh3Default::new();
    let mut rows = Rows::default();
    let mut bitmap = Vec::new();
    let (mut coding, mut framing) = (Duration::ZERO, Duration::ZERO);
    let (mut runs, mut coded, mut framed, mut frames) = (0u64, 0u64, 0u64, 0u64);
    while let Some(frame) = reader.next().unwrap() {
        if frames == most {
            break;
        }
        frames += 1;
        let (w, h) = (usize::from(frame.size.0), usize::from(frame.size.1));
        let stride = w * 4;
        let (cols, tile_rows) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let mut cells = vec![false; cols * tile_rows];
        match frame.changed {
            None => cells.fill(true),
            Some(rects) => {
                for r in rects {
                    for row in usize::from(r.y) / TILE..(usize::from(r.y) + usize::from(r.height)).div_ceil(TILE) {
                        for col in usize::from(r.x) / TILE..(usize::from(r.x) + usize::from(r.width)).div_ceil(TILE) {
                            cells[row * cols + col] = true;
                        }
                    }
                }
            }
        }
        let pixels = frame.pixels;
        for row in 0..tile_rows {
            let mut col = 0;
            while col < cols {
                if !cells[row * cols + col] {
                    col += 1;
                    continue;
                }
                let from = col;
                while col < cols && cells[row * cols + col] {
                    col += 1;
                }
                let (x, y) = (from * TILE, row * TILE);
                let (rw, rh) = ((col * TILE).min(w) - x, TILE.min(h - y));
                let at = &pixels[y * stride + x * 4..];
                bitmap.clear();
                let began = Instant::now();
                encode(at, stride, rw as u16, rh as u16, &mut rows, &mut bitmap);
                let coded_at = Instant::now();
                let body = pack(at, stride, rw as u16, rh as u16).unwrap();
                framing += coded_at.elapsed();
                coding += coded_at - began;
                hash.write(&bitmap);
                runs += 1;
                coded += bitmap.len() as u64;
                framed += body.len() as u64;
            }
        }
    }
    println!(
        "{frames} frames {runs} runs coded {coded} bytes {:.1} ms packed {framed} bytes {:.1} ms (coded again and framed) hash {:016x}",
        coding.as_secs_f64() * 1e3,
        framing.as_secs_f64() * 1e3,
        hash.digest()
    );
}
