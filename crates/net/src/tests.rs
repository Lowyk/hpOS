extern crate std;
use crate::{
    http, rndis,
    stack::{Event, Inbox, Ip, Link, Lookup, Mac, Stack, TcpEnd, TcpState, parse_ip},
    usb,
    wire::{be16, be32, checksum, put_be16, put_be32},
};
use std::{borrow::ToOwned, collections::VecDeque, vec, vec::Vec};

#[test]
fn checksum_matches_rfc1071_example() {
    let data = [0x00, 0x01, 0xf2, 0x03, 0xf4, 0xf5, 0xf6, 0xf7];
    assert_eq!(checksum(&[&data]), !0xddf2);
    // Splitting at an odd offset gives the same sum.
    assert_eq!(checksum(&[&data[..3], &data[3..]]), !0xddf2);
}

#[test]
fn rndis_messages_round_trip() {
    let mut b = [0u8; 64];
    assert_eq!(rndis::initialize(7, &mut b), 24);
    assert_eq!(&b[..8], &[2, 0, 0, 0, 24, 0, 0, 0]);
    assert_eq!(
        rndis::set(9, rndis::OID_PACKET_FILTER, rndis::FILTER, &mut b),
        32
    );
    assert_eq!(&b[28..32], &[0x2f, 0, 0, 0]);
    // A query completion carrying a MAC address.
    let mut reply = vec![0u8; 30];
    for (at, v) in [0x8000_0004u32, 30, 5, 0, 6, 16].iter().enumerate() {
        reply[at * 4..at * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    reply[24..30].copy_from_slice(&[2, 0x11, 0x22, 0x33, 0x44, 0x55]);
    match rndis::reply(&reply) {
        Some(rndis::Reply::Done {
            kind,
            id,
            status,
            data,
        }) => {
            assert_eq!((kind, id, status), (rndis::QUERY, 5, 0));
            assert_eq!(data, &[2, 0x11, 0x22, 0x33, 0x44, 0x55]);
        }
        other => panic!("{other:?}"),
    }
    // Two packets in one transfer, the second with padding after it.
    let mut transfer = [0u8; 200];
    let mut first = [0u8; 44 + 60];
    first[44..].fill(0xaa);
    let n = rndis::packet(&mut first, 60);
    transfer[..n].copy_from_slice(&first);
    let mut second = [0u8; 44 + 20];
    second[44..].fill(0xbb);
    let m = rndis::packet(&mut second, 20);
    transfer[n..n + m].copy_from_slice(&second);
    let frames: Vec<&[u8]> = rndis::frames(&transfer[..n + m + 1]).collect();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0], &[0xaa; 60][..]);
    assert_eq!(frames[1], &[0xbb; 20][..]);
}

/// An Android RNDIS gadget with ADB alongside, as `lsusb -v` shows it.
fn android_configuration() -> Vec<u8> {
    let mut c = vec![9, 2, 0, 0, 3, 1, 0, 0x80, 250];
    c.extend([8, 0x0b, 0, 2, 0xe0, 1, 3, 0]); // IAD
    c.extend([9, 4, 0, 0, 1, 0xe0, 1, 3, 5]); // RNDIS control
    c.extend([5, 0x24, 0, 0x10, 1]); // CDC header
    c.extend([5, 0x24, 1, 0, 1]); // call management
    c.extend([4, 0x24, 2, 0]); // ACM
    c.extend([5, 0x24, 6, 0, 1]); // union
    c.extend([7, 5, 0x81, 3, 8, 0, 9]); // notification
    c.extend([9, 4, 1, 0, 2, 0x0a, 0, 0, 6]); // data
    c.extend([7, 5, 0x82, 2, 0, 2, 0]);
    c.extend([7, 5, 0x01, 2, 0, 2, 0]);
    c.extend([9, 4, 2, 0, 2, 0xff, 0x42, 1, 7]); // ADB
    c.extend([7, 5, 0x03, 2, 0, 2, 0]);
    c.extend([7, 5, 0x83, 2, 0, 2, 0]);
    let len = c.len() as u16;
    c[2..4].copy_from_slice(&len.to_le_bytes());
    c
}

