"""Refuse to publish a crate tarball that leaks, misdeclares, or ships what it must not.

Every check here is a defect that was FOUND in the real 0.4.0 tarball by an audit run hours before
the first publish, and every one is unfixable afterwards: a crates.io version can never be deleted,
only yanked, and its files are served forever.

  * tests/ shipped, and `tests/vendor_neutral.rs` -- the guard that keeps the board vendor's name
    out of anything a customer reads -- EXEMPTS ITSELF from its own scan. Correct for an in-repo
    guard whose fixtures are the forbidden strings; catastrophic in a tarball, because CI stays
    green while the package publishes the manufacturer, the board model and a bench serial.
  * no LICENSE and no NOTICE, while `license` asserted Apache-2.0 over 8.2 MB of third-party
    binaries that statically embed Lua 5.3.1 (MIT), whose notice we are obliged to reproduce.
  * a specific instrument's serial number rendered in a public docs.rs comment.
  * a README telling readers the licence was "an open item" and the crate "not on crates.io".

    python3 tools/check_publishable.py kdi/rs/target/package/kdi-0.4.0.crate
"""
from __future__ import annotations
import pathlib, re, sys, tarfile

# Strings that must not appear in ANY shipped text file. Not a style list -- each one identifies a
# person, a place, a machine or a party that a customer has no business receiving.
FORBIDDEN = {
    "board serial":  re.compile(r"\b24[0-9]{5}[A-Z]{2,3}\b"),
    "site name":     re.compile(r"\b(hangzhou|maoming)\b", re.I),
    "vendor name":   re.compile(r"\b(opal\s*kelly|xem7310)\b", re.I),
    "private repo":  re.compile(r"\bkeyvast-fpga\b"),
}
# The ONE sanctioned exception: dlsym resolves these byte for byte and `strings` shows them anyway.
# White-labelling, not concealment -- and it is bounded to marked blocks in the driver binding.
CARVE_FILE = "src/usb3.rs"
CARVE_BEGIN, CARVE_END = "VENDOR-NAMES-BEGIN", "VENDOR-NAMES-END"
TEXT = (".rs", ".md", ".toml", ".txt", ".json")


