# Contributing

**You are in the public repository for the Keyvast Device Interface.** It holds the contract
(`spec/`) and the source of the [`kdi`](https://crates.io/crates/kdi) crate (`rs/`). The instrument's
gateware, firmware and build system are developed elsewhere, in a private repository.

## What that means for a pull request

| you want to change | where it actually lives | what to do |
|---|---|---|
| `rs/**` — the Rust SDK | private repo; `rs/` here is a **one-way mirror**, overwritten on every release | open an issue, or a PR to show the diff — say what it fixes. The change lands upstream and appears here next release |
| `spec/**` — descriptor, schema, vectors, manifest | **generated** from a contract definition upstream | open an issue. A hand edit here is overwritten and never reaches a device |
| prose — this file, `README.md`, `CHANGELOG.md` | here | a normal pull request |

None of that is a brush-off: an issue against `rs/` or `spec/` is the most direct route to a fix, and
it will be answered. What cannot happen is your commit being merged into a mirror, because the next
sync would silently delete it — better to say so than to let you find out afterwards.

## The contribution we want most

**A frame the golden vectors do not cover.**

`spec/vectors/` exists because a contract is only as testable as its vectors, and this project has
already shipped the failure that proves it: the published bundle was once a single 64-byte
`adio_dig` frame, which has `rows == 1` — and at one row, a row-major and a lane-major decoder emit
*identical bytes*. A decoder that transposed `rhd_matrix`'s 35 rows against its lanes passed 100 % of
the published oracle while emitting plausible neural data at the wrong channel index.

So if you are implementing a host and you find a frame shape, an edge case or a malformed input the
bundle fails to pin — that is worth more to us than a code change. Open an issue with the bytes.

## Reporting something that looks wrong on a device

Include the `gateware_sha` your board reports (`Device::gateware_sha()`), the contract version from
the handshake, and the crate version. Those three identify exactly what you were running; without
them a report cannot be reproduced, and a result that does not name what it ran against cannot be
acted on.

## Sign your commits (DCO)

For the pull requests that can be merged here, every commit needs a `Signed-off-by` line:

```text
git commit -s -m "your message"
```

That is the [Developer Certificate of Origin](https://developercertificate.org/) 1.1: you certify
that you wrote the change, or have the right to submit it, and that it may be redistributed under
this project's licence, [Apache-2.0](LICENSE).