#[test]
fn rndis_function_is_found_in_a_composite_configuration() {
    let c = android_configuration();
    assert_eq!(usb::total_length(&c), Some(c.len() as u16));
    assert_eq!(
        usb::find_rndis(&c),
        Ok(usb::Rndis {
            configuration: 1,
            control: 0,
            data: 1,
            data_alternate: 0,
            bulk_in: 2,
            bulk_out: 1,
            max_in: 512,
            max_out: 512,
        })
    );
    // MTP only: reports the interface class instead.
    let mtp = [
        9, 2, 39, 0, 1, 1, 0, 0x80, 250, 9, 4, 0, 0, 3, 6, 1, 1, 0, 7, 5, 0x81, 2, 0, 2, 0, 7, 5,
        1, 2, 0, 2, 0, 7, 5, 0x82, 3, 28, 0, 6,
    ];
    assert_eq!(usb::find_rndis(&mtp), Err([0x060101, 0, 0]));
    let device = [
        18, 1, 0, 2, 0, 0, 0, 64, 0xd1, 0x12, 0x2e, 0x10, 0, 2, 1, 2, 3, 1,
    ];
    assert_eq!(
        usb::device(&device),
        Some(usb::Device {
            max_packet: 64,
            vendor: 0x12d1,
            product: 0x102e,
            configurations: 1,
        })
    );
}

#[test]
fn urls_parse_and_resolve() {
    let u = http::parse("http://Example.com:8080/a/b?q=1#top").unwrap();
    assert_eq!(
        (u.secure, u.host, u.port, u.path),
        (false, "Example.com", 8080, "/a/b?q=1")
    );
    let u = http::parse("frogfind.com").unwrap();
    assert_eq!((u.host, u.port, u.path), ("frogfind.com", 80, "/"));
    assert!(http::parse("https://a.ru").unwrap().secure);
    assert_eq!(http::parse("ftp://x"), None);
    assert_eq!(http::parse("http://bad host/"), None);
    let mut out = [0u8; 256];
    let mut check = |base: &str, href: &str, expected: Option<&str>| {
        let got = http::resolve(base, href, &mut out)
            .map(|n| std::str::from_utf8(&out[..n]).unwrap().to_owned());
        assert_eq!(got.as_deref(), expected, "{base} + {href}");
    };
    check(
        "http://a.com/x/y.html",
        "z.html",
        Some("http://a.com/x/z.html"),
    );
    check("http://a.com/x/y.html", "../z", Some("http://a.com/z"));
    check("http://a.com/x/y/", "./", Some("http://a.com/x/y/"));
    check("http://a.com/x/y?q", "/r?s=1", Some("http://a.com/r?s=1"));
    check("http://a.com/x/y?q", "?p", Some("http://a.com/x/y?p"));
    check("http://a.com:81/", "//b.org/c", Some("http://b.org/c"));
    check("http://a.com:81/", "d", Some("http://a.com:81/d"));
    check("http://a.com/", "https://s.com/#x", Some("https://s.com/"));
    check("http://a.com/", "mailto:x@y", None);
    check("http://a.com/", "#only", None);
    check(
        "http://a.com/a/b/c",
        "../../../../d",
        Some("http://a.com/d"),
    );
    let mut q = [0u8; 64];
    let n = http::encode_query("кот & dog", &mut q).unwrap();
    assert_eq!(&q[..n], b"%D0%BA%D0%BE%D1%82+%26+dog");
}

#[test]
fn http_heads_and_chunked_bodies() {
    let url = http::parse("http://frogfind.com/?q=hp os").unwrap();
    let mut out = [0u8; 512];
    let n = http::request(&url, &mut out).unwrap();
    let text = std::str::from_utf8(&out[..n]).unwrap();
    assert!(text.starts_with("GET /?q=hp%20os HTTP/1.1\r\nHost: frogfind.com\r\n"));
    assert!(text.ends_with("Connection: close\r\n\r\n"));

    let response = b"HTTP/1.1 301 Moved\r\nLocation:  /new \r\nContent-Type: text/html; charset=Windows-1251\r\nContent-Length: 0\r\n\r\n";
    let h = http::head(response).unwrap();
    assert_eq!(h.status, 301);
    assert_eq!(h.length, response.len());
    let (a, b) = h.location.unwrap();
    assert_eq!(&response[a..b], b"/new");
    assert_eq!(
        (h.content, h.charset, h.content_length),
        (1, Some(1), Some(0))
    );
    assert_eq!(http::head(b"HTTP/1.1 200 OK\r\nX: y\r\n"), None);

    let mut body =
        b"4\r\nWiki\r\n6;ext=1\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\n\r\n".to_vec();
    assert_eq!(http::chunked_complete(&body), Some(24));
    assert_eq!(http::chunked_complete(&body[..20]), None);
    let n = http::dechunk(&mut body);
    assert_eq!(&body[..n], b"Wikipedia in \r\n\r\nchunks.");
}

// ---- A fake phone for the IPv4 stack ----

