//! The UDP reference binding (`bindings.udp`, kdi/contract.yaml:660-679).
//!
//! Its peer is the reference implementation's software device model, which is not distributed
//! with this crate. Published rather than left
//! implicit because it is the peer every non-Python implementation tests against with no hardware:
//! an undocumented test transport makes the conformance suite unreproducible outside this repo.
//!
//! Names are the wire here — an identity map — so this binding resolves no addresses at all.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use serde_json::{json, Value};

use crate::{io_err, reply_from, Error, Reply, COMMANDS};
use std::io;

/// One request datagram -> one reply datagram, max 65535 B (`bindings.udp.rpc.framing`).
const MAX_DGRAM: usize = 65535;

/// The abstract request envelope's `args` is an OBJECT KEYED BY ARGUMENT NAME (the Python reference host,
/// the Python reference host); only the vUART line is positional, and flattening one into the other
/// is that binding's job (the Python reference host). A JSON ARRAY here validated as an envelope
/// with NO arguments at all, so every argumented command over this binding answered
/// `bad_args`/`missing` — which reads exactly like a rejected value, and the conformance test that
/// asserted only the token passed on it.
///
/// The names come from the generated `COMMANDS`, in declared order, so there is no second copy of
/// the registry to go stale (`request.arg_order: declared`, kdi/contract.yaml:770).
fn named(name: &str, args: &[&str]) -> Value {
    // Unknown command, or more arguments than it declares: hand the envelope over as-is. The
    // device answers `unknown_cmd` before it looks at arguments, and a surplus argument must reach
    // it to be rejected rather than be silently dropped by a zip.
    let Some((_, keys)) = COMMANDS.iter().find(|(n, _)| *n == name) else {
        return json!(args.iter().map(|a| scalar(a)).collect::<Vec<Value>>());
    };
    if args.len() > keys.len() {
        return json!(args.iter().map(|a| scalar(a)).collect::<Vec<Value>>());
    }
    // Fewer is legal and ordinary: a trailing optional argument is simply absent.
    Value::Object(
        keys.iter()
            .zip(args)
            .map(|(k, a)| ((*k).to_string(), scalar(a)))
            .collect(),
    )
}

/// A bare token is untyped on the wire; JSON is not. An integer-looking argument is sent as a
/// number because that is what the contract declares (`type: u8`) and what the device validates
/// against.
fn scalar(a: &str) -> Value {
    match a.parse::<i64>() {
        Ok(n) => json!(n),
        Err(_) => json!(a),
    }
}

pub(crate) struct Udp {
    sock: UdpSocket,
    rx: Vec<u8>,
}

impl Udp {
    pub(crate) fn connect(peer: SocketAddr, timeout: Duration) -> Result<Udp, Error> {
        let sock = UdpSocket::bind(bind_any(peer)).map_err(Error::Io)?;
        sock.set_read_timeout(Some(timeout)).map_err(Error::Io)?;
        // `connect` so the kernel drops datagrams from anyone else: this socket is a point-to-point
        // link to one device, and an unrelated sender must not be able to answer for it.
        sock.connect(peer).map_err(Error::Io)?;
        Ok(Udp {
            sock,
            rx: vec![0u8; MAX_DGRAM],
        })
    }

    /// Bytes of the reply datagram, borrowed from the receive buffer.
    fn rpc(&mut self, req: &Value) -> Result<usize, Error> {
        let out = serde_json::to_vec(req)
            .map_err(|e| io_err(io::ErrorKind::InvalidData, e.to_string()))?;
        self.sock.send(&out).map_err(Error::Io)?;
        match self.sock.recv(&mut self.rx) {
            Ok(n) => Ok(n),
            // One rule, one definition. `io_or_timeout` maps a read deadline to `host_timeout` —
            // the closed set's token, never a minted one (`contract.yaml:90-94`) — and everything
            // else to `Io`. `simlink.rs` had its own copy of exactly this.
            Err(e) => Err(crate::io_or_timeout(e)),
        }
    }

    fn rpc_json(&mut self, req: &Value) -> Result<Value, Error> {
        let n = self.rpc(req)?;
        let v: Value = serde_json::from_slice(&self.rx[..n]).map_err(|e| {
            io_err(
                io::ErrorKind::InvalidData,
                format!("undecodable reply: {e}"),
            )
        })?;
        // ANY op may answer `{"err": token}` (kdi/contract.yaml:673-676). These are the
        // register-drawer tokens — `no_such_register`, `ro_register` — which are host bugs, not
        // device data: unlike a command reply there is no `rc`, no id and nothing to act on, so
        // they surface as an error rather than as an `Ok`.
        if let Some(t) = v.get("err").and_then(Value::as_str) {
            return Err(io_err(
                io::ErrorKind::InvalidData,
                format!("device refused {}: {t}", req["op"]),
            ));
        }
        Ok(v)
    }

    pub(crate) fn discover(&mut self) -> Result<Value, Error> {
        self.rpc_json(&json!({"op": "discover"}))
    }

