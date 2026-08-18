//! Bind a device, read what it can do, and drive the typed command channel. NO HARDWARE NEEDED.
//!
//! Same setup as `stream.rs`. From the repo root, in one terminal:
//!
//! ```text
//! ```
//!
//! and in another:
//!
//! ```text
//! cargo run --example control                # first device found
//! cargo run --example control -- KVDEMO      # that serial specifically
//! ```
//!
//! Every command here is read-only or refused. It never powers a rail up and never drives a pin.

use kdi::{Cap, ChMode, Commands, ConnectOpts, Device, DeviceErr, Error, Filter};

fn main() -> Result<(), Error> {
    let filter = Filter {
        serial: std::env::args().nth(1),
        ..Filter::default()
    };
    let (found, errs) = kdi::find(&filter);
    for e in &errs {
        eprintln!("enumeration error: {e}");
    }
    let Some(info) = found.first() else {
        eprintln!("no device found - connect an instrument and build with --features usb3");
        return Ok(());
    };

    // Refuse the bind up front if the device cannot do what this program needs. A CAPABILITY, not
    // a version comparison: a KDI minor is additive, so a version says nothing about what a build
    // actually implements.
    let mut dev = Device::open(
        info,
        &ConnectOpts {
            need_caps: vec![Cap::CommandProtocol],
            ..ConnectOpts::default()
        },
    )?;

    let (major, minor) = dev.kdi();
    println!(
        "{}  contract {major}.{minor}  gateware {:08x}  lease {:?}",
        info.serial,
        dev.gateware_sha(),
        dev.lease(),
    );
    let caps: Vec<&str> = dev.caps().iter().map(Cap::token).collect();
    println!("caps: {}", caps.join(", "));

    // ── typed commands: the reply is a struct, and a missing key is an error rather than a zero ──
    let hello = dev.sys_hello()?;
    println!(
        "sys.hello: proto {} cmdset {} kdi {} board 0x{:x} fw {} gw {}",
        hello.proto, hello.cmdset, hello.kdi, hello.board_id, hello.fw, hello.gw,
    );

    let power = dev.power_status()?;
    println!(
        "power.status: present 0x{:02x} reverify {}",
        power.present, power.reverify,
    );

    // `valid` is a HARDWARE flag (bit 16 of the sample register), not a transport check, and
    // `false` here is the expected reading rather than a fault: a conversion only completes on a
    // channel that is in `adc` mode, and this program deliberately does not set a mode on a
    // populated slot -- that would reconfigure someone else's instrument. Measured on silicon with
    // slot 0 seated: `codes [0, 0] valid [false, false]`. Read a code as data only when its
    // `valid` is true; the pair is reported together for exactly that reason.
    if let Some(slot) = (0..8).find(|s| power.present >> s & 1 != 0) {
        let adc = dev.adio_adc(slot, 0, Some(2))?;
        println!(
            "adio.adc slot {} ch {}: codes {:?} valid {:?}",
            adc.slot, adc.ch, adc.codes, adc.valid,
        );
    }

    // ── what a refusal looks like ────────────────────────────────────────────────────────────
    // `Device::raw_cmd` would hand a device error back as `Ok(Reply)` — a device error is data —
    // but a TYPED method was asked for an `AdioMode`, and a refusal contains none, so it arrives
    // as `Err(Error::Device)`.
    //
    // THE MODE MATTERS. The firmware gates on DRIVING modes only: `out`/`dac` against a slot the
    // power tree says is empty is refused `not_present`, while a non-driving mode is allowed
    // because it drives nothing (sw/zephyr/app/console/src/kdi_ctl.c:269-277). An earlier version
    // of this example asked for `Off, Off` here and called it a refusal, so on real hardware it
    // silently took the Ok branch and demonstrated nothing — measured on silicon as
    // `adio.mode slot 1: ch_mode 0x0000`, printed by the success arm of a block titled "what a
    // refusal looks like".
    //
    // Asking for a DRIVE on that same empty slot is the refusal, and it is safe by construction:
    // the firmware decides with the SAME present-mask this program just read, so a slot that reads
    // empty here is refused there before any pin is driven.
    if let Some(empty) = (0..8).find(|s| power.present >> s & 1 == 0) {
        match dev.adio_mode(empty, ChMode::Out, ChMode::Off) {
            // Reaching here means the safety gate did NOT fire and a drive was configured on a
            // slot the power tree calls empty. Say so; a success is the alarming outcome.
            Ok(m) => println!(
                "adio.mode slot {}: NOT REFUSED, ch_mode 0x{:04x} - the not_present gate did not \
                 fire for a driving mode",
                m.slot, m.ch_mode
            ),
            Err(Error::Device(r)) => {
                let err = r.err;
                println!(
                    "adio.mode slot {empty} refused: {} (rc {}, retryable {})",
                    err.map_or("?", DeviceErr::token),
                    r.rc,
                    err.is_some_and(DeviceErr::retryable),
                );
            }
            Err(e) => return Err(e),
        }
    }

    // A range the contract declares is checked HOST-SIDE: `n` is 1..=16, so this never reaches the
    // wire and costs no round trip to learn.
    match dev.adio_adc(0, 0, Some(99)) {
        Err(Error::Host(h)) => println!("adio.adc n=99 refused before the wire: {}", h.token()),
        other => println!("expected a host-side refusal, got {other:?}"),
    }

    dev.close()
}