const PHONE_MAC: Mac = [0x02, 0, 0, 0, 0, 1];
const OUR_MAC: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
const PHONE: Ip = [192, 168, 42, 129];
const LEASED: Ip = [192, 168, 42, 10];
const SERVER: Ip = [93, 184, 216, 34];

#[derive(Default)]
struct Wire {
    to_phone: VecDeque<Vec<u8>>,
}
impl Link for Wire {
    fn transmit(&mut self, frame: &[u8]) {
        self.to_phone.push_back(frame.to_vec());
    }
}

struct Phone {
    to_host: VecDeque<Vec<u8>>,
    /// Drop the n-th TCP data segment sent to the host (1-based), once.
    drop_segment: Option<usize>,
    segments_sent: usize,
    response: Vec<u8>,
    seq: u32,
    rcv: u32,
    request: Vec<u8>,
    sent_response: bool,
    host_port: u16,
    fin_acked: bool,
}

fn ip_packet(from: Ip, to: Ip, protocol: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![0u8; 14 + 20];
    f[0..6].copy_from_slice(&OUR_MAC);
    f[6..12].copy_from_slice(&PHONE_MAC);
    put_be16(&mut f, 12, 0x0800);
    f[14] = 0x45;
    put_be16(&mut f, 16, (20 + payload.len()) as u16);
    f[22] = 64;
    f[23] = protocol;
    f[26..30].copy_from_slice(&from);
    f[30..34].copy_from_slice(&to);
    let sum = checksum(&[&f[14..34]]);
    put_be16(&mut f, 24, sum);
    f.extend_from_slice(payload);
    f
}
fn pseudo(from: Ip, to: Ip, protocol: u8, len: usize) -> [u8; 12] {
    let mut p = [0u8; 12];
    p[0..4].copy_from_slice(&from);
    p[4..8].copy_from_slice(&to);
    p[9] = protocol;
    put_be16(&mut p, 10, len as u16);
    p
}
fn udp(from: Ip, to: Ip, sport: u16, dport: u16, data: &[u8]) -> Vec<u8> {
    let mut u = vec![0u8; 8];
    put_be16(&mut u, 0, sport);
    put_be16(&mut u, 2, dport);
    put_be16(&mut u, 4, (8 + data.len()) as u16);
    u.extend_from_slice(data);
    let sum = checksum(&[&pseudo(from, to, 17, u.len()), &u]);
    put_be16(&mut u, 6, sum);
    ip_packet(from, to, 17, &u)
}

