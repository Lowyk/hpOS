//! Network logic for hpOS without hardware access: RNDIS messages, USB
//! descriptor parsing, a small IPv4 stack (ARP, ICMP, UDP, DHCP, DNS, TCP)
//! and HTTP/1.1 helpers. The network service drives it with Ethernet frames.
#![no_std]

pub mod http;
pub mod rndis;
pub mod stack;
pub mod usb;
pub mod wire;

#[cfg(test)]
mod tests;
