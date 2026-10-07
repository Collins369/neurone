//! Minimal, safe Borsh reader.
//!
//! Every accessor returns `None` on truncation, so a malformed or
//! newer-version payload can never panic or read past the buffer. Decoders
//! stop at the first field they cannot read, which is how forward
//! compatibility is achieved.

use crate::events::MarketKey;

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.buf.len() {
            return None;
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Some(out)
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    pub fn bool(&mut self) -> Option<bool> {
        self.u8().map(|b| b != 0)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn u64(&mut self) -> Option<u64> {
        let b = self.take(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Some(u64::from_le_bytes(a))
    }

    pub fn u128(&mut self) -> Option<u128> {
        let b = self.take(16)?;
        let mut a = [0u8; 16];
        a.copy_from_slice(b);
        Some(u128::from_le_bytes(a))
    }

    pub fn i64(&mut self) -> Option<i64> {
        self.u64().map(|v| v as i64)
    }

    pub fn i128(&mut self) -> Option<i128> {
        self.u128().map(|v| v as i128)
    }

    pub fn pubkey(&mut self) -> Option<MarketKey> {
        MarketKey::from_slice(self.take(32)?)
    }

    /// Borsh string: `u32` little-endian length followed by UTF-8 bytes.
    pub fn string(&mut self) -> Option<String> {
        let len = self.u32()? as usize;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes).ok().map(str::to_owned)
    }

    /// Peek whether the buffer starts with an Anchor discriminator.
    pub fn starts_with(&self, disc: &[u8; 8]) -> bool {
        self.buf.len() >= 8 && &self.buf[..8] == disc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_scalars_and_rejects_truncation() {
        let mut r = Reader::new(&[1, 2, 3, 4]);
        assert_eq!(r.u16(), Some(0x0201));
        assert_eq!(r.u8(), Some(3));
        assert_eq!(r.u8(), Some(4));
        assert_eq!(r.u8(), None); // exhausted
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn oversized_read_does_not_advance() {
        let mut r = Reader::new(&[0u8; 4]);
        assert_eq!(r.u64(), None);
        assert_eq!(r.remaining(), 4);
    }

    #[test]
    fn string_roundtrip() {
        let mut buf = vec![3, 0, 0, 0, b'b', b'u', b'y'];
        let mut r = Reader::new(&buf);
        assert_eq!(r.string().as_deref(), Some("buy"));
        buf.push(0);
    }
}
