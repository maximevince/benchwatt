pub const MAGIC: u32 = 0x7FB5_2CA3;
pub const MAX_PAYLOAD: usize = 4128;

pub fn encode(payload: &[u8]) -> Vec<u8> {
    assert!(
        payload.len() <= MAX_PAYLOAD,
        "payload exceeds protocol maximum"
    );
    let mut out = Vec::with_capacity(payload.len() + 8);
    out.extend_from_slice(&MAGIC.to_le_bytes());
    out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    out.extend_from_slice(&checksum(payload).to_le_bytes());
    out.extend_from_slice(payload);
    out
}

fn checksum(bytes: &[u8]) -> u16 {
    bytes
        .iter()
        .fold(0u16, |sum, &b| sum.wrapping_add(b as u16))
}

#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
    bad_checksums: u64,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn bad_checksum_count(&self) -> u64 {
        self.bad_checksums
    }
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut result = Vec::new();
        loop {
            let Some(pos) = self.buf.windows(4).position(|w| w == MAGIC.to_le_bytes()) else {
                let magic = MAGIC.to_le_bytes();
                let keep = (1..=3)
                    .rev()
                    .find(|n| self.buf.ends_with(&magic[..*n]))
                    .unwrap_or(0);
                if keep == 0 {
                    self.buf.clear();
                } else {
                    let start = self.buf.len() - keep;
                    self.buf.drain(..start);
                }
                break;
            };
            if pos != 0 {
                self.buf.drain(..pos);
            }
            if self.buf.len() < 8 {
                break;
            }
            let len = u16::from_le_bytes([self.buf[4], self.buf[5]]) as usize;
            if len > MAX_PAYLOAD {
                self.buf.drain(..4);
                continue;
            }
            if self.buf.len() < 8 + len {
                break;
            }
            let expected = u16::from_le_bytes([self.buf[6], self.buf[7]]);
            let payload = self.buf[8..8 + len].to_vec();
            self.buf.drain(..8 + len);
            if checksum(&payload) == expected {
                result.push(payload);
            } else {
                self.bad_checksums += 1;
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_round_trip_and_resync() {
        let frame = encode(b"hello");
        let mut d = Decoder::new();
        assert!(d.push(&[1, 2, 3, frame[0], frame[1]]).is_empty());
        assert_eq!(d.push(&frame[2..]), vec![b"hello".to_vec()]);
    }
    #[test]
    fn bad_checksum_is_counted() {
        let mut frame = encode(b"x");
        frame[6] ^= 1;
        let mut d = Decoder::new();
        assert!(d.push(&frame).is_empty());
        assert_eq!(d.bad_checksum_count(), 1);
    }
}
