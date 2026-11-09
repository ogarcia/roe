//! A Reticulum-rs interface that sends and receives packets through the
//! `roe` helper process.
//!
//! This is the same arrangement as `RoeInterface.py`: a `SOCK_SEQPACKET`
//! socket pair, one end kept here and the other given to
//! `roe --packet <device>` as its standard input and output. Each message on
//! the socket is one Reticulum packet. Only `roe` needs `CAP_NET_RAW`.
//!
//! Nothing from the roe crate is linked in: the protocol between the two
//! processes is all there is to it (docs/protocol.md, "Packet mode").

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use reticulum::buffer::{InputBuffer, OutputBuffer};
use reticulum::iface::{Interface, InterfaceContext, RxMessage};
use reticulum::packet::Packet;
use reticulum::serde::Serialize;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

/// Largest packet RoE carries in a standard 1500-byte Ethernet frame.
const ROE_MAX_PAYLOAD: usize = 1496;
const RESPAWN_DELAY: Duration = Duration::from_secs(2);

pub struct RoeInterface {
    binary: String,
    device: String,
}

impl RoeInterface {
    /// `binary` is the path to `roe`, `device` the network interface.
    pub fn new(binary: impl Into<String>, device: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
            device: device.into(),
        }
    }

    /// The interface task, for `InterfaceManager::spawn`. Restarts `roe`
    /// whenever it exits, until the transport shuts down.
    pub async fn spawn(context: InterfaceContext<Self>) {
        let (binary, device) = {
            let inner = context.inner.lock().unwrap();
            (inner.binary.clone(), inner.device.clone())
        };
        let address = context.channel.address;
        let cancel = context.cancel.clone();
        let (rx_channel, mut tx_channel) = context.channel.split();

        while !cancel.is_cancelled() {
            let (sock, mut child) = match start(&binary, &device) {
                Ok(started) => started,
                Err(e) => {
                    log::error!("roe: cannot start {binary}: {e}");
                    tokio::time::sleep(RESPAWN_DELAY).await;
                    continue;
                }
            };
            log::info!("roe: started {binary} --packet {device}");

            let mut buf = vec![0u8; ROE_MAX_PAYLOAD + 1];
            let mut out = vec![0u8; ROE_MAX_PAYLOAD];
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    // roe -> Reticulum-rs
                    n = recv(&sock, &mut buf) => match n {
                        Ok(0) => {
                            log::warn!("roe: helper closed its end");
                            break;
                        }
                        Ok(n) => match Packet::deserialize(&mut InputBuffer::new(&buf[..n])) {
                            Ok(packet) => {
                                let _ = rx_channel.send(RxMessage { address, packet }).await;
                            }
                            Err(_) => log::debug!("roe: dropped a packet that did not decode"),
                        },
                        Err(e) => {
                            log::warn!("roe: receive failed: {e}");
                            break;
                        }
                    },
                    // Reticulum-rs -> roe
                    Some(message) = tx_channel.recv() => {
                        let mut output = OutputBuffer::new(&mut out);
                        if message.packet.serialize(&mut output).is_err() {
                            log::warn!("roe: dropped a packet that did not fit a frame");
                            continue;
                        }
                        if let Err(e) = send(&sock, output.as_slice()).await {
                            log::warn!("roe: send failed: {e}");
                            break;
                        }
                    }
                }
            }

            let _ = child.kill();
            let _ = child.wait();
            if !cancel.is_cancelled() {
                tokio::time::sleep(RESPAWN_DELAY).await;
            }
        }
    }
}

impl Interface for RoeInterface {
    fn mtu() -> usize {
        ROE_MAX_PAYLOAD
    }
}

/// Creates the socket pair and starts `roe` on one end of it.
fn start(binary: &str, device: &str) -> io::Result<(AsyncFd<OwnedFd>, Child)> {
    let mut fds = [0; 2];
    // SAFETY: fds has room for the two descriptors socketpair returns.
    let r = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if r < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: both descriptors were just created and are owned by nobody else.
    let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };

    let child = Command::new(binary)
        .arg("--packet")
        .arg(device)
        .stdin(Stdio::from(theirs.try_clone()?))
        .stdout(Stdio::from(theirs))
        .spawn()?;

    // SAFETY: ours is a valid descriptor; fcntl only changes its flags.
    unsafe {
        let flags = libc::fcntl(ours.as_raw_fd(), libc::F_GETFL);
        libc::fcntl(ours.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    // SAFETY: an OwnedFd always returns the same, open descriptor until it
    // is dropped, and the AsyncFd owns it from here on.
    let sock = unsafe { AsyncFd::register(ours) }.map_err(|e| e.into_parts().1)?;
    Ok((sock, child))
}

/// Receives one packet. A message longer than `buf` would be truncated, so
/// it is dropped instead (MSG_TRUNC reports its real length).
async fn recv(sock: &AsyncFd<OwnedFd>, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let n = sock
            .async_io(Interest::READABLE, |fd| {
                // SAFETY: buf is valid for writes of buf.len() bytes.
                let n = unsafe {
                    libc::recv(
                        fd.as_raw_fd(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                        libc::MSG_TRUNC,
                    )
                };
                if n < 0 {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(n as usize)
                }
            })
            .await?;
        if n <= ROE_MAX_PAYLOAD {
            return Ok(n);
        }
        log::warn!("roe: dropped an oversized packet of {n} bytes");
    }
}

/// Sends one packet as one message.
async fn send(sock: &AsyncFd<OwnedFd>, packet: &[u8]) -> io::Result<()> {
    sock.async_io(Interest::WRITABLE, |fd| {
        // SAFETY: packet is valid for reads of packet.len() bytes.
        let n = unsafe {
            libc::send(
                fd.as_raw_fd(),
                packet.as_ptr().cast(),
                packet.len(),
                libc::MSG_NOSIGNAL,
            )
        };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
    .await
}
