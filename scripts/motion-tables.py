"""Markdown tables from the logs of motion-bench and walk-sim, a JSON object a
line (JSONL):

    motion-bench --quality 90 CAPTURE... > q90-held.jsonl
    motion-bench --quality 90 --walk-at 50 CAPTURE... > q90-walk50.jsonl
    motion-bench check --quality 90 CAPTURE... > check.jsonl
    walk-sim CAPTURE --link 10 > window-10.jsonl
    uv run --python 3.13 python -I scripts/motion-tables.py LOG.jsonl...

Each log is named by its file name, less `.jsonl`, and is a section of its
own: a bench's a row per capture per coder, as the bench measured it; a
check's a row per capture; walk-sim's a row per run, its lines gathered from
every log given. Given more than one bench's log, the tiles of each capture
come last in one table, a column per log, and so does `WLSV`, a column per
chroma and ceiling it was coded at: what each cost the processor, the link,
and where the picture ended.
"""
import json
import math
import sys
from pathlib import Path


def read(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def db(mse):
    return "exact" if mse == 0 else f"{10 * math.log10(255 * 255 / mse):.1f} dB"


def mb(n):
    return n / 1e6


def bench_section(name, rows):
    out = [f"## {name}", "", "| capture | coder | frames | seconds | CPU ms | ms/frame | p95 ms/frame | MB | Mbit/s | while it ran | frames exact | ends at | detail |", "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|---|"]
    for r in rows:
        coder = f"WLSV {r['chroma'][0]}:{r['chroma'][1]}:{r['chroma'][2]}" if r["coder"] == "wlsv" else "tiles"
        rate = f"{r['bytes'] * 8 / r['seconds'] / 1e6:.1f}" if r["seconds"] > 0 else "–"
        exact = 100 * r["exact_frames"] / r["frames"] if r["frames"] else 0
        ends = "exact" if r["end_off"] == 0 else f"{r['end_off']} px off, {db(r['end_mse'])}"
        if r["coder"] == "wlsv":
            detail = f"{r['keyframes']} keyframes" + (f", settled at {r['quality']} for {r['tail_cpu_ms']:.1f} ms" if r["tail_cpu_ms"] > 0 else "")
        else:
            detail = f"PNG {mb(r['png']):.2f} MB, WebP {mb(r['webp']):.2f} MB, H.264 {mb(r['video']):.2f} MB in {r['streams']} streams; {'settled' if r['settled'] else 'NOT settled'} in {r['tail_s']:.2f} s after, {r['tail_cpu_ms']:.1f} ms"
        out.append(f"| {r['capture']} | {coder} | {r['frames']} | {r['seconds']:.1f} | {r['cpu_ms']:.0f} | {r['frame_ms']:.2f} | {r['p95_ms']:.2f} | {mb(r['bytes']):.2f} | {rate} | {db(r['held_mse'])} | {exact:.0f}% | {ends} | {detail} |")
    return out


def check_section(name, rows):
    out = [f"## {name}", "", "| capture | settled in | 5 s quiet | 10 redraws | ends at | |", "|---|---:|---:|---:|---|---|"]
    for r in rows:
        settled = f"{r['settled_s']:.2f} s" if r["settled"] else "never"
        ends = "exact" if r["end_off"] == 0 else f"{r['end_off']} px off, {db(r['end_mse'])}"
        out.append(f"| {r['capture']} | {settled} | {r['quiet_bytes']} B | {r['redraw_bytes']} B | {ends} | {'pass' if r['pass'] else '**FAIL**'} |")
    return out


def walk_section(rows):
    out = ["## walk-sim", "", "| capture | link Mbit/s | rtt | ceiling | walk | frames/s | motion q mean | p10 | shown mean ms | p50 | p95 | max | delivery p50 ms | p95 | Mbit/s | moves | down |", "|---|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for r in rows:
        link = ", ".join(f"{s['mbps']:g}" + (f" from {s['from_s']:g} s" if s["from_s"] else "") for s in r["link"])
        out.append(f"| {r['capture']} | {link} | {r['rtt_ms']:g} | {r['ceiling']} | {'held' if r['held'] else 'walked'} | {r['fps']:.1f} | {r['motion_q_mean']:.1f} | {r['motion_q_p10']:.0f} | {r['shown_mean_ms']:.0f} | {r['shown_p50_ms']:.0f} | {r['shown_p95_ms']:.0f} | {r['shown_max_ms']:.0f} | {r['delivery_p50_ms']:.0f} | {r['delivery_p95_ms']:.0f} | {r['mbps']:.2f} | {r['moves']} | {r['downs']} |")
    return out


def cell(r):
    return f"{r['cpu_ms']:.0f} ms · {mb(r['bytes']):.2f} MB · {'exact' if r['end_off'] == 0 else db(r['end_mse'])}"


def across(title, columns):
    """One table, a row per capture and a column per (name, {capture: row})."""
    captures = []
    for _, by in columns:
        captures += [c for c in by if c not in captures]
    out = [f"## {title}", "", "Each cell is the CPU time, the bytes and where the picture ended.", "", "| capture | " + " | ".join(n for n, _ in columns) + " |", "|---|" + "---|" * len(columns)]
    for c in captures:
        out.append(f"| {c} | " + " | ".join(cell(by[c]) if c in by else "–" for _, by in columns) + " |")
    return out


def main(paths):
    if not paths:
        sys.exit(__doc__)
    sections, benches, walks = [], [], []
    for path in paths:
        name = Path(path).name.removesuffix(".jsonl")
        rows = read(path)
        if not rows:
            continue
        if "link" in rows[0]:
            walks += rows
        elif "pass" in rows[0]:
            sections.append(check_section(name, rows))
        else:
            sections.append(bench_section(name, rows))
            benches.append((name, rows))
    if walks:
        sections.append(walk_section(walks))
    if len(benches) > 1:
        tiles = [(name, {r["capture"]: r for r in rows if r["coder"] == "tiles"}) for name, rows in benches]
        sections.append(across("The tiles, log by log", tiles))
        wlsv, seen = [], set()
        for _, rows in benches:
            for r in rows:
                key = (r["chroma"], r["quality"])
                if r["coder"] == "wlsv" and key not in seen:
                    seen.add(key)
                    wlsv.append((f"{r['chroma'][0]}:{r['chroma'][1]}:{r['chroma'][2]} q{r['quality']}", {x["capture"]: x for x in rows if x["coder"] == "wlsv"}))
        sections.append(across("WLSV, by chroma and ceiling", wlsv))
    print("\n\n".join("\n".join(s) for s in sections))


if __name__ == "__main__":
    main(sys.argv[1:])
