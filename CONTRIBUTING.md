# Contributing

## Sign your commits (DCO)

Every commit must carry a `Signed-off-by` line:

```text
git commit -s -m "your message"
```

That line is the [Developer Certificate of Origin](https://developercertificate.org/) 1.1: you are
certifying that you wrote the change, or have the right to submit it, and that it may be
redistributed under this project's licence.

**Why this is required rather than encouraged.** A project can relicense its own future versions
freely only while it can account for who owns every line. The moment a contribution lands without a
clear provenance record, changing the licence later needs that contributor's consent — which is what
has made real-world relicensing efforts take years. The DCO keeps that option open at the cost of one
`-s` flag, and it is far cheaper to require from the first contribution than to retrofit.

Contributions are accepted under [Apache-2.0](LICENSE), the licence this repository is published
under.

## What belongs here

This repository is **generated output**: the descriptor, its schema, the golden vectors and a
manifest. The source is the contract definition in a separate repository, so a change to the
*contract* cannot be made here — a pull request editing `descriptor.json` by hand would be
overwritten by the next release.

What is useful here: corrections to prose, and — most of all — **a vector or a case this bundle does
not cover**. The published set exists because a single-frame oracle once certified a decoder that
read the wrong channel; if you find a frame shape it fails to pin, that is the contribution worth
making.
