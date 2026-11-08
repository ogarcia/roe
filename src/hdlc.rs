//! Simplified HDLC framing, as used by Reticulum's `PipeInterface`.
//!
//! Every packet travels as `FLAG <escaped payload> FLAG`. Inside the payload,
//! `FLAG` and `ESC` bytes are replaced by `ESC, byte ^ ESC_MASK`.

/// Frame delimiter.
pub const FLAG: u8 = 0x7E;
/// Escape byte: the next byte is XORed with [`ESC_MASK`].
pub const ESC: u8 = 0x7D;
/// Value XORed into escaped bytes.
pub const ESC_MASK: u8 = 0x20;

/// Appends `data` to `out` as a complete HDLC frame.
pub fn encode_into(data: &[u8], out: &mut Vec<u8>) {
    out.reserve(data.len() + 2);
    out.push(FLAG);
    for &b in data {
        if b == FLAG || b == ESC {
            out.push(ESC);
            out.push(b ^ ESC_MASK);
        } else {
            out.push(b);
        }
    }
    out.push(FLAG);
}

/// Returns `data` as a complete HDLC frame.
pub fn encode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(data, &mut out);
    out
}

/// Streaming decoder: feed bytes, get complete frames back.
#[derive(Debug)]
pub struct Decoder {
    buf: Vec<u8>,
    in_frame: bool,
    escape: bool,
    overflow: bool,
    max_len: usize,
}

impl Decoder {
    /// Creates a decoder that drops frames longer than `max_len` bytes.
    pub fn new(max_len: usize) -> Self {
        Self {
            buf: Vec::with_capacity(max_len),
            in_frame: false,
            escape: false,
            overflow: false,
            max_len,
        }
    }

    /// Feeds one byte. Returns a frame when `byte` closes a non-empty one.
    pub fn push(&mut self, byte: u8) -> Option<Vec<u8>> {
        if byte == FLAG {
            // A flag always closes the current frame and opens the next one,
            // so both `F..F F..F` and `F..F..F` streams are accepted.
            let done = self.in_frame && !self.overflow && !self.buf.is_empty();
            let frame = done.then(|| std::mem::take(&mut self.buf));
            self.buf.clear();
            self.in_frame = true;
            self.escape = false;
            self.overflow = false;
            return frame;
        }
        if !self.in_frame || self.overflow {
            return None;
        }
        if byte == ESC {
            self.escape = true;
            return None;
        }
        let b = if self.escape {
            self.escape = false;
            byte ^ ESC_MASK
        } else {
            byte
        };
        if self.buf.len() >= self.max_len {
            self.overflow = true;
            self.buf.clear();
        } else {
            self.buf.push(b);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(dec: &mut Decoder, bytes: &[u8]) -> Vec<Vec<u8>> {
        bytes.iter().filter_map(|&b| dec.push(b)).collect()
    }

    #[test]
    fn roundtrip_with_special_bytes() {
        let data = [0x00, FLAG, 0x01, ESC, ESC_MASK, FLAG ^ ESC_MASK, 0xFF];
        let enc = encode(&data);
        assert_eq!(enc.iter().filter(|&&b| b == FLAG).count(), 2);
        let mut dec = Decoder::new(1064);
        assert_eq!(decode_all(&mut dec, &enc), vec![data.to_vec()]);
    }

    #[test]
    fn back_to_back_and_shared_flags() {
        let mut dec = Decoder::new(1064);
        let mut stream = encode(b"one");
        stream.extend(encode(b"two"));
        stream.extend_from_slice(b"three");
        stream.push(FLAG);
        let frames = decode_all(&mut dec, &stream);
        assert_eq!(
            frames,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()]
        );
    }

    #[test]
    fn ignores_noise_before_first_flag() {
        let mut dec = Decoder::new(1064);
        let mut stream = b"garbage".to_vec();
        stream.extend(encode(b"ok"));
        assert_eq!(decode_all(&mut dec, &stream), vec![b"ok".to_vec()]);
    }

    #[test]
    fn drops_oversized_frame_and_recovers() {
        let mut dec = Decoder::new(4);
        let mut stream = encode(b"too long");
        stream.extend(encode(b"fine"));
        assert_eq!(decode_all(&mut dec, &stream), vec![b"fine".to_vec()]);
    }
}
