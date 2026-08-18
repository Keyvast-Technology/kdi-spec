//! The USB3 binding (`bindings.usb3`, kdi/contract.yaml:693-803), over the board vendor's C API
//! loaded at run time with `libloading`.
//!
//! **THIS MODULE IS THE ONLY PLACE IN THE CRATE THAT MAY NAME THE BOARD VENDOR**, and only inside
//! the two marked carve-outs below — the driver's file name and its exported symbols, both of which
//! are strings the loader must match exactly. `tests/vendor_neutral.rs`
//! enforces that: no public item, doc comment, error message, README or CHANGELOG entry anywhere in
//! this crate may mention it, because a customer reads those and a customer is buying a Keyvast
//! instrument. The same rule the Python
//! reference states in the Python reference host's header, applied here.
//!
//! Those literals are NOT concealment and must not be turned into one. `strings` on any binary
//! built from this crate shows every symbol name below, because `dlsym` resolves them from these
//! bytes; obfuscating, encrypting or splitting them would buy nothing against a reverse engineer
//! and would cost the one thing the plain form gives — a load failure that says which symbol was
//! missing. See the comment on the symbol block itself.
//!
//! **CI compiles this but cannot run it against a driver or board.** The board is remote
//!, so the failure this file is designed against is a WRONG GUESS
//! AT A C SYMBOL. Every entry point is therefore resolved BY NAME at load time and the miss is
//! reported as `Error::Sdk("<symbol>")` on the bench — not as a link failure, and never as a
//! silent no-op.
//!
//! That error path earned its keep: the original symbol list WAS a guess from the C API's
//! documented shape, and almost none of it existed in the shipped library. The names below are now
//! taken from its own export table and their arities from its disassembly (see `Fp`), and the
//! handshake was read back against the reference host on a real board — same contract version,
//! caps and gateware sha. Do not "tidy" a name here to look more like the published C API.
//!
//! The driver is optional and off by default: `libloading` is the only dependency that can reach
//! outside the process, and containing it is the point of the feature gate
//!.

use std::ffi::{c_char, c_int, c_long, c_uchar, c_ulong, c_void, CStr, CString};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use libloading::Library;
use serde_json::{json, Value};

use crate::{
    io_err, reg, reply_from, stream_regs, Addr, DeviceInfo, Error, HostErr, RegBind, Reply, Stream,
    TransportKind, RESP_LEN_DIGITS, RESP_SENTINEL, USB3_MSG, USB3_READ_ALIGNMENT, USB3_STREAM,
};

/// Which driver call failed, or which symbol was not there.
///
/// A symbol miss carries the C name it looked for, because that is the only string that identifies
/// it; every other message here names the operation, not the vendor's entry point.
#[derive(Debug)]
pub struct SdkErr(pub String);

impl fmt::Display for SdkErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device driver: {}", self.0)
    }
}

fn sdk(msg: impl Into<String>) -> Error {
    Error::Sdk(SdkErr(msg.into()))
}

/// The environment override for the driver's location — a path list in the platform's `PATH`
/// syntax, searched before the embedded copy. It is named in the load failure, so it is part of
/// this crate's surface and not an implementation detail.
///
/// It replaced `KDI_FP_DIR`, which is NOT read any more: the old name is the vendor's initials and
/// keeping it alive would have meant documenting it. The Python reference still uses that name for
/// its own loader — the two are separate variables now, and a bench that sets both is fine.
const DRIVER_DIR_ENV: &str = "KDI_DRIVER_DIR";

type Handle = *mut c_void;

/// The driver's "no error" code. Every negative status is a failure; a positive one has never been
/// observed.
const OK_NO_ERROR: c_int = 0;

