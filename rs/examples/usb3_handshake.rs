//! Open a real board over USB3 and read its handshake — the first ten minutes on Windows.
//!
//! ```text
//! cargo run --features usb3 --example usb3_handshake -- [serial] [driver-dir]
//! ```
//!
//! This is the smallest program that proves the `usb3` binding works end to end: it resolves the
//! device driver, opens the named board (or the first one found), and reads the four identity
//! registers **by name** through the generated binding — no endpoint number appears here, which is
//! the whole point of the descriptor indirection.
//!
//! It is also how the `bundled` feature is tested: built with it, this runs from a directory that
//! has no driver beside it and on a machine with none installed. That is a TEST of the mechanism,
//! not the shape of the product — what ships is a library a customer links.
//!
//! **It does NOT flash.** `open_usb3` binds whatever bitstream is already running, because a host
//! that configures the FPGA before reading its identity has learned nothing about the device it
//! found. Nothing here is
//! state-changing, so it is safe to run against a shared instrument — which is exactly what
//! `usb3_configure`, the example that DOES load an image, is not.
//!
//! A device on a newer contract MINOR binds normally and is expected: this prints what it read
//! rather than asserting a version, so the same binary is useful against any board in the family.
//! Only a MAJOR mismatch refuses, inside `open_usb3`.

use kdi::Commands;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args().skip(1);
    let serial = args.next().unwrap_or_default();
    let driver_dir = args.next();

    let opened = kdi::Device::open_usb3(&serial, driver_dir.as_deref().map(std::path::Path::new));

    let mut dev = match opened {
        Ok(d) => d,
        Err(e) => {
            // Naming the failure matters more than the exit code: "no board" and "the driver is
            // present but a symbol did not resolve" are the two outcomes worth telling apart, and
            // every entry point is resolved by name at dlopen precisely so the second one reports
            // which symbol rather than failing to link.
            eprintln!("could not open the board: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let (major, minor) = dev.kdi();
    let caps = dev.caps();
    println!("serial          {}", dev.info().serial);
    println!("transport       {:?}", dev.info().transport);
    println!("contract        {major}.{minor}");
    println!("caps            0x{:08x}", caps.0);
    for c in caps.iter() {
        println!("                  {}", c.token());
    }
    println!("gateware_sha    {:08x}", dev.gateware_sha());
    if caps.has(kdi::Cap::CommandProtocol) {
        match dev.sys_hello() {
            Ok(h) => println!("fw_sha          {}", h.fw),
            Err(e) => {
                eprintln!("sys.hello failed: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    } else {
        println!("fw_sha          -");
    }

    match dev.wait_ready(std::time::Duration::from_millis(kdi::READY_TIMEOUT_MS)) {
        Ok(()) => println!("contract_ready  yes"),
        Err(e) => {
            // Not fatal for a handshake read: a board still calibrating is a normal transient, and
            // the point of this program is to report what the device says.
            println!("contract_ready  NO ({e})");
        }
    }

    if let Err(e) = dev.close() {
        eprintln!("close: {e}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
