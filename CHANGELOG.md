# Changelog

All notable, **wire-visible** changes to the Keyvast Device Interface. One entry
per contract version.

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
