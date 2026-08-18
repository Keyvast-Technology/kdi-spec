//! The USB3 device driver this crate ships, its provenance, and the staging that makes it loadable.
//!
//! **`--features bundled` compiles the target platform's driver into the artifact**, so a build
//! runs on a machine with no driver installed. There is one bitstream in this repository's
//! release process (`make all` / the GitHub release asset). This crate does not carry a second
//! copy: [`crate::Device::open_usb3_configured`] takes the image the caller already has.
//!
//! The bytes are here in the source either way, because Cargo packages a crate's whole source
//! directory. [`VENDORED`] is compiled UNCONDITIONALLY, feature or not. `tests/provenance.rs`
//! hashes the files on disk against that table, so swapping a blob without updating its record
//! fails the build's tests.
//!
//! Two things are deliberate and would be wrong to "fix":
//!
//! * **The files are `vendor/driver-<arch>-<os>.bin`, not anyone else's file names.** What a
//!   customer browsing the source or a package listing sees is ours. This is white-labelling, not
//!   concealment — see `usb3.rs`, which resolves the driver's exported symbols by `dlsym` and
//!   therefore contains their real names as plain string literals on purpose.
//! * **A platform with no vendored driver is a COMPILE ERROR**, named below. A silent fallback to
//!   "search the machine" would make `bundled` mean something different per platform, and the
//!   difference would only show up on a customer's machine.

/// Where one vendored blob came from, and what it must still be.
///
/// Provenance for a binary that ships inside a public crate: "some file that was on a PC" is not
/// good enough, and neither is a claim stronger than what was actually recorded — see the driver's
/// [`Vendored::note`].
#[derive(Debug)]
pub struct Vendored {
    /// Path within this crate, relative to its manifest directory.
    pub file: &'static str,
    /// Where the bytes came from.
    pub source: &'static str,
    /// Lowercase hex SHA-256 of the file, asserted by `tests/provenance.rs`.
    pub sha256: &'static str,
    /// Byte length, asserted by the same test.
    pub len: usize,
    /// What is NOT known about it. Read this before quoting the row above as a chain of custody.
    pub note: &'static str,
}

/// The one release every vendored driver was extracted from.
///
/// A single archived source is the point: the drivers are no longer "a file that happened to be on
/// a machine", they are an artifact of a release this organisation archives and can re-fetch, and
/// each row's SHA-256 says which bytes came out of it. That is what a blob inside a published
/// crate needs in order to be auditable by the person receiving it.
///
/// It does not name the board's maker, per the white-labelling rule that
/// `tests/vendor_neutral.rs` enforces — a customer meets our provenance, not theirs. The upstream
/// coordinates are not secret, merely not published here: they are recorded in this repository's
/// `the project's engineering notes`, which does not ship.
const RELEASE: &str = "host API release 6.0.0 (2026-07-21), archived in this organisation's \
                       private hardware-support repository";