/// The entry points, **measured from the shipped driver library** rather than copied from a header
/// — there is no C header on the bench. The names are in the symbol block in `load` and come from
/// the library's own export table; arities and argument widths come from disassembling each entry
/// point, and the call ORDER from the Python reference (the Python reference host), which drives
/// this same library through SWIG.
///
/// Two shapes here are not what the C API's documented look suggests, and both were silent traps:
///
/// * **There is no open-by-serial and no device-count entry point on the board object.** Opening
///   goes through a separate devices manager, exactly as the reference's
///   `<manager>().Open(serial)` does.
/// * **Wires, triggers and pipes are not on the board object either.** They live on a separate
///   "classic data port" handle, mirroring the reference's `dev.GetFPGADataPortClassic()`; every
///   one of the calls below `get_data_port` takes THAT handle.
///
/// `c_long` is deliberate for the pipe lengths: it is 32-bit under the Windows ABI, and the
/// implementations sign-test the result as a 32-bit value, so a 64-bit declaration would misread
/// every short read.
struct Fp {
    /// `(realm, out) -> handle`. The realm is `strlen`'d with NO null check, so `""` is passed —
    /// the C++ default. The second argument IS null-checked, so `null_mut()` is safe there.
    devices_construct: unsafe extern "C" fn(*const c_char, *mut c_void) -> Handle,
    devices_destruct: unsafe extern "C" fn(Handle),
    devices_get_count: unsafe extern "C" fn(Handle) -> c_int,
    /// Copies at most 10 characters and NUL-terminates at offset 10, so the buffer needs 11 bytes.
    devices_get_serial: unsafe extern "C" fn(Handle, c_int, *mut c_char),
    /// Returns an OWNED front-panel handle, released out of a `shared_ptr` by the wrapper —
    /// the `destruct` entry point below is its deallocator. NULL is returned when nothing opened.
    devices_open: unsafe extern "C" fn(Handle, *const c_char) -> Handle,
    destruct: unsafe extern "C" fn(Handle),
    close: unsafe extern "C" fn(Handle),
    /// `(handle, image, length) -> status`. Takes the FRONT-PANEL handle, and is called BEFORE
    /// `get_data_port` — the port is a view of a configured device, and the reference tool orders
    /// it the same way (`tools/rhd_term.py:242-245`). The length is the driver's `unsigned long`,
    /// which is 32-bit under the Windows ABI, so `c_ulong` is the width measured on the
    /// hardware-tested target.
    /// OPTIONAL, and resolved lazily for a reason. Every other entry here is required to talk to
    /// a board at all, so a missing one is a fatal, honest failure. This one is needed ONLY to
    /// configure — so binding it eagerly would make `open_usb3`, which never configures and works
    /// today, start failing on any driver build that lacks the symbol. A capability the caller may
    /// not use must not be a precondition for the calls they do use.
    configure_from_memory: Option<unsafe extern "C" fn(Handle, *const c_uchar, c_ulong) -> c_int>,
    /// **The port comes back through an OUT-PARAMETER, not in the return register** — reading it
    /// as a return value yields whatever the inner call happened to leave behind. The port is
    /// borrowed from the front panel and must not be freed: the DLL exports no destructor for it.
    get_data_port: unsafe extern "C" fn(Handle, *mut Handle) -> c_int,
    // Everything below takes the DATA PORT handle, never the front-panel one.
    update_wire_outs: unsafe extern "C" fn(Handle) -> c_int,
    get_wire_out_value: unsafe extern "C" fn(Handle, c_int) -> u32,
    set_wire_in_value: unsafe extern "C" fn(Handle, c_int, u32, u32) -> c_int,
    update_wire_ins: unsafe extern "C" fn(Handle) -> c_int,
    activate_trigger_in: unsafe extern "C" fn(Handle, c_int, c_int) -> c_int,
    read_from_pipe_out: unsafe extern "C" fn(Handle, c_int, c_long, *mut u8) -> c_long,
    read_from_block_pipe_out: unsafe extern "C" fn(Handle, c_int, c_int, c_long, *mut u8) -> c_long,
    // LAST FIELD ON PURPOSE: fields drop in declaration order, so the library outlives every
    // pointer taken out of it.
    _lib: Library,
}

/// Copy a symbol's address out of the library. Sound only because `Fp` keeps the `Library` alive
/// for exactly as long as the pointers, and because a C function pointer is `Copy`.
unsafe fn sym<T: Copy>(lib: &Library, name: &str) -> Result<T, Error> {
    let s: libloading::Symbol<T> = lib
        .get(CString::new(name).unwrap().as_bytes_with_nul())
        .map_err(|_| sdk(name))?;
    Ok(*s)
}

