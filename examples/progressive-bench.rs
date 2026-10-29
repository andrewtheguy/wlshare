//! Scratch: the progressive coder alone over a capture, one thread.
//! `progressive-bench CAPTURE STAGE [FRAMES]`: every tile a frame changed
//! coded as `first_or_full` at STAGE; prints the time and a hash of every
//! byte coded.

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

use rfb::progressive::{Encoder, Tables, TILE};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let stage: usize = args[2].parse().unwrap();
    let most: u64 = args.get(3).map_or(u64::MAX, |n| n.parse().unwrap());
    let input: Box<dyn Read> = if args[1].ends_with(".zst") {
        let mut child = Command::new("zstd").arg("-dc").arg(&args[1]).stdout(Stdio::piped()).spawn().unwrap();
        Box::new(child.stdout.take().unwrap())
    } else {
        Box::new(std::fs::File::open(&args[1]).unwrap())
    };
    let mut reader = wlcap::Reader::new(BufReader::with_capacity(1 << 20, input)).unwrap();
    let mut enc: Option<Encoder> = None;
    let mut hash = xxhash_rust::xxh3::Xxh3Default::new();
    let (mut spent, mut tiles, mut bytes, mut frames) = (Duration::ZERO, 0u64, 0u64, 0u64);
    while let Some(frame) = reader.next().unwrap() {
        if frames == most {
            break;
        }
        frames += 1;
        let (w, h) = (usize::from(frame.size.0), usize::from(frame.size.1));
        let enc = enc.get_or_insert_with(|| Encoder::new(frame.size.0, Tables::protocol()));
        let stride = w * 4;
        let (cols, rows) = (w.div_ceil(TILE), h.div_ceil(TILE));
        let mut cells = vec![false; cols * rows];
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
        let began = Instant::now();
        for (i, _) in cells.iter().enumerate().filter(|(_, c)| **c) {
            let (col, row) = (i % cols, i / cols);
            let (x, y) = (col * TILE, row * TILE);
            let pass = enc.first_or_full((col as u16, row as u16), &pixels[y * stride + x * 4..], stride, TILE.min(w - x), TILE.min(h - y), stage, 1.25);
            hash.write(&pass.block);
            tiles += 1;
            bytes += pass.block.len() as u64;
        }
        spent += began.elapsed();
    }
    println!("{frames} frames {tiles} tiles {bytes} bytes {:.1} ms {:.2} us/tile hash {:016x}", spent.as_secs_f64() * 1e3, spent.as_secs_f64() * 1e6 / tiles as f64, hash.digest());
}
