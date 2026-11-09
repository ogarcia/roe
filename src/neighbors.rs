//! Neighbor table: which MAC address to send each Reticulum packet to.
//!
//! RoE sends every frame to its multicast group by default, so that every
//! node on the segment receives it, as Reticulum expects from a shared
//! medium. Most packets are meant for a single neighbor, though, and on some
//! media multicast is much worse than unicast: Wi-Fi access points hold
//! multicast frames until the next DTIM beacon, send them at the lowest
//! rate and never retransmit them.
//!
//! Reticulum does not tell an interface which neighbor a packet is for, but
//! the packet header says enough to work it out from the traffic already
//! received. [`Neighbors`] learns, from each received packet and the MAC
//! address it came from:
//!
//! - from an announce sent by its origin, where that destination is;
//! - from an announce rebroadcast by a transport node, where that transport
//!   node is (packets in transport name it as their next hop);
//! - from a packet on a link, where the other end of that link is;
//! - from a link request, where to send the proof that opens the link;
//! - from any other packet for a single destination, where to send its
//!   proof.
//!
//! [`Neighbors::next_hop`] then looks up an outgoing packet and returns the
//! MAC address of the neighbor it is for, or `None` to send it to the group:
//! announces, path requests and other broadcasts, packets for unknown
//! neighbors, and packets with an interface access code, whose header is
//! masked. A key heard from two different addresses is ambiguous and goes to
//! the group for a while too, so a wrong guess never loses a packet that
//! multicast would have delivered.
//!
//! The table is pure: callers pass the current time, which makes it easy to
//! test and to use from any event loop.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::frame::Mac;
use crate::sha256;

/// Length of Reticulum's truncated hashes: destinations, transport
/// identities, links and packet hashes.
pub const HASH_LEN: usize = 16;

/// A Reticulum truncated hash.
pub type Hash = [u8; HASH_LEN];

/// How long a location learned from an announce is kept.
pub const ANNOUNCE_TTL: Duration = Duration::from_secs(30 * 60);
/// How long the other end of a link is kept after the last packet on it.
/// Longer than Reticulum's link keepalive interval, so active links stay.
pub const LINK_TTL: Duration = Duration::from_secs(15 * 60);
/// How long the sender of a packet is kept, to send its proof back.
pub const PROOF_TTL: Duration = Duration::from_secs(2 * 60);
/// How long a key heard from two different addresses goes to the group.
pub const CONFLICT_HOLD: Duration = Duration::from_secs(60);
/// Default maximum number of entries.
pub const DEFAULT_CAPACITY: usize = 16384;

// Reticulum header fields (RNS.Packet and RNS.Destination).
const IFAC_FLAG: u8 = 0x80;
const HEADER_2: u8 = 1;
const DATA: u8 = 0x00;
const ANNOUNCE: u8 = 0x01;
const LINKREQUEST: u8 = 0x02;
const SINGLE: u8 = 0x00;
const LINK: u8 = 0x03;
/// Public key size in a link request; anything after it is signalling,
/// which is left out of the link id.
const LINK_ECPUBSIZE: usize = 64;

/// The parts of a Reticulum packet header that routing depends on.
struct Header<'a> {
    header_2: bool,
    destination_type: u8,
    packet_type: u8,
    transport_id: Option<&'a [u8]>,
    destination: &'a [u8],
    /// Bytes after the context byte.
    data_len: usize,
}

impl<'a> Header<'a> {
    /// Parses a header, or returns `None` if the packet is malformed or its
    /// header is masked by an interface access code.
    fn parse(p: &'a [u8]) -> Option<Self> {
        let flags = *p.first()?;
        if flags & IFAC_FLAG != 0 {
            return None;
        }
        let header_2 = (flags >> 6) & 1 == HEADER_2;
        let (transport_id, dst_at) = if header_2 {
            (Some(p.get(2..2 + HASH_LEN)?), 2 + HASH_LEN)
        } else {
            (None, 2)
        };
        let destination = p.get(dst_at..dst_at + HASH_LEN)?;
        let data_at = dst_at + HASH_LEN + 1;
        if p.len() <= data_at {
            return None;
        }
        Some(Self {
            header_2,
            destination_type: (flags >> 2) & 0b11,
            packet_type: flags & 0b11,
            transport_id,
            destination,
            data_len: p.len() - data_at,
        })
    }
}

/// The part of a packet Reticulum hashes: the low nibble of the flags and
/// everything after the hops and transport id, which change in transit.
fn hashable_part<'a>(p: &'a [u8], h: &Header) -> (u8, &'a [u8]) {
    let from = if h.header_2 { 2 + HASH_LEN } else { 2 };
    (p[0] & 0x0F, &p[from..])
}

