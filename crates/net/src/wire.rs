//! Byte-order helpers and the Internet checksum.

pub fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
pub fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
pub fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
pub fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
pub fn put_be16(b: &mut [u8], at: usize, value: u16) {
    b[at..at + 2].copy_from_slice(&value.to_be_bytes());
}
pub fn put_be32(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_be_bytes());
}
pub fn put_le32(b: &mut [u8], at: usize, value: u32) {
    b[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

/// One's-complement sum over the concatenation of `parts`.
pub fn checksum(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    let mut odd = false;
    for part in parts {
        for &byte in *part {
            sum += if odd { byte as u32 } else { (byte as u32) << 8 };
            odd = !odd;
        }
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}