impl Fp {
    /// Load the driver, in this order — and the order is the API, not an implementation detail:
    ///
    /// 1. `driver_dir`, the explicit argument to [`crate::Device::open_usb3`];
    /// 2. `$KDI_DRIVER_DIR`, a path list in the platform's `PATH` syntax;
    /// 3. the copy compiled into this artifact, staged under the temp dir (`bundled`);
    /// 4. the OS search path.
    ///
    /// A developer's own copy therefore always beats the embedded one, which is what keeps a bench
    /// workflow able to test a driver this build was not made with. The embedded copy is tried
    /// BEFORE the OS search path on purpose: a machine can have more than one copy installed and
    /// the wrong one is not a graceful failure — on the bench it hard-aborts the process
    /// (the Python reference host) — whereas the embedded bytes are the ones this build was
    /// tested against.
    fn load(driver_dir: Option<&Path>) -> Result<Fp, Error> {
        // ── VENDOR-NAMES-BEGIN ── the driver's file name on each platform. It is the vendor's
        // file, so this is the vendor's spelling; nothing else in the crate says it. See the
        // module docs — this is a name a loader must match, not something to be hidden.
        let names: &[&str] = if cfg!(target_os = "windows") {
            &["okFrontPanel.dll"]
        } else if cfg!(target_os = "macos") {
            &["libokFrontPanel.dylib"]
        } else {
            &["libokFrontPanel.so"]
        };
        // ── VENDOR-NAMES-END ──
        // The name we stage under, so copying that file into `$KDI_DRIVER_DIR` works. Outside the
        // carve-out: these are ours.
        let staged: &[&str] = if cfg!(target_os = "windows") {
            &["kdi_driver.dll"]
        } else if cfg!(target_os = "macos") {
            &["libkdi_driver.dylib"]
        } else {
            &["libkdi_driver.so"]
        };
        let mut dirs: Vec<PathBuf> = driver_dir.into_iter().map(PathBuf::from).collect();
        if let Some(p) = std::env::var_os(DRIVER_DIR_ENV) {
            dirs.extend(std::env::split_paths(&p));
        }
        let mut tried = Vec::new();
        let lib = 'found: {
            for d in &dirs {
                for n in names.iter().chain(staged.iter()) {
                    let path = d.join(n);
                    match unsafe { Library::new(&path) } {
                        Ok(l) => break 'found l,
                        // THE DIRECTORY, NOT THE FILE — and not the loader's error either. This
                        // string reaches a customer; the file name is the vendor's, and so is
                        // whatever `libloading` puts in `Display`. Where we looked is the useful
                        // half. Same reason the OS-search entry below says "the OS search path".
                        Err(_) => tried.push(d.display().to_string()),
                    }
                }
            }
            // The staging failure is RETURNED, not pushed onto `tried`: a temp dir that cannot be
            // written is a permission fault with a fix, and letting it fall through would report it
            // as NotFound — which `enumerate` reads as "no board on this machine" and swallows.
            #[cfg(feature = "bundled")]
            {
                let path = crate::bundled::staged_path().map_err(|e| {
                    io_err(
                        e.kind(),
                        format!(
                            "the bundled device driver could not be unpacked: {e}. Set \
                             ${DRIVER_DIR_ENV} to a directory holding the driver to bypass it."
                        ),
                    )
                })?;
                match unsafe { Library::new(&path) } {
                    Ok(l) => break 'found l,
                    // Reached when the temp dir is mounted `noexec`: the bytes are there and the
                    // mapping is refused. `$KDI_DRIVER_DIR` is the way out. The directory, not
                    // the staged file name and not the loader's error (it names the file).
                    Err(_) => tried.push(format!(
                        "{} (embedded copy)",
                        path.parent().unwrap_or(path.as_path()).display()
                    )),
                }
            }
            for n in names {
                match unsafe { Library::new(n) } {
                    Ok(l) => break 'found l,
                    Err(_) => tried.push("the OS search path".into()),
                }
            }
            // NotFound, not `Sdk`: "this machine has no driver installed" is a fact about the
            // machine, and `enumerate` has to tell it apart from "the driver is here and broken"
            // WITHOUT matching on message text. That distinction is the whole reason `find`
            // returns its errors.
            return Err(io_err(
                std::io::ErrorKind::NotFound,
                format!(
                    "no device driver loaded (set ${DRIVER_DIR_ENV} to the directory holding it); \
                     tried {}",
                    tried.join("; ")
                ),
            ));
        };
        // EVERY symbol is resolved here, including ones this session may never call: a missing
        // symbol found at load names itself, while one found at first use surfaces mid-recording.
        //
        // ── VENDOR-NAMES-BEGIN ──
        // These are the driver's exported symbol names. They are resolved by `dlsym`, so they MUST
        // exist as plain string literals in the binary and `strings` will show them. That is not a
        // leak to be plugged: do not obfuscate, encrypt or split them. The goal is that nobody
        // READING our API, our docs or an error message meets the board vendor — not to defeat a
        // reverse engineer, which is impossible here and would only cost the load-time error the
        // ability to say which symbol was missing.
        unsafe {
            Ok(Fp {
                devices_construct: sym(&lib, "okFrontPanelDevices_Construct")?,
                devices_destruct: sym(&lib, "okFrontPanelDevices_Destruct")?,
                devices_get_count: sym(&lib, "okFrontPanelDevices_GetCount")?,
                devices_get_serial: sym(&lib, "okFrontPanelDevices_GetSerial")?,
                devices_open: sym(&lib, "okFrontPanelDevices_Open")?,
                destruct: sym(&lib, "okFrontPanel_Destruct")?,
                close: sym(&lib, "okFrontPanel_Close")?,
                configure_from_memory: sym(&lib, "okFrontPanel_ConfigureFPGAFromMemory").ok(),
                get_data_port: sym(&lib, "okFrontPanel_GetFPGADataPortClassic")?,
                update_wire_outs: sym(&lib, "okFPGADataPortClassic_UpdateWireOuts")?,
                get_wire_out_value: sym(&lib, "okFPGADataPortClassic_GetWireOutValue")?,
                set_wire_in_value: sym(&lib, "okFPGADataPortClassic_SetWireInValue")?,
                update_wire_ins: sym(&lib, "okFPGADataPortClassic_UpdateWireIns")?,
                activate_trigger_in: sym(&lib, "okFPGADataPortClassic_ActivateTriggerIn")?,
                read_from_pipe_out: sym(&lib, "okFPGADataPortClassic_ReadFromPipeOut")?,
                read_from_block_pipe_out: sym(&lib, "okFPGADataPortClassic_ReadFromBlockPipeOut")?,
                _lib: lib,
            })
        }
        // ── VENDOR-NAMES-END ──
    }
}

