//! The SIM harness binding: the SHIPPING SpinalHDL fabric under Verilator, poked over TCP.
//!
//! **This is not a KDI transport binding and must never be described as one.** The contract
//! publishes `bindings.udp` and `bindings.usb3`; a `tcp` binding does not exist and is not being
//! implemented here (`kdi/rs/README.md`, the "not built" table). What is on this socket is
//! `hw/spinal/src/test/scala/kdi/KdiSimServer.scala`'s DUMB ADDRESS-LEVEL POKE protocol — the same
//! wire the Python reference host drives — with every KDI concept left on this side:
//!
//! ```text
//! {"op":"wireout_read","addr":N}                    -> {"value":N}
//! {"op":"wirein_write","addr":N,"value":N,"mask":N} -> {"ok":true}
//! {"op":"trigger","addr":N,"bit":N}                 -> {"ok":true}
//! ```
//!
//! WHY IT EXISTS: the usb3 binding resolves every register through the generated `USB3_REG` table
//! and **has never opened a board** (`kdi/rs/README.md`, "hardware verification of `usb3`"). Nothing
//! had ever checked that those addresses name WireOuts the fabric actually drives — the Python host
//! has that check (`tools/kdi_conform_sim.py`), the Rust one had none. This gives it one, against
//! real elaborated RTL, with no bench.
//!
//! WHAT IT DOES NOT TEST, stated plainly:
//!
//! * **Not the driver FFI.** `usb3.rs`'s `dlopen`, its symbol resolution and its C calls are
//!   not on this path at all. A board is still the only thing that
//!   exercises them.
//! * **Not the sample stream, not the command channel.** The harness stubs the DDR3 app bus and
//!   runs no firmware (`KdiSimServer.scala:33-36`), so [`Sim::stream_read`] and [`Sim::message`]
//!   REFUSE rather than return plausible emptiness — a host that reads "no records" from a device
//!   that cannot produce any has learnt nothing, and this repo has been bitten by pass-by-skip
//!   often enough to make the refusal explicit.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{
    io_err, io_or_timeout, Addr, ConnectOpts, Device, DeviceInfo, Error, Link, RegBind, Reply,
    Stream, TransportKind,
};
use std::io;

/// One RPC deadline, generous on purpose: Verilator advances the whole fabric one clock per
/// service loop, so a poke that would be microseconds on USB3 is milliseconds here and a tight
/// timeout buys nothing but a flaky failure (the Python reference host uses 30 s for the same reason).
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// The fabric's own per-slot SPI-running word — **NOT a KDI register**, and the only reason a
/// literal endpoint appears in this crate outside the generated table.
///
/// It is here because it is the ONLY witness this DUT offers that a WireIn write arrived: the
/// harness is `rhd.RhdAssembly`, whose sole WireIn-sensitive path is `run_samples` ->
/// `kdi_engine_req` -> the SPI engine -> `slot_running` (`hw/spinal/src/test/scala/rhd/RhdAssembly.scala:90`,
/// `hw/spinal/src/main/scala/rhd/RhdAggregator.scala:534`). Without it a WireIn write is
/// unobservable and the address half of the binding stays unproven in the write direction — which
/// is exactly the gap this module was built to close. It lives in the binding module because that
/// is where this crate keeps endpoint numbers (see the `lib.rs` module docs).
const WO_SLOT_RUNNING: u8 = 0x34;

/// The legacy RHX host-reset word (WireIn 0x00, bit 0) — **NOT a KDI register** either, and the
/// second and last literal endpoint here.
///
/// It is the only thing that STOPS the SPI engine: the cores latch `continuous` from
/// `kdi_engine_req` at the config fan-out and never re-read it
/// (`hw/spinal/src/main/scala/rhd/RhdAggregator.scala:219`), so once the engine is up, clearing
/// `run_samples` changes nothing observable. After a reset the fabric re-requests the start IFF
/// `run_samples` is still asserted (`:195`, the documented "free retry") — which is what turns the
/// survival of a field across a NEIGHBOURING field's write into something real RTL can witness.
const WI_HOST_RESET: u8 = 0x00;

pub(crate) struct Sim {
    sock: TcpStream,
}

impl Sim {
    pub(crate) fn connect(addr: SocketAddr, timeout: Duration) -> Result<Sim, Error> {
        let sock = TcpStream::connect_timeout(&addr, timeout).map_err(Error::Io)?;
        // Nagle would batch a poke behind the previous reply on a wire that is strictly
        // request/response, adding a 40 ms stall to every register read.
        sock.set_nodelay(true).map_err(Error::Io)?;
        sock.set_read_timeout(Some(RPC_TIMEOUT))
            .map_err(Error::Io)?;
        sock.set_write_timeout(Some(RPC_TIMEOUT))
            .map_err(Error::Io)?;
        Ok(Sim { sock })
    }

