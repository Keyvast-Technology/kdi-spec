# Changelog

All notable, **wire-visible** changes to the Keyvast Device Interface. One entry
per contract version.

## v0.0.2 — 2026-07-25

**The typed command protocol is live on hardware.** The device now answers framed
request/response commands over the control channel (capability `command_protocol`),
verified on a real XEM7310: 10/10 cases including every negative path.

- Commands published: `sys.hello`, `power.status`, `power.up`, `adio.mode`, `adio.adc`.
  All five are implemented in the shipping firmware and hardware-verified.
- `power.status` returns `{present, reverify}` (was `{rail, present}`): `present` is the
  module mask from the last power-sequence pass, which is the only meaningful source —
  a raw detect read stops reflecting presence once those bits drive the DCDC enables.
- `adio.adc` gained `valid[]` alongside `codes[]`: a code whose valid bit is clear is
  meaningless, so the host is told rather than left to guess.
- Arguments are **range-checked, never clamped** — out-of-range or malformed args are
  refused with `bad_args` (the human console clamps; a machine caller gets told).
- Removed (not implemented, so not published — publishing a command the device answers
  `unknown_cmd` to is a contract lie): `adio.dacsweep`. Also withheld from the internal
  set: `dbg.peek` (an unguarded raw read can DECERR-halt the control CPU; it needs a
  mapped-page allowlist first) and `eeprom.write` (identity-rewriting; needs the factory
  unlock + confirm-echo).
- Device capabilities now report `command_protocol`; `caps` reads `0x7e`.

Pre-1.0 note: `kdi` stays `0.1` — the register/frame surface is unchanged. Before 1.0 the
command set may still change; after 1.0 these would be major/minor bumps under §13 of the
design document.

## v0.0.1 — 2026-07-24

- First published KDI descriptor (contract `kdi 0.1`), software reference,
  pre-bitstream.
- Command-first contract: 6 public commands (`sys.hello`, `power.status`,
  `power.up`, `adio.mode`, `adio.adc`, `adio.dacsweep`); private service/factory
  commands withheld by construction.
- Two register drawers: identity/caps/heartbeat/`contract_ready`, and a
  CPU-bypass fast-path (`run`/`occupancy`/`overflow`/`frame_counter`/`data_ready`).
- One stream (`samples`), self-describing header, descriptor-driven frame size.
- Capabilities: clean_frame, command_protocol, ddr3, adio, grounding, ttl_in,
  slot_health, field_update.
- Conformance: 11/11 checks pass against the software device (see `manifest.json`).