impl Phone {
    fn new(response: &[u8]) -> Self {
        Self {
            to_host: VecDeque::new(),
            drop_segment: None,
            segments_sent: 0,
            response: response.to_vec(),
            seq: 5000,
            rcv: 0,
            request: Vec::new(),
            sent_response: false,
            host_port: 0,
            fin_acked: false,
        }
    }
    fn tcp(&mut self, flags: u8, data: &[u8], options: &[u8]) {
        let header = 20 + options.len();
        let mut t = vec![0u8; header];
        put_be16(&mut t, 0, 80);
        put_be16(&mut t, 2, self.host_port);
        put_be32(&mut t, 4, self.seq);
        put_be32(&mut t, 8, self.rcv);
        t[12] = (header as u8 / 4) << 4;
        t[13] = flags;
        put_be16(&mut t, 14, 65535);
        t[20..].copy_from_slice(options);
        t.extend_from_slice(data);
        let sum = checksum(&[&pseudo(SERVER, LEASED, 6, t.len()), &t]);
        put_be16(&mut t, 16, sum);
        let frame = ip_packet(SERVER, LEASED, 6, &t);
        if !data.is_empty() {
            self.segments_sent += 1;
            if self.drop_segment == Some(self.segments_sent) {
                self.drop_segment = None;
                return;
            }
        }
        self.to_host.push_back(frame);
    }
    fn handle(&mut self, f: &[u8]) {
        match be16(f, 12) {
            0x0806 => {
                // ARP request for the phone.
                if be16(f, 20) == 1 && f[38..42] == PHONE {
                    let mut r = f.to_vec();
                    r[0..6].copy_from_slice(&f[6..12]);
                    r[6..12].copy_from_slice(&PHONE_MAC);
                    put_be16(&mut r, 20, 2);
                    r[22..28].copy_from_slice(&PHONE_MAC);
                    r[28..32].copy_from_slice(&PHONE);
                    r[32..38].copy_from_slice(&f[22..28]);
                    r[38..42].copy_from_slice(&f[28..32]);
                    self.to_host.push_back(r);
                }
            }
            0x0800 => {
                let p = &f[14..];
                assert_eq!(checksum(&[&p[..20]]), 0, "bad IP header checksum");
                let to: Ip = p[16..20].try_into().unwrap();
                let from: Ip = p[12..16].try_into().unwrap();
                let body = &p[20..be16(p, 2) as usize];
                match p[9] {
                    17 => {
                        assert_eq!(checksum(&[&pseudo(from, to, 17, body.len()), body]), 0);
                        let data = &body[8..];
                        match be16(body, 2) {
                            67 => self.dhcp(data),
                            53 => self.dns(from, be16(body, 0), data),
                            _ => {}
                        }
                    }
                    1 => {
                        assert_eq!(checksum(&[body]), 0);
                        let mut r = body.to_vec();
                        r[0] = 0;
                        r[2] = 0;
                        r[3] = 0;
                        let sum = checksum(&[&r]);
                        put_be16(&mut r, 2, sum);
                        self.to_host.push_back(ip_packet(to, from, 1, &r));
                    }
                    6 if to == SERVER => {
                        assert_eq!(
                            checksum(&[&pseudo(from, to, 6, body.len()), body]),
                            0,
                            "bad TCP checksum"
                        );
                        self.server(body);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    fn dhcp(&mut self, m: &[u8]) {
        let kind = m[242];
        let reply_kind = match kind {
            1 => 2,
            3 => 5,
            _ => return,
        };
        let mut r = vec![0u8; 240];
        r[0] = 2;
        r[1] = 1;
        r[2] = 6;
        r[4..8].copy_from_slice(&m[4..8]);
        r[16..20].copy_from_slice(&LEASED);
        r[28..34].copy_from_slice(&m[28..34]);
        put_be32(&mut r, 236, 0x6382_5363);
        r.extend([53, 1, reply_kind, 54, 4]);
        r.extend(PHONE);
        r.extend([1, 4, 255, 255, 255, 0, 3, 4]);
        r.extend(PHONE);
        r.extend([6, 4]);
        r.extend(PHONE);
        r.extend([51, 4, 0, 0, 0x0e, 0x10, 255]);
        self.to_host.push_back(udp(PHONE, [255; 4], 67, 68, &r));
    }
    fn dns(&mut self, from: Ip, port: u16, q: &[u8]) {
        let mut r = q.to_vec();
        put_be16(&mut r, 2, 0x8180);
        put_be16(&mut r, 6, 2);
        // A CNAME first, then the address, both pointing at the question name.
        r.extend([0xc0, 12, 0, 5, 0, 1, 0, 0, 0, 60, 0, 2, 0xc0, 12]);
        r.extend([0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4]);
        r.extend(SERVER);
        self.to_host.push_back(udp(PHONE, from, 53, port, &r));
    }
    fn server(&mut self, t: &[u8]) {
        let flags = t[13];
        let seq = be32(t, 4);
        let offset = (t[12] >> 4) as usize * 4;
        let data = &t[offset..];
        if flags & 0x02 != 0 {
            assert_eq!(&t[20..24], &[2, 4, 0x05, 0xb4], "MSS option");
            self.host_port = be16(t, 0);
            self.rcv = seq.wrapping_add(1);
            self.tcp(0x12, &[], &[2, 4, 0x02, 0x00]);
            self.seq += 1;
            return;
        }
        if flags & 0x04 != 0 {
            return;
        }
        if !data.is_empty() && seq == self.rcv {
            self.request.extend_from_slice(data);
            self.rcv = self.rcv.wrapping_add(data.len() as u32);
        }
        if flags & 0x01 != 0 && seq.wrapping_add(data.len() as u32) == self.rcv {
            self.rcv = self.rcv.wrapping_add(1);
            self.tcp(0x10, &[], &[]);
        }
        if flags & 0x10 != 0 && be32(t, 8) == self.seq && self.sent_response {
            self.fin_acked = true;
        }
        if self.request.ends_with(b"\r\n\r\n") && !self.sent_response {
            self.sent_response = true;
            let response = self.response.clone();
            let start = self.seq;
            // Everything at once, as fast senders do; segments of 512 bytes.
            for chunk in response.chunks(512) {
                self.tcp(0x18, chunk, &[]);
                self.seq += chunk.len() as u32;
            }
            self.tcp(0x11, &[], &[]);
            self.seq += 1;
            let _ = start;
        } else if self.sent_response
            && be32(t, 8) != self.seq
            && flags & 0x10 != 0
            && data.is_empty()
        {
            // A duplicate ACK: resend from where the host is.
            let acked = be32(t, 8);
            let offset =
                acked.wrapping_sub(self.seq.wrapping_sub(self.response.len() as u32 + 1)) as usize;
            if offset < self.response.len() {
                let saved = self.seq;
                self.seq = acked;
                let rest = self.response[offset..].to_vec();
                for chunk in rest.chunks(512) {
                    self.tcp(0x18, chunk, &[]);
                    self.seq += chunk.len() as u32;
                }
                self.tcp(0x11, &[], &[]);
                self.seq = saved;
            }
        }
    }
}

/// Runs both sides until `done` or the time limit.
fn run(
    stack: &mut Stack,
    phone: &mut Phone,
    inbox: &mut Inbox,
    now: &mut u64,
    mut done: impl FnMut(&mut Stack, &Inbox) -> bool,
) {
    let mut wire = Wire::default();
    for _ in 0..20_000 {
        if done(stack, inbox) {
            return;
        }
        while let Some(f) = phone.to_host.pop_front() {
            stack.receive(&f, *now, &mut wire, inbox);
        }
        stack.poll(*now, &mut wire, inbox);
        while let Some(f) = wire.to_phone.pop_front() {
            assert!(f.len() <= 1514);
            phone.handle(&f);
        }
        *now += 10;
    }
    panic!("timed out");
}

#[test]
fn dhcp_dns_and_an_http_fetch_over_a_lossy_link() {
    let mut body = Vec::new();
    for n in 0..300 {
        body.extend_from_slice(std::format!("<p>line {n} строка</p>\n").as_bytes());
    }
    let mut response = std::format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    let mut phone = Phone::new(&response);
    phone.drop_segment = Some(3);
    let mut stack = Stack::new(OUR_MAC, 42);
    let mut space = vec![0u8; 64 * 1024];
    let mut inbox = Inbox {
        data: &mut space,
        len: 0,
    };
    let mut now = 1000;
    let mut wire = Wire::default();
    stack.start_dhcp(now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.bound()
    });
    let config = stack.config.unwrap();
    assert_eq!(
        (config.address, config.gateway, config.dns),
        (LEASED, PHONE, PHONE)
    );
    assert!(matches!(stack.take_event(), Some(Event::Lease(_))));
    assert_eq!(stack.neighbour(PHONE), Some(PHONE_MAC));

    stack.ping(PHONE, now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        matches!(s.take_event(), Some(Event::Pong { sequence: 1, .. }))
    });

    stack.resolve("example.com", now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.lookup() != Lookup::Pending
    });
    assert_eq!(stack.lookup(), Lookup::Found(SERVER));

    stack.connect(SERVER, 80, inbox.free(), now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.tcp_state() == TcpState::Established
    });
    let url = http::parse("http://example.com/").unwrap();
    let mut request = [0u8; 512];
    let n = http::request(&url, &mut request).unwrap();
    assert!(stack.send(&request[..n]));
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.tcp_end().is_some()
    });
    assert_eq!(stack.tcp_end(), Some(TcpEnd::Finished));
    assert_eq!(&inbox.data[..inbox.len], &response[..]);
    let h = http::head(&inbox.data[..inbox.len]).unwrap();
    assert_eq!((h.status, h.content_length), (200, Some(body.len())));
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.tcp_state() == TcpState::Closed
    });
    assert!(phone.fin_acked);
}

