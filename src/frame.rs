//! Ethernet frame layout for carrying Reticulum packets directly over L2.
//!
//! ```text
//! +---------+---------+-----------+-------+---------+--------+---------+
//! | dst (6) | src (6) | type (2)  | magic | version | len(2) | payload |
//! +---------+---------+-----------+-------+---------+--------+---------+
//! ```
//!
//! The EtherType is `0x88B5`, reserved by IEEE for local experimental use.
//! Since other experiments may share it, a magic byte and a version byte
//! follow. The explicit length is needed because Ethernet pads short frames
//! up to 60 bytes, so the frame size alone does not tell the payload size.

pub const ETHERTYPE: u16 = 0x88B5;
pub const MAGIC: u8 = 0x52; // 'R'
pub const VERSION: u8 = 0x01;

pub const MAC_LEN: usize = 6;
pub const ETH_HEADER_LEN: usize = 14;
pub const RTP_HEADER_LEN: usize = 4;
pub const HEADER_LEN: usize = ETH_HEADER_LEN + RTP_HEADER_LEN;
/// Standard Ethernet payload size, without jumbo frames.
pub const ETH_MTU: usize = 1500;
pub const MAX_PAYLOAD: usize = ETH_MTU - RTP_HEADER_LEN;
/// Reticulum's base MTU: every interface must carry packets of this size.
pub const RETICULUM_MTU: usize = 500;

/// Largest payload that fits an interface with the given MTU.
pub fn max_payload_for(if_mtu: usize) -> usize {
    if_mtu.saturating_sub(RTP_HEADER_LEN).min(MAX_PAYLOAD)
}

/// Group address every node sends to and listens on.
///
/// Locally administered multicast (first octet `0x03`), followed by "RNS"
/// in ASCII. Unlike broadcast, NICs of hosts that have not joined the group
/// drop these frames in hardware, so bystanders on the segment never see
/// Reticulum traffic reach their kernel.
pub const GROUP: Mac = [0x03, 0x52, 0x4E, 0x53, 0x00, 0x01];

pub type Mac = [u8; MAC_LEN];

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    TooLong(usize),
    Truncated,
    WrongEtherType(u16),
    WrongMagic(u8),
    UnsupportedVersion(u8),
    BadLength { declared: usize, available: usize },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::TooLong(n) => write!(f, "payload of {n} bytes exceeds {MAX_PAYLOAD}"),
            Error::Truncated => write!(f, "frame shorter than header"),
            Error::WrongEtherType(t) => write!(f, "unexpected EtherType {t:#06x}"),
            Error::WrongMagic(m) => write!(f, "unexpected magic byte {m:#04x}"),
            Error::UnsupportedVersion(v) => write!(f, "unsupported version {v}"),
            Error::BadLength { declared, available } => {
                write!(f, "declared length {declared} but only {available} bytes present")
            }
        }
    }
}

impl std::error::Error for Error {}

/// Builds a complete Ethernet frame (without FCS) carrying `payload`.
pub fn build(dst: &Mac, src: &Mac, payload: &[u8]) -> Result<Vec<u8>, Error> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::TooLong(payload.len()));
    }
    let mut f = Vec::with_capacity(HEADER_LEN + payload.len());
    f.extend_from_slice(dst);
    f.extend_from_slice(src);
    f.extend_from_slice(&ETHERTYPE.to_be_bytes());
    f.push(MAGIC);
    f.push(VERSION);
    f.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    f.extend_from_slice(payload);
    Ok(f)
}

/// A received frame, borrowing its payload from the receive buffer.
#[derive(Debug, PartialEq, Eq)]
pub struct Frame<'a> {
    pub dst: Mac,
    pub src: Mac,
    pub payload: &'a [u8],
}

/// Parses a complete Ethernet frame, ignoring any trailing padding.
pub fn parse(buf: &[u8]) -> Result<Frame<'_>, Error> {
    if buf.len() < HEADER_LEN {
        return Err(Error::Truncated);
    }
    let ethertype = u16::from_be_bytes([buf[12], buf[13]]);
    if ethertype != ETHERTYPE {
        return Err(Error::WrongEtherType(ethertype));
    }
    if buf[14] != MAGIC {
        return Err(Error::WrongMagic(buf[14]));
    }
    if buf[15] != VERSION {
        return Err(Error::UnsupportedVersion(buf[15]));
    }
    let declared = u16::from_be_bytes([buf[16], buf[17]]) as usize;
    let available = buf.len() - HEADER_LEN;
    if declared > available {
        return Err(Error::BadLength { declared, available });
    }
    Ok(Frame {
        dst: buf[0..6].try_into().unwrap(),
        src: buf[6..12].try_into().unwrap(),
        payload: &buf[HEADER_LEN..HEADER_LEN + declared],
    })
}

/// Formats a MAC address as `aa:bb:cc:dd:ee:ff`.
pub fn fmt_mac(mac: &Mac) -> String {
    mac.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: Mac = [0x02, 0, 0, 0, 0, 1];

    #[test]
    fn roundtrip() {
        let f = build(&GROUP, &SRC, b"hello").unwrap();
        let p = parse(&f).unwrap();
        assert_eq!(p, Frame { dst: GROUP, src: SRC, payload: b"hello" });
    }

    #[test]
    fn ignores_ethernet_padding() {
        let mut f = build(&GROUP, &SRC, b"hi").unwrap();
        f.resize(60, 0);
        assert_eq!(parse(&f).unwrap().payload, b"hi");
    }

    #[test]
    fn rejects_foreign_and_broken_frames() {
        let good = build(&GROUP, &SRC, b"x").unwrap();

        let mut f = good.clone();
        f[12..14].copy_from_slice(&0x0800u16.to_be_bytes());
        assert_eq!(parse(&f), Err(Error::WrongEtherType(0x0800)));

        let mut f = good.clone();
        f[14] = 0;
        assert_eq!(parse(&f), Err(Error::WrongMagic(0)));

        let mut f = good.clone();
        f[15] = 9;
        assert_eq!(parse(&f), Err(Error::UnsupportedVersion(9)));

        let mut f = good.clone();
        f[16..18].copy_from_slice(&100u16.to_be_bytes());
        assert_eq!(parse(&f), Err(Error::BadLength { declared: 100, available: 1 }));

        assert_eq!(parse(&good[..10]), Err(Error::Truncated));
    }

    #[test]
    fn payload_limit_follows_interface_mtu() {
        assert_eq!(max_payload_for(1500), MAX_PAYLOAD);
        assert_eq!(max_payload_for(9000), MAX_PAYLOAD);
        assert_eq!(max_payload_for(1000), 996);
        assert_eq!(max_payload_for(2), 0);
    }

    #[test]
    fn payload_limits() {
        let max = vec![0u8; MAX_PAYLOAD];
        assert_eq!(build(&GROUP, &SRC, &max).unwrap().len(), HEADER_LEN + ETH_MTU - RTP_HEADER_LEN);
        let over = vec![0u8; MAX_PAYLOAD + 1];
        assert_eq!(build(&GROUP, &SRC, &over), Err(Error::TooLong(MAX_PAYLOAD + 1)));
    }
}
