//! Remote NDIS messages: control messages over endpoint 0 and data packets
//! over the bulk endpoints. All fields are little-endian.
use crate::wire::{le32, put_le32};

pub const PACKET: u32 = 0x1;
pub const INITIALIZE: u32 = 0x2;
pub const QUERY: u32 = 0x4;
pub const SET: u32 = 0x5;
pub const INDICATE_STATUS: u32 = 0x7;
pub const KEEPALIVE: u32 = 0x8;
pub const COMPLETION: u32 = 0x8000_0000;
pub const KEEPALIVE_DONE: u32 = KEEPALIVE | COMPLETION;

pub const OID_PERMANENT_ADDRESS: u32 = 0x0101_0101;
pub const OID_PACKET_FILTER: u32 = 0x0001_010e;
/// Directed, multicast, all-multicast, broadcast and promiscuous, as Linux sets.
pub const FILTER: u32 = 0x0000_002f;

/// Bytes before the Ethernet frame in a data packet.
pub const PACKET_HEADER: usize = 44;
/// The largest bulk transfer the host accepts from the device.
pub const MAX_TRANSFER: u32 = 0x4000;

pub fn initialize(id: u32, out: &mut [u8]) -> usize {
    for (at, value) in [INITIALIZE, 24, id, 1, 0, MAX_TRANSFER].iter().enumerate() {
        put_le32(out, at * 4, *value);
    }
    24
}
pub fn query(id: u32, oid: u32, out: &mut [u8]) -> usize {
    for (at, value) in [QUERY, 28, id, oid, 0, 20, 0].iter().enumerate() {
        put_le32(out, at * 4, *value);
    }
    28
}
pub fn set(id: u32, oid: u32, value: u32, out: &mut [u8]) -> usize {
    for (at, word) in [SET, 32, id, oid, 4, 20, 0, value].iter().enumerate() {
        put_le32(out, at * 4, *word);
    }
    32
}
pub fn keepalive_done(id: u32, out: &mut [u8]) -> usize {
    for (at, value) in [KEEPALIVE_DONE, 16, id, 0].iter().enumerate() {
        put_le32(out, at * 4, *value);
    }
    16
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply<'a> {
    /// Completion for `request`: RNDIS status and, for queries, the data.
    Done {
        kind: u32,
        id: u32,
        status: u32,
        data: &'a [u8],
    },
    /// The device asks whether the host is alive.
    Keepalive(u32),
    /// Media or other status notices; nothing to answer.
    Notice,
}
/// Parses a control response from GET_ENCAPSULATED_RESPONSE.
pub fn reply(b: &[u8]) -> Option<Reply<'_>> {
    if b.len() < 12 {
        return None;
    }
    let kind = le32(b, 0);
    let length = (le32(b, 4) as usize).min(b.len());
    let id = le32(b, 8);
    match kind {
        KEEPALIVE => Some(Reply::Keepalive(id)),
        INDICATE_STATUS => Some(Reply::Notice),
        _ if kind & COMPLETION != 0 && length >= 16 => {
            let status = le32(b, 12);
            let mut data: &[u8] = &[];
            if kind == QUERY | COMPLETION && length >= 24 {
                let len = le32(b, 16) as usize;
                let start = 8usize.saturating_add(le32(b, 20) as usize);
                if start.checked_add(len).is_some_and(|end| end <= length) {
                    data = &b[start..start + len];
                }
            }
            Some(Reply::Done {
                kind: kind & !COMPLETION,
                id,
                status,
                data,
            })
        }
        _ => None,
    }
}
/// INITIALIZE completion fields: (max packets per transfer, max transfer size).
pub fn limits(b: &[u8]) -> Option<(u32, u32)> {
    (b.len() >= 40 && le32(b, 0) == INITIALIZE | COMPLETION).then(|| (le32(b, 32), le32(b, 36)))
}

/// Wraps `frame` (already placed at `out[PACKET_HEADER..]`) in a packet header.
pub fn packet(out: &mut [u8], frame_len: usize) -> usize {
    let total = PACKET_HEADER + frame_len;
    out[..PACKET_HEADER].fill(0);
    put_le32(out, 0, PACKET);
    put_le32(out, 4, total as u32);
    put_le32(out, 8, (PACKET_HEADER - 8) as u32);
    put_le32(out, 12, frame_len as u32);
    total
}
/// Ethernet frames in one bulk IN transfer, which may carry several packets.
pub struct Frames<'a> {
    rest: &'a [u8],
}
pub fn frames(transfer: &[u8]) -> Frames<'_> {
    Frames { rest: transfer }
}
impl<'a> Iterator for Frames<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        loop {
            let b = self.rest;
            if b.len() < PACKET_HEADER || le32(b, 0) != PACKET {
                return None;
            }
            let length = le32(b, 4) as usize;
            if length < PACKET_HEADER || length > b.len() {
                return None;
            }
            self.rest = &b[length..];
            let start = 8usize.saturating_add(le32(b, 8) as usize);
            let len = le32(b, 12) as usize;
            if start.checked_add(len).is_some_and(|end| end <= length) && len >= 14 {
                return Some(&b[start..start + len]);
            }
        }
    }
}
