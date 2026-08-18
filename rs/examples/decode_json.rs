//! Decode a KDI blob and print what THIS codec saw, as JSON — the Rust half of the
//! cross-implementation differential harness (the Python reference host).
//!
//! WHY AN EXAMPLE RATHER THAN A TEST. The comparator has to run both decoders over bytes that
//! exist only for the length of one seeded run, so the Rust side has to be a program a Python
//! process can hand bytes to; `cargo test` cannot be that. And it calls `kdi::codec` rather than
//! in `kdi` because the claim under test is the NORMATIVE decoder's — routing it through the host
//! crate would add a layer (this crate's stream layer) that the Python reference host has no counterpart for,
//! so a disagreement could no longer be attributed.
//!
//! Usage:
//!   decode_json < blob          one JSON object on stdout
//!   decode_json FILE [FILE..]   one JSON object per line, in argv order
//!   decode_json DIR             every `*.bin` in DIR, sorted — what difftest.py passes
//!
//! THIS PRINTS A DESCRIPTION, NEVER A VERDICT: every header field, every descriptor, every element
//! value, the three walk counters, the tail length, and for a rejected frame the contract's exact
//! token plus the byte offset it was found at. Any judgement made here (skipping a field, deciding
//! two shapes are "equivalent") is a judgement the diff can no longer see, which is how a
//! differential harness turns into a harness that agrees with itself.
//!
//! One asymmetry is structural and belongs in the comparator, not here: [`Walk`] yields a rejected
//! frame as ONE `Err` item and carries on, while `kdi.frame.walk` raises and loses everything it
//! had already decoded (the Python reference host; the judgement is argued at
//! `kdi/rs/kdi/src/codec/mod.rs:602-609`). So the items array can hold frames AFTER a reject;
//! difftest.py compares the first reject and says so.

use kdi::codec::{check_run_announcements, Frame, Header, Walk};
use serde_json::{json, Value};
use std::io::{Read, Write};

/// Everything one frame carries, flat enough that a field-by-field diff can name what differs.
/// `values` is materialised through [`kdi::codec::Section::row`] rather than read out of `body()`:
/// the element ACCESSOR is what row-major order is a property of, and dumping the raw body would
/// compare the bytes Python encoded with the bytes Python encoded (contract.yaml:250-256).
fn frame_value(f: &Frame) -> Value {
    let h = f.header();
    let sections: Vec<Value> = f
        .sections()
        .map(|s| {
            let d = s.desc();
            // `row` is Some for every r < rows, and `section_words` was verified against the
            // geometry at parse time, so neither unwrap can fire on a frame that parsed.
            let values: Vec<Vec<u64>> = (0..d.rows)
                .map(|r| s.row(r).expect("r < rows").collect())
                .collect();
            json!({
                "kind": d.kind,
                "lane_ids": s.lane_ids().collect::<Vec<u16>>(),
                "rows": d.rows,
                "element_bits": d.element_bits,
                "section_words": d.section_words,
                "tick_num": d.tick_num,
                "tick_den": d.tick_den,
                "values": values,
            })
        })
        .collect();
    json!({
        "timestamp": h.timestamp,
        "flags": h.flags.0,
        "run_id": h.run_id,
        "layout": h.layout,
        "frame_words": h.frame_words,
        "hdr_words": h.hdr_words,
        "n_sections": h.n_sections,
        "contract_rev": h.contract_rev,
        "desc_words": h.desc_words,
        "sections": sections,
    })
}

fn decode_value(path: &str, blob: &[u8]) -> Value {
    let mut walk = Walk::new(blob);
    let mut items: Vec<Value> = Vec::new();
    let mut headers: Vec<Header> = Vec::new();
    for item in walk.by_ref() {
        match item {
            Ok(f) => {
                headers.push(*f.header());
                items.push(json!({ "frame": frame_value(&f) }));
            }
            // The offset is blob-relative and points at the FIELD that failed. Python's `walk`
            // raises a `FrameError` carrying only the token (`kdi/frame.py:57-62`), so the
            // comparator can diff the token and not this — it is printed for the human replaying.
            Err(e) => items.push(json!({"reject": {
                "reason": e.reason.token(),
                "offset": e.offset,
            }})),
        }
    }
    let c = walk.counters();
    json!({
        "path": path,
        "items": items,
        "counters": {
            "resync_bytes": c.resync_bytes,
            "unknown_kind": c.unknown_kind,
            "format_skipped": c.format_skipped,
        },
        "tail_bytes": walk.tail().len(),
        // The one CROSS-FRAME rule. Python runs it inside `walk` and raises; here it is a separate
        // call over the accepted headers so a decoder's verdict cannot depend on chunk size
        // (`kdi/rs/kdi/src/codec/mod.rs:719-728`). Reported as a token so the two are comparable.
        "run_announcements": match check_run_announcements(&headers) {
            Ok(()) => "ok",
            Err(r) => r.token(),
        },
    })
}

fn emit(out: &mut impl Write, path: &std::path::Path) -> std::io::Result<()> {
    let blob = std::fs::read(path)?;
    writeln!(out, "{}", decode_value(&path.display().to_string(), &blob))
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    if args.is_empty() {
        let mut blob = Vec::new();
        std::io::stdin().read_to_end(&mut blob)?;
        writeln!(out, "{}", decode_value("-", &blob))?;
        return out.flush();
    }
    for a in &args {
        let p = std::path::Path::new(a);
        // A DIRECTORY is one argument for a whole corpus, and that is not a convenience: the
        // harness runs this program inside a container through `sh -c`, where several hundred
        // argv paths have to survive a nested quoting layer. One sorted directory does not.
        if p.is_dir() {
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(p)?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.extension().is_some_and(|x| x == "bin"))
                .collect();
            files.sort();
            for f in &files {
                emit(&mut out, f)?;
            }
        } else {
            emit(&mut out, p)?;
        }
    }
    out.flush()
}
