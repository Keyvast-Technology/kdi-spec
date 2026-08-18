//! Load a caller-supplied bitstream onto a board, bind it, and print what came back.
//!
//! ```text
//! cargo run --features usb3 --example usb3_configure -- <bit> [serial]
//! ```
//!
//! `image` is **the** release bitstream — `make all` / the GitHub release asset — not a second
//! copy this crate vendors. Configuration is **volatile**: it does not touch flash and a power
//! cycle undoes it. It is still state-changing on a shared instrument — whatever was running is
//! gone until someone loads it again — so unlike `usb3_handshake`, this is not safe to run against
//! a board someone else is using.
//!
//! The path is required and the example fails closed if it is missing. The configure's status is
//! checked inside `open_usb3_configured`; a failure names the driver status and leaves the board
//! on whatever was resident. This program does not compare the identity against a compiled-in
//! constant — without a known image the library cannot know the WireOut sha. It prints
//! `gateware_sha` / `contract` / `fw_sha` so a caller (or `make kdi-rs-bench`) can compare them
//! to the artifact that was just sent.

use std::fs;
use std::process::ExitCode;

use kdi::Commands;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(bit) = args.next() else {
        eprintln!("usage: usb3_configure <bit> [serial]");
        return ExitCode::FAILURE;
    };
    let serial = args.next().unwrap_or_default();

    let image = match fs::read(&bit) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("{bit}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut dev = match kdi::Device::open_usb3_configured(&serial, &image) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("configure + open failed: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (major, minor) = dev.kdi();
    let sha = dev.gateware_sha();
    let caps = dev.caps();
    println!("serial          {}", dev.info().serial);
    println!("contract        {major}.{minor}");
    println!("gateware_sha    {sha:08x}");
    println!("caps            0x{:08x}", caps.0);
    if caps.has(kdi::Cap::CommandProtocol) {
        match dev.sys_hello() {
            Ok(h) => println!("fw_sha          {}", h.fw),
            Err(e) => {
                eprintln!("sys.hello failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        println!("fw_sha          -");
    }

    if let Err(e) = dev.close() {
        eprintln!("close: {e}");
        return ExitCode::FAILURE;
    }
    println!("configure OK");
    ExitCode::SUCCESS
}
