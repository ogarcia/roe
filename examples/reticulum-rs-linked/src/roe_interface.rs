//! A Reticulum-rs interface that speaks RoE itself, with the
//! reticulum-over-ethernet crate: no helper process, so this program needs
//! `CAP_NET_RAW`.
//!
//! - [`RawLink`] is the raw socket on the device;
//! - [`frame`] builds and parses RoE frames;
//! - [`Neighbors`] picks the MAC address of the neighbor each packet is for,
//!   or the RoE group, as `roe` does.

use std::io::{self, ErrorKind};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use reticulum::buffer::{InputBuffer, OutputBuffer};
use reticulum::iface::{Interface, InterfaceContext, RxMessage};
use reticulum::packet::Packet;
use reticulum::serde::Serialize;
use reticulum_over_ethernet::frame::{self, GROUP};
use reticulum_over_ethernet::link::{self, RawLink};
use reticulum_over_ethernet::neighbors::Neighbors;

pub struct RoeInterface {
    device: String,
}

impl RoeInterface {
    pub fn new(device: impl Into<String>) -> Self {
        Self {
            device: device.into(),
        }
    }

    /// The interface task, for `InterfaceManager::spawn`.
    pub async fn spawn(context: InterfaceContext<Self>) {
        let device = context.inner.lock().unwrap().device.clone();
        let address = context.channel.address;
        let cancel = context.cancel.clone();
        let (rx_channel, mut tx_channel) = context.channel.split();

        let link = match RawLink::open(&device) {
            Ok(link) => Arc::new(link),
            Err(e) => {
                log::error!("roe: cannot open {device}: {e} (CAP_NET_RAW is needed)");
                return;
            }
        };
        let limit = match link.mtu() {
            Ok(mtu) => frame::max_payload_for(mtu),
            Err(e) => {
                log::error!("roe: cannot read the MTU of {device}: {e}");
                return;
            }
        };
        let neighbors = Arc::new(Mutex::new(Neighbors::new()));
        log::info!(
            "roe: up on {device}, mac {}, packets up to {limit} bytes",
            frame::fmt_mac(&link.mac())
        );

        // Receiving blocks, with a one-second timeout, so it gets a thread.
        let rx = {
            let (link, neighbors, cancel) = (link.clone(), neighbors.clone(), cancel.clone());
            let device = device.clone();
            tokio::task::spawn_blocking(move || {
                let mut buf = [0u8; 2048];
                while !cancel.is_cancelled() {
                    let n = match link.recv(&mut buf) {
                        Ok(n) => n,
                        Err(e) if e.kind() == ErrorKind::WouldBlock || link::is_link_down(&e) => {
                            if !link.still_present() {
                                log::error!("roe: {device} was removed");
                                return;
                            }
                            continue;
                        }
                        Err(e) => {
                            log::error!("roe: receive failed: {e}");
                            return;
                        }
                    };
                    let Ok(f) = frame::parse(&buf[..n]) else {
                        continue;
                    };
                    neighbors
                        .lock()
                        .unwrap()
                        .learn(&f.src, f.payload, Instant::now());
                    match Packet::deserialize(&mut InputBuffer::new(f.payload)) {
                        Ok(packet) => {
                            let _ = rx_channel.blocking_send(RxMessage { address, packet });
                        }
                        Err(_) => log::debug!("roe: dropped a packet that did not decode"),
                    }
                }
            })
        };

        let mut out = vec![0u8; frame::MAX_PAYLOAD];
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                Some(message) = tx_channel.recv() => {
                    let mut output = OutputBuffer::new(&mut out);
                    if message.packet.serialize(&mut output).is_err() {
                        log::warn!("roe: dropped a packet that did not fit a frame");
                        continue;
                    }
                    let payload = output.as_slice();
                    if payload.len() > limit {
                        log::warn!("roe: dropped a {}-byte packet, over the interface MTU", payload.len());
                        continue;
                    }
                    let dst = neighbors
                        .lock()
                        .unwrap()
                        .next_hop(payload, Instant::now())
                        .unwrap_or(GROUP);
                    if let Err(e) = frame::build(&dst, &link.mac(), payload)
                        .map_err(io::Error::other)
                        .and_then(|eth| link.send(&eth))
                    {
                        log::warn!("roe: send failed: {e}");
                    }
                }
            }
        }
        let _ = rx.await;
    }
}

impl Interface for RoeInterface {
    fn mtu() -> usize {
        frame::MAX_PAYLOAD
    }
}