impl Fp {
    /// The devices manager. Short-lived ON PURPOSE: the Python reference's `<manager>().Open(serial)`
    /// drops the temporary the moment `Open` returns, and the board it handed back stays open —
    /// holding the manager for the session would keep an enumeration object alive for nothing.
    unsafe fn devices(&self) -> Result<Handle, Error> {
        // `c""`, not `b"\0"`. It was the byte string while the crate declared MSRV 1.75, where
        // C-string literals (1.77) were unavailable. The MSRV now states what is actually built,
        // so the literal is back — and clippy's `manual_c_str_literals` asks for it, which is the
        // same lint the false 1.75 floor had been suppressing.
        let h = unsafe { (self.devices_construct)(c"".as_ptr(), std::ptr::null_mut()) };
        if h.is_null() {
            return Err(sdk("the device manager could not be constructed"));
        }
        Ok(h)
    }

    fn close_device(&self, hnd: Handle) {
        unsafe {
            (self.close)(hnd);
            (self.destruct)(hnd);
        }
    }
}

pub(crate) struct Usb3 {
    fp: Fp,
    hnd: Handle,
    /// The classic-data-port view of `hnd` — every wire, trigger and pipe goes through this,
    /// never through `hnd`. Borrowed from the front panel, so `Drop` does not free it.
    port: Handle,
    serial: String,
    /// Console bytes read but not yet framed. The vUART is shared with the human console, so a
    /// drain routinely returns a banner or the tail of someone's `kv` output.
    console: Vec<u8>,
}

