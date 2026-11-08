//! Bridge between Reticulum's `PipeInterface` (HDLC over stdin/stdout) and a
//! raw Ethernet interface.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use rtp::frame::{self, GROUP};
use rtp::hdlc;
use rtp::link::RawLink;

const USAGE: &str = "usage: rtp [-v] <interface>";

struct Args {
    ifname: String,
    verbose: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut ifname = None;
    let mut verbose = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            s if ifname.is_none() => ifname = Some(s.to_owned()),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Args { ifname: ifname.ok_or(USAGE)?, verbose })
}

macro_rules! log {
    ($($t:tt)*) => { eprintln!("[rtp] {}", format!($($t)*)) };
}

/// Ethernet -> stdout. Returns only on a fatal error.
fn rx_loop(link: &RawLink, verbose: bool) -> io::Error {
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(4096);
    let mut stdout = io::stdout().lock();
    loop {
        let n = match link.recv(&mut buf) {
            Ok(n) => n,
            Err(e) => return e,
        };
        let f = match frame::parse(&buf[..n]) {
            Ok(f) => f,
            Err(e) => {
                if verbose {
                    log!("dropped inbound frame: {e}");
                }
                continue;
            }
        };
        if verbose {
            log!("rx {} bytes from {}", f.payload.len(), frame::fmt_mac(&f.src));
        }
        out.clear();
        hdlc::encode_into(f.payload, &mut out);
        if let Err(e) = stdout.write_all(&out).and_then(|_| stdout.flush()) {
            return e;
        }
    }
}

/// stdin -> Ethernet. Returns when stdin closes or on a fatal error.
fn tx_loop(link: &RawLink, verbose: bool) -> io::Result<()> {
    let src = link.mac();
    let mut dec = hdlc::Decoder::new(frame::MAX_PAYLOAD);
    let mut buf = [0u8; 4096];
    let mut stdin = io::stdin().lock();
    loop {
        let n = match stdin.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        for &b in &buf[..n] {
            let Some(payload) = dec.push(b) else { continue };
            let eth = match frame::build(&GROUP, &src, &payload) {
                Ok(f) => f,
                Err(e) => {
                    log!("dropped outbound packet: {e}");
                    continue;
                }
            };
            link.send(&eth)?;
            if verbose {
                log!("tx {} bytes", payload.len());
            }
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(2);
        }
    };

    let link = match RawLink::open(&args.ifname) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            log!("cannot open {}: {e}", args.ifname);
            if e.kind() == io::ErrorKind::PermissionDenied {
                log!("hint: grant CAP_NET_RAW, e.g. `setcap cap_net_raw+ep <binary>`");
            }
            return ExitCode::FAILURE;
        }
    };
    log!(
        "up on {} (ifindex {}, mac {}, ethertype {:#06x})",
        args.ifname,
        link.ifindex(),
        frame::fmt_mac(&link.mac()),
        frame::ETHERTYPE
    );

    let rx_link = Arc::clone(&link);
    let verbose = args.verbose;
    thread::spawn(move || {
        let e = rx_loop(&rx_link, verbose);
        log!("receive loop failed: {e}");
        std::process::exit(1);
    });

    match tx_loop(&link, args.verbose) {
        Ok(()) => {
            log!("stdin closed, exiting");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log!("transmit loop failed: {e}");
            ExitCode::FAILURE
        }
    }
}
