# Keyvast Device Interface (KDI) — the published contract

This repository is the **public contract** for the Keyvast acquisition instrument:
the machine-readable descriptor, its JSON Schema, golden vectors, and a manifest —
everything a host needs to talk to the device, in any language, over any transport.

> **Status: v0.1.0 — frame format 2 pinned, NOT YET EMITTED BY GATEWARE.**
> The contract, its codec and its vectors are complete and proven in software, and the
> control plane is hardware-verified on the bench. The **data plane is not**: no shipping
> bitstream emits format 2 yet, and the device correspondingly leaves the `clean_frame`
> capability bit CLEAR — so a host that requires the clean frame will correctly refuse to
> bind to today's hardware. The format is published now, ahead of the emitter, precisely so
> that it is fixed before anything binds to it. This is generated output; the source lives
> in a separate repo.

## What's here

| File | Purpose |
|------|---------|
| `descriptor.json` | **the contract** — device identity, capabilities, the two register drawers, the sample-stream layout, and the public command set. |
| `schema.json` | JSON Schema (draft 2020-12) — validate a descriptor without our code. |
| `manifest.json` | sha256 of every artifact + contract/build identity + the conformance result. The only place hashes live. |
| `vectors/golden_frame.{bin,json}` | a real frame, its expected decode, **and 7 negative frames with the exact reason token each must be rejected with** — so a decoder is testable with no hardware, in both directions. |
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
