//! Desktop side of the network service: commands go out over IPC; state,
//! log and fetched pages come back through the shared `net::State` page.
use crate::runtime;
use core::ptr::{addr_of, addr_of_mut, copy_nonoverlapping, read_volatile, write_volatile};
use hpos_desktop::{
    Shell,
    browser::{self, Page},
    web::{SHOWN_EVENTS, Url},
};
use hpos_service_protocol::{
    NET_SHARED,
    net::{self, State, content, fetch},
};

const QUEUE: usize = 4;
const HANDLE: u64 = 4;

fn state() -> *mut State {
    NET_SHARED as *mut State
}

pub struct Net {
    sequence: u32,
    version: u32,
    events: u32,
    /// Sequence of the fetch the browser waits for.
    fetch: Option<u32>,
    queue: [Option<(u64, u32, Url)>; QUEUE],
    sent: u32,
}

/// Events that arrived since the last poll, oldest first.
pub struct Fresh {
    pub events: [net::Event; SHOWN_EVENTS],
    pub count: usize,
}

impl Net {
    pub const fn new() -> Self {
        Self {
            sequence: 0,
            version: u32::MAX,
            events: 0,
            fetch: None,
            queue: [None; QUEUE],
            sent: 0,
        }
    }
    /// Queues a command for the network service; returns its sequence.
    pub fn send(&mut self, op: u64, text: &str) -> u32 {
        self.sequence = self.sequence.wrapping_add(1).max(1);
        let entry = (op, self.sequence, Url::new(text));
        match self.queue.iter_mut().find(|q| q.is_none()) {
            Some(slot) => *slot = Some(entry),
            None => {
                self.queue.rotate_left(1);
                self.queue[QUEUE - 1] = Some(entry);
            }
        }
        self.flush();
        self.sequence
    }
    /// Hands the oldest queued command over once the service has copied
    /// the previous request text.
    fn flush(&mut self) {
        let Some((op, sequence, text)) = self.queue[0] else {
            return;
        };
        let s = state();
        unsafe {
            if read_volatile(addr_of!((*s).taken)) != self.sent {
                return;
            }
            let bytes = text.as_str().as_bytes();
            copy_nonoverlapping(bytes.as_ptr(), addr_of_mut!((*s).request).cast::<u8>(), bytes.len());
            write_volatile(addr_of_mut!((*s).request_len), bytes.len() as u32);
            write_volatile(addr_of_mut!((*s).request_sequence), sequence);
        }
        if runtime::call(1, HANDLE, op | (sequence as u64) << 8).status == 0 {
            self.sent = sequence;
            self.queue.rotate_left(1);
            self.queue[QUEUE - 1] = None;
        }
    }
    /// Starts loading `url` in the browser.
    pub fn navigate(&mut self, shell: &mut Shell, url: &str, remember: bool) {
        let b = &mut shell.browser;
        if remember && b.has_page {
            b.remember();
        }
        b.loading = true;
        b.error = 0;
        b.progress = 0;
        b.editing = false;
        b.address = Url::new(url);
        self.fetch = Some(self.send(net::FETCH, url));
        shell.browser_changed();
    }
    pub fn stop(&mut self) {
        self.send(net::STOP, "");
    }
    /// Copies what changed into the shell; loads a finished page.
    pub fn poll(&mut self, shell: &mut Shell, page: &mut Page) -> Fresh {
        self.flush();
        let mut fresh = Fresh {
            events: [net::Event { code: 0, a: 0, b: 0, c: 0 }; SHOWN_EVENTS],
            count: 0,
        };
        let s = state();
        let version = unsafe { read_volatile(addr_of!((*s).version)) };
        if version == self.version {
            return fresh;
        }
        self.version = version;
        let n = &mut shell.net;
        unsafe {
            n.stage = read_volatile(addr_of!((*s).stage));
            n.address = read_volatile(addr_of!((*s).address));
            n.gateway = read_volatile(addr_of!((*s).gateway));
            n.dns = read_volatile(addr_of!((*s).dns));
            n.mac = read_volatile(addr_of!((*s).mac));
            n.vendor = read_volatile(addr_of!((*s).vendor));
            n.product = read_volatile(addr_of!((*s).product));
            n.received = read_volatile(addr_of!((*s).received));
            n.sent = read_volatile(addr_of!((*s).sent));
            let total = read_volatile(addr_of!((*s).events));
            let first = total.saturating_sub(SHOWN_EVENTS as u32);
            n.event_count = (total - first) as usize;
            for (slot, index) in (first..total).enumerate() {
                n.events[slot] = read_volatile(addr_of!((*s).log[index as usize % net::LOG]));
            }
            let start = self.events.max(first);
            for index in start..total {
                fresh.events[fresh.count] = read_volatile(addr_of!((*s).log[index as usize % net::LOG]));
                fresh.count += 1;
            }
            self.events = total;
        }
        shell.network_changed();
        if let Some(sequence) = self.fetch {
            let (current, stage) = unsafe {
                (
                    read_volatile(addr_of!((*s).fetch_sequence)),
                    read_volatile(addr_of!((*s).fetch)),
                )
            };
            if current == sequence {
                match stage {
                    fetch::DONE => {
                        self.fetch = None;
                        self.show(shell, page);
                    }
                    fetch::FAILED => {
                        self.fetch = None;
                        shell.browser.loading = false;
                        shell.browser.error = unsafe { read_volatile(addr_of!((*s).fetch_error)) };
                    }
                    _ => shell.browser.progress = unsafe { read_volatile(addr_of!((*s).progress)) },
                }
                shell.browser_changed();
            }
        }
        fresh
    }
    fn show(&mut self, shell: &mut Shell, page: &mut Page) {
        let s = state();
        let (len, kind, set, status, url_len) = unsafe {
            (
                (read_volatile(addr_of!((*s).body_len)) as usize).min(net::BODY_BYTES),
                read_volatile(addr_of!((*s).content)),
                read_volatile(addr_of!((*s).charset)),
                read_volatile(addr_of!((*s).status)),
                (read_volatile(addr_of!((*s).url_len)) as usize).min(net::URL_BYTES),
            )
        };
        let body = unsafe {
            core::slice::from_raw_parts((NET_SHARED as usize + net::BODY_OFFSET) as *const u8, len)
        };
        let mut url = [0u8; net::URL_BYTES];
        unsafe { copy_nonoverlapping(addr_of!((*s).url).cast::<u8>(), url.as_mut_ptr(), url_len) };
        let url = core::str::from_utf8(&url[..url_len]).unwrap_or("");
        let starts_with_tag = body.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'<');
        let binary = body.iter().take(512).filter(|b| **b < 9).count() > 4;
        if kind == content::OTHER && binary {
            let message: &[u8] = match shell.lang {
                hpos_desktop::lang::Lang::En => b"<h2>This is not a text page</h2><p>hpOS shows only HTML and plain text.</p>",
                hpos_desktop::lang::Lang::Uk => "<h2>Це не текстова сторінка</h2><p>hpOS показує лише HTML і текст.</p>".as_bytes(),
                _ => "<h2>Это не текстовая страница</h2><p>hpOS показывает только HTML и текст.</p>".as_bytes(),
            };
            browser::load(page, message, browser::charset::UTF8, true);
        } else {
            let html = kind == content::HTML || (kind == content::OTHER && starts_with_tag);
            browser::load(page, body, set, html);
        }
        browser::layout(page, shell.page_width());
        let b = &mut shell.browser;
        b.has_page = true;
        b.loading = false;
        b.error = 0;
        b.status = status;
        b.current = Url::new(url);
        if !b.editing {
            b.address = b.current;
        }
        b.scroll = 0;
        b.page_height = page.height;
        b.hover_link = None;
    }
}