/// Every binary blob in this crate's source, with its provenance. See [`Vendored`].
pub const VENDORED: &[Vendored] = &[
    // The four drivers all come from ONE release we control, and each row's sha256 is the file as
    // extracted from that release's tarball for its platform. The Windows row additionally records
    // that the copy running on the bench is byte-identical to it — which is what lets a bench
    // result stand for the vendored blob rather than merely resembling it.
    Vendored {
        file: "vendor/driver-x86_64-windows.bin",
        source: RELEASE,
        sha256: "51fd8539027e23600bfc6c9427e6f0a74f31f19046732caf39e850e5a09ec8f6",
        len: 2_117_704,
        note: "verified byte-identical to the copy installed on the instrument bench, so the \
               hardware results recorded against that machine are results for THIS blob",
    },
    // The release offers SEVERAL Linux builds and this is the oldest-glibc one ON PURPOSE, because
    // a glibc floor is a compatibility CEILING: a library runs on any system at or above the
    // version it was built against and on none below it. Measured, not assumed — the newest build
    // needs GLIBC_2.38, which excludes Ubuntu 22.04, Debian 12 and RHEL/Rocky 9, all current and
    // all plausible instrument hosts; this one needs 2.34 and clears every one of them at the same
    // size. Re-measure before taking a newer build: read the `GLIBC_*` tags out of the blob and
    // take the maximum.
    Vendored {
        file: "vendor/driver-x86_64-linux.bin",
        source: RELEASE,
        sha256: "4033682b24d877ffa00e4efb9448541a6edf13f347eba6ea286d460ff9ef25f6",
        len: 2_328_536,
        note: "needs glibc >= 2.34, libstdc++ >= 3.4.30 and libudev at run time; glibc-only, so a \
               musl system (Alpine) cannot load it. NOT exercised against a board — the bench \
               workstation is Windows",
    },
    // aarch64 Linux, for ARM acquisition hosts (single-board machines, ARM servers). Upstream ships
    // it as a Raspbian 12 build, but the LIBRARY's floor is what binds, not the distribution it was
    // built on: it needs glibc 2.34, the same as the x86_64 row above, so it reaches every ARM
    // distribution that one reaches on Intel rather than being narrowed to Debian 12 and newer.
    Vendored {
        file: "vendor/driver-aarch64-linux.bin",
        source: RELEASE,
        sha256: "ef2afb3a3660d107fe94b3d307614a6b8a2a3fa467a7761d7afdea483e03a3dd",
        len: 2_350_768,
        note: "needs glibc >= 2.34 and libudev at run time; glibc-only, so a musl system (Alpine) \
               cannot load it. NOT exercised against a board",
    },
    // macOS ships ONE fat library and this row is its arm64 slice, carved out so the target embeds
    // only its own architecture. The Intel slice was vendored too and is no longer: Apple stopped
    // selling Intel Macs, and every blob here is a redistribution obligation and an audit surface
    // shipped to EVERY consumer, not only to the platform that can use it. An Intel Mac can still
    // use this crate -- it loads a driver from disk; only `--features bundled` is unavailable.
    Vendored {
        file: "vendor/driver-aarch64-macos.bin",
        source: "the arm64 slice of the universal library in the release below",
        sha256: "a1a4a16ccb18d5a68066d31b5608a82419f9ba939d3b44c3d30e5248f49b6607",
        len: 2_088_080,
        note: "sliced out of the universal binary, not shipped separately upstream; NOT exercised \
               against a board",
    },
];

// ─────────────────────────────────────────────────────── the bytes, under `--features bundled`

#[cfg(feature = "bundled")]
pub(crate) use embed::staged_path;

#[cfg(feature = "bundled")]
mod embed {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static PART_SEQ: AtomicU64 = AtomicU64::new(0);

