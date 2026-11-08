//! Packet-oriented upstream: a `SOCK_SEQPACKET` socket on stdin/stdout.
//!
//! Each message on the socket is exactly one Reticulum packet, so there is no
//! framing to encode or decode. This is what `RoeInterface` uses.

use std::io;
use std::mem;
use std::os::fd::RawFd;

/// Standard input, where `roe --packet` receives packets from Reticulum.
pub const STDIN: RawFd = 0;
/// Standard output, where `roe --packet` delivers packets to Reticulum.
pub const STDOUT: RawFd = 1;

/// Fails unless `fd` is a `SOCK_SEQPACKET` socket.
pub fn check_seqpacket(fd: RawFd) -> io::Result<()> {
    let mut ty: libc::c_int = 0;
    let mut len = mem::size_of::<libc::c_int>() as libc::socklen_t;
    let ret = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_TYPE,
            (&mut ty as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    if ret < 0 {
        let e = io::Error::last_os_error();
        return Err(io::Error::new(
            e.kind(),
            format!("fd {fd} is not a socket: {e}"),
        ));
    }
    if ty != libc::SOCK_SEQPACKET {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("fd {fd} is not a SOCK_SEQPACKET socket"),
        ));
    }
    Ok(())
}

/// Receives one packet. Returns 0 when the peer has closed the socket.
///
/// A packet longer than `buf` is truncated by the kernel; callers detect this
/// with `MSG_TRUNC`, which makes the returned length the real packet size.
pub fn recv(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        let n = unsafe { libc::recv(fd, buf.as_mut_ptr().cast(), buf.len(), libc::MSG_TRUNC) };
        if n >= 0 {
            return Ok(n as usize);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

/// Sends one packet as a single message.
pub fn send(fd: RawFd, data: &[u8]) -> io::Result<()> {
    loop {
        let n = unsafe { libc::send(fd, data.as_ptr().cast(), data.len(), libc::MSG_NOSIGNAL) };
        if n >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    fn pair(ty: libc::c_int) -> (OwnedFd, OwnedFd) {
        let mut fds = [0; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, ty, 0, fds.as_mut_ptr()) },
            0
        );
        unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) }
    }

    #[test]
    fn keeps_message_boundaries() {
        let (a, b) = pair(libc::SOCK_SEQPACKET);
        send(a.as_raw_fd(), b"first").unwrap();
        send(a.as_raw_fd(), b"second").unwrap();
        let mut buf = [0u8; 64];
        let n = recv(b.as_raw_fd(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"first");
        let n = recv(b.as_raw_fd(), &mut buf).unwrap();
        assert_eq!(&buf[..n], b"second");
    }

    #[test]
    fn reports_truncation_and_eof() {
        let (a, b) = pair(libc::SOCK_SEQPACKET);
        send(a.as_raw_fd(), &[7u8; 100]).unwrap();
        let mut buf = [0u8; 10];
        assert_eq!(recv(b.as_raw_fd(), &mut buf).unwrap(), 100);
        drop(a);
        assert_eq!(recv(b.as_raw_fd(), &mut buf).unwrap(), 0);
    }

    #[test]
    fn checks_socket_type() {
        let (a, _b) = pair(libc::SOCK_SEQPACKET);
        assert!(check_seqpacket(a.as_raw_fd()).is_ok());
        let (s, _t) = pair(libc::SOCK_STREAM);
        assert!(check_seqpacket(s.as_raw_fd()).is_err());
    }
}