    pub(crate) fn reg_read(&mut self, name: &str) -> Result<u32, Error> {
        let v = self.rpc_json(&json!({"op": "reg_read", "name": name}))?;
        v.get("value")
            .and_then(Value::as_u64)
            .map(|x| x as u32)
            .ok_or_else(|| {
                io_err(
                    io::ErrorKind::InvalidData,
                    format!("reg_read {name}: reply carries no integer `value`"),
                )
            })
    }

    pub(crate) fn reg_write(&mut self, name: &str, value: u32) -> Result<(), Error> {
        self.rpc_json(&json!({"op": "reg_write", "name": name, "value": value}))?;
        Ok(())
    }

    pub(crate) fn stream_read(&mut self, name: &str, buf: &mut [u8]) -> Result<usize, Error> {
        // One datagram caps one reply, so ask for no more than can come back. The host's rule is
        // the same on every binding: read, decode what arrived, read again if you wanted more.
        let want = buf.len().min(MAX_DGRAM - 1);
        if want == 0 {
            return Ok(0);
        }
        let n = self.rpc(&json!({"op": "stream_read", "name": name, "max_bytes": want}))?;
        let data = &self.rx[..n];
        // 0x01 then raw bytes; an error comes back as JSON, which cannot start with 0x01
        // (`bindings.udp.rpc.errors`).
        match data.first() {
            Some(1) => {
                let body = &data[1..];
                if body.len() > buf.len() {
                    return Err(io_err(
                        io::ErrorKind::InvalidData,
                        format!(
                            "stream_read returned {} B for a {want} B request",
                            body.len()
                        ),
                    ));
                }
                buf[..body.len()].copy_from_slice(body);
                Ok(body.len())
            }
            _ => {
                let token = serde_json::from_slice::<Value>(data)
                    .ok()
                    .and_then(|v| v.get("err").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_else(|| "unframed stream reply".to_string());
                Err(io_err(
                    io::ErrorKind::InvalidData,
                    format!("stream_read {name}: {token}"),
                ))
            }
        }
    }

    /// The abstract request envelope `{id, name, args}` (kdi/contract.yaml:498-510).
    ///
    /// ARGS GO BY NAME on this binding, because the envelope's `args` is an OBJECT keyed by
    /// argument name — that is what the reference device validates against
    /// (the Python reference host). Only the usb3 line form is positional, and the two are different
    /// encodings of the same declared order.
    ///
    /// This was an array once, and the failure is worth keeping: every argumented command answered
    /// `bad_args` with `why: missing`, while the conformance test asserted only `err == bad_args`
    /// and so passed on entirely the wrong reason. The names come from the generated `COMMANDS`
    /// registry, which exists precisely so the order lives in one place; a command the registry
    /// does not know still goes as an array, so the device rejects it rather than a `zip` silently
    /// dropping arguments.
    ///
    /// `token` is an ENVELOPE KEY — a sibling of `id`/`name`/`args`, never a member of `args`
    /// (kdi/contract.yaml:499-506). Inside `args` it would be validated as an undeclared argument
    /// and the command rejected; omitted entirely, the device refuses every command that is not
    /// `safety: ro` with `not_claimed` (the Python reference host). It is sent on every request,
    /// including the claim that mints the lease, exactly as the reference does
    /// (the Python reference host).
    pub(crate) fn message(
        &mut self,
        id: &str,
        name: &str,
        args: &[&str],
        token: &str,
    ) -> Result<Reply, Error> {
        let v = self.rpc_json(&json!({
            "op": "message",
            "req": {"id": id, "name": name, "args": named(name, args), "token": token},
        }))?;
        let resp = v.get("resp").cloned().ok_or_else(|| {
            io_err(
                io::ErrorKind::InvalidData,
                format!("message {name}: reply carries no `resp`"),
            )
        })?;
        let reply = reply_from(resp)?;
        // Correlate, always. A late reply from a previously timed-out command must never be
        // returned as this command's answer (`response.rules`, kdi/contract.yaml:788-792) — on a
        // datagram wire that is a stray reply the kernel queued, not a theoretical case.
        if reply.id != id {
            return Err(io_err(
                io::ErrorKind::InvalidData,
                format!("reply id {:?} does not match request {id:?}", reply.id),
            ));
        }
        Ok(reply)
    }
}

/// A one-shot discover datagram: the probe that turns an announcement file into evidence of a
/// live device (the Python reference host).
pub(crate) fn probe(addr: SocketAddr, timeout: Duration) -> Result<Value, Error> {
    let mut u = Udp::connect(addr, timeout)?;
    u.discover()
}

fn bind_any(peer: SocketAddr) -> SocketAddr {
    // Match the peer's family, or a v4 server is unreachable from a v6 socket and vice versa.
    if peer.is_ipv4() {
        SocketAddr::from(([0, 0, 0, 0], 0))
    } else {
        SocketAddr::from(([0u16; 8], 0))
    }
}