    /// 4-byte big-endian length + payload, both ways (`KdiSimServer.scala:30`, and
    /// the Python reference host's TcpTransport before it).
    fn rpc(&mut self, req: &Value) -> Result<Vec<u8>, Error> {
        let body = serde_json::to_vec(req)
            .map_err(|e| io_err(io::ErrorKind::InvalidData, e.to_string()))?;
        let len = u32::try_from(body.len()).map_err(|_| {
            io_err(
                io::ErrorKind::InvalidData,
                "request longer than the framing",
            )
        })?;
        self.sock
            .write_all(&len.to_be_bytes())
            .map_err(io_or_timeout)?;
        self.sock.write_all(&body).map_err(io_or_timeout)?;
        self.sock.flush().map_err(io_or_timeout)?;

        let mut hdr = [0u8; 4];
        self.sock.read_exact(&mut hdr).map_err(io_or_timeout)?;
        let mut rsp = vec![0u8; u32::from_be_bytes(hdr) as usize];
        self.sock.read_exact(&mut rsp).map_err(io_or_timeout)?;
        Ok(rsp)
    }

    fn json(&mut self, req: &Value) -> Result<Value, Error> {
        let rsp = self.rpc(req)?;
        let v: Value = serde_json::from_slice(&rsp).map_err(|e| {
            io_err(
                io::ErrorKind::InvalidData,
                format!("undecodable reply to {}: {e}", req["op"]),
            )
        })?;
        // The server answers `{"err":"bad_op: …"}` for anything it does not implement. That is a
        // HOST bug — an op this binding sent that the harness does not have — so it surfaces as an
        // error rather than as a value.
        if let Some(t) = v.get("err").and_then(Value::as_str) {
            return Err(io_err(
                io::ErrorKind::InvalidData,
                format!("simulator refused {}: {t}", req["op"]),
            ));
        }
        Ok(v)
    }

    /// Identity, tier-A: what is knowable without asking the gateware. The serial matches
    /// the Python reference host so one harness has one name in both hosts' logs. `vendor`/`compatible`
    /// stay EMPTY for the same reason `usb3::identity` leaves them empty — they are not in the
    /// generated spec, and a literal here would be a second source of truth.
    pub(crate) fn identity(&self) -> Value {
        json!({"serial": "KDISIM01", "transport": "sim"})
    }

    /// Read a WireOut and extract the field, exactly as `usb3::reg_read` does — the shift and mask
    /// are the host's on this binding, which is half of what is under test.
    pub(crate) fn reg_read(&mut self, r: RegBind) -> Result<u32, Error> {
        if r.kind != "wireout" {
            return Err(io_err(
                io::ErrorKind::InvalidInput,
                format!("{} endpoints are not readable", r.kind),
            ));
        }
        let v = self.wireout(r.addr)?;
        Ok((v & r.mask()) >> r.lo)
    }

    fn wireout(&mut self, addr: u8) -> Result<u32, Error> {
        let v = self.json(&json!({"op": "wireout_read", "addr": addr}))?;
        v.get("value")
            .and_then(Value::as_u64)
            .map(|x| x as u32)
            .ok_or_else(|| {
                io_err(
                    io::ErrorKind::InvalidData,
                    format!("wireout_read 0x{addr:02x}: reply carries no integer `value`"),
                )
            })
    }

    /// `word` is the CALLER's shadow of the WHOLE WireIn, already masked by `Device::write_field`.
    /// The mask sent here is therefore wide, byte-for-byte what `usb3::reg_write` hands
    /// the driver's wire-in write: the host owns the read-modify-write, and that is the
    /// `masked_field_write` invariant this harness lets a test observe against real RTL.
    pub(crate) fn reg_write(&mut self, r: RegBind, word: u32) -> Result<(), Error> {
        match r.kind {
            "wirein" => {
                self.json(&json!({
                    "op": "wirein_write", "addr": r.addr, "value": word, "mask": u32::MAX,
                }))?;
                Ok(())
            }
            "triggerin" => {
                self.json(&json!({"op": "trigger", "addr": r.addr, "bit": r.lo}))?;
                Ok(())
            }
            k => Err(io_err(
                io::ErrorKind::InvalidInput,
                format!("{k} endpoints are not writable"),
            )),
        }
    }