impl Usb3 {
    /// Open a board, optionally CONFIGURING it from `image` first — see
    /// [`crate::Device::open_usb3_configured`] for what that means and does not mean.
    pub(crate) fn open(
        serial: &str,
        driver_dir: Option<&Path>,
        image: Option<&[u8]>,
    ) -> Result<Usb3, Error> {
        // Validate before loading the driver or opening the board. An empty configure could be a
        // no-op that leaves the resident image running, and a too-large image must not leak the
        // handle we would otherwise have opened before discovering it cannot cross the ABI.
        let image = match image {
            Some([]) => {
                return Err(io_err(
                    std::io::ErrorKind::InvalidInput,
                    "empty gateware image",
                ))
            }
            Some(img) => Some((
                img,
                img.len().try_into().map_err(|_| {
                    io_err(
                        std::io::ErrorKind::InvalidInput,
                        "gateware image is too large to send",
                    )
                })?,
            )),
            None => None,
        };
        let c = CString::new(serial)
            .map_err(|_| io_err(std::io::ErrorKind::InvalidInput, "serial contains a NUL"))?;
        let fp = Fp::load(driver_dir)?;
        // An empty serial reaches the driver as an empty string, which it reads as "the first device"
        // — the same thing an empty serial means in the Python reference.
        let devs = unsafe { fp.devices()? };
        let hnd = unsafe { (fp.devices_open)(devs, c.as_ptr()) };
        unsafe { (fp.devices_destruct)(devs) };
        if hnd.is_null() {
            return Err(sdk(format!("opening device {serial:?} returned no handle")));
        }
        // THE RETURN CODE IS CHECKED, AND THAT IS THE WHOLE POINT OF THIS BLOCK. The repo's scar:
        // every Python caller guarded the configure on a `NoError` attribute of the DEVICE CLASS,
        // where it does not exist — it lives on the error-code enum — so the `and` short-circuited,
        // the return value was never compared, and A FAILED FLASH TESTED GREEN.
        // The board then ran whatever bitstream was already resident and presented as a wedged
        // console (`tools/vuart_term.py:228-249`, AGENTS.md "Bench and host tools"). So: any
        // status but `OK_NO_ERROR` closes the handle and returns an error naming the operation,
        // the status and the image length. Nothing downstream may run on a device whose configure
        // did not take.
        if let Some((img, len)) = image {
            // Resolved lazily (see the field): a driver without this entry point can still do
            // everything except configure, so the refusal names the missing capability rather than
            // failing at load and taking `open_usb3` down with it.
            let Some(configure) = fp.configure_from_memory else {
                fp.close_device(hnd);
                return Err(sdk(
                    "this device driver cannot configure a device from memory; \
                     bind an already-configured board with open_usb3 instead",
                ));
            };
            let rc = unsafe { configure(hnd, img.as_ptr().cast::<c_uchar>(), len) };
            if rc != OK_NO_ERROR {
                fp.close_device(hnd);
                return Err(sdk(format!(
                    "configuring the device with a {} byte gateware image failed (rc = {rc}); \
                     the board is still running whatever was resident",
                    img.len()
                )));
            }
        }
        // GATED ON THE OUT-PARAMETER, not on `rc`: the out-param is what was measured to carry the
        // port, and a null one would turn every later wire read into a plausible-looking zero.
        // `rc` is reported because it is the only thing that says WHY.
        let mut port: Handle = std::ptr::null_mut();
        let rc = unsafe { (fp.get_data_port)(hnd, &mut port) };
        if port.is_null() {
            fp.close_device(hnd);
            return Err(sdk(format!(
                "the device exposes no classic data port (rc = {rc})"
            )));
        }
        Ok(Usb3 {
            fp,
            hnd,
            port,
            serial: serial.to_string(),
            console: Vec::new(),
        })
    }

    /// Tier-A identity: what is knowable without the gateware answering. Deliberately NOT read
    /// from WireOut 0x3e, which carries the legacy RHX BOARD_ID — a different thing from KDI's
    /// `board_id` (the Python reference host).
    pub(crate) fn identity(&self) -> Value {
        json!({"serial": self.serial, "transport": "usb3"})
    }

    /// The update is CHECKED. The wire-out read reports no error of its own — it returns 0 for an
    /// address it cannot serve — so an unchecked failed transfer here would surface as a perfectly
    /// plausible zero register rather than as an error.
    pub(crate) fn reg_read(&mut self, r: RegBind) -> Result<u32, Error> {
        let rc = unsafe { (self.fp.update_wire_outs)(self.port) };
        if rc != OK_NO_ERROR {
            return Err(sdk(format!("register-block read failed (rc = {rc})")));
        }
        let v = unsafe { (self.fp.get_wire_out_value)(self.port, c_int::from(r.addr)) };
        Ok((v & r.mask()) >> r.lo)
    }

    /// `word` is the CALLER's shadow of the whole WireIn, already masked — see
    /// `Device::write_field`. The driver's own mask argument is set wide here precisely because the
    /// host, not the driver, owns the read-modify-write: a second host process holding a different
    /// idea of the word is exactly the case the shadow makes visible.
    pub(crate) fn reg_write(&mut self, r: RegBind, word: u32) -> Result<(), Error> {
        match r.kind {
            "wirein" => {
                let rc = unsafe {
                    (self.fp.set_wire_in_value)(self.port, c_int::from(r.addr), word, u32::MAX)
                };
                if rc != OK_NO_ERROR {
                    return Err(sdk(format!(
                        "register 0x{:02x} write failed (rc = {rc})",
                        r.addr
                    )));
                }
                // Checked: the wire-in write only stages a shadow word, so an unchecked failure here
                // is a write that silently never reached the board.
                let rc = unsafe { (self.fp.update_wire_ins)(self.port) };
                if rc != OK_NO_ERROR {
                    return Err(sdk(format!("register-block write failed (rc = {rc})")));
                }
                Ok(())
            }
            // A triggerin is a one-cycle pulse on `.b`; there is no value to write
            // (`endpoint_grammar`, kdi/contract.yaml:702-709).
            "triggerin" => self.trigger(r.addr, r.lo),
            k => Err(io_err(
                std::io::ErrorKind::InvalidInput,
                format!("{k} endpoints are not writable"),
            )),
        }
    }