#[test]
fn a_full_inbox_stops_taking_data_and_unanswered_syns_time_out() {
    let response = vec![b'x'; 5000];
    let mut phone = Phone::new(&response);
    let mut stack = Stack::new(OUR_MAC, 7);
    let mut space = vec![0u8; 1000];
    let mut inbox = Inbox {
        data: &mut space,
        len: 0,
    };
    let mut now = 0;
    let mut wire = Wire::default();
    stack.start_dhcp(now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.bound()
    });
    stack.connect(SERVER, 80, inbox.free(), now, &mut wire);
    phone.handle(&wire.to_phone.pop_front().unwrap());
    run(&mut stack, &mut phone, &mut inbox, &mut now, |s, _| {
        s.tcp_state() == TcpState::Established
    });
    stack.send(b"GET / HTTP/1.1\r\n\r\n");
    run(&mut stack, &mut phone, &mut inbox, &mut now, |_, i| {
        i.len == 1000
    });
    assert_eq!(inbox.free(), 0);

    // Nobody answers on this address.
    let mut silent = Phone::new(b"");
    let target = parse_ip("10.1.2.3").unwrap();
    stack.connect(target, 80, inbox.free(), now, &mut wire);
    wire.to_phone.clear();
    let start = now;
    run(&mut stack, &mut silent, &mut inbox, &mut now, |s, _| {
        s.tcp_end().is_some()
    });
    assert_eq!(stack.tcp_end(), Some(TcpEnd::Timeout));
    assert!(now - start >= 15_000);
}