def carved(text: str) -> set[int]:
    """1-based line numbers inside a VENDOR-NAMES block."""
    out, depth = set(), 0
    for n, line in enumerate(text.splitlines(), 1):
        if CARVE_BEGIN in line:
            depth += 1
        if depth:
            out.add(n)
        if CARVE_END in line:
            depth = max(0, depth - 1)
    return out


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    crate = pathlib.Path(sys.argv[1])
    if not crate.is_file():
        print(f"FAIL: {crate} does not exist -- run `cargo package` first")
        return 1

    # THE TARBALL MUST BE NEWER THAN THE SOURCE IT CLAIMS TO PACKAGE. Without this the gate happily
    # certifies a stale artifact: measured, a .crate built 24 minutes earlier by a container that
    # could not see `.git` was missing `.cargo_vcs_info.json` entirely, so the gate passed 6 files
    # while the 7 that would actually upload were never examined. A verdict on the wrong bytes is
    # worse than no verdict, because it is indistinguishable from a real one.
    root = crate.parent.parent.parent          # target/package/x.crate -> the crate directory
    newest, newest_f = 0.0, None
    for f in list(root.rglob("*.rs")) + list(root.glob("*.toml")) + list(root.glob("*")):
        if f.is_file() and "target" not in f.parts and f.stat().st_mtime > newest:
            newest, newest_f = f.stat().st_mtime, f
    if newest and crate.stat().st_mtime < newest:
        print(f"FAIL: {crate.name} is OLDER than {newest_f.name} -- it does not describe the "
              f"current source. Re-run `cargo package`.")
        return 1

    bad: list[str] = []
    with tarfile.open(crate) as tf:
        members = [m for m in tf.getmembers() if m.isfile()]
        names = {m.name.split("/", 1)[1] for m in members if "/" in m.name}

        # 1. the test tree must not ship
        shipped_tests = sorted(n for n in names if n.startswith("tests/"))
        if shipped_tests:
            bad.append(f"tests/ is packaged ({len(shipped_tests)} files, e.g. {shipped_tests[0]}) -- "
                       "vendor_neutral.rs exempts itself from its own scan, so this ships the very "
                       "names it exists to forbid")

        # 2. the licence obligations must ship, not merely be declared. LICENSE always; NOTICE only
        #    when there is third-party content to attribute -- a placeholder carrying nothing but
        #    our own code owes no attribution, and demanding an empty NOTICE would teach the next
        #    person that this gate is bureaucratic rather than load-bearing.
        vendored = any(n.startswith("vendor/") and n.endswith(".bin") for n in names)
        required = ("LICENSE", "NOTICE") if vendored else ("LICENSE",)
        for f in required:
            if f not in names:
                bad.append(f"{f} is not in the tarball, but the manifest declares a licence")

        # 3. NOTICE must actually carry the embedded-Lua attribution we are obliged to reproduce
        if "NOTICE" in names:
            notice = tf.extractfile(next(m for m in members if m.name.endswith("/NOTICE"))).read().decode("utf-8", "replace")
            if "Lua.org" not in notice:
                bad.append("NOTICE does not reproduce the Lua copyright notice, which the bundled "
                           "drivers statically embed and MIT requires")

        # 4. nothing that identifies a person, place, machine or party
        for m in members:
            rel = m.name.split("/", 1)[1] if "/" in m.name else m.name
            if not rel.endswith(TEXT):
                continue
            body = tf.extractfile(m).read().decode("utf-8", "replace")
            exempt = carved(body) if rel == CARVE_FILE else set()
            for label, pat in FORBIDDEN.items():
                for n, line in enumerate(body.splitlines(), 1):
                    if n in exempt or not pat.search(line):
                        continue
                    # our own repo path is a deliberate, masked citation
                    if label == "vendor name" and "frontpanel.py" in line.lower():
                        continue
                    bad.append(f"{rel}:{n} leaks a {label}: {line.strip()[:88]}")

        # 5. the licence EXPRESSION must cover what actually ships. `license` is the only
        #    machine-readable statement crates.io, docs.rs, cargo-deny, cargo-about and distro
        #    packagers ever read -- none of them parse NOTICE. A bare `Apache-2.0` over a package
        #    whose bytes are 96% third-party proprietary binaries makes every downstream SBOM
        #    classify those blobs as Apache-2.0 and drop the Lua MIT attribution, and per-version
        #    crates.io metadata is IMMUTABLE: a yank does not amend it.
        if vendored:
            cm = tf.extractfile(next(m for m in members if m.name.endswith("/Cargo.toml"))).read().decode("utf-8", "replace")
            lic = re.search(r'^license\s*=\s*"([^"]+)"', cm, re.M)
            expr = lic.group(1) if lic else ""
            if not expr:
                bad.append("no `license` in the packaged manifest while vendor binaries ship")
            else:
                if "LicenseRef" not in expr:
                    bad.append(f'license is "{expr}" but proprietary vendor binaries ship -- an SPDX '
                               "expression needs a LicenseRef- term for content with no SPDX id")
                if "MIT" not in expr:
                    bad.append(f'license is "{expr}" but the bundled drivers embed Lua 5.3.1 (MIT)')

        # 6. the README a customer reads must not contradict the manifest
        if "README.md" in names:
            rd = tf.extractfile(next(m for m in members if m.name.endswith("/README.md"))).read().decode("utf-8", "replace")
            for phrase, why in (("open item", "declares the licence unsettled"),
                                ("Not on crates.io", "says the crate is unpublished")):
                if phrase.lower() in rd.lower():
                    bad.append(f"README {why} -- it renders on the crates.io front page")
            for rel_link in re.findall(r"\]\((\.\./[^)]+)\)", rd):
                bad.append(f"README links outside the tarball ({rel_link}) -- 404 once rendered")

    if bad:
        print(f"NOT PUBLISHABLE -- {len(bad)} problem(s), each permanent once uploaded:\n")
        for b in bad:
            print(f"  * {b}")
        return 1
    # SAY WHAT WAS ACTUALLY CHECKED. A fixed success string is how a verdict comes to claim work it
    # did not do -- this project has already shipped one oracle that printed its pass reason on a
    # failing run. The NOTICE and licence-expression checks only apply when third-party bytes ship.
    did = [f"{len(names)} files", "tests not shipped", "LICENSE present"]
    did.append("NOTICE carries the Lua attribution" if vendored
               else "no third-party bytes, so no NOTICE owed")
    did.append("licence expression covers the vendored content" if vendored
               else "licence is ours alone")
    did += ["no identifying strings outside the sanctioned carve-out", "README consistent"]
    print(f"PUBLISHABLE: {crate.name} -- " + "; ".join(did))
    return 0


if __name__ == "__main__":
    sys.exit(main())
