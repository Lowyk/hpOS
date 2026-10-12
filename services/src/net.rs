#![no_std]
#![no_main]
//! Network service: USB tethering (RNDIS) over the USB 2.0 or 1.1 controllers, the
//! IPv4 stack and HTTP fetches for the desktop. Everything it knows is
//! published in the shared `net::State` page.
mod ehci;
mod host;
mod runtime;
mod uhci;

use core::ptr::{addr_of, addr_of_mut, copy_nonoverlapping, read_volatile, write_volatile};
use host::{Host, Port};
use hpos_net::{
    http, rndis,
    stack::{Event, Inbox, Ip, Link, Lookup, Stack, TcpEnd, TcpState},
    usb,
};
use hpos_service_protocol::{
    NET_SHARED,
    net::{self, State, charset, error, event, fetch, stage},
};

const ADDRESS: u8 = 1;
/// Pages that are only reachable over HTTPS are read through this
/// plain-HTTP proxy, which simplifies them for old browsers.
const PROXY: &str = "http://frogfind.com/read.php?a=";

fn state() -> *mut State {
    NET_SHARED as *mut State
}
fn ticks() -> u64 {
    runtime::call(5, 0, 0).word
}
fn now_ms() -> u64 {
    ticks() * 10
}
fn bump() {
    unsafe {
        let v = addr_of_mut!((*state()).version);
        write_volatile(v, read_volatile(v).wrapping_add(1));
    }
}
fn set_stage(value: u32) {
    unsafe { write_volatile(addr_of_mut!((*state()).stage), value) };
    bump();
}
fn log(code: u32, a: u32, b: u32, c: u32) {
    unsafe {
        let s = state();
        let n = read_volatile(addr_of!((*s).events));
        write_volatile(
            addr_of_mut!((*s).log[n as usize % net::LOG]),
            net::Event { code, a, b, c },
        );
        write_volatile(addr_of_mut!((*s).events), n.wrapping_add(1));
    }
    bump();
}
fn pack(ip: Ip) -> u32 {
    u32::from_be_bytes(ip)
}
fn body() -> &'static mut [u8] {
    unsafe {
        core::slice::from_raw_parts_mut(
            (NET_SHARED as usize + net::BODY_OFFSET) as *mut u8,
            net::BODY_BYTES,
        )
    }
}

/// Where the device is: a USB 2.0 port, or a port of USB 1.1 controller `n`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Fast(u32),
    Slow(usize, u32),
}

const UHCI: usize = 4;