    /// The driver for the target platform.
    ///
    /// `include_bytes!`, so it is in the artifact's data, not read from disk at run time.
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    pub(crate) const DRIVER: &[u8] = include_bytes!("../vendor/driver-x86_64-windows.bin");
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    pub(crate) const DRIVER: &[u8] = include_bytes!("../vendor/driver-x86_64-linux.bin");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    pub(crate) const DRIVER: &[u8] = include_bytes!("../vendor/driver-aarch64-linux.bin");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) const DRIVER: &[u8] = include_bytes!("../vendor/driver-aarch64-macos.bin");

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    const DRIVER_FILE: &str = "vendor/driver-x86_64-windows.bin";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    const DRIVER_FILE: &str = "vendor/driver-x86_64-linux.bin";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    const DRIVER_FILE: &str = "vendor/driver-aarch64-linux.bin";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    const DRIVER_FILE: &str = "vendor/driver-aarch64-macos.bin";

    // A target with no vendored driver fails HERE, at build time, naming what is missing — rather
    // than building something that quietly behaves like a non-bundled build on that platform only.
    //
    // Vendoring another platform's library is: extract it from the release named in `VENDORED`,
    // drop it in `vendor/` under the same naming scheme, and add its `cfg` arm above plus its row
    // in `VENDORED`. Nothing else — `staged_path`'s unix hardening is written against every unix,
    // not against a target list.
    #[cfg(not(any(
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    compile_error!(
        "`bundled` has no vendored device driver for this target. Vendored: Windows x86_64, \
         Linux x86_64 and aarch64, and macOS aarch64. Build without the `bundled` feature and \
         supply the driver in a directory ($KDI_DRIVER_DIR), or vendor the library for this \
         target - see src/bundled.rs."
    );

    /// The staged file's name. NEUTRAL ON PURPOSE: this is what lands in a customer's temp
    /// directory, and it is the one name in the whole scheme that we get to choose.
    #[cfg(target_os = "windows")]
    const STAGED_NAME: &str = "kdi_driver.dll";
    #[cfg(target_os = "macos")]
    const STAGED_NAME: &str = "libkdi_driver.dylib";
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    const STAGED_NAME: &str = "libkdi_driver.so";

    /// Write `DRIVER` to a private directory under the system temp dir if it is not already there,
    /// and hand back the path to load.
    ///
    /// The directory name is the first 16 hex chars of the driver's recorded SHA-256, which is
    /// what makes a STALE COPY impossible to load: a build with a different driver stages a
    /// different directory, so an older version's file is never picked up, and nothing has to be
    /// cleaned up on upgrade. The file itself is written to a per-process, per-call temporary name
    /// and renamed into place, so two processes — or two threads in one process — racing their
    /// first use cannot see a half-written library. The loser of the race finds the winner's file
    /// already correct and uses it.
    ///
    /// Errors are ordinary [`io::Error`]s with the directory in the message: a read-only temp dir,
    /// or one the process may not write, is a real deployment condition and the caller has a way
    /// out (`$KDI_DRIVER_DIR`). A temp dir mounted `noexec` fails later, at `dlopen`, and `usb3.rs`
    /// reports it there.
    ///
    /// UNIX: the directory is created 0700 and then CHECKED, because on unix the temp dir is
    /// shared between users and this directory's name is a hash of public bytes — so another local
    /// user can compute it and pre-create it, and whatever library they leave inside is what this
    /// process would `dlopen`. [`trusted_dir`] is what closes that, and it runs BEFORE the
    /// already-staged shortcut below, which would otherwise hand back an attacker's file. Windows'
    /// temp dir is per-user, so the check is a no-op there.
    pub(crate) fn staged_path() -> io::Result<PathBuf> {
        let sha16 = &super::VENDORED
            .iter()
            .find(|v| v.file == DRIVER_FILE)
            .expect("VENDORED has a row for this platform's driver")
            .sha256[..16];
        let dir = std::env::temp_dir().join(format!("kdi-driver-{sha16}"));
        let path = dir.join(STAGED_NAME);
        create_dir_private(&dir)?;
        trusted_dir(&dir)?;
        if is_staged(&path) {
            return Ok(path);
        }
        // Pid plus a counter: two threads in one process share a pid, and both writes must finish
        // before either is renamed.
        let n = PART_SEQ.fetch_add(1, Ordering::Relaxed);
        let part = dir.join(format!("{STAGED_NAME}.{}.{n}.part", std::process::id()));
        fs::write(&part, DRIVER)?;
        #[cfg(unix)]
        fs::set_permissions(
            &part,
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
        )?;
        // A rename onto an existing file fails on Windows, and "it already exists" is precisely the
        // race being handled — so the rename failing is only an error if what is there is not the
        // library we wanted.
        if let Err(e) = fs::rename(&part, &path) {
            let _ = fs::remove_file(&part);
            if !is_staged(&path) {
                return Err(io::Error::new(
                    e.kind(),
                    format!(
                        "could not stage the device driver at {}: {e}",
                        path.display()
                    ),
                ));
            }
        }
        Ok(path)
    }

    /// Create the staging directory, 0700 on unix so that nobody else can put a file in it.
    ///
    /// Already-exists is success — the reuse across processes is the whole point of the hashed
    /// name — and [`trusted_dir`] is what decides whether an existing one may be used.
    fn create_dir_private(dir: &std::path::Path) -> io::Result<()> {
        let mut b = fs::DirBuilder::new();
        b.recursive(true);
        #[cfg(unix)]
        <fs::DirBuilder as std::os::unix::fs::DirBuilderExt>::mode(&mut b, 0o700);
        b.create(dir)
    }

    /// Why a unix staging directory must not be used. Split out so the 0755-other-user case can
    /// be asserted without being able to chown.
    #[cfg(any(unix, test))]
    fn trust_reason(is_dir: bool, mode: u32, uid: u32, euid: u32) -> Option<&'static str> {
        if !is_dir {
            return Some("not a directory");
        }
        if mode & 0o022 != 0 {
            return Some("writable by others");
        }
        if uid != euid {
            return Some("owned by another user");
        }
        None
    }

    /// Refuse a staging directory another local user could have put a file into.
    ///
    /// Group/other-writable is one hole: we could write, they could write. A 0755 directory they
    /// own is the other: we can traverse and `dlopen`, they planted the file. Permissions alone
    /// are not decisive — the owner must be us too.
    ///
    /// `symlink_metadata`, not `metadata`: a symlink planted at this path would otherwise be
    /// followed and the real directory's permissions checked instead of the attacker's.
    #[cfg(unix)]
    fn trusted_dir(dir: &std::path::Path) -> io::Result<()> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let md = fs::symlink_metadata(dir)?;
        let why = trust_reason(
            md.file_type().is_dir(),
            md.permissions().mode(),
            md.uid(),
            euid(),
        );
        if let Some(why) = why {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "refusing to load the device driver from {}: it is {why} and so another user \
                     on this machine could choose what is loaded. Remove it, or point \
                     $KDI_DRIVER_DIR at a directory you control.",
                    dir.display(),
                ),
            ));
        }
        Ok(())
    }

    #[cfg(unix)]
    fn euid() -> u32 {
        extern "C" {
            fn geteuid() -> u32;
        }
        // SAFETY: geteuid is a POSIX libc call with no preconditions.
        unsafe { geteuid() }
    }

    /// Windows' temp directory is per-user, so there is nothing to check.
    #[cfg(not(unix))]
    fn trusted_dir(_dir: &std::path::Path) -> io::Result<()> {
        Ok(())
    }

    /// Length, not contents: the file is 2 MB and this runs on the way to every device open. The
    /// name it sits under is already a hash of the bytes, and [`trusted_dir`] has already required
    /// that we own a 0700 directory, so length is the second half of a check whose first half is
    /// the directory.
    fn is_staged(path: &std::path::Path) -> bool {
        fs::metadata(path).is_ok_and(|m| m.len() == DRIVER.len() as u64)
    }

    /// The staging is a file-system dance with a race in it, and this is the check that it works:
    /// stage twice, get the same path, and find the whole library there both times. The second call
    /// is the one that exercises "already staged" — the branch that would otherwise only be hit in
    /// production.
    #[cfg(test)]
    mod tests {
        #[test]
        fn staging_is_idempotent_and_complete() {
            let a = super::staged_path().expect("stage the driver");
            let b = super::staged_path().expect("stage it again");
            assert_eq!(a, b);
            assert_eq!(
                std::fs::read(&b).expect("read it back").len(),
                super::DRIVER.len()
            );
            // The name a customer meets in their temp directory is ours, not anyone else's.
            let name = b.file_name().unwrap().to_string_lossy().to_lowercase();
            assert!(name.contains("kdi"), "staged as {name}");
            let dir = a.parent().unwrap().file_name().unwrap().to_string_lossy();
            assert!(
                dir.starts_with("kdi-driver-") && dir.len() == "kdi-driver-".len() + 16,
                "content-addressed dir, got {dir}"
            );
        }

        #[test]
        fn trust_reason_catches_mode_and_owner() {
            assert_eq!(
                super::trust_reason(false, 0o700, 1, 1),
                Some("not a directory")
            );
            assert_eq!(
                super::trust_reason(true, 0o777, 1, 1),
                Some("writable by others")
            );
            assert_eq!(
                super::trust_reason(true, 0o755, 2, 1),
                Some("owned by another user")
            );
            assert_eq!(super::trust_reason(true, 0o755, 1, 1), None);
            assert_eq!(super::trust_reason(true, 0o700, 1, 1), None);
        }

        /// The attack the unix hardening exists for: another local user gets there first and leaves
        /// a directory anyone can write to. Loading out of it would mean loading their library.
        ///
        /// Asserted on a directory built here rather than by racing the real one, because the real
        /// path is a fixed hash and a test must not depend on winning a race against itself.
        #[cfg(unix)]
        #[test]
        fn a_world_writable_staging_directory_is_refused() {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!("kdi-trust-test-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("make the directory");

            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .expect("lock it down");
            super::trusted_dir(&dir).expect("a private directory is fine");

            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o777))
                .expect("open it up");
            let err = super::trusted_dir(&dir).expect_err("world-writable must be refused");
            assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
            // The message has to tell the user what to do about it, not just that it failed.
            assert!(err.to_string().contains("KDI_DRIVER_DIR"), "{err}");

            std::fs::remove_dir_all(&dir).ok();
        }
    }
}