    fn trigger(&mut self, addr: u8, bit: u8) -> Result<(), Error> {
        let rc = unsafe {
            (self.fp.activate_trigger_in)(self.port, c_int::from(addr), c_int::from(bit))
        };
        if rc != OK_NO_ERROR {
            return Err(sdk(format!(
                "trigger 0x{addr:02x}.{bit} failed (rc = {rc})"
            )));
        }
        Ok(())
    }

    /// `bindings.usb3.stream_read_rule` (kdi/contract.yaml:741-745), exactly.
    ///
    /// A plain `okPipeOut` is NOT paced by the FPGA: it returns whatever it has and, past the end
    /// of the resident data, zero fill — which the decoder walks as a bogus header and rejects at
    /// the first byte past real data. So the size comes from the stream's own occupancy word:
    /// `words32 * 4`, clamped to what the caller wants, rounded DOWN to `read_alignment`, and
    /// skipped entirely at 0.
    pub(crate) fn stream_read(&mut self, s: Stream, buf: &mut [u8]) -> Result<usize, Error> {
        let (kind, addr) = stream_ep(s)?;
        let avail = self.reg_read(reg(stream_regs(s).2)?)? & 0xFFFF;
        let mut n = (avail as usize * 4).min(buf.len());
        n -= n % USB3_READ_ALIGNMENT;
        if n == 0 {
            return Ok(0);
        }
        // HONOUR THE PIPE KIND THE BINDING DECLARES. Only an okBTPipeOut has
        // ep_ready/ep_blockstrobe; block-reading a plain pipe cannot work against this gateware
        // (the reference host does the same).
        let rc = if kind.eq_ignore_ascii_case("okbtpipeout") {
            unsafe {
                (self.fp.read_from_block_pipe_out)(
                    self.port,
                    c_int::from(addr),
                    USB3_READ_ALIGNMENT as c_int,
                    n as c_long,
                    buf.as_mut_ptr(),
                )
            }
        } else {
            unsafe {
                (self.fp.read_from_pipe_out)(
                    self.port,
                    c_int::from(addr),
                    n as c_long,
                    buf.as_mut_ptr(),
                )
            }
        };
        if rc < 0 {
            return Err(sdk(format!("stream pipe 0x{addr:02x} read {n} = {rc}")));
        }
        let got = rc as usize;
        if got < n {
            // The framing said `n` and the transport gave less: the closed host set has a token
            // for exactly this, and it must not be papered over as a short but valid read.
            return Err(Error::Host(HostErr::HostShortRead));
        }
        Ok(got.min(buf.len()))
    }

