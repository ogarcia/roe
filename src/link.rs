//! Raw Ethernet access through Linux `AF_PACKET` sockets.
//!
//! Needs `CAP_NET_RAW` in the network namespace that owns the interface.

use std::ffi::CString;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

use crate::frame::{ETHERTYPE, MAC_LEN, Mac};

pub struct RawLink {
    fd: OwnedFd,
    ifindex: i32,
    mac: Mac,
}

fn cvt(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret) }
}

fn cvt_size(ret: libc::ssize_t) -> io::Result<usize> {
    if ret < 0 { Err(io::Error::last_os_error()) } else { Ok(ret as usize) }
}

impl RawLink {
    /// Opens a packet socket bound to `ifname` that only sees our EtherType.
    pub fn open(ifname: &str) -> io::Result<Self> {
        let cname = CString::new(ifname)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "interface name contains NUL"))?;
        let ifindex = unsafe { libc::if_nametoindex(cname.as_ptr()) };
        if ifindex == 0 {
            return Err(io::Error::last_os_error());
        }
        let ifindex = ifindex as i32;
        let proto = ETHERTYPE.to_be();

        let raw = cvt(unsafe {
            libc::socket(libc::AF_PACKET, libc::SOCK_RAW | libc::SOCK_CLOEXEC, proto as libc::c_int)
        })?;
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };

        let mut addr: libc::sockaddr_ll = unsafe { mem::zeroed() };
        addr.sll_family = libc::AF_PACKET as u16;
        addr.sll_protocol = proto;
        addr.sll_ifindex = ifindex;
        cvt(unsafe {
            libc::bind(
                fd.as_raw_fd(),
                &addr as *const _ as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        })?;

        let mac = hw_addr(fd.as_raw_fd(), &cname)?;
        Ok(Self { fd, ifindex, mac })
    }

    pub fn mac(&self) -> Mac {
        self.mac
    }

    pub fn ifindex(&self) -> i32 {
        self.ifindex
    }

    /// Sends a complete Ethernet frame (header included).
    pub fn send(&self, frame: &[u8]) -> io::Result<()> {
        let n = cvt_size(unsafe {
            libc::send(self.fd.as_raw_fd(), frame.as_ptr().cast(), frame.len(), 0)
        })?;
        if n != frame.len() {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "short write on packet socket"));
        }
        Ok(())
    }

    /// Receives the next frame addressed to this host (or broadcast).
    ///
    /// Frames we transmitted ourselves are skipped, so a caller never sees
    /// its own traffic looped back.
    pub fn recv(&self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            let mut from: libc::sockaddr_ll = unsafe { mem::zeroed() };
            let mut len = mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
            let n = cvt_size(unsafe {
                libc::recvfrom(
                    self.fd.as_raw_fd(),
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    0,
                    &mut from as *mut _ as *mut libc::sockaddr,
                    &mut len,
                )
            });
            let n = match n {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                r => r?,
            };
            if from.sll_pkttype == libc::PACKET_OUTGOING {
                continue;
            }
            return Ok(n);
        }
    }
}

fn hw_addr(fd: libc::c_int, cname: &CString) -> io::Result<Mac> {
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    let name = cname.as_bytes_with_nul();
    if name.len() > req.ifr_name.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long"));
    }
    for (dst, &src) in req.ifr_name.iter_mut().zip(name) {
        *dst = src as libc::c_char;
    }
    cvt(unsafe { libc::ioctl(fd, libc::SIOCGIFHWADDR as _, &mut req) })?;
    let data = unsafe { req.ifr_ifru.ifru_hwaddr.sa_data };
    let mut mac = [0u8; MAC_LEN];
    for (dst, &src) in mac.iter_mut().zip(data.iter()) {
        *dst = src as u8;
    }
    Ok(mac)
}
