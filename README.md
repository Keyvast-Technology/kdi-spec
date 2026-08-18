# Keyvast Device Interface (KDI) — the published contract

This repository is the **public contract** for the Keyvast acquisition instrument:
the machine-readable descriptor, its JSON Schema, golden vectors, and a manifest —
everything a host needs to talk to the device, in any language, over any transport.

> **Status: v0.4.0 — format 2 is emitted by shipping gateware and verified on hardware.**
> The device emits format-2 frames on two independent streams and advertises the `clean_frame`
> capability, so a host that requires it will bind. The data plane is hardware-verified: ~6,600
> probe-runs across sample rates and lane masks, bounded and free-running, with zero failures —
> which bounds the failure rate under 0.045% at 95% confidence. The control plane, the command
> channel and the flash path are likewise exercised on silicon, including their failure modes.
>
> **One thing is NOT hardware-verified, and you should know which:** the `rhd_matrix` ROW ORDER.
> Checking it on a bench needs a *driven* input — with floating inputs both the amplifier rows and
> the slow aux rows are noise and the comparison decides nothing. The authority for row order is a
> golden chip-model simulation that drives known values through a modelled headstage. The order
> published here is that model's, and it is the one the gateware is built against.
>
> This is generated output; the source lives in a separate repo.

## What's here

| File | Purpose |
|------|---------|
| `descriptor.json` | **the contract** — device identity, capabilities, the two register drawers, the sample-stream layout, and the public command set. |
| `schema.json` | JSON Schema (draft 2020-12) — validate a descriptor without our code. |
| `manifest.json` | sha256 of every artifact + contract/build identity + the conformance result. The only place hashes live. |
| `vectors/*.bin` + `vectors/golden_frame.json` | **six** frames and their expected decodes, a streaming case, **and 20 negative frames each with the exact reason token it must be rejected with** — so a decoder is testable with no hardware, in both directions. The set is deliberately wider than one case: at `rows == 1` a row-major and a lane-major decoder emit IDENTICAL bytes, so a single-frame oracle certifies a decoder that transposes `rhd_matrix`'s 35 rows against its lanes and emits plausible data at the wrong channel. `golden_wide` exists for the same reason in the other direction: it uses descriptor strides and element widths no current device emits, so a decoder that compiles those in is caught before a future device meets it. |
| `CHANGELOG.md` | one entry per contract version; wire-visible changes only. |

## How to consume it

A host binds to the device by **identity** (serial/board_id), reads the descriptor,
and speaks the contract via three primitives — `reg_read/write(name)`,
`stream(name)`, `message(request) → response` — resolving every name through the
descriptor's `bindings` block. It never hardcodes a physical endpoint.

1. Read `descriptor.json` (validate against `schema.json`).
2. Handshake: check `contract_version` major, poll `contract_ready`, require the
   capability bits you need.
3. Send typed commands from `commands`; read `streams.samples` and decode with
   `bytes_per_frame` (never a hand-rolled formula).

The command set is **command-first**; only two tiny register "drawers" (pre-boot
identity/caps + a CPU-bypass fast-path) are raw registers. Private
(service/factory) commands are **not** in this descriptor by construction.

## Versioning

`kdi` (major.minor) is the contract; a host checks the **major** and refuses on
mismatch, then branches on **capability bits**, never on a version range. See
`CHANGELOG.md`.
