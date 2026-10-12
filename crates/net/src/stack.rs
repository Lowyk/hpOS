//! A small IPv4 host: ARP, ICMP echo, UDP for DHCP and DNS, and one TCP
//! client connection. Time is in milliseconds; frames go out through [`Link`].
use crate::wire::{be16, be32, checksum, put_be16, put_be32};

pub type Ip = [u8; 4];
pub type Mac = [u8; 6];
const BROADCAST: Mac = [0xff; 6];
const ANY: Ip = [0; 4];
const ETH: usize = 14;
const IP: usize = 20;
pub const MTU: usize = 1500;
const MSS: u16 = 1460;
const ARP_SLOTS: usize = 8;
const DHCP_SERVER: u16 = 67;
const DHCP_CLIENT: u16 = 68;
const DNS_PORT: u16 = 53;
const TX_BYTES: usize = 2048;

pub trait Link {
    fn transmit(&mut self, frame: &[u8]);
}

/// Where TCP payload goes. `len` grows as in-order data arrives.
pub struct Inbox<'a> {
    pub data: &'a mut [u8],
    pub len: usize,
}
impl Inbox<'_> {
    pub fn free(&self) -> usize {
        self.data.len() - self.len
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    pub address: Ip,
    pub mask: Ip,
    pub gateway: Ip,
    pub dns: Ip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Lease(Config),
    DhcpTimeout,
    Pong {
        address: Ip,
        rtt: u64,
        sequence: u16,
    },
    PingLost {
        address: Ip,
        sequence: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dhcp {
    Idle,
    Discovering,
    Requesting { offer: Ip, server: Ip },
    Bound,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    Idle,
    Pending,
    Found(Ip),
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    SynSent,
    Established,
    /// The peer finished and our FIN is out.
    Closing,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpEnd {
    /// The peer closed after sending everything.
    Finished,
    Refused,
    Reset,
    Timeout,
}

struct Timer {
    deadline: u64,
    tries: u32,
}

pub struct Stack {
    pub mac: Mac,
    pub config: Option<Config>,
    pub received: u32,
    pub sent: u32,
    arp: [(Ip, Mac); ARP_SLOTS],
    arp_next: usize,
    arp_asked: (Ip, u64),
    seed: u64,
    ip_id: u16,
    events: [Option<Event>; 8],
    dhcp: Dhcp,
    dhcp_xid: u32,
    dhcp_timer: Timer,
    lookup: Lookup,
    dns_id: u16,
    dns_port: u16,
    dns_name: [u8; 256],
    dns_len: usize,
    dns_timer: Timer,
    ping: Option<(Ip, u16, u64)>,
    ping_sequence: u16,
    tcp: Tcp,
}

struct Tcp {
    state: TcpState,
    end: Option<TcpEnd>,
    local: u16,
    remote: Ip,
    remote_port: u16,
    iss: u32,
    snd_una: u32,
    snd_nxt: u32,
    rcv_nxt: u32,
    peer_mss: u16,
    peer_window: u16,
    tx: [u8; TX_BYTES],
    tx_len: usize,
    fin_sent: bool,
    rto: u64,
    deadline: u64,
    tries: u32,
    progress: u64,
}

fn same_subnet(a: Ip, b: Ip, mask: Ip) -> bool {
    (0..4).all(|i| a[i] & mask[i] == b[i] & mask[i])
}
fn ip_at(b: &[u8], at: usize) -> Ip {
    [b[at], b[at + 1], b[at + 2], b[at + 3]]
}
/// TCP sequence comparison: a < b modulo 2^32.
fn before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

impl Stack {
    pub fn new(mac: Mac, seed: u64) -> Self {
        Self {
            mac,
            config: None,
            received: 0,
            sent: 0,
            arp: [(ANY, [0; 6]); ARP_SLOTS],
            arp_next: 0,
            arp_asked: (ANY, 0),
            seed: seed | 1,
            ip_id: seed as u16,
            events: [None; 8],
            dhcp: Dhcp::Idle,
            dhcp_xid: 0,
            dhcp_timer: Timer {
                deadline: 0,
                tries: 0,
            },
            lookup: Lookup::Idle,
            dns_id: 0,
            dns_port: 0,
            dns_name: [0; 256],
            dns_len: 0,
            dns_timer: Timer {
                deadline: 0,
                tries: 0,
            },
            ping: None,
            ping_sequence: 0,
            tcp: Tcp {
                state: TcpState::Closed,
                end: None,
                local: 0,
                remote: ANY,
                remote_port: 0,
                iss: 0,
                snd_una: 0,
                snd_nxt: 0,
                rcv_nxt: 0,
                peer_mss: 536,
                peer_window: 0,
                tx: [0; TX_BYTES],
                tx_len: 0,
                fin_sent: false,
                rto: 1000,
                deadline: 0,
                tries: 0,
                progress: 0,
            },
        }
    }
    fn random(&mut self) -> u32 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        (self.seed.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as u32
    }
    fn event(&mut self, event: Event) {
        if let Some(slot) = self.events.iter_mut().find(|e| e.is_none()) {
            *slot = Some(event);
        }
    }
    pub fn take_event(&mut self) -> Option<Event> {
        let first = self.events[0].take();
        self.events.rotate_left(1);
        first
    }
    pub fn learn(&mut self, ip: Ip, mac: Mac) {
        if ip == ANY || mac == BROADCAST {
            return;
        }
        if let Some(slot) = self.arp.iter_mut().find(|(a, _)| *a == ip) {
            slot.1 = mac;
            return;
        }
        self.arp[self.arp_next] = (ip, mac);
        self.arp_next = (self.arp_next + 1) % ARP_SLOTS;
    }
    pub fn neighbour(&self, ip: Ip) -> Option<Mac> {
        self.arp
            .iter()
            .find(|(a, _)| *a == ip && ip != ANY)
            .map(|(_, m)| *m)
    }

    // ---- Output ----

    fn ethernet(&self, frame: &mut [u8], to: Mac, kind: u16) {
        frame[0..6].copy_from_slice(&to);
        frame[6..12].copy_from_slice(&self.mac);
        put_be16(frame, 12, kind);
    }
    fn arp_request(&mut self, target: Ip, link: &mut dyn Link) {
        let mut f = [0u8; 42];
        self.ethernet(&mut f, BROADCAST, 0x0806);
        let own = self.config.map_or(ANY, |c| c.address);
        f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
        f[22..28].copy_from_slice(&self.mac);
        f[28..32].copy_from_slice(&own);
        f[38..42].copy_from_slice(&target);
        link.transmit(&f);
        self.sent += 1;
    }
    /// Sends an IPv4 datagram whose payload is `payload` (header parts are
    /// concatenated). Returns false while the next hop's MAC is unknown;
    /// an ARP request is then on its way and the caller's timer retries.
    fn send_ip(
        &mut self,
        to: Ip,
        protocol: u8,
        payload: &[&[u8]],
        now: u64,
        link: &mut dyn Link,
    ) -> bool {
        let length: usize = payload.iter().map(|p| p.len()).sum();
        if IP + length > MTU {
            return false;
        }
        let from = self.config.map_or(ANY, |c| c.address);
        let mac = if to == [255; 4] {
            BROADCAST
        } else {
            let Some(config) = self.config else {
                return false;
            };
            let hop = if same_subnet(to, config.address, config.mask) {
                to
            } else {
                config.gateway
            };
            match self.neighbour(hop) {
                Some(mac) => mac,
                None => {
                    if self.arp_asked.0 != hop || now >= self.arp_asked.1 + 300 {
                        self.arp_asked = (hop, now);
                        self.arp_request(hop, link);
                    }
                    return false;
                }
            }
        };
        let mut f = [0u8; ETH + MTU];
        self.ethernet(&mut f, mac, 0x0800);
        let h = &mut f[ETH..ETH + IP];
        h[0] = 0x45;
        put_be16(h, 2, (IP + length) as u16);
        self.ip_id = self.ip_id.wrapping_add(1);
        put_be16(h, 4, self.ip_id);
        put_be16(h, 6, 0x4000);
        h[8] = 64;
        h[9] = protocol;
        h[12..16].copy_from_slice(&from);
        h[16..20].copy_from_slice(&to);
        let sum = checksum(&[h]);
        put_be16(h, 10, sum);
        let mut at = ETH + IP;
        for part in payload {
            f[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        link.transmit(&f[..at]);
        self.sent += 1;
        true
    }
    fn pseudo(&self, to: Ip, protocol: u8, length: usize) -> [u8; 12] {
        let from = self.config.map_or(ANY, |c| c.address);
        let mut p = [0u8; 12];
        p[0..4].copy_from_slice(&from);
        p[4..8].copy_from_slice(&to);
        p[9] = protocol;
        put_be16(&mut p, 10, length as u16);
        p
    }
    fn send_udp(
        &mut self,
        to: Ip,
        from_port: u16,
        to_port: u16,
        data: &[u8],
        now: u64,
        link: &mut dyn Link,
    ) -> bool {
        let mut h = [0u8; 8];
        put_be16(&mut h, 0, from_port);
        put_be16(&mut h, 2, to_port);
        put_be16(&mut h, 4, (8 + data.len()) as u16);
        let pseudo = self.pseudo(to, 17, 8 + data.len());
        let sum = match checksum(&[&pseudo, &h, data]) {
            0 => 0xffff,
            s => s,
        };
        put_be16(&mut h, 6, sum);
        self.send_ip(to, 17, &[&h, data], now, link)
    }

    // ---- DHCP ----

    pub fn start_dhcp(&mut self, now: u64, link: &mut dyn Link) {
        self.config = None;
        self.dhcp = Dhcp::Discovering;
        self.dhcp_xid = self.random();
        self.dhcp_timer = Timer {
            deadline: now,
            tries: 0,
        };
        self.poll_dhcp(now, link);
    }
    pub fn bound(&self) -> bool {
        self.dhcp == Dhcp::Bound
    }
    fn dhcp_message(&mut self, kind: u8, link: &mut dyn Link, now: u64) {
        let mut m = [0u8; 300];
        m[0] = 1;
        m[1] = 1;
        m[2] = 6;
        put_be32(&mut m, 4, self.dhcp_xid);
        put_be16(&mut m, 10, 0x8000);
        m[28..34].copy_from_slice(&self.mac);
        put_be32(&mut m, 236, 0x6382_5363);
        let mut at = 240;
        let mut option = |m: &mut [u8; 300], code: u8, value: &[u8]| {
            m[at] = code;
            m[at + 1] = value.len() as u8;
            m[at + 2..at + 2 + value.len()].copy_from_slice(value);
            at += 2 + value.len();
        };
        option(&mut m, 53, &[kind]);
        let mut client = [1u8; 7];
        client[1..].copy_from_slice(&self.mac);
        option(&mut m, 61, &client);
        if let Dhcp::Requesting { offer, server } = self.dhcp {
            option(&mut m, 50, &offer);
            option(&mut m, 54, &server);
        }
        option(&mut m, 55, &[1, 3, 6, 51]);
        option(&mut m, 12, b"hpos");
        m[at] = 255;
        self.send_udp([255; 4], DHCP_CLIENT, DHCP_SERVER, &m, now, link);
    }
    fn poll_dhcp(&mut self, now: u64, link: &mut dyn Link) {
        if !matches!(self.dhcp, Dhcp::Discovering | Dhcp::Requesting { .. })
            || now < self.dhcp_timer.deadline
        {
            return;
        }
        if self.dhcp_timer.tries >= 8 {
            self.dhcp = Dhcp::Idle;
            self.event(Event::DhcpTimeout);
            return;
        }
        if self.dhcp_timer.tries == 4 {
            // Start over: the offer may have gone stale.
            self.dhcp = Dhcp::Discovering;
        }
        let kind = if self.dhcp == Dhcp::Discovering { 1 } else { 3 };
        self.dhcp_message(kind, link, now);
        self.dhcp_timer.tries += 1;
        self.dhcp_timer.deadline = now + 1000 * self.dhcp_timer.tries.min(4) as u64;
    }
    fn dhcp_input(&mut self, m: &[u8], source: Mac, now: u64, link: &mut dyn Link) {
        if m.len() < 240
            || m[0] != 2
            || be32(m, 4) != self.dhcp_xid
            || m[28..34] != self.mac
            || be32(m, 236) != 0x6382_5363
        {
            return;
        }
        let offered = ip_at(m, 16);
        let (mut kind, mut mask, mut router, mut dns, mut server) = (0, ANY, ANY, ANY, ANY);
        let mut at = 240;
        while at < m.len() {
            let code = m[at];
            if code == 255 {
                break;
            }
            if code == 0 {
                at += 1;
                continue;
            }
            if at + 1 >= m.len() {
                break;
            }
            let len = m[at + 1] as usize;
            let value = &m[(at + 2).min(m.len())..(at + 2 + len).min(m.len())];
            match (code, value.len()) {
                (53, 1..) => kind = value[0],
                (1, 4..) => mask = ip_at(value, 0),
                (3, 4..) => router = ip_at(value, 0),
                (6, 4..) => dns = ip_at(value, 0),
                (54, 4..) => server = ip_at(value, 0),
                _ => {}
            }
            at += 2 + len;
        }
        match (self.dhcp, kind) {
            (Dhcp::Discovering, 2) => {
                self.dhcp = Dhcp::Requesting {
                    offer: offered,
                    server,
                };
                self.dhcp_timer = Timer {
                    deadline: now,
                    tries: 0,
                };
                self.poll_dhcp(now, link);
            }
            (Dhcp::Requesting { .. }, 5) => {
                let gateway = if router == ANY { server } else { router };
                let config = Config {
                    address: offered,
                    mask: if mask == ANY {
                        [255, 255, 255, 0]
                    } else {
                        mask
                    },
                    gateway,
                    dns: if dns == ANY { gateway } else { dns },
                };
                self.config = Some(config);
                self.dhcp = Dhcp::Bound;
                // The phone is both DHCP server and router.
                if server == gateway {
                    self.learn(gateway, source);
                }
                self.event(Event::Lease(config));
            }
            (Dhcp::Requesting { .. }, 6) => {
                self.dhcp = Dhcp::Discovering;
                self.dhcp_xid = self.random();
                self.dhcp_timer = Timer {
                    deadline: now,
                    tries: 0,
                };
            }
            _ => {}
        }
    }

    // ---- DNS ----

    /// Starts resolving `name`; a dotted address resolves at once.
    pub fn resolve(&mut self, name: &str, now: u64, link: &mut dyn Link) {
        if let Some(ip) = parse_ip(name) {
            self.lookup = Lookup::Found(ip);
            return;
        }
        if name.is_empty() || name.len() > 250 || self.config.is_none() {
            self.lookup = Lookup::Failed;
            return;
        }
        self.dns_name[..name.len()].copy_from_slice(name.as_bytes());
        self.dns_len = name.len();
        self.dns_id = self.random() as u16;
        self.dns_port = 49152 + (self.random() % 16000) as u16;
        self.lookup = Lookup::Pending;
        self.dns_timer = Timer {
            deadline: now,
            tries: 0,
        };
        self.poll_dns(now, link);
    }
    pub fn lookup(&self) -> Lookup {
        self.lookup
    }
    fn poll_dns(&mut self, now: u64, link: &mut dyn Link) {
        if self.lookup != Lookup::Pending || now < self.dns_timer.deadline {
            return;
        }
        let Some(config) = self.config else {
            self.lookup = Lookup::Failed;
            return;
        };
        if self.dns_timer.tries >= 4 {
            self.lookup = Lookup::Failed;
            return;
        }
        let mut q = [0u8; 300];
        put_be16(&mut q, 0, self.dns_id);
        put_be16(&mut q, 2, 0x0100);
        put_be16(&mut q, 4, 1);
        let mut at = 12;
        let name = self.dns_name;
        for label in name[..self.dns_len].split(|b| *b == b'.') {
            if label.is_empty() || label.len() > 63 {
                continue;
            }
            q[at] = label.len() as u8;
            q[at + 1..at + 1 + label.len()].copy_from_slice(label);
            at += 1 + label.len();
        }
        q[at] = 0;
        put_be16(&mut q, at + 1, 1);
        put_be16(&mut q, at + 3, 1);
        at += 5;
        self.send_udp(config.dns, self.dns_port, DNS_PORT, &q[..at], now, link);
        self.dns_timer.tries += 1;
        self.dns_timer.deadline = now + 1500;
    }
    fn dns_input(&mut self, m: &[u8]) {
        if self.lookup != Lookup::Pending || m.len() < 12 || be16(m, 0) != self.dns_id {
            return;
        }
        let flags = be16(m, 2);
        if flags & 0x8000 == 0 {
            return;
        }
        if flags & 0xf != 0 {
            self.lookup = Lookup::Failed;
            return;
        }
        let questions = be16(m, 4);
        let answers = be16(m, 6);
        let mut at = 12;
        for _ in 0..questions {
            let Some(next) = skip_name(m, at) else {
                return;
            };
            at = next + 4;
        }
        for _ in 0..answers {
            let Some(next) = skip_name(m, at) else {
                break;
            };
            if next + 10 > m.len() {
                break;
            }
            let (kind, class, len) = (be16(m, next), be16(m, next + 2), be16(m, next + 8) as usize);
            let data = next + 10;
            if data + len > m.len() {
                break;
            }
            if kind == 1 && class == 1 && len == 4 {
                self.lookup = Lookup::Found(ip_at(m, data));
                return;
            }
            at = data + len;
        }
        self.lookup = Lookup::Failed;
    }

    // ---- ICMP ----

    pub fn ping(&mut self, address: Ip, now: u64, link: &mut dyn Link) {
        self.ping_sequence = self.ping_sequence.wrapping_add(1);
        self.ping = Some((address, self.ping_sequence, now));
        let mut m = [0u8; 40];
        m[0] = 8;
        put_be16(&mut m, 4, 0x4850);
        put_be16(&mut m, 6, self.ping_sequence);
        m[8..40].copy_from_slice(b"hpOS ping: Huynya Polneyshaya OS");
        let sum = checksum(&[&m]);
        put_be16(&mut m, 2, sum);
        self.send_ip(address, 1, &[&m], now, link);
    }
    fn icmp_input(&mut self, from: Ip, m: &[u8], now: u64, link: &mut dyn Link) {
        if m.len() < 8 || checksum(&[m]) != 0 {
            return;
        }
        match m[0] {
            8 => {
                let mut reply = [0u8; MTU - IP];
                let len = m.len().min(reply.len());
                reply[..len].copy_from_slice(&m[..len]);
                reply[0] = 0;
                reply[2] = 0;
                reply[3] = 0;
                let sum = checksum(&[&reply[..len]]);
                put_be16(&mut reply, 2, sum);
                self.send_ip(from, 1, &[&reply[..len]], now, link);
            }
            0 => {
                if let Some((address, sequence, sent)) = self.ping
                    && from == address
                    && be16(m, 4) == 0x4850
                    && be16(m, 6) == sequence
                {
                    self.ping = None;
                    self.event(Event::Pong {
                        address,
                        rtt: now.saturating_sub(sent),
                        sequence,
                    });
                }
            }
            _ => {}
        }
    }

    // ---- TCP ----

    /// Opens the TCP connection; `window` is the receive space on offer.
    pub fn connect(&mut self, remote: Ip, port: u16, window: usize, now: u64, link: &mut dyn Link) {
        let iss = self.random();
        let local = 49152 + (self.random() % 16000) as u16;
        let t = &mut self.tcp;
        t.state = TcpState::SynSent;
        t.end = None;
        t.local = local;
        t.remote = remote;
        t.remote_port = port;
        t.iss = iss;
        t.snd_una = iss;
        t.snd_nxt = iss;
        t.rcv_nxt = 0;
        t.peer_mss = 536;
        t.peer_window = 0;
        t.tx_len = 0;
        t.fin_sent = false;
        t.rto = 1000;
        t.tries = 0;
        t.deadline = now + t.rto;
        t.progress = now;
        self.tcp_output(now, link, window, true);
    }
    /// Queues request bytes; they go out once the connection is up.
    pub fn send(&mut self, data: &[u8]) -> bool {
        let t = &mut self.tcp;
        if t.tx_len + data.len() > TX_BYTES || t.state == TcpState::Closed {
            return false;
        }
        t.tx[t.tx_len..t.tx_len + data.len()].copy_from_slice(data);
        t.tx_len += data.len();
        true
    }
    pub fn tcp_state(&self) -> TcpState {
        self.tcp.state
    }
    pub fn tcp_end(&self) -> Option<TcpEnd> {
        self.tcp.end
    }
    /// Drops the connection, telling the peer.
    pub fn abort(&mut self, now: u64, link: &mut dyn Link) {
        if matches!(self.tcp.state, TcpState::Established | TcpState::Closing) {
            let t = &self.tcp;
            let (seq, ack) = (t.snd_nxt, t.rcv_nxt);
            self.tcp_segment(seq, ack, 0x14, &[], 0, now, link);
        }
        self.tcp.state = TcpState::Closed;
    }
    #[allow(clippy::too_many_arguments)]
    fn tcp_segment(
        &mut self,
        seq: u32,
        ack: u32,
        flags: u8,
        data: &[u8],
        window: usize,
        now: u64,
        link: &mut dyn Link,
    ) -> bool {
        let syn = flags & 0x02 != 0;
        let header = if syn { 24 } else { 20 };
        let mut h = [0u8; 24];
        put_be16(&mut h, 0, self.tcp.local);
        put_be16(&mut h, 2, self.tcp.remote_port);
        put_be32(&mut h, 4, seq);
        put_be32(&mut h, 8, ack);
        h[12] = (header as u8 / 4) << 4;
        h[13] = flags;
        put_be16(&mut h, 14, window.min(0xffff) as u16);
        if syn {
            h[20] = 2;
            h[21] = 4;
            put_be16(&mut h, 22, MSS);
        }
        let remote = self.tcp.remote;
        let pseudo = self.pseudo(remote, 6, header + data.len());
        let sum = checksum(&[&pseudo, &h[..header], data]);
        put_be16(&mut h, 16, sum);
        self.send_ip(remote, 6, &[&h[..header], data], now, link)
    }
    /// Sends what the state calls for: SYN, unsent data, FIN, or a bare ACK.
    fn tcp_output(&mut self, now: u64, link: &mut dyn Link, window: usize, force_ack: bool) {
        let t = &self.tcp;
        match t.state {
            TcpState::Closed => {}
            TcpState::SynSent => {
                let iss = t.iss;
                self.tcp_segment(iss, 0, 0x02, &[], window, now, link);
                self.tcp.snd_nxt = iss.wrapping_add(1);
            }
            TcpState::Established | TcpState::Closing => {
                let sent = t.snd_nxt.wrapping_sub(t.iss.wrapping_add(1)) as usize;
                let mut acked_something = false;
                if sent < t.tx_len {
                    let room = (t.peer_window as usize)
                        .saturating_sub(t.snd_nxt.wrapping_sub(t.snd_una) as usize);
                    let len = (t.tx_len - sent).min(t.peer_mss as usize).min(room);
                    if len > 0 {
                        let mut chunk = [0u8; TX_BYTES];
                        chunk[..len].copy_from_slice(&t.tx[sent..sent + len]);
                        let (seq, ack) = (t.snd_nxt, t.rcv_nxt);
                        if self.tcp_segment(seq, ack, 0x18, &chunk[..len], window, now, link) {
                            self.tcp.snd_nxt = seq.wrapping_add(len as u32);
                            acked_something = true;
                        }
                    }
                }
                let t = &self.tcp;
                if t.state == TcpState::Closing && !t.fin_sent {
                    let (seq, ack) = (t.snd_nxt, t.rcv_nxt);
                    if self.tcp_segment(seq, ack, 0x11, &[], window, now, link) {
                        self.tcp.snd_nxt = seq.wrapping_add(1);
                        self.tcp.fin_sent = true;
                        acked_something = true;
                    }
                }
                if force_ack && !acked_something {
                    let t = &self.tcp;
                    let (seq, ack) = (t.snd_nxt, t.rcv_nxt);
                    self.tcp_segment(seq, ack, 0x10, &[], window, now, link);
                }
            }
        }
    }
    fn tcp_timer(&mut self, now: u64, link: &mut dyn Link, window: usize) {
        let t = &mut self.tcp;
        if t.state == TcpState::Closed {
            return;
        }
        if now.saturating_sub(t.progress) > 30_000 {
            t.state = TcpState::Closed;
            t.end.get_or_insert(TcpEnd::Timeout);
            return;
        }
        let outstanding = t.snd_nxt != t.snd_una
            || (t.state != TcpState::SynSent
                && (t.snd_nxt.wrapping_sub(t.iss.wrapping_add(1)) as usize) < t.tx_len);
        if !outstanding || now < t.deadline {
            return;
        }
        if t.tries >= 7 {
            t.state = TcpState::Closed;
            t.end.get_or_insert(TcpEnd::Timeout);
            return;
        }
        // Go back and resend everything not yet acknowledged.
        t.tries += 1;
        t.deadline = now + t.rto;
        t.rto = (t.rto * 2).min(8000);
        t.snd_nxt = t.snd_una;
        if t.state == TcpState::Closing && t.fin_sent {
            t.fin_sent = false;
        }
        self.tcp_output(now, link, window, false);
    }
    #[allow(clippy::too_many_arguments)]
    fn tcp_input(&mut self, from: Ip, s: &[u8], now: u64, link: &mut dyn Link, inbox: &mut Inbox) {
        if s.len() < 20 {
            return;
        }
        let offset = (s[12] >> 4) as usize * 4;
        if offset < 20 || offset > s.len() {
            return;
        }
        let t = &self.tcp;
        if t.state == TcpState::Closed
            || from != t.remote
            || be16(s, 0) != t.remote_port
            || be16(s, 2) != t.local
        {
            return;
        }
        let (seq, ack, flags, window) = (be32(s, 4), be32(s, 8), s[13], be16(s, 14));
        let payload = &s[offset..];
        let has_ack = flags & 0x10 != 0;
        if flags & 0x04 != 0 {
            let acceptable = match t.state {
                TcpState::SynSent => has_ack && ack == t.iss.wrapping_add(1),
                _ => seq == t.rcv_nxt,
            };
            if acceptable {
                let refused = t.state == TcpState::SynSent;
                let end = if refused {
                    TcpEnd::Refused
                } else {
                    TcpEnd::Reset
                };
                self.tcp.state = TcpState::Closed;
                self.tcp.end.get_or_insert(end);
            }
            return;
        }
        if t.state == TcpState::SynSent {
            if flags & 0x12 == 0x12 && ack == t.iss.wrapping_add(1) {
                let mut mss = 536;
                let mut at = 20;
                while at < offset {
                    match s[at] {
                        0 => break,
                        1 => at += 1,
                        kind => {
                            let len = s.get(at + 1).copied().unwrap_or(0) as usize;
                            if len < 2 || at + len > offset {
                                break;
                            }
                            if kind == 2 && len == 4 {
                                mss = be16(s, at + 2).clamp(64, MSS);
                            }
                            at += len;
                        }
                    }
                }
                let t = &mut self.tcp;
                t.rcv_nxt = seq.wrapping_add(1);
                t.snd_una = ack;
                t.peer_mss = mss;
                t.peer_window = window;
                t.state = TcpState::Established;
                t.tries = 0;
                t.rto = 1000;
                t.deadline = now + t.rto;
                t.progress = now;
                self.tcp_output(now, link, inbox.free(), true);
            }
            return;
        }
        let t = &mut self.tcp;
        if has_ack && before(t.snd_una, ack) && !before(t.snd_nxt, ack) {
            t.snd_una = ack;
            t.tries = 0;
            t.rto = 1000;
            t.deadline = now + t.rto;
            t.progress = now;
        }
        t.peer_window = window;
        let fin = flags & 0x01 != 0;
        let mut ack_now = false;
        if !payload.is_empty() || fin {
            ack_now = true;
            if seq == t.rcv_nxt {
                let take = payload.len().min(inbox.free());
                inbox.data[inbox.len..inbox.len + take].copy_from_slice(&payload[..take]);
                inbox.len += take;
                t.rcv_nxt = t.rcv_nxt.wrapping_add(take as u32);
                if take > 0 {
                    t.progress = now;
                }
                if fin && take == payload.len() && t.state == TcpState::Established {
                    t.rcv_nxt = t.rcv_nxt.wrapping_add(1);
                    t.state = TcpState::Closing;
                    t.end.get_or_insert(TcpEnd::Finished);
                }
            }
        }
        if t.state == TcpState::Closing && t.fin_sent && t.snd_una == t.snd_nxt {
            t.state = TcpState::Closed;
        }
        let free = inbox.free();
        self.tcp_output(now, link, free, ack_now);
    }

    // ---- Input and timers ----

    /// Handles one received Ethernet frame.
    pub fn receive(&mut self, f: &[u8], now: u64, link: &mut dyn Link, inbox: &mut Inbox) {
        if f.len() < ETH || (f[0..6] != self.mac && f[0..6] != BROADCAST) {
            return;
        }
        self.received += 1;
        let source: Mac = [f[6], f[7], f[8], f[9], f[10], f[11]];
        match be16(f, 12) {
            0x0806 => self.arp_input(&f[ETH..], link),
            0x0800 => self.ip_input(&f[ETH..], source, now, link, inbox),
            _ => {}
        }
    }
    fn arp_input(&mut self, a: &[u8], link: &mut dyn Link) {
        if a.len() < 28 || be16(a, 0) != 1 || be16(a, 2) != 0x0800 || a[4] != 6 || a[5] != 4 {
            return;
        }
        let op = be16(a, 6);
        let sender_mac: Mac = [a[8], a[9], a[10], a[11], a[12], a[13]];
        let sender = ip_at(a, 14);
        let target = ip_at(a, 24);
        let Some(config) = self.config else {
            return;
        };
        if target == config.address || self.neighbour(sender).is_some() {
            self.learn(sender, sender_mac);
        }
        if op == 1 && target == config.address {
            let mut f = [0u8; 42];
            self.ethernet(&mut f, sender_mac, 0x0806);
            f[14..22].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 2]);
            f[22..28].copy_from_slice(&self.mac);
            f[28..32].copy_from_slice(&config.address);
            f[32..38].copy_from_slice(&sender_mac);
            f[38..42].copy_from_slice(&sender);
            link.transmit(&f);
            self.sent += 1;
        }
    }
    fn ip_input(
        &mut self,
        p: &[u8],
        source: Mac,
        now: u64,
        link: &mut dyn Link,
        inbox: &mut Inbox,
    ) {
        if p.len() < IP || p[0] >> 4 != 4 {
            return;
        }
        let header = (p[0] & 0xf) as usize * 4;
        let total = be16(p, 2) as usize;
        if header < IP || total < header || total > p.len() || checksum(&[&p[..header]]) != 0 {
            return;
        }
        // Fragments are not reassembled.
        if be16(p, 6) & 0x3fff != 0 {
            return;
        }
        let from = ip_at(p, 12);
        let to = ip_at(p, 16);
        let own = self.config.map(|c| c.address);
        let body = &p[header..total];
        match p[9] {
            1 if Some(to) == own => self.icmp_input(from, body, now, link),
            6 if Some(to) == own => {
                if checksum(&[&self.pseudo_from(from, 6, body.len()), body]) == 0 {
                    self.tcp_input(from, body, now, link, inbox);
                }
            }
            17 if body.len() >= 8 => {
                let (sport, dport) = (be16(body, 0), be16(body, 2));
                let len = (be16(body, 4) as usize).clamp(8, body.len());
                let data = &body[8..len];
                if dport == DHCP_CLIENT && sport == DHCP_SERVER {
                    self.dhcp_input(data, source, now, link);
                } else if Some(to) == own && sport == DNS_PORT && dport == self.dns_port {
                    self.dns_input(data);
                }
            }
            _ => {}
        }
    }
    /// Pseudo header for a segment received from `from`.
    fn pseudo_from(&self, from: Ip, protocol: u8, length: usize) -> [u8; 12] {
        let to = self.config.map_or(ANY, |c| c.address);
        let mut p = [0u8; 12];
        p[0..4].copy_from_slice(&from);
        p[4..8].copy_from_slice(&to);
        p[9] = protocol;
        put_be16(&mut p, 10, length as u16);
        p
    }
    /// Retransmissions and timeouts.
    pub fn poll(&mut self, now: u64, link: &mut dyn Link, inbox: &mut Inbox) {
        self.poll_dhcp(now, link);
        self.poll_dns(now, link);
        if let Some((address, sequence, sent)) = self.ping
            && now >= sent + 3000
        {
            self.ping = None;
            self.event(Event::PingLost { address, sequence });
        }
        self.tcp_timer(now, link, inbox.free());
    }
}

/// Skips a possibly compressed DNS name; returns the offset after it.
fn skip_name(m: &[u8], mut at: usize) -> Option<usize> {
    for _ in 0..128 {
        let len = *m.get(at)? as usize;
        if len == 0 {
            return Some(at + 1);
        }
        if len & 0xc0 == 0xc0 {
            return (at + 2 <= m.len()).then_some(at + 2);
        }
        at += 1 + len;
    }
    None
}
/// "192.168.1.1" to bytes.
pub fn parse_ip(text: &str) -> Option<Ip> {
    let mut ip = [0u8; 4];
    let mut parts = text.split('.');
    for byte in &mut ip {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *byte = part.parse::<u16>().ok().filter(|v| *v < 256)? as u8;
    }
    parts.next().is_none().then_some(ip)
}