/// All host controllers; bulk OUT to the active one is the stack's link,
/// each frame in its own RNDIS packet.
struct Usb {
    ehci: Option<ehci::Controller>,
    uhci: [Option<uhci::Controller>; UHCI],
    active: Place,
    max_out: u16,
    errors: u32,
}
impl Usb {
    fn host(&mut self) -> &mut dyn Host {
        match self.active {
            Place::Fast(_) => self.ehci.as_mut().unwrap(),
            Place::Slow(n, _) => self.uhci[n].as_mut().unwrap(),
        }
    }
    fn places(&self) -> impl Iterator<Item = Place> + '_ {
        let fast = self.ehci.as_ref().map_or(0, |c| c.ports);
        (0..fast).map(Place::Fast).chain(
            (0..UHCI)
                .filter(|n| self.uhci[*n].is_some())
                .flat_map(|n| (0..2).map(move |port| Place::Slow(n, port))),
        )
    }
    fn connected(&self, place: Place) -> bool {
        match place {
            Place::Fast(port) => self.ehci.as_ref().is_some_and(|c| c.connected(port)),
            Place::Slow(n, port) => self.uhci[n].as_ref().is_some_and(|c| c.connected(port)),
        }
    }
    fn take_change(&self, place: Place) -> bool {
        match place {
            Place::Fast(port) => self.ehci.as_ref().is_some_and(|c| c.take_change(port)),
            Place::Slow(n, port) => self.uhci[n].as_ref().is_some_and(|c| c.take_change(port)),
        }
    }
    fn reset(&mut self, place: Place) -> Port {
        match place {
            Place::Fast(port) => self.ehci.as_mut().map_or(Port::Empty, |c| c.reset_port(port)),
            Place::Slow(n, port) => self.uhci[n].as_mut().map_or(Port::Empty, |c| c.reset_port(port)),
        }
    }
}
impl Link for Usb {
    fn transmit(&mut self, frame: &[u8]) {
        let max = self.max_out.max(1) as usize;
        let host = self.host();
        match host.out_buffer() {
            Ok(buffer) => {
                let len = frame.len().min(buffer.len() - rndis::PACKET_HEADER - 1);
                buffer[rndis::PACKET_HEADER..rndis::PACKET_HEADER + len].copy_from_slice(&frame[..len]);
                let mut total = rndis::packet(buffer, len);
                // A transfer ending on a packet boundary would need a
                // zero-length packet; one spare byte avoids it.
                if total % max == 0 {
                    buffer[total] = 0;
                    total += 1;
                }
                host.submit(total);
            }
            Err(_) => {
                let _ = host.clear_halt(false);
                self.errors += 1;
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Abort {
    Stop,
    /// A new FETCH replaced the running one.
    Replaced,
}

struct Service {
    usb: Option<Usb>,
    stack: Stack,
    port: Option<Place>,
    /// A device that is not RNDIS: ignored until it reconnects.
    ignored: Option<Place>,
    rndis: Option<usb::Rndis>,
    request_id: u32,
    inbox_len: usize,
    last_port_check: u64,
    last_publish: u64,
    dhcp_retry: Option<u64>,
    reported_errors: u32,
    /// A FETCH that arrived while another was running.
    pending: Option<(u32, usize, [u8; net::URL_BYTES])>,
}

impl Service {
    // ---- Commands from the desktop ----

    /// Copies the request text and acknowledges it.
    fn take_request(sequence: u32) -> (usize, [u8; net::URL_BYTES]) {
        let mut text = [0u8; net::URL_BYTES];
        unsafe {
            let s = state();
            let len = (read_volatile(addr_of!((*s).request_len)) as usize).min(net::URL_BYTES);
            copy_nonoverlapping(addr_of!((*s).request).cast::<u8>(), text.as_mut_ptr(), len);
            write_volatile(addr_of_mut!((*s).taken), sequence);
            (len, text)
        }
    }
    fn next_command() -> Option<(u64, u32)> {
        let message = runtime::call(2, 1, 0);
        (message.status == 0 && message.sender == 4)
            .then(|| (message.word & 0xff, (message.word >> 8) as u32))
    }
    /// Checked while a long operation runs.
    fn interrupted(&mut self) -> Option<Abort> {
        while let Some((op, sequence)) = Self::next_command() {
            match op {
                net::STOP => return Some(Abort::Stop),
                net::FETCH => {
                    let (len, text) = Self::take_request(sequence);
                    self.pending = Some((sequence, len, text));
                    return Some(Abort::Replaced);
                }
                _ => {}
            }
        }
        None
    }

    // ---- USB device ----

    fn start(&mut self) {
        if self.usb.is_some() {
            return;
        }
        set_stage(stage::STARTING);
        let mut usb = Usb {
            ehci: None,
            uhci: [const { None }; UHCI],
            active: Place::Fast(0),
            max_out: 64,
            errors: 0,
        };
        // USB 2.0 first: it decides which ports stay with the companions.
        if let Some(mut controller) = ehci::Controller::open() {
            log(event::CONTROLLER, controller.id, controller.ports, 2);
            match controller.start() {
                Ok(polite) => {
                    log(event::HANDOFF, polite as u32, 0, 0);
                    usb.ehci = Some(controller);
                }
                Err(ehci::StartError(step)) => log(event::CONTROLLER_FAILED, step, 2, 0),
            }
        }
        for n in 0..UHCI {
            if let Some(mut controller) = uhci::Controller::open(n as u64) {
                log(event::CONTROLLER, controller.id, 2, 1);
                if controller.start() {
                    usb.uhci[n] = Some(controller);
                } else {
                    log(event::CONTROLLER_FAILED, 2, 1, 0);
                }
            }
        }
        if usb.ehci.is_none() && usb.uhci.iter().all(|c| c.is_none()) {
            set_stage(stage::NO_CONTROLLER);
            return;
        }
        self.usb = Some(usb);
        set_stage(stage::NO_DEVICE);
        self.watch_ports(true);
    }
    fn controller(&mut self) -> &mut dyn Host {
        self.usb.as_mut().unwrap().host()
    }
    /// Notices plugged and unplugged devices.
    fn watch_ports(&mut self, force: bool) {
        let now = ticks();
        let Some(usb) = self.usb.as_ref() else {
            return;
        };
        if !force && now.wrapping_sub(self.last_port_check) < 25 {
            return;
        }
        self.last_port_check = now;
        if let Some(place) = self.port {
            let changed = usb.take_change(place);
            if !changed && usb.connected(place) {
                return;
            }
            self.link_down();
            log(event::DISCONNECTED, 0, 0, 0);
        }
        let mut places = [Place::Fast(0); 24];
        let mut count = 0;
        for place in self.usb.as_ref().unwrap().places() {
            if count < places.len() {
                places[count] = place;
                count += 1;
            }
        }
        for &place in &places[..count] {
            let usb = self.usb.as_mut().unwrap();
            let changed = usb.take_change(place);
            if changed && self.ignored == Some(place) {
                self.ignored = None;
            }
            if self.ignored == Some(place) || !usb.connected(place) {
                continue;
            }
            // Let the plug settle before resetting.
            runtime::sleep(10);
            let (number, slow, controller) = match place {
                Place::Fast(port) => (port, 0, 0),
                Place::Slow(n, port) => (port, 1, n as u32),
            };
            let low_speed = match usb.reset(place) {
                Port::Empty => continue,
                Port::Slow { before, after } => {
                    log(event::SLOW_DEVICE, number, after, before);
                    continue;
                }
                Port::Ready { low_speed } => low_speed,
            };
            log(event::PORT, number, slow, controller);
            usb.take_change(place);
            usb.active = place;
            // Phones are often not ready right after they appear (they also
            // reconnect when tethering turns on): retry with a fresh reset.
            for attempt in 0..3 {
                if attempt > 0 {
                    runtime::sleep(30);
                    let usb = self.usb.as_mut().unwrap();
                    if !usb.connected(place) || !matches!(usb.reset(place), Port::Ready { .. }) {
                        break;
                    }
                }
                match self.enumerate(low_speed) {
                    Ok(()) => {
                        self.port = Some(place);
                        return;
                    }
                    Err(Some((step, status))) => {
                        let debug = self.controller().debug(number);
                        log(event::USB_FAILED, step | slow << 8, status, debug);
                    }
                    Err(None) => break,
                }
            }
            let usb = self.usb.as_mut().unwrap();
            if usb.connected(place) && !usb.take_change(place) {
                self.ignored = Some(place);
            }
            set_stage(stage::NO_DEVICE);
        }
    }
    fn link_down(&mut self) {
        self.port = None;
        self.rndis = None;
        self.stack.config = None;
        self.dhcp_retry = None;
        unsafe {
            let s = state();
            write_volatile(addr_of_mut!((*s).address), [0; 4]);
        }
        set_stage(stage::NO_DEVICE);
    }
    /// Errors carry (step, status) for a retry, or `None` once logged.
    fn enumerate(&mut self, low_speed: bool) -> Result<(), Option<(u32, u32)>> {
        set_stage(stage::DEVICE);
        let c = self.controller();
        c.reset_device(low_speed);
        let mut buffer = [0u8; 2048];
        let get = |c: &mut dyn Host, kind: u16, index: u16, buffer: &mut [u8]| {
            let setup = usb::setup(0x80, usb::GET_DESCRIPTOR, kind << 8 | index, 0, buffer.len() as u16);
            c.control_in(setup, buffer)
        };
        // The first 8 bytes tell endpoint 0's packet size, needed for the rest.
        match get(c, usb::DEVICE, 0, &mut buffer[..8]) {
            Ok(8) => c.set_address(0, (buffer[7] as u16).clamp(8, 64)),
            Ok(_) => return Err(Some((1, 0))),
            Err(status) => {
                return Err(Some((1, status)));
            }
        }
        let device = match get(c, usb::DEVICE, 0, &mut buffer[..18]) {
            Ok(n) => usb::device(&buffer[..n]),
            Err(status) => {
                return Err(Some((1, status)));
            }
        };
        let Some(device) = device else {
            return Err(Some((1, 0)));
        };
        unsafe {
            let s = state();
            write_volatile(addr_of_mut!((*s).vendor), device.vendor);
            write_volatile(addr_of_mut!((*s).product), device.product);
        }
        log(event::DEVICE, device.vendor as u32, device.product as u32, device.configurations as u32);
        let setup = usb::setup(0x00, usb::SET_ADDRESS, ADDRESS as u16, 0, 0);
        if let Err(status) = c.control_out(setup, &[]) {
            return Err(Some((2, status)));
        }
        runtime::sleep(2);
        c.set_address(ADDRESS, device.max_packet.max(8) as u16);
        // Some phones refuse a configuration request now and then (STALL);
        // a refusal moves on to the next configuration and is retried once.
        let mut found = Err([0; 3]);
        let mut parsed = false;
        let mut refused = None;
        for index in 0..device.configurations.max(1) as u16 {
            let step = 3 | ((index as u32 + 1).min(15)) << 4;
            for _ in 0..2 {
                let total = match get(c, usb::CONFIGURATION, index, &mut buffer[..9]) {
                    Ok(n) => usb::total_length(&buffer[..n]).unwrap_or(9) as usize,
                    Err(status) => {
                        refused = Some((step, status));
                        runtime::sleep(2);
                        continue;
                    }
                };
                let total = total.clamp(9, buffer.len());
                match get(c, usb::CONFIGURATION, index, &mut buffer[..total]) {
                    Ok(n) => {
                        parsed = true;
                        found = usb::find_rndis(&buffer[..n]);
                        break;
                    }
                    Err(status) => {
                        refused = Some((step, status));
                        runtime::sleep(2);
                    }
                }
            }
            if found.is_ok() {
                break;
            }
        }
        if found.is_err() && !parsed {
            return Err(refused);
        }
        if let (Err(_), Some((step, status))) = (found, refused) {
            // A configuration was read but had no modem, another refused: say both.
            log(event::USB_FAILED, step, status, 0);
        }
        let rndis = match found {
            Ok(rndis) => rndis,
            Err([a, b, c]) => {
                log(event::NOT_RNDIS, a, b, c);
                return Err(None);
            }
        };
        let setup = usb::setup(0x00, usb::SET_CONFIGURATION, rndis.configuration as u16, 0, 0);
        if let Err(status) = c.control_out(setup, &[]) {
            return Err(Some((4, status)));
        }
        log(event::RNDIS, rndis.configuration as u32, rndis.control as u32, rndis.data as u32);
        c.configure_bulk((rndis.bulk_in, rndis.max_in), (rndis.bulk_out, rndis.max_out));
        if let Some(usb) = self.usb.as_mut() {
            usb.max_out = rndis.max_out;
        }
        self.rndis = Some(rndis);
        self.rndis_start().map_err(|()| None)
    }
    /// Sends one RNDIS control message and waits for its completion, which
    /// is copied to `reply`. Returns the reply length.
    fn command(&mut self, message: &[u8], kind: u32, reply: &mut [u8]) -> Result<usize, u32> {
        let control = self.rndis.map_or(0, |r| r.control) as u16;
        let id = self.request_id;
        let c = self.controller();
        c.control_out(usb::setup(0x21, 0x00, 0, control, message.len() as u16), message)?;
        for attempt in 0..60 {
            if attempt > 0 {
                runtime::sleep(1);
            }
            let setup = usb::setup(0xa1, 0x01, 0, control, 1025);
            let Ok(n) = c.control_in(setup, &mut reply[..1025]) else {
                continue;
            };
            match rndis::reply(&reply[..n]) {
                Some(rndis::Reply::Done { kind: k, id: i, status, .. }) if k == kind && i == id => {
                    return if status == 0 { Ok(n) } else { Err(status) };
                }
                Some(rndis::Reply::Keepalive(i)) => {
                    let mut answer = [0u8; 16];
                    let len = rndis::keepalive_done(i, &mut answer);
                    let _ = c.control_out(usb::setup(0x21, 0x00, 0, control, len as u16), &answer[..len]);
                }
                _ => {}
            }
        }
        Err(host::TIMEOUT)
    }
    fn rndis_start(&mut self) -> Result<(), ()> {
        let mut message = [0u8; 64];
        let mut reply = [0u8; 1100];
        self.request_id += 1;
        let n = rndis::initialize(self.request_id, &mut message);
        if let Err(status) = self.command(&message[..n], rndis::INITIALIZE, &mut reply) {
            log(event::RNDIS_FAILED, 1, status, 0);
            return Err(());
        }
        self.request_id += 1;
        let n = rndis::query(self.request_id, rndis::OID_PERMANENT_ADDRESS, &mut message);
        let mac = match self.command(&message[..n], rndis::QUERY, &mut reply) {
            Ok(len) => match rndis::reply(&reply[..len]) {
                Some(rndis::Reply::Done { data, .. }) if data.len() >= 6 => {
                    [data[0], data[1], data[2], data[3], data[4], data[5]]
                }
                _ => {
                    log(event::RNDIS_FAILED, 2, 0, 0);
                    return Err(());
                }
            },
            Err(status) => {
                log(event::RNDIS_FAILED, 2, status, 0);
                return Err(());
            }
        };
        self.request_id += 1;
        let n = rndis::set(self.request_id, rndis::OID_PACKET_FILTER, rndis::FILTER, &mut message);
        if let Err(status) = self.command(&message[..n], rndis::SET, &mut reply) {
            log(event::RNDIS_FAILED, 3, status, 0);
            return Err(());
        }
        unsafe {
            write_volatile(addr_of_mut!((*state()).mac), mac);
        }
        log(
            event::LINK_UP,
            u32::from_be_bytes([mac[0], mac[1], mac[2], mac[3]]),
            u32::from_be_bytes([0, 0, mac[4], mac[5]]),
            0,
        );
        set_stage(stage::LINK);
        let seed = unsafe { core::arch::x86_64::_rdtsc() };
        self.stack = Stack::new(mac, seed);
        self.controller().receive();
        self.start_dhcp();
        Ok(())
    }
    fn start_dhcp(&mut self) {
        set_stage(stage::ADDRESS);
        log(event::DHCP_DISCOVER, 0, 0, 0);
        self.dhcp_retry = None;
        let now = now_ms();
        if let Some(usb) = self.usb.as_mut() {
            self.stack.start_dhcp(now, usb);
        }
    }

    // ---- Moving frames ----

    /// Receives, runs timers and reports; returns whether any frame arrived.
    fn pump(&mut self) -> bool {
        self.watch_ports(false);
        if self.port.is_none() {
            return false;
        }
        let now = now_ms();
        let mut inbox = Inbox {
            data: body(),
            len: self.inbox_len,
        };
        let Some(usb) = self.usb.as_mut() else {
            return false;
        };
        let mut work = false;
        match usb.host().received() {
            Some(Ok(transfer)) => {
                work = true;
                for frame in rndis::frames(transfer) {
                    self.stack.receive(frame, now, usb, &mut inbox);
                }
                usb.host().receive();
            }
            Some(Err(_)) => {
                usb.errors += 1;
                let _ = usb.host().clear_halt(true);
                usb.host().receive();
            }
            None => {}
        }
        self.stack.poll(now, usb, &mut inbox);
        self.inbox_len = inbox.len;
        let errors = usb.errors;
        if errors != self.reported_errors && errors % 16 == 1 {
            self.reported_errors = errors;
            log(event::USB_ERROR, errors, 0, 0);
        }
        while let Some(e) = self.stack.take_event() {
            match e {
                Event::Lease(config) => {
                    unsafe {
                        let s = state();
                        write_volatile(addr_of_mut!((*s).address), config.address);
                        write_volatile(addr_of_mut!((*s).gateway), config.gateway);
                        write_volatile(addr_of_mut!((*s).dns), config.dns);
                        write_volatile(addr_of_mut!((*s).mask), config.mask);
                    }
                    log(event::LEASE, pack(config.address), pack(config.gateway), pack(config.dns));
                    set_stage(stage::ONLINE);
                }
                Event::DhcpTimeout => {
                    log(event::DHCP_TIMEOUT, 0, 0, 0);
                    self.dhcp_retry = Some(ticks() + 500);
                }
                Event::Pong { address, rtt, sequence } => {
                    log(event::PONG, pack(address), rtt as u32, sequence as u32);
                }
                Event::PingLost { address, sequence } => {
                    log(event::PING_LOST, pack(address), 0, sequence as u32);
                }
            }
        }
        if self.dhcp_retry.is_some_and(|t| ticks() >= t) {
            self.start_dhcp();
        }
        let tick = ticks();
        if tick.wrapping_sub(self.last_publish) >= 10 {
            self.last_publish = tick;
            unsafe {
                let s = state();
                write_volatile(addr_of_mut!((*s).received), self.stack.received);
                write_volatile(addr_of_mut!((*s).sent), self.stack.sent);
            }
            bump();
        }
        work
    }
    /// Pumps until `done` holds, the deadline (ticks) passes or the desktop
    /// interrupts. Returns false when interrupted or timed out.
    fn pump_until(&mut self, limit: u64, mut done: impl FnMut(&mut Self) -> bool) -> Result<bool, Abort> {
        let start = ticks();
        loop {
            if done(self) {
                return Ok(true);
            }
            if let Some(abort) = self.interrupted() {
                return Err(abort);
            }
            if self.port.is_none() || ticks().wrapping_sub(start) > limit {
                return Ok(false);
            }
            if !self.pump() {
                runtime::sleep(1);
            }
        }
    }
    fn online(&self) -> bool {
        self.port.is_some() && self.stack.bound()
    }

    // ---- Requests ----

    fn resolve(&mut self, host: &str) -> Result<Option<Ip>, Abort> {
        let now = now_ms();
        let Some(usb) = self.usb.as_mut() else {
            return Ok(None);
        };
        self.stack.resolve(host, now, usb);
        self.pump_until(800, |s| s.stack.lookup() != Lookup::Pending)?;
        Ok(match self.stack.lookup() {
            Lookup::Found(ip) => Some(ip),
            _ => None,
        })
    }
    fn ping(&mut self, host: &str) -> Result<(), Abort> {
        if !self.online() {
            log(event::FETCH_FAILED, error::OFFLINE, 0, 0);
            return Ok(());
        }
        let Some(address) = self.resolve(host)? else {
            log(event::FETCH_FAILED, error::DNS, 0, 0);
            return Ok(());
        };
        for _ in 0..3 {
            let now = now_ms();
            if let Some(usb) = self.usb.as_mut() {
                self.stack.ping(address, now, usb);
            }
            let sent = ticks();
            self.pump_until(100, |_| ticks().wrapping_sub(sent) >= 100)?;
        }
        Ok(())
    }
    fn set_fetch(stage: u32, error_code: u32) {
        unsafe {
            let s = state();
            write_volatile(addr_of_mut!((*s).fetch_error), error_code);
            write_volatile(addr_of_mut!((*s).fetch), stage);
        }
        if stage == fetch::FAILED {
            log(event::FETCH_FAILED, error_code, 0, 0);
        }
        bump();
    }
    fn fetch(&mut self, sequence: u32, request: &[u8]) {
        unsafe {
            let s = state();
            write_volatile(addr_of_mut!((*s).fetch_sequence), sequence);
            write_volatile(addr_of_mut!((*s).progress), 0);
            write_volatile(addr_of_mut!((*s).body_len), 0);
            write_volatile(addr_of_mut!((*s).status), 0);
        }
        let result = self.load(request);
        let now = now_ms();
        if let Some(usb) = self.usb.as_mut() {
            self.stack.abort(now, usb);
        }
        match result {
            Ok(()) => {}
            Err(Ok(code)) => Self::set_fetch(fetch::FAILED, code),
            Err(Err(Abort::Stop)) => Self::set_fetch(fetch::FAILED, error::STOPPED),
            Err(Err(Abort::Replaced)) => Self::set_fetch(fetch::FAILED, error::STOPPED),
        }
    }
    /// Follows redirects and leaves the body at the start of the body area.
    fn load(&mut self, request: &[u8]) -> Result<(), Result<u32, Abort>> {
        let mut url = [0u8; net::URL_BYTES];
        let mut url_len = request.len().min(url.len());
        url[..url_len].copy_from_slice(&request[..url_len]);
        for _ in 0..6 {
            if !self.online() {
                return Err(Ok(error::OFFLINE));
            }
            let text = core::str::from_utf8(&url[..url_len]).map_err(|_| Ok(error::BAD_URL))?;
            let parsed = http::parse(text).ok_or(Ok(error::BAD_URL))?;
            if parsed.secure {
                // Reach HTTPS pages through the plain-HTTP reader proxy.
                let mut proxied = [0u8; net::URL_BYTES];
                proxied[..PROXY.len()].copy_from_slice(PROXY.as_bytes());
                let n = http::encode_query(text, &mut proxied[PROXY.len()..]).ok_or(Ok(error::BAD_URL))?;
                url_len = PROXY.len() + n;
                url[..url_len].copy_from_slice(&proxied[..url_len]);
                continue;
            }
            unsafe {
                let s = state();
                copy_nonoverlapping(url.as_ptr(), addr_of_mut!((*s).url).cast::<u8>(), url_len);
                write_volatile(addr_of_mut!((*s).url_len), url_len as u32);
            }
            Self::set_fetch(fetch::RESOLVING, 0);
            let address = self.resolve(parsed.host).map_err(Err)?.ok_or(Ok(error::DNS))?;
            log(event::RESOLVED, pack(address), 0, 0);
            Self::set_fetch(fetch::CONNECTING, 0);
            log(event::CONNECTING, pack(address), parsed.port as u32, 0);
            self.inbox_len = 0;
            let now = now_ms();
            if let Some(usb) = self.usb.as_mut() {
                self.stack.connect(address, parsed.port, net::BODY_BYTES, now, usb);
            }
            self.pump_until(3500, |s| s.stack.tcp_state() != TcpState::SynSent)
                .map_err(Err)?;
            if self.stack.tcp_state() != TcpState::Established {
                return Err(Ok(match self.stack.tcp_end() {
                    Some(TcpEnd::Refused) | Some(TcpEnd::Reset) => error::CONNECT,
                    _ => error::TIMEOUT,
                }));
            }
            let mut message = [0u8; 2048];
            let n = http::request(&parsed, &mut message).ok_or(Ok(error::BAD_URL))?;
            self.stack.send(&message[..n]);
            Self::set_fetch(fetch::LOADING, 0);
            let mut shown = 0;
            self.pump_until(6000, |s| {
                let len = s.inbox_len;
                if len != shown && len.wrapping_sub(shown) > 4096 {
                    shown = len;
                    unsafe { write_volatile(addr_of_mut!((*state()).progress), len as u32) };
                    bump();
                }
                s.stack.tcp_end().is_some() || len == net::BODY_BYTES || response_complete(&body()[..len])
            })
            .map_err(Err)?;
            let len = self.inbox_len;
            let buffer = &mut body()[..len];
            let Some(head) = http::head(buffer) else {
                return Err(Ok(match self.stack.tcp_end() {
                    Some(TcpEnd::Reset) => error::RESET,
                    Some(TcpEnd::Timeout) | None => error::TIMEOUT,
                    _ => error::PROTOCOL,
                }));
            };
            if matches!(head.status, 301 | 302 | 303 | 307 | 308) {
                if let Some((start, end)) = head.location {
                    let mut next = [0u8; net::URL_BYTES];
                    let location = core::str::from_utf8(&buffer[start..end]).unwrap_or("");
                    let base = core::str::from_utf8(&url[..url_len]).unwrap_or("");
                    if let Some(n) = http::resolve(base, location, &mut next) {
                        let now = now_ms();
                        if let Some(usb) = self.usb.as_mut() {
                            self.stack.abort(now, usb);
                        }
                        url[..n].copy_from_slice(&next[..n]);
                        url_len = n;
                        continue;
                    }
                }
            }
            let mut content = &mut buffer[head.length..];
            if head.chunked {
                let n = http::dechunk(content);
                content = &mut content[..n];
            } else if let Some(length) = head.content_length {
                let n = length.min(content.len());
                content = &mut content[..n];
            }
            let body_len = content.len();
            let start = head.length;
            body().copy_within(start..start + body_len, 0);
            unsafe {
                let s = state();
                write_volatile(addr_of_mut!((*s).status), head.status as u32);
                write_volatile(addr_of_mut!((*s).content), head.content);
                write_volatile(addr_of_mut!((*s).charset), head.charset.unwrap_or(charset::UNKNOWN));
                write_volatile(addr_of_mut!((*s).body_len), body_len as u32);
                write_volatile(addr_of_mut!((*s).progress), len as u32);
            }
            log(event::RESPONSE, head.status as u32, body_len as u32, 0);
            Self::set_fetch(fetch::DONE, 0);
            return Ok(());
        }
        Err(Ok(error::REDIRECTS))
    }
}

/// True once a response with a known length has fully arrived.
fn response_complete(buffer: &[u8]) -> bool {
    let Some(head) = http::head(buffer) else {
        return false;
    };
    let rest = &buffer[head.length..];
    if head.chunked {
        http::chunked_complete(rest).is_some()
    } else if let Some(length) = head.content_length {
        rest.len() >= length
    } else {
        head.status == 204 || head.status == 304 || (100..200).contains(&head.status)
    }
}

#[unsafe(no_mangle)]
extern "C" fn service_main() -> ! {
    // The desktop-only status call must be refused to this service.
    if runtime::call(5, 1, 0).status != 3 {
        runtime::fail();
    }
    let mut service = Service {
        usb: None,
        stack: Stack::new([0; 6], 1),
        port: None,
        ignored: None,
        rndis: None,
        request_id: 0,
        inbox_len: 0,
        last_port_check: 0,
        last_publish: 0,
        dhcp_retry: None,
        reported_errors: 0,
        pending: None,
    };
    set_stage(stage::OFF);
    let mut report = 1;
    loop {
        let mut job: Option<(u64, u32, usize, [u8; net::URL_BYTES])> = service
            .pending
            .take()
            .map(|(sequence, len, text)| (net::FETCH, sequence, len, text));
        if job.is_none() {
            if let Some((op, sequence)) = Service::next_command() {
                let (len, text) = Service::take_request(sequence);
                job = Some((op, sequence, len, text));
            }
        }
        match job {
            Some((net::CONNECT, ..)) => {
                if service.usb.is_none() {
                    service.start();
                } else {
                    service.watch_ports(true);
                }
            }
            Some((net::FETCH, sequence, len, text)) => service.fetch(sequence, &text[..len]),
            Some((net::PING, _, len, text)) => {
                let host = core::str::from_utf8(&text[..len]).unwrap_or("");
                let _ = service.ping(host);
            }
            _ => {
                if !service.pump() {
                    runtime::sleep(1);
                }
            }
        }
        runtime::report(report, 0);
        report += 1;
    }
}