    /// The message channel (`bindings.usb3.message`): a `kdi ` line into the console RX FIFO, a
    /// sentinel-framed JSON reply out of the console TX pipe.
    ///
    /// THE LEASE TOKEN IS DELIBERATELY NOT SENT, and that asymmetry with the udp binding is the
    /// contract's, not an omission here: "`token` and `confirm` are envelope keys of the abstract
    /// request, but this binding's line form carries positional args only… no envelope key has a
    /// wire encoding here yet" (`envelope_on_the_wire`, kdi/contract.yaml:779-783). Inventing one
    /// — a trailing positional, a `token=` word — would be a private extension the firmware does
    /// not parse, and this wire is shared with a human shell where a stray word is a `kv`
    /// argument. The consequence is benign and expected: today's firmware implements no session
    /// command, so `sys.claim` answers `unknown_cmd` and the device reports
    /// [`crate::Lease::Unsupported`], which is the published meaning of that answer.
    pub(crate) fn message(
        &mut self,
        id: &str,
        name: &str,
        args: &[&str],
        _token: &str,
    ) -> Result<Reply, Error> {
        // `request.line`: "kdi <id> <name> <arg>*\r", args positional in declared order.
        let mut line = String::from(crate::REQ_TAG);
        line.push_str(id);
        line.push(' ');
        line.push_str(name);
        for a in args {
            line.push(' ');
            line.push_str(a);
        }
        // `terminator: "\r"`. The charset check that makes this safe already ran in `Device::cmd`.
        line.push('\r');

        // Drain first: the console emits banners and prior output on this same wire, and a stale
        // frame left in the buffer would be scanned as this command's answer (`response.rules`).
        self.drain()?;
        self.console.clear();
        self.send(&line, Duration::from_secs(2))?;

        let deadline = Instant::now() + Duration::from_secs(4);
        let mut scanned = 0usize;
        loop {
            let got = self.drain()?;
            if got == 0 {
                if Instant::now() >= deadline {
                    return Err(Error::Host(HostErr::HostTimeout));
                }
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            // Scan PAST non-matching ids rather than returning the first frame: a late reply from
            // a previously timed-out command must never be returned as this command's answer.
            while let Some((v, end)) = next_frame(&self.console, scanned) {
                scanned = end;
                if let Ok(r) = reply_from(v) {
                    if r.id == id {
                        return Ok(r);
                    }
                }
            }
        }
    }

    /// Push ASCII into the console RX FIFO, 4 bytes per word, gated on `status[31:16]`
    /// (`rx_framing`). The FIFO has no back-pressure of its own.
    fn send(&mut self, s: &str, timeout: Duration) -> Result<(), Error> {
        let rx_data = msg_ep("rx_data")?;
        let rx_count = msg_ep("rx_count")?;
        let rx_push = msg_ep("rx_push")?;
        let deadline = Instant::now() + timeout;
        for chunk in s.as_bytes().chunks(4) {
            loop {
                if self.status()? >> 16 >= chunk.len() as u32 {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(Error::Host(HostErr::HostTimeout));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            let word = chunk
                .iter()
                .enumerate()
                .fold(0u32, |w, (i, b)| w | u32::from(*b) << (8 * i));
            self.reg_write(rx_data, word)?;
            self.reg_write(rx_count, chunk.len() as u32)?;
            self.trigger(rx_push.addr, rx_push.lo)?;
        }
        Ok(())
    }

    fn status(&mut self) -> Result<u32, Error> {
        self.reg_read(msg_ep("status")?)
    }

    /// One batch of console TX, decoded from the self-framing 32-bit words: byte 0 is a count of
    /// 0..3, bytes 1..3 are that many payload bytes (`tx_framing`). Returns how many payload bytes
    /// were appended.
    ///
    /// Rounds the USB3 request **up** to 16 bytes so a 1–3 word occupancy is still readable.
    /// `got < want` is therefore legal — that is the difference from [`Usb3::stream_read`], which
    /// rounds **down** and then demands a full `n`. Reject `rc < 0` and a length that is not a
    /// whole number of words; do not copy `HostShortRead` onto this path.
    fn drain(&mut self) -> Result<usize, Error> {
        let words = self.status()? & 0xFFFF;
        if words == 0 {
            return Ok(0);
        }
        let tx = msg_ep("tx_pipe")?;
        let want =
            (words.min(256) as usize * 4).div_ceil(USB3_READ_ALIGNMENT) * USB3_READ_ALIGNMENT;
        let mut raw = vec![0u8; want];
        let rc = unsafe {
            (self.fp.read_from_pipe_out)(
                self.port,
                c_int::from(tx.addr),
                want as c_long,
                raw.as_mut_ptr(),
            )
        };
        if rc < 0 {
            return Err(sdk(format!("console pipe 0x{:02x} read = {rc}", tx.addr)));
        }
        if rc % 4 != 0 {
            return Err(sdk(format!(
                "console pipe 0x{:02x} read = {rc} (not a whole number of words)",
                tx.addr
            )));
        }
        let mut n = 0;
        for w in raw[..(rc as usize).min(raw.len())].chunks_exact(4) {
            let cnt = (w[0] as usize).min(3);
            self.console.extend_from_slice(&w[1..1 + cnt]);
            n += cnt;
        }
        Ok(n)
    }
}

impl Drop for Usb3 {
    fn drop(&mut self) {
        self.fp.close_device(self.hnd);
    }
}

/// `0x1e`, then EXACTLY 3 lowercase ASCII hex digits of body length, then that many bytes of UTF-8
/// JSON (`response.framing`). Returns the parsed body and the offset just past it.
///
/// A body that fails to parse is treated as ABSENT and the scan continues — the console is a
/// shared wire and a truncated frame is an ordinary event on it, not an error to report upward.
fn next_frame(cap: &[u8], from: usize) -> Option<(Value, usize)> {
    let mut i = from.min(cap.len());
    while let Some(rel) = cap[i..].iter().position(|b| *b == RESP_SENTINEL) {
        let at = i + rel;
        let body_at = at + 1 + RESP_LEN_DIGITS;
        if body_at > cap.len() {
            return None; // the length digits have not arrived yet
        }
        let len = std::str::from_utf8(&cap[at + 1..body_at])
            .ok()
            .and_then(|s| usize::from_str_radix(s, 16).ok());
        if let Some(len) = len {
            if body_at + len > cap.len() {
                return None; // the body has not arrived yet; keep the sentinel for the next pass
            }
            if let Ok(v) = serde_json::from_slice::<Value>(&cap[body_at..body_at + len]) {
                return Some((v, body_at + len));
            }
        }
        // Not a frame after all (a 0x1e in ordinary console output, or an unparseable body):
        // step past this sentinel and keep scanning.
        i = at + 1;
    }
    None
}

fn stream_ep(s: Stream) -> Result<(&'static str, u8), Error> {
    USB3_STREAM
        .iter()
        .find(|(n, ..)| *n == s.token())
        .map(|&(_, kind, addr)| (kind, addr))
        .ok_or_else(|| {
            io_err(
                std::io::ErrorKind::InvalidInput,
                format!("the usb3 binding maps no stream {}", s.token()),
            )
        })
}

/// A message-channel endpoint, resolved by ROLE. These were host-side constants in the reference
/// for three revisions — the one part of the contract a host could not resolve by name — and this
/// project has already moved the vUART status word once, 0x26 -> 0x30
/// (kdi/contract.yaml:754-763).
fn msg_ep(role: &str) -> Result<RegBind, Error> {
    USB3_MSG
        .iter()
        .find(|(r, ..)| *r == role)
        .map(|&(_, kind, addr, bit)| RegBind {
            kind,
            addr,
            lo: bit.unwrap_or(0),
            width: if bit.is_some() { 1 } else { 32 },
        })
        .ok_or_else(|| {
            io_err(
                std::io::ErrorKind::InvalidInput,
                format!("the usb3 message channel declares no {role} endpoint"),
            )
        })
}

/// Attached boards, by serial. Transport-native and cheap: no gateware, no `ConfigureFPGA` — the
/// analog of reading a USB descriptor (the Python reference host).
///
/// A driver that is present but broken must be DISTINGUISHABLE from an empty bench, so every
/// failure here is returned to `find` rather than swallowed.
pub(crate) fn enumerate() -> Result<Vec<DeviceInfo>, Error> {
    let fp = match Fp::load(None) {
        Ok(fp) => fp,
        // No driver installed is not a fault: it is a machine with no USB3 binding, which is
        // the ordinary case everywhere except the bench. A driver that IS there and fails to give
        // up a symbol is `Error::Sdk` and travels back to the caller.
        Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let hnd = unsafe { fp.devices()? };
    let n = unsafe { (fp.devices_get_count)(hnd) };
    let mut out = Vec::new();
    for i in 0..n.max(0) {
        // The driver copies at most 10 characters and NUL-terminates at offset 10, so 11 bytes is the
        // real floor; 32 is slack against a future model with a longer serial.
        let mut raw = [0 as c_char; 32];
        unsafe { (fp.devices_get_serial)(hnd, i, raw.as_mut_ptr()) };
        let serial = unsafe { CStr::from_ptr(raw.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        out.push(DeviceInfo {
            serial: serial.clone(),
            // Empty, not guessed: `device.vendor`/`device.compatible` are in contract.yaml but not
            // in the generated spec, and a literal here would be a second source of truth. See
            // `Device::open_usb3`.
            vendor: String::new(),
            compatible: String::new(),
            board_id: None,
            kdi: None,
            transport: TransportKind::Usb3,
            addr: Addr::Serial(serial),
        });
    }
    unsafe { (fp.devices_destruct)(hnd) };
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_empty_image_is_rejected_before_the_driver_is_loaded() {
        let Err(crate::Error::Io(e)) = super::Usb3::open("", None, Some(&[])) else {
            panic!("an empty image reached the device driver");
        };
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput);
        assert!(e.to_string().contains("empty"));
    }
}