    /// REFUSED, not empty. The harness stubs the DDR3 app bus, so there are no frames to read
    /// (`KdiSimServer.scala:145-148` answers every pipe read with zero bytes). Returning `Ok(0)`
    /// would make a caller's "no records in 10 s" indistinguishable from a device that is simply
    /// not producing any — the pass-by-skip shape the project's engineering notes names.
    pub(crate) fn stream_read(&mut self, s: Stream, _buf: &mut [u8]) -> Result<usize, Error> {
        Err(io_err(
            io::ErrorKind::Unsupported,
            format!(
                "the sim harness stubs DDR3 and emits no frames: stream `{}` cannot be read here \
                 (rhd.DdrPipeFifoSim covers that path)",
                s.token()
            ),
        ))
    }

    /// REFUSED for the same reason: the message channel is the vUART into FIRMWARE, and this
    /// harness runs no CPU. `Link::has_message` is false here, so [`crate::Device::claim`] reports
    /// [`crate::Lease::Unsupported`] without sending — a dead vUART is not "this build has no
    /// lease", but a binding with no firmware is.
    pub(crate) fn message(
        &mut self,
        _id: &str,
        name: &str,
        _args: &[&str],
        _token: &str,
    ) -> Result<Reply, Error> {
        Err(io_err(
            io::ErrorKind::Unsupported,
            format!("the sim harness runs no firmware: `{name}` has no channel to reach"),
        ))
    }
}

impl Device {
    /// Bind the elaborated gateware served by `make kdi-sim` (Verilator) over its poke protocol.
    ///
    /// The address is the harness's, not a device's: there is no discovery here, because a
    /// simulator does not announce itself. Everything after the socket is the ORDINARY bind —
    /// `contract_version`, the major check, `contract_ready`, `caps`, `gateware_sha` — resolved
    /// through the generated `USB3_REG` table, which is the point of the exercise.
    ///
    /// Behind `--features sim`, and it changes nothing about a default build.
    #[cfg_attr(docsrs, doc(cfg(feature = "sim")))]
    pub fn connect_sim(addr: SocketAddr, timeout: Duration) -> Result<Device, Error> {
        let link = Sim::connect(addr, timeout)?;
        let info = DeviceInfo {
            serial: "KDISIM01".to_string(),
            vendor: String::new(),
            compatible: String::new(),
            board_id: None,
            kdi: None,
            transport: TransportKind::Sim,
            addr: Addr::Socket(addr),
        };
        Device::bind(
            Link::Sim(link),
            info,
            &ConnectOpts {
                need_caps: Vec::new(),
                ready_timeout: timeout,
            },
        )
    }

    /// Is the fabric's SPI engine running? The one WireIn effect this DUT can be asked about —
    /// see [`WO_SLOT_RUNNING`] for why it is a raw endpoint and what it witnesses.
    ///
    /// SIM ONLY. `slot_running` is not a KDI register and no board-facing code may read it.
    #[cfg_attr(docsrs, doc(cfg(feature = "sim")))]
    pub fn sim_engine_running(&mut self) -> Result<bool, Error> {
        match &mut self.link {
            Link::Sim(s) => Ok(s.wireout(WO_SLOT_RUNNING)? != 0),
            _ => Err(io_err(
                io::ErrorKind::Unsupported,
                "sim_engine_running is only meaningful against the Verilator harness",
            )),
        }
    }

    /// Pulse the fabric's host reset, stopping the SPI engine — see [`WI_HOST_RESET`] for why a
    /// test needs it and why it is a raw endpoint.
    ///
    /// SIM ONLY, and it would be actively dangerous on a board: it is the legacy RHX reset, it
    /// belongs to no KDI register, and this host's WireIn shadow does not model it.
    #[cfg_attr(docsrs, doc(cfg(feature = "sim")))]
    pub fn sim_host_reset(&mut self) -> Result<(), Error> {
        let Link::Sim(s) = &mut self.link else {
            return Err(io_err(
                io::ErrorKind::Unsupported,
                "sim_host_reset is only meaningful against the Verilator harness",
            ));
        };
        s.json(
            &json!({"op": "wirein_write", "addr": WI_HOST_RESET, "value": 1, "mask": u32::MAX}),
        )?;
        // Long enough for the cores' async reset to reach `slot_running` and for the aggregator's
        // two-flop synchroniser to carry it back into okClk; the harness free-runs, so wall time
        // here is simulated time.
        std::thread::sleep(Duration::from_millis(100));
        let Link::Sim(s) = &mut self.link else {
            unreachable!("the link cannot change kind mid-call")
        };
        s.json(
            &json!({"op": "wirein_write", "addr": WI_HOST_RESET, "value": 0, "mask": u32::MAX}),
        )?;
        Ok(())
    }
}
