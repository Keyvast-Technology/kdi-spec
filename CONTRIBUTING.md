# Contributing

**This is a distribution repository, not a development one.** `spec/` is generated and `rs/` is a
one-way mirror — a commit against either is deleted by the next release sync, so please open an
issue instead of a pull request. Prose (`README`, `CHANGELOG`, this file) is normal.

That is not a brush-off. An issue is the *faster* route: it goes to the repository where the fix can
actually be made, and it will be answered.

**Most useful of all: a frame the golden vectors do not cover.** `spec/vectors/` was once a single
64-byte `adio_dig` frame — and at `rows == 1` a row-major and a lane-major decoder emit *identical
bytes*, so a decoder transposing `rhd_matrix`'s 35 rows against its lanes passed 100 % of it while
emitting plausible neural data at the wrong channel. If you are writing a host and find a shape the
bundle fails to pin, send the bytes.

**Reporting something wrong on a device?** Include `Device::gateware_sha()`, the contract version
from the handshake, and the crate version — those three identify what you were running, and a result
that does not name what it ran against cannot be acted on.

Merged prose commits need a `Signed-off-by` line (`git commit -s`) — the
[DCO](https://developercertificate.org/) 1.1, under [Apache-2.0](LICENSE).