fn truncated_hash(flags: u8, rest: &[u8]) -> Hash {
    sha256::digest(&[&[flags], rest])[..HASH_LEN]
        .try_into()
        .unwrap()
}

/// The truncated hash of a packet, which its proof is addressed to.
fn packet_hash(p: &[u8], h: &Header) -> Hash {
    let (flags, rest) = hashable_part(p, h);
    truncated_hash(flags, rest)
}

/// The id of the link a link request opens, which its proof and every later
/// packet on the link are addressed to.
fn link_id(p: &[u8], h: &Header) -> Hash {
    let (flags, rest) = hashable_part(p, h);
    let signalling = h.data_len.saturating_sub(LINK_ECPUBSIZE);
    truncated_hash(flags, &rest[..rest.len() - signalling])
}

fn hash(bytes: &[u8]) -> Hash {
    bytes.try_into().unwrap()
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    mac: Mac,
    expires: Instant,
    /// Until when the entry is ambiguous, after being heard from two
    /// different addresses.
    conflict_until: Option<Instant>,
}

/// Where each known neighbor is, learned from received packets.
#[derive(Debug)]
pub struct Neighbors {
    entries: HashMap<Hash, Entry>,
    capacity: usize,
}

impl Default for Neighbors {
    fn default() -> Self {
        Self::new()
    }
}

