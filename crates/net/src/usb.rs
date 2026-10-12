//! USB descriptors and standard requests needed to find an RNDIS function.
use crate::wire::le16;

pub const GET_DESCRIPTOR: u8 = 6;
pub const SET_ADDRESS: u8 = 5;
pub const SET_CONFIGURATION: u8 = 9;
pub const CLEAR_FEATURE: u8 = 1;
pub const DEVICE: u16 = 1;
pub const CONFIGURATION: u16 = 2;

/// An 8-byte SETUP packet.
pub fn setup(request_type: u8, request: u8, value: u16, index: u16, length: u16) -> [u8; 8] {
    let [v0, v1] = value.to_le_bytes();
    let [i0, i1] = index.to_le_bytes();
    let [l0, l1] = length.to_le_bytes();
    [request_type, request, v0, v1, i0, i1, l0, l1]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Device {
    pub max_packet: u8,
    pub vendor: u16,
    pub product: u16,
    pub configurations: u8,
}
pub fn device(b: &[u8]) -> Option<Device> {
    if b.len() < 18 || b[1] != DEVICE as u8 {
        return None;
    }
    Some(Device {
        max_packet: b[7],
        vendor: le16(b, 8),
        product: le16(b, 10),
        configurations: b[17],
    })
}
/// wTotalLength of a configuration descriptor header.
pub fn total_length(b: &[u8]) -> Option<u16> {
    (b.len() >= 4 && b[1] == CONFIGURATION as u8).then(|| le16(b, 2))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rndis {
    pub configuration: u8,
    pub control: u8,
    pub data: u8,
    pub data_alternate: u8,
    pub bulk_in: u8,
    pub bulk_out: u8,
    pub max_in: u16,
    pub max_out: u16,
}
fn rndis_control(class: u8, subclass: u8, protocol: u8) -> bool {
    matches!(
        (class, subclass, protocol),
        (0xe0, 0x01, 0x03) | (0x02, 0x02, 0xff) | (0xef, 0x04, 0x01)
    )
}
/// Finds the RNDIS control and data interfaces in a full configuration
/// descriptor. On failure returns the class, subclass and protocol
/// (class << 16 | subclass << 8 | protocol) of the first three different
/// interfaces, 0 where there are fewer, for diagnostics.
pub fn find_rndis(b: &[u8]) -> Result<Rndis, [u32; 3]> {
    if b.len() < 9 || b[1] != CONFIGURATION as u8 {
        return Err([0; 3]);
    }
    let configuration = b[5];
    let mut seen = [0u32; 3];
    let mut seen_count = 0;
    let mut control = None;
    // Interface being scanned: number, alternate, is a data interface.
    let mut current: Option<(u8, u8, bool)> = None;
    let mut endpoints = (None, None);
    let mut at = 0;
    while at + 2 <= b.len() {
        let len = b[at] as usize;
        if len < 2 || at + len > b.len() {
            break;
        }
        let d = &b[at..at + len];
        match d[1] {
            4 if len >= 9 => {
                let (class, subclass, protocol) = (d[5], d[6], d[7]);
                let triple = (class as u32) << 16 | (subclass as u32) << 8 | protocol as u32;
                if seen_count < 3 && !seen[..seen_count].contains(&triple) {
                    seen[seen_count] = triple;
                    seen_count += 1;
                }
                if control.is_none() && rndis_control(class, subclass, protocol) {
                    control = Some(d[2]);
                }
                current = Some((d[2], d[3], class == 0x0a && control.is_some()));
                endpoints = (None, None);
            }
            5 if len >= 7 => {
                if let Some((number, alternate, true)) = current {
                    let address = d[2];
                    let max = le16(d, 4) & 0x7ff;
                    if d[3] & 3 == 2 {
                        if address & 0x80 != 0 {
                            endpoints.0 = Some((address & 0x0f, max));
                        } else {
                            endpoints.1 = Some((address & 0x0f, max));
                        }
                    }
                    if let (Some((bulk_in, max_in)), Some((bulk_out, max_out)), Some(control)) =
                        (endpoints.0, endpoints.1, control)
                    {
                        return Ok(Rndis {
                            configuration,
                            control,
                            data: number,
                            data_alternate: alternate,
                            bulk_in,
                            bulk_out,
                            max_in,
                            max_out,
                        });
                    }
                }
            }
            _ => {}
        }
        at += len;
    }
    Err(seen)
}
