//! RoE, Reticulum over Ethernet: carry [Reticulum] packets directly in
//! Ethernet frames, without IP.
//!
//! This crate provides the `roe` binary, which Reticulum runs as a helper
//! process through the `RoeInterface` module (see the [repository] for how to
//! set that up). The building blocks of `roe` are also available as a
//! library, for instance for a Reticulum implementation in Rust that wants to
//! speak RoE directly instead of going through the helper.
//!
//! # Modules
//!
//! - [`frame`]: the RoE frame format. Builds and parses Ethernet frames
//!   carrying one Reticulum packet each. Pure, works anywhere.
//! - [`link`]: a raw `AF_PACKET` socket bound to one interface, which sends
//!   and receives RoE frames. Linux only, and opening it needs `CAP_NET_RAW`.
//! - [`neighbors`]: learns which neighbor each Reticulum packet is for,
//!   from the packets received, so that `roe` can send it by unicast
//!   instead of to the multicast group. Pure, works anywhere.
//! - [`hdlc`] and [`packet`]: the two framings `roe` uses towards Reticulum,
//!   over a pipe (HDLC, for Reticulum's `PipeInterface`) or over a
//!   `SOCK_SEQPACKET` socket (for `RoeInterface`).
//!
//! The wire format is specified in [docs/protocol.md].
//!
//! # Examples
//!
//! Building and parsing a frame:
//!
//! ```
//! use reticulum_over_ethernet::frame::{self, GROUP};
//!
//! let src = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
//! let eth = frame::build(&GROUP, &src, b"a Reticulum packet")?;
//!
//! let parsed = frame::parse(&eth)?;
//! assert_eq!(parsed.src, src);
//! assert_eq!(parsed.payload, b"a Reticulum packet");
//! # Ok::<(), frame::Error>(())
//! ```
//!
//! Sending and receiving on an interface (Linux, with `CAP_NET_RAW`):
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! use std::io::ErrorKind;
//! use reticulum_over_ethernet::frame::{self, GROUP};
//! use reticulum_over_ethernet::link::{self, RawLink};
//!
//! let link = RawLink::open("eth0")?;
//! // Packets larger than this do not fit the interface MTU.
//! let limit = frame::max_payload_for(link.mtu()?);
//!
//! let packet = b"a Reticulum packet";
//! assert!(packet.len() <= limit);
//! link.send(&frame::build(&GROUP, &link.mac(), packet)?)?;
//!
//! let mut buf = [0u8; 2048];
//! loop {
//!     match link.recv(&mut buf) {
//!         Ok(n) => match frame::parse(&buf[..n]) {
//!             Ok(f) => println!("{} bytes from {:02x?}", f.payload.len(), f.src),
//!             Err(e) => eprintln!("ignoring frame: {e}"),
//!         },
//!         // Nothing received for a while, or the interface is down: check
//!         // whether it still exists, as `roe` does, then keep waiting.
//!         Err(e) if e.kind() == ErrorKind::WouldBlock || link::is_link_down(&e) => {
//!             if !link.still_present() {
//!                 return Err("interface removed".into());
//!             }
//!         }
//!         Err(e) => return Err(e.into()),
//!     }
//! }
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```
//!
//! # Stability
//!
//! RoE is experimental. Until 1.0, minor versions may change both this API
//! and the wire format (which carries its own [version](frame::VERSION)).
//!
//! [Reticulum]: https://reticulum.network
//! [repository]: https://github.com/ogarcia/roe
//! [docs/protocol.md]: https://github.com/ogarcia/roe/blob/master/docs/protocol.md

#![warn(missing_docs)]

pub mod frame;
pub mod hdlc;
#[cfg(target_os = "linux")]
pub mod link;
pub mod neighbors;
pub mod packet;
mod sha256;
