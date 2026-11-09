//! An echo server on Reticulum-rs, reachable over RoE.
//!
//! Announces the destination `roe_example.echo` every few seconds, accepts
//! links to it and sends every message received on a link back on it.
//!
//! Usage: roe-example-reticulum-rs-linked <device>
//!
//! RoE is linked in, so this program needs `CAP_NET_RAW` itself.

mod roe_interface;

use std::time::Duration;

use rand_core::OsRng;
use reticulum::destination::DestinationName;
use reticulum::destination::link::LinkEvent;
use reticulum::identity::PrivateIdentity;
use reticulum::transport::{Transport, TransportConfig};

use roe_interface::RoeInterface;

const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    let [_, device] = &args[..] else {
        eprintln!("usage: {} <device>", args[0]);
        std::process::exit(2);
    };

    let id = PrivateIdentity::new_from_rand(OsRng);
    let mut transport = Transport::new(TransportConfig::new("echo", &id));
    transport
        .iface_manager()
        .lock()
        .await
        .spawn(RoeInterface::new(device.as_str()), RoeInterface::spawn);

    let destination = transport
        .add_destination(id, DestinationName::new("roe_example", "echo"))
        .await;
    // Printed on its own line so that scripts can pick it up.
    println!(
        "echo destination {}",
        destination.lock().await.desc.address_hash
    );

    let mut links = transport.in_link_events();
    let mut announce = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = announce.tick() => transport.send_announce(&destination, None).await,
            event = links.recv() => {
                let Ok(event) = event else { continue };
                match event.event {
                    LinkEvent::Activated => log::info!("link {} activated", event.id),
                    LinkEvent::Data(payload) => {
                        log::info!("link {}: echoing {} bytes", event.id, payload.len());
                        if let Some(link) = transport.find_in_link(&event.id).await {
                            let packet = link.lock().await.data_packet(payload.as_slice());
                            if let Ok(packet) = packet {
                                transport.send_packet(packet).await;
                            }
                        }
                    }
                    LinkEvent::Closed => log::info!("link {} closed", event.id),
                    _ => {}
                }
            }
        }
    }
}
