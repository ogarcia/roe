//! An echo server on rsReticulum, reachable over RoE.
//!
//! rsReticulum creates its interfaces from a Reticulum configuration file,
//! and it has a `PipeInterface` like Python's: it runs a command and
//! exchanges HDLC-framed packets with it over standard input and output.
//! That is the default mode of `roe`, so this program only writes such a
//! configuration:
//!
//! ```ini
//! [[RoE]]
//!   type = PipeInterface
//!   enabled = yes
//!   command = roe eth0
//! ```
//!
//! It then announces the destination `roe_example.echo` every few seconds,
//! accepts links to it and sends every message received on a link back.
//!
//! Usage: [ROE=/path/to/roe] roe-example-rsreticulum-process <device>
//!
//! `roe` is taken from `$ROE`, or from `PATH`. It needs `CAP_NET_RAW`; this
//! program does not.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use rns_runtime::lifecycle::install_signal_handlers;
use rns_runtime::prelude::*;

const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Writes a configuration directory with RoE as the only interface.
fn write_config(device: &str, roe: &str) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir().join(format!("roe-example-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("config"),
        format!(
            "[reticulum]
  enable_transport = No
  share_instance = No

[logging]
  loglevel = 4

[interfaces]
  [[RoE]]
    type = PipeInterface
    enabled = yes
    command = {roe} {device}
    respawn_delay = 2
"
        ),
    )?;
    Ok(dir)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[tokio::main]
async fn main() -> Result {
    let args: Vec<String> = std::env::args().collect();
    let [_, device] = &args[..] else {
        eprintln!("usage: [ROE=/path/to/roe] {} <device>", args[0]);
        std::process::exit(2);
    };
    let roe = std::env::var("ROE").unwrap_or_else(|_| "roe".into());
    let config_dir = write_config(device, &roe)?;

    let shutdown = ShutdownSignal::new();
    let _signals = install_signal_handlers(shutdown.clone());
    let runtime = init(
        config_dir.to_str(),
        None,
        shutdown.clone(),
        Arc::new(AtomicBool::new(true)),
    )
    .await?;

    let mut destination = runtime
        .register_destination(
            Identity::new(),
            "roe_example.echo".to_string(),
            DestinationRuntimeOptions::default(),
        )
        .await?;
    // Printed on its own line so that scripts can pick it up.
    println!(
        "echo destination {}",
        hex(&destination.handle.destination_hash())
    );

    let mut announce = tokio::time::interval(ANNOUNCE_INTERVAL);
    loop {
        tokio::select! {
            _ = shutdown.wait() => break,
            _ = announce.tick() => {
                destination.handle.announce(DestinationAnnounceOptions::default()).await?;
            }
            packet = destination.events.link_packets.recv() => {
                let Some((data, link_id)) = packet else { break };
                eprintln!("link {}: echoing {} bytes", hex(&link_id), data.len());
                destination.handle.send_link_packet(link_id, data).await?;
            }
        }
    }

    destination.close().await?;
    runtime.shutdown_and_wait().await;
    let _ = std::fs::remove_dir_all(config_dir);
    Ok(())
}
