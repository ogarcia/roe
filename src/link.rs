//! Raw Ethernet access through Linux `AF_PACKET` sockets.
//!
//! Needs `CAP_NET_RAW` in the network namespace that owns the interface.

use std::ffi::CString;
use std::io;
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::time::Duration;

use crate::frame::{ETHERTYPE, GROUP, MAC_LEN, Mac};

/// How long `recv` blocks before returning `WouldBlock`, so callers get a
/// chance to notice that the interface has been removed. A socket bound to a
/// deleted interface would otherwise block forever.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

pub struct RawLink {
    fd: OwnedFd,
    name: CString,
    ifindex: i32,
    mac: Mac,
    group_joined: bool,
}

/// True for errors that mean the interface is down or gone for now, rather
/// than a fault in the socket itself.
pub fn is_link_down(e: &io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ENETDOWN | libc::ENXIO | libc::ENODEV | libc::ENOBUFS)
    )
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

        // Some environments cannot set SOL_PACKET options at all (qemu user
        // mode answers ENOPROTOOPT without reaching the kernel). Carry on
        // without the membership and let the caller warn: veth delivers
        // group frames anyway, but a real NIC would filter them out.
        let group_joined = match join_group(fd.as_raw_fd(), ifindex, &GROUP) {
            Ok(()) => true,
            Err(e) if e.raw_os_error() == Some(libc::ENOPROTOOPT) => false,
            Err(e) => return Err(e),
        };
        set_recv_timeout(fd.as_raw_fd(), POLL_INTERVAL)?;
        let mac = hw_addr(fd.as_raw_fd(), &cname)?;
        Ok(Self { fd, name: cname, ifindex, mac, group_joined })
    }

    /// True while the interface name still refers to the interface this
    /// socket is bound to. False once it has been removed, even if another
    /// interface has since appeared under the same name: the socket must
    /// then be reopened.
    pub fn still_present(&self) -> bool {
        unsafe { libc::if_nametoindex(self.name.as_ptr()) as i32 == self.ifindex }
    }

    /// Current MTU of the interface: the largest Ethernet payload it sends.
    pub fn mtu(&self) -> io::Result<usize> {
        if_mtu(self.fd.as_raw_fd(), &self.name)
    }

    /// False if the NIC could not be told to accept our group address.
    pub fn group_joined(&self) -> bool {
        self.group_joined
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

    /// Receives the next frame addressed to this host or to our group.
    ///
    /// Frames we transmitted ourselves are skipped, so a caller never sees
    /// its own traffic looped back. Returns `WouldBlock` after
    /// [`POLL_INTERVAL`] without traffic.
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

fn set_recv_timeout(fd: libc::c_int, timeout: Duration) -> io::Result<()> {
    let tv = libc::timeval {
        tv_sec: timeout.as_secs() as _,
        tv_usec: timeout.subsec_micros() as _,
    };
    cvt(unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_RCVTIMEO,
            &tv as *const _ as *const libc::c_void,
            mem::size_of::<libc::timeval>() as libc::socklen_t,
        )
    })?;
    Ok(())
}

/// Programs the NIC filter to accept frames sent to `group`. The kernel drops
/// the membership when the socket closes.
fn join_group(fd: libc::c_int, ifindex: i32, group: &Mac) -> io::Result<()> {
    let mut mreq: libc::packet_mreq = unsafe { mem::zeroed() };
    mreq.mr_ifindex = ifindex;
    mreq.mr_type = libc::PACKET_MR_MULTICAST as u16;
    mreq.mr_alen = MAC_LEN as u16;
    mreq.mr_address[..MAC_LEN].copy_from_slice(group);
    cvt(unsafe {
        libc::setsockopt(
            fd,
            libc::SOL_PACKET,
            libc::PACKET_ADD_MEMBERSHIP,
            &mreq as *const _ as *const libc::c_void,
            mem::size_of::<libc::packet_mreq>() as libc::socklen_t,
        )
    })?;
    Ok(())
}

fn ifreq_for(cname: &CString) -> io::Result<libc::ifreq> {
    let mut req: libc::ifreq = unsafe { mem::zeroed() };
    let name = cname.as_bytes_with_nul();
    if name.len() > req.ifr_name.len() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "interface name too long"));
    }
    for (dst, &src) in req.ifr_name.iter_mut().zip(name) {
        *dst = src as libc::c_char;
    }
    Ok(req)
}

fn if_mtu(fd: libc::c_int, cname: &CString) -> io::Result<usize> {
    let mut req = ifreq_for(cname)?;
    cvt(unsafe { libc::ioctl(fd, libc::SIOCGIFMTU as _, &mut req) })?;
    Ok(unsafe { req.ifr_ifru.ifru_mtu } as usize)
}

fn hw_addr(fd: libc::c_int, cname: &CString) -> io::Result<Mac> {
    let mut req = ifreq_for(cname)?;
    cvt(unsafe { libc::ioctl(fd, libc::SIOCGIFHWADDR as _, &mut req) })?;
    let data = unsafe { req.ifr_ifru.ifru_hwaddr.sa_data };
    let mut mac = [0u8; MAC_LEN];
    for (dst, &src) in mac.iter_mut().zip(data.iter()) {
        *dst = src as u8;
    }
    Ok(mac)
}