impl Neighbors {
    /// An empty table with [`DEFAULT_CAPACITY`].
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// An empty table holding at most `capacity` entries. When it is full,
    /// expired entries are dropped; if none have expired, new keys are not
    /// learned, and packets for them go to the group.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            capacity,
        }
    }

    /// Number of entries, including expired ones not yet dropped.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Learns from a Reticulum packet received from `src` at `now`.
    pub fn learn(&mut self, src: &Mac, packet: &[u8], now: Instant) {
        let Some(h) = Header::parse(packet) else {
            return;
        };
        let (key, ttl) = if h.packet_type == ANNOUNCE {
            // Transport nodes rebroadcast announces in transport, naming
            // themselves; the origin sends them, path responses for its
            // own destinations included, without a transport id.
            (hash(h.transport_id.unwrap_or(h.destination)), ANNOUNCE_TTL)
        } else if h.destination_type == LINK {
            (hash(h.destination), LINK_TTL)
        } else if h.packet_type == LINKREQUEST {
            (link_id(packet, &h), LINK_TTL)
        } else if h.packet_type == DATA && h.destination_type == SINGLE {
            (packet_hash(packet, &h), PROOF_TTL)
        } else {
            // Proofs, and packets for plain and group destinations, which
            // nothing is sent back to.
            return;
        };
        self.insert(key, *src, now, ttl);
    }

    fn insert(&mut self, key: Hash, mac: Mac, now: Instant, ttl: Duration) {
        let expires = now + ttl;
        if let Some(e) = self.entries.get_mut(&key) {
            if e.mac != mac && e.expires > now {
                e.conflict_until = Some(now + CONFLICT_HOLD);
            }
            e.mac = mac;
            e.expires = e.expires.max(expires);
            return;
        }
        if self.entries.len() >= self.capacity {
            self.entries
                .retain(|_, e| e.expires > now || e.conflict_until.is_some_and(|c| c > now));
            if self.entries.len() >= self.capacity {
                return;
            }
        }
        self.entries.insert(
            key,
            Entry {
                mac,
                expires,
                conflict_until: None,
            },
        );
    }

    /// The MAC address to send an outgoing Reticulum packet to at `now`, or
    /// `None` to send it to the group.
    pub fn next_hop(&self, packet: &[u8], now: Instant) -> Option<Mac> {
        let h = Header::parse(packet)?;
        if h.packet_type == ANNOUNCE {
            return None;
        }
        let key = match h.transport_id {
            // In transport: the next hop is the transport node it names.
            Some(id) => id,
            None if h.destination_type == SINGLE || h.destination_type == LINK => h.destination,
            // Plain and group destinations: broadcasts, path requests.
            None => return None,
        };
        let e = self.entries.get(key)?;
        if e.expires <= now || e.conflict_until.is_some_and(|c| c > now) {
            return None;
        }
        Some(e.mac)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Packets made by Reticulum 1.6.0, with the hashes it computes for them
    // (truncated_packet_hash, Link.link_id_from_lr_packet).
    const ANNOUNCE_H1: &str = "0100adc107e9d92a9e55e2e4645eea461607002729820851b5d1df6abf16beeb692e9a173b6ed335fefeb89083a49cfd516f013c761c86ffe13fd0737a3ce8caab06f0924a0c6b93792b7c470e32ed17c201d53399d6c14ced5786c4dd147e415fa0006af19ab1675ab58d3fb3b06a08c3cd444079561ecaa254a7f8889d0075c19f120bd998adb5c9aa01f0aabea47b9bf903ddbac89d5facd351c84ef2f090fc0afddccbd60d";
    const ANNOUNCE_DEST: &str = "adc107e9d92a9e55e2e4645eea461607";
    // The same announce rebroadcast in transport by 000102...0f.
    const ANNOUNCE_H2: &str = "5101000102030405060708090a0b0c0d0e0fadc107e9d92a9e55e2e4645eea461607002729820851b5d1df6abf16beeb692e9a173b6ed335fefeb89083a49cfd516f013c761c86ffe13fd0737a3ce8caab06f0924a0c6b93792b7c470e32ed17c201d53399d6c14ced5786c4dd147e415fa0006af19ab1675ab58d3fb3b06a08c3cd444079561ecaa254a7f8889d0075c19f120bd998adb5c9aa01f0aabea47b9bf903ddbac89d5facd351c84ef2f090fc0afddccbd60d";
    const TRANSPORT_ID: &str = "000102030405060708090a0b0c0d0e0f";
    // A link request with 3 bytes of MTU signalling after the public keys.
    const LR: &str = "0200adc107e9d92a9e55e2e4645eea461607005a4736aecd0e628533064a5720c2bf851a2d4400aa136189a5e004af3b9a09d525d677468b308b81426db6fadc818caec68b4104d1ec73240b10355ac8b398d4210596";
    const LR_LINK_ID: &str = "399ed4d11844f3375e2b2495df52dd21";
    const DATA_H1: &str = "0000adc107e9d92a9e55e2e4645eea46160700df432e478937ac534bebb0c72492b5c878133f42f36df86fbc8e8a0c2f24282556e88e56070ab49d665dcfcb72c8e91ac0b9611e0abf2fc33a720d022ecb5260eb6765bb9f6bf174f12893e2b938eb86ea80ec92158f521749cd06309c706b1b";
    // The same packet in transport: the hash does not change.
    const DATA_H2: &str = "5000000102030405060708090a0b0c0d0e0fadc107e9d92a9e55e2e4645eea46160700df432e478937ac534bebb0c72492b5c878133f42f36df86fbc8e8a0c2f24282556e88e56070ab49d665dcfcb72c8e91ac0b9611e0abf2fc33a720d022ecb5260eb6765bb9f6bf174f12893e2b938eb86ea80ec92158f521749cd06309c706b1b";
    const DATA_HASH: &str = "95be8c89d3a7e83c4220656de80e49a2";

    const A: Mac = [0x02, 0, 0, 0, 0, 0x0a];
    const B: Mac = [0x02, 0, 0, 0, 0, 0x0b];

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    /// A packet of the given type for a destination, in HEADER_1.
    fn packet(packet_type: u8, destination_type: u8, dst: &[u8], context: u8) -> Vec<u8> {
        let mut p = vec![(destination_type << 2) | packet_type, 0];
        p.extend_from_slice(dst);
        p.push(context);
        p.extend_from_slice(b"payload");
        p
    }

    /// A packet in transport through `transport_id`.
    fn in_transport(p: &[u8], transport_id: &[u8]) -> Vec<u8> {
        let mut t = vec![p[0] | 0x50, p[1]];
        t.extend_from_slice(transport_id);
        t.extend_from_slice(&p[2..]);
        t
    }

    #[test]
    fn hashes_match_reticulum() {
        for (p, want) in [(DATA_H1, DATA_HASH), (DATA_H2, DATA_HASH)] {
            let p = hex(p);
            let h = Header::parse(&p).unwrap();
            assert_eq!(packet_hash(&p, &h).to_vec(), hex(want));
        }
        let lr = hex(LR);
        let h = Header::parse(&lr).unwrap();
        assert_eq!(link_id(&lr, &h).to_vec(), hex(LR_LINK_ID));
    }

    #[test]
    fn announce_from_origin_locates_destination() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        n.learn(&A, &hex(ANNOUNCE_H1), now);
        let dst = hex(ANNOUNCE_DEST);
        assert_eq!(n.next_hop(&packet(DATA, SINGLE, &dst, 0), now), Some(A));
        assert_eq!(
            n.next_hop(&packet(LINKREQUEST, SINGLE, &dst, 0), now),
            Some(A)
        );
        // Announces themselves always go to the group.
        assert_eq!(n.next_hop(&hex(ANNOUNCE_H1), now), None);
    }

    #[test]
    fn rebroadcast_announce_locates_transport_node() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        n.learn(&B, &hex(ANNOUNCE_H2), now);
        // The destination itself is not here, only the transport node.
        let dst = hex(ANNOUNCE_DEST);
        assert_eq!(n.next_hop(&packet(DATA, SINGLE, &dst, 0), now), None);
        assert_eq!(n.next_hop(&hex(DATA_H2), now), Some(B));
    }

    #[test]
    fn proof_goes_back_to_sender() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        for p in [DATA_H1, DATA_H2] {
            n.learn(&A, &hex(p), now);
            let proof = packet(0x03, SINGLE, &hex(DATA_HASH), 0);
            assert_eq!(n.next_hop(&proof, now), Some(A));
            assert_eq!(n.next_hop(&proof, now + PROOF_TTL), None);
        }
    }

    #[test]
    fn link_proof_and_traffic_go_to_other_end() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        // The responder learns the requester from the link request...
        n.learn(&A, &hex(LR), now);
        let link = hex(LR_LINK_ID);
        let lrproof = packet(0x03, LINK, &link, 0xFF);
        assert_eq!(n.next_hop(&lrproof, now), Some(A));
        // ...and the requester learns the responder from the proof.
        let mut m = Neighbors::new();
        m.learn(&B, &lrproof, now);
        assert_eq!(m.next_hop(&packet(DATA, LINK, &link, 0), now), Some(B));
        // Traffic keeps the entry alive.
        let later = now + LINK_TTL - Duration::from_secs(1);
        m.learn(&B, &packet(DATA, LINK, &link, 0), later);
        assert_eq!(
            m.next_hop(
                &packet(DATA, LINK, &link, 0),
                later + Duration::from_secs(2)
            ),
            Some(B)
        );
    }

    #[test]
    fn broadcasts_go_to_group() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        let dst = [7u8; HASH_LEN];
        n.learn(&A, &packet(DATA, LINK, &dst, 0), now);
        // Plain (path requests) and group destinations.
        for t in [0x01, 0x02] {
            assert_eq!(n.next_hop(&packet(DATA, t, &dst, 0), now), None);
            // Nothing is learned from them either.
            n.learn(&A, &packet(DATA, t, &[8u8; HASH_LEN], 0), now);
        }
        assert_eq!(n.len(), 1);
    }

    #[test]
    fn masked_and_malformed_packets_go_to_group() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        let dst = [7u8; HASH_LEN];
        n.learn(&A, &packet(DATA, LINK, &dst, 0), now);
        let mut masked = packet(DATA, LINK, &dst, 0);
        masked[0] |= IFAC_FLAG;
        assert_eq!(n.next_hop(&masked, now), None);
        n.learn(&B, &masked, now);
        assert_eq!(n.next_hop(&packet(DATA, LINK, &dst, 0), now), Some(A));
        for len in 0..20 {
            let short = &packet(DATA, LINK, &dst, 0)[..len];
            assert_eq!(n.next_hop(short, now), None);
            n.learn(&B, short, now);
        }
        assert_eq!(n.next_hop(&in_transport(b"\x00\x00", &dst), now), None);
    }

    #[test]
    fn conflicting_sources_go_to_group_for_a_while() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        let link = [7u8; HASH_LEN];
        let p = packet(DATA, LINK, &link, 0);
        n.learn(&A, &p, now);
        n.learn(&B, &p, now);
        assert_eq!(n.next_hop(&p, now), None);
        // Once heard only from B for long enough, B is trusted.
        let later = now + CONFLICT_HOLD;
        n.learn(&B, &p, later);
        assert_eq!(n.next_hop(&p, later), Some(B));
    }

    #[test]
    fn transport_packet_uses_transport_id() {
        let now = Instant::now();
        let mut n = Neighbors::new();
        let tid = hex(TRANSPORT_ID);
        let dst = [9u8; HASH_LEN];
        // A destination one hop away that is also our next hop.
        n.learn(&A, &packet(ANNOUNCE, SINGLE, &dst, 0), now);
        let p = in_transport(&packet(DATA, SINGLE, &dst, 0), &tid);
        assert_eq!(n.next_hop(&p, now), None);
        n.learn(&B, &hex(ANNOUNCE_H2), now);
        assert_eq!(n.next_hop(&p, now), Some(B));
    }

    #[test]
    fn full_table_drops_expired_entries() {
        let now = Instant::now();
        let mut n = Neighbors::with_capacity(2);
        n.learn(&A, &packet(DATA, LINK, &[1; HASH_LEN], 0), now);
        n.learn(&A, &packet(DATA, LINK, &[2; HASH_LEN], 0), now);
        let p3 = packet(DATA, LINK, &[3; HASH_LEN], 0);
        n.learn(&A, &p3, now);
        assert_eq!(n.next_hop(&p3, now), None);
        let later = now + LINK_TTL;
        n.learn(&A, &p3, later);
        assert_eq!(n.next_hop(&p3, later), Some(A));
        assert_eq!(n.len(), 1);
    }
}
