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

/// Sending side state shared by both upstream modes.
struct Tx<'a> {
    link: &'a RawLink,
    src: frame::Mac,
    /// Largest payload the interface MTU allows.
    limit: usize,
    /// Set while the interface is down, so the outage is logged once.
    dropping: bool,
    verbose: bool,
}

impl<'a> Tx<'a> {
    fn new(link: &'a RawLink, limit: usize, verbose: bool) -> Self {
        Self { link, src: link.mac(), limit, dropping: false, verbose }
    }

    /// Sends one packet from upstream as an Ethernet frame.
    ///
    /// While the interface is down, packets are dropped instead of failing:
    /// Reticulum already copes with loss. Packets larger than the interface
    /// MTU allows are dropped too. If the MTU shrinks while running, this
    /// fails so that rtp exits and is restarted, letting RtpInterface adapt.
    fn send(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.len() > self.limit {
            log!("dropped outbound packet: {} bytes exceeds {} (interface MTU)", payload.len(), self.limit);
            return Ok(());
        }
        let eth = match frame::build(&GROUP, &self.src, payload) {
            Ok(f) => f,
            Err(e) => {
                log!("dropped outbound packet: {e}");
                return Ok(());
            }
        };
        match self.link.send(&eth) {
            Ok(()) => {
                if self.dropping {
                    log!("sending again");
                    self.dropping = false;
                }
            }
            Err(e) if link::is_link_down(&e) => {
                if !self.dropping {
                    log!("cannot send ({e}), dropping packets until the interface is back");
                    self.dropping = true;
                }
                return Ok(());
            }
            Err(e) if e.raw_os_error() == Some(libc::EMSGSIZE) => {
                let mtu = self.link.mtu().map_or_else(|e| format!("unknown ({e})"), |m| m.to_string());
                return Err(io::Error::other(format!(
                    "frame too large, interface MTU is now {mtu}; restarting to adapt"
                )));
            }
            Err(e) => return Err(e),
        }
        if self.verbose {
            log!("tx {} bytes", payload.len());
        }
        Ok(())
    }
}

/// HDLC stdin -> Ethernet. Returns when stdin closes or on a fatal error.
fn tx_loop_hdlc(mut tx: Tx) -> io::Result<()> {
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
                tx.send(&payload)?;
            }
        }
    }
}

/// Packet stdin -> Ethernet. Returns when the peer closes or on a fatal error.
fn tx_loop_packet(mut tx: Tx) -> io::Result<()> {
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
        tx.send(&buf[..n])?;
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
    let limit = match link.mtu() {
        Ok(mtu) => frame::max_payload_for(mtu),
        Err(e) => {
            log!("cannot read the MTU of {}: {e}", args.ifname);
            return ExitCode::FAILURE;
        }
    };
    if limit < frame::RETICULUM_MTU {
        log!(
            "interface MTU of {} is too small: Reticulum needs packets of {} bytes, \
             which take an MTU of at least {}",
            args.ifname,
            frame::RETICULUM_MTU,
            frame::RETICULUM_MTU + frame::RTP_HEADER_LEN
        );
        return ExitCode::FAILURE;
    }
    if limit < frame::MAX_PAYLOAD {
        log!("interface MTU limits packets to {limit} bytes");
    }

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

    let tx = Tx::new(&link, limit, args.verbose);
    let res = match args.upstream {
        Upstream::Hdlc => tx_loop_hdlc(tx),
        Upstream::Packet => tx_loop_packet(tx),
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
