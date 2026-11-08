//! Bridge between Reticulum and a raw Ethernet interface.
//!
//! Two upstream modes are supported:
//!
//! - HDLC (default): packets framed with simplified HDLC over stdin/stdout,
//!   as Reticulum's built-in `PipeInterface` expects.
//! - Packet (`--packet`): stdin/stdout is a `SOCK_SEQPACKET` socket and each
//!   message is one packet, as used by the `RtpInterface` module.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::sync::Arc;
use std::thread;

use rtp::frame::{self, GROUP};
use rtp::hdlc;
use rtp::link::{self, RawLink};
use rtp::packet;

const USAGE: &str = "usage: rtp [-v] [--packet] <interface>";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Upstream {
    Hdlc,
    Packet,
}

struct Args {
    ifname: String,
    verbose: bool,
    upstream: Upstream,
}

fn parse_args() -> Result<Args, String> {
    let mut ifname = None;
    let mut verbose = false;
    let mut upstream = Upstream::Hdlc;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--packet" => upstream = Upstream::Packet,
            "-h" | "--help" => return Err(USAGE.into()),
            s if s.starts_with('-') => return Err(format!("unknown option {s}\n{USAGE}")),
            s if ifname.is_none() => ifname = Some(s.to_owned()),
            _ => return Err(USAGE.into()),
        }
    }
    Ok(Args { ifname: ifname.ok_or(USAGE)?, verbose, upstream })
}

macro_rules! log {
    ($($t:tt)*) => { eprintln!("[rtp] {}", format!($($t)*)) };
}

/// Ethernet -> upstream. Returns only on a fatal error.
///
/// While the interface is down the socket is kept: once it comes back up,
/// traffic resumes on it. If the interface is removed, this returns so the
/// process exits and gets restarted, reopening the interface by name.
fn rx_loop(link: &RawLink, upstream: Upstream, verbose: bool) -> io::Error {
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(4096);
    let mut stdout = io::stdout().lock();
    let mut down = false;
    loop {
        let n = match link.recv(&mut buf) {
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock || link::is_link_down(&e) => {
                if !link.still_present() {
                    return io::Error::new(io::ErrorKind::NotFound, "interface was removed");
                }
                if link::is_link_down(&e) && !down {
                    log!("interface down ({e}), waiting for it to come back");
                    down = true;
                }
                continue;
            }
            Err(e) => return e,
        };
        if down {
            log!("interface back up");
            down = false;
        }
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
        let res = match upstream {
            Upstream::Packet => packet::send(packet::STDOUT, f.payload),
            Upstream::Hdlc => {
                out.clear();
                hdlc::encode_into(f.payload, &mut out);
                stdout.write_all(&out).and_then(|_| stdout.flush())
            }
        };
        if let Err(e) = res {
            return e;
        }
    }
}

/// Sends one packet from upstream as an Ethernet frame.
///
/// While the interface is down, packets are dropped instead of failing:
/// Reticulum already copes with loss. `dropping` tracks that state so the
/// outage is logged once, not once per packet.
fn transmit(
    link: &RawLink,
    src: &frame::Mac,
    payload: &[u8],
    dropping: &mut bool,
    verbose: bool,
) -> io::Result<()> {
    let eth = match frame::build(&GROUP, src, payload) {
        Ok(f) => f,
        Err(e) => {
            log!("dropped outbound packet: {e}");
            return Ok(());
        }
    };
    match link.send(&eth) {
        Ok(()) => {
            if *dropping {
                log!("sending again");
                *dropping = false;
            }
        }
        Err(e) if link::is_link_down(&e) => {
            if !*dropping {
                log!("cannot send ({e}), dropping packets until the interface is back");
                *dropping = true;
            }
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    if verbose {
        log!("tx {} bytes", payload.len());
    }
    Ok(())
}

/// HDLC stdin -> Ethernet. Returns when stdin closes or on a fatal error.
fn tx_loop_hdlc(link: &RawLink, verbose: bool) -> io::Result<()> {
    let src = link.mac();
    let mut dropping = false;
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
            if let Some(payload) = dec.push(b) {
                transmit(link, &src, &payload, &mut dropping, verbose)?;
            }
        }
    }
}

/// Packet stdin -> Ethernet. Returns when the peer closes or on a fatal error.
fn tx_loop_packet(link: &RawLink, verbose: bool) -> io::Result<()> {
    let src = link.mac();
    let mut dropping = false;
    let mut buf = vec![0u8; frame::MAX_PAYLOAD + 1];
    loop {
        let n = packet::recv(packet::STDIN, &mut buf)?;
        if n == 0 {
            return Ok(());
        }
        if n > frame::MAX_PAYLOAD {
            log!("dropped outbound packet: {n} bytes exceeds {}", frame::MAX_PAYLOAD);
            continue;
        }
        transmit(link, &src, &buf[..n], &mut dropping, verbose)?;
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

    if args.upstream == Upstream::Packet
        && let Err(e) = packet::check_seqpacket(packet::STDIN)
    {
        log!("--packet needs stdin/stdout to be a SOCK_SEQPACKET socket: {e}");
        return ExitCode::from(2);
    }

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
        "up on {} (ifindex {}, mac {}, ethertype {:#06x}, {} upstream)",
        args.ifname,
        link.ifindex(),
        frame::fmt_mac(&link.mac()),
        frame::ETHERTYPE,
        match args.upstream {
            Upstream::Hdlc => "hdlc",
            Upstream::Packet => "packet",
        }
    );

    if !link.group_joined() {
        log!(
            "warning: could not join group {} (no SOL_PACKET support, e.g. qemu user mode); \
             frames from other nodes will only arrive if the interface does not filter multicast",
            frame::fmt_mac(&GROUP)
        );
    }

    let rx_link = Arc::clone(&link);
    let (upstream, verbose) = (args.upstream, args.verbose);
    thread::spawn(move || {
        let e = rx_loop(&rx_link, upstream, verbose);
        log!("receive loop failed: {e}");
        std::process::exit(1);
    });

    let res = match args.upstream {
        Upstream::Hdlc => tx_loop_hdlc(&link, args.verbose),
        Upstream::Packet => tx_loop_packet(&link, args.verbose),
    };
    match res {
        Ok(()) => {
            log!("upstream closed, exiting");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log!("transmit loop failed: {e}");
            ExitCode::FAILURE
        }
    }
}
