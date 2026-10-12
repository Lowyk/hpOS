//! State of the «Браузер» and «Сеть» windows. The desktop service talks to
//! the network service and copies what it reports into [`NetView`].
use crate::lang::{Lang, pick};
use hpos_service_protocol::net::{self, error, event, stage};

pub const URL_MAX: usize = 1024;
const HISTORY: usize = 16;
pub const SHOWN_EVENTS: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Url {
    bytes: [u8; URL_MAX],
    len: u16,
}
impl Url {
    pub const EMPTY: Self = Self {
        bytes: [0; URL_MAX],
        len: 0,
    };
    pub fn new(text: &str) -> Self {
        let mut url = Self::EMPTY;
        for ch in text.chars() {
            if !url.push(ch) {
                break;
            }
        }
        url
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn push(&mut self, ch: char) -> bool {
        let mut buffer = [0; 4];
        let encoded = ch.encode_utf8(&mut buffer).as_bytes();
        let len = self.len as usize;
        if (ch as u32) < 0x20 || len + encoded.len() > URL_MAX {
            return false;
        }
        self.bytes[len..len + encoded.len()].copy_from_slice(encoded);
        self.len += encoded.len() as u16;
        true
    }
    pub fn pop(&mut self) {
        if let Some(ch) = self.as_str().chars().last() {
            self.len -= ch.len_utf8() as u16;
        }
    }
}

pub struct Browser {
    /// Text in the address field.
    pub address: Url,
    pub editing: bool,
    /// The page on screen (after redirects).
    pub current: Url,
    history: [Url; HISTORY],
    history_len: usize,
    pub scroll: i32,
    /// Height of the laid-out page; set by the desktop service.
    pub page_height: i32,
    pub has_page: bool,
    pub loading: bool,
    pub progress: u32,
    /// HTTP status of the page on screen.
    pub status: u32,
    /// Error code from `net::error` for the last failed load, or 0.
    pub error: u32,
    /// Link under the pointer, zero-based; set by the desktop service.
    pub hover_link: Option<usize>,
}
impl Browser {
    pub const fn new() -> Self {
        Self {
            address: Url::EMPTY,
            editing: false,
            current: Url::EMPTY,
            history: [Url::EMPTY; HISTORY],
            history_len: 0,
            scroll: 0,
            page_height: 0,
            has_page: false,
            loading: false,
            progress: 0,
            status: 0,
            error: 0,
            hover_link: None,
        }
    }
    /// Remembers the page on screen before leaving it.
    pub fn remember(&mut self) {
        if self.current.is_empty() {
            return;
        }
        if self.history_len == HISTORY {
            self.history.rotate_left(1);
            self.history_len -= 1;
        }
        self.history[self.history_len] = self.current;
        self.history_len += 1;
    }
    pub fn can_go_back(&self) -> bool {
        self.history_len > 0
    }
    pub fn back(&mut self) -> Option<Url> {
        self.history_len = self.history_len.checked_sub(1)?;
        Some(self.history[self.history_len])
    }
    pub fn scroll_by(&mut self, pixels: i32, visible: i32) {
        let max = (self.page_height - visible).max(0);
        self.scroll = (self.scroll + pixels).clamp(0, max);
    }
}
impl Default for Browser {
    fn default() -> Self {
        Self::new()
    }
}

/// Turns what was typed into a URL: addresses stay, anything else becomes
/// a search on FrogFind, which serves plain HTTP.
pub fn address_to_url(text: &str, out: &mut [u8]) -> Option<usize> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let looks_like_address = !text.contains(' ')
        && (text.contains("://")
            || (text.contains('.') && !text.ends_with('.') && hpos_net::http::parse(text).is_some()));
    if looks_like_address {
        let prefix: &[u8] = if text.contains("://") { b"" } else { b"http://" };
        let n = prefix.len() + text.len();
        if n > out.len() {
            return None;
        }
        out[..prefix.len()].copy_from_slice(prefix);
        out[prefix.len()..n].copy_from_slice(text.as_bytes());
        return Some(n);
    }
    const SEARCH: &[u8] = b"http://frogfind.com/?q=";
    out.get_mut(..SEARCH.len())?.copy_from_slice(SEARCH);
    let n = hpos_net::http::encode_query(text, &mut out[SEARCH.len()..])?;
    Some(SEARCH.len() + n)
}

/// What the desktop shows of the network service.
#[derive(Clone, Copy)]
pub struct NetView {
    pub stage: u32,
    pub address: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub mac: [u8; 6],
    pub vendor: u16,
    pub product: u16,
    pub received: u32,
    pub sent: u32,
    /// Newest last.
    pub events: [net::Event; SHOWN_EVENTS],
    pub event_count: usize,
}
impl NetView {
    pub const fn new() -> Self {
        Self {
            stage: stage::OFF,
            address: [0; 4],
            gateway: [0; 4],
            dns: [0; 4],
            mac: [0; 6],
            vendor: 0,
            product: 0,
            received: 0,
            sent: 0,
            events: [net::Event {
                code: 0,
                a: 0,
                b: 0,
                c: 0,
            }; SHOWN_EVENTS],
            event_count: 0,
        }
    }
    pub fn events(&self) -> &[net::Event] {
        &self.events[..self.event_count]
    }
}
impl Default for NetView {
    fn default() -> Self {
        Self::new()
    }
}

/// A small text buffer for status lines.
pub struct Line {
    bytes: [u8; 192],
    len: usize,
}
impl Line {
    pub const fn new() -> Self {
        Self {
            bytes: [0; 192],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    pub fn text(&mut self, s: &str) -> &mut Self {
        let mut end = s.len().min(self.bytes.len() - self.len);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.bytes[self.len..self.len + end].copy_from_slice(&s.as_bytes()[..end]);
        self.len += end;
        self
    }
    pub fn number(&mut self, mut value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        for i in (0..n).rev() {
            self.text(core::str::from_utf8(&digits[i..i + 1]).unwrap_or(""));
        }
        self
    }
    pub fn hex(&mut self, value: u64, digits: u32) -> &mut Self {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for i in (0..digits).rev() {
            let d = HEX[(value >> (i * 4) & 15) as usize];
            self.text(core::str::from_utf8(&[d]).unwrap_or(""));
        }
        self
    }
    pub fn ip(&mut self, ip: [u8; 4]) -> &mut Self {
        for (i, b) in ip.iter().enumerate() {
            if i > 0 {
                self.text(".");
            }
            self.number(*b as u64);
        }
        self
    }
    pub fn mac(&mut self, mac: [u8; 6]) -> &mut Self {
        for (i, b) in mac.iter().enumerate() {
            if i > 0 {
                self.text(":");
            }
            self.hex(*b as u64, 2);
        }
        self
    }
}
impl Default for Line {
    fn default() -> Self {
        Self::new()
    }
}

pub fn stage_text(lang: Lang, value: u32) -> &'static str {
    pick(
        lang,
        match value {
            stage::NO_CONTROLLER => [
                "Контроллер USB 2.0 не найден",
                "No USB 2.0 controller",
                "Контролер USB 2.0 не знайдено",
            ],
            stage::STARTING => ["Запуск USB...", "Starting USB...", "Запуск USB..."],
            stage::NO_DEVICE => ["Ждём телефон", "Waiting for a phone", "Чекаємо телефон"],
            stage::DEVICE => [
                "Знакомимся с устройством...",
                "Talking to the device...",
                "Знайомимося з пристроєм...",
            ],
            stage::LINK => ["Телефон на связи", "Phone linked", "Телефон на зв'язку"],
            stage::ADDRESS => [
                "Получаем адрес...",
                "Getting an address...",
                "Отримуємо адресу...",
            ],
            stage::ONLINE => ["В сети", "Online", "У мережі"],
            stage::FAILED => ["Ошибка USB", "USB error", "Помилка USB"],
            _ => ["Сеть выключена", "Network is off", "Мережу вимкнено"],
        },
    )
}
pub fn stage_hint(lang: Lang, value: u32) -> &'static str {
    pick(
        lang,
        match value {
            stage::NO_CONTROLLER => [
                "Без USB 2.0 модем не подключить",
                "A USB modem needs USB 2.0",
                "Без USB 2.0 модем не під'єднати",
            ],
            stage::NO_DEVICE | stage::DEVICE => [
                "Подключите телефон кабелем и включите «USB-модем»",
                "Plug the phone in and turn on USB tethering",
                "Під'єднайте телефон і ввімкніть «USB-модем»",
            ],
            stage::ONLINE => [
                "Откройте «Браузер» (F7)",
                "Open the Browser (F7)",
                "Відкрийте «Браузер» (F7)",
            ],
            stage::FAILED => [
                "Нажмите «Подключить» ещё раз",
                "Press Connect again",
                "Натисніть «Під'єднати» ще раз",
            ],
            stage::OFF => [
                "Нажмите «Подключить», затем включите USB-модем",
                "Press Connect, then turn on USB tethering",
                "Натисніть «Під'єднати», потім увімкніть USB-модем",
            ],
            _ => ["", "", ""],
        },
    )
}
pub fn error_text(lang: Lang, code: u32) -> &'static str {
    pick(
        lang,
        match code {
            error::OFFLINE => ["нет сети", "offline", "немає мережі"],
            error::BAD_URL => ["неверный адрес", "bad address", "невірна адреса"],
            error::DNS => ["имя не найдено", "name not found", "ім'я не знайдено"],
            error::CONNECT => ["сервер отказал", "connection refused", "сервер відмовив"],
            error::RESET => ["соединение сброшено", "connection reset", "з'єднання скинуто"],
            error::TIMEOUT => ["сервер не ответил", "timed out", "сервер не відповів"],
            error::HTTPS => ["только HTTPS", "HTTPS only", "лише HTTPS"],
            error::REDIRECTS => [
                "слишком много перенаправлений",
                "too many redirects",
                "забагато перенаправлень",
            ],
            error::PROTOCOL => [
                "непонятный ответ сервера",
                "bad server response",
                "незрозуміла відповідь сервера",
            ],
            error::STOPPED => ["остановлено", "stopped", "зупинено"],
            _ => ["ошибка", "error", "помилка"],
        },
    )
}
fn unpack(value: u32) -> [u8; 4] {
    value.to_be_bytes()
}
/// One log line for the «Сеть» window and the terminal.
pub fn describe(lang: Lang, e: &net::Event, out: &mut Line) {
    let t = |text: [&'static str; 3]| pick(lang, text);
    match e.code {
        event::CONTROLLER => {
            out.text(if e.c == 1 { "USB 1.1 " } else { "USB 2.0 " })
                .hex((e.a & 0xffff) as u64, 4)
                .text(":")
                .hex((e.a >> 16) as u64, 4)
                .text(t([", портов: ", ", ports: ", ", портів: "]))
                .number(e.b as u64);
        }
        event::HANDOFF => {
            out.text(if e.a != 0 {
                t(["BIOS отдал USB", "BIOS released USB", "BIOS віддав USB"])
            } else {
                t([
                    "BIOS молчал, USB забран силой",
                    "BIOS silent, USB taken over",
                    "BIOS мовчав, USB забрано силоміць",
                ])
            });
        }
        event::CONTROLLER_FAILED => {
            out.text(if e.b == 1 { "USB 1.1" } else { "USB 2.0" })
                .text(t([
                    " не запустился, шаг ",
                    " failed at step ",
                    " не запустився, крок ",
                ]))
                .number(e.a as u64);
        }
        event::PORT => {
            if e.b == 1 {
                out.text("USB 1.1 #").number(e.c as u64 + 1).text(t([", порт ", ", port ", ", порт "]));
            } else {
                out.text(t(["Порт ", "Port ", "Порт "]));
            }
            out
                .number(e.a as u64 + 1)
                .text(t([": устройство", ": device", ": пристрій"]));
            if e.b != 1 {
                out.text(" USB 2.0");
            }
        }
        event::SLOW_DEVICE => {
            out.text(t(["Порт ", "Port ", "Порт "]))
                .number(e.a as u64 + 1)
                .text(t([": не USB 2.0 (", ": not USB 2.0 (", ": не USB 2.0 ("]))
                .hex(e.c as u64, 4)
                .text("/")
                .hex(e.b as u64, 4)
                .text(t(["), пробуем USB 1.1", "), trying USB 1.1", "), пробуємо USB 1.1"]));
        }
        event::DEVICE => {
            out.text(t(["Устройство ", "Device ", "Пристрій "]))
                .hex(e.a as u64, 4)
                .text(":")
                .hex(e.b as u64, 4);
            if e.c != 0 {
                out.text(t([", конфигураций: ", ", configurations: ", ", конфігурацій: "]))
                    .number(e.c as u64);
            }
        }
        event::USB_FAILED => {
            out.text(if e.a >> 8 == 1 { "USB 1.1" } else { "USB 2.0" })
                .text(t([": ошибка, шаг ", ": error at step ", ": помилка, крок "]))
                .number((e.a & 0xf) as u64);
            if e.a >> 4 & 0xf != 0 {
                out.text(t([" (конфигурация ", " (configuration ", " (конфігурація "]))
                    .number((e.a >> 4 & 0xf) as u64)
                    .text(")");
            }
            out
                .text(t([", код ", ", code ", ", код "]))
                .hex(e.b as u64, 3)
                .text(" [")
                .hex(e.c as u64, 8)
                .text("]");
        }
        event::NOT_RNDIS => {
            out.text(t(["Не модем: ", "Not a modem: ", "Не модем: "]));
            for (i, class) in [e.a, e.b, e.c].iter().enumerate() {
                if i > 0 && *class == 0 {
                    break;
                }
                if i > 0 {
                    out.text(" ");
                }
                out.hex(*class as u64, 6).text(match class >> 16 {
                    0x08 => t(["(накопитель)", "(storage)", "(накопичувач)"]),
                    0x06 => t(["(фото)", "(photos)", "(фото)"]),
                    0xff => t(["(свой)", "(vendor)", "(свій)"]),
                    0x02 | 0x0a => t(["(сеть?)", "(network?)", "(мережа?)"]),
                    _ => "",
                });
            }
        }
        event::RNDIS => {
            out.text(t([
                "USB-модем RNDIS, конфигурация ",
                "RNDIS modem, configuration ",
                "USB-модем RNDIS, конфігурація ",
            ]))
            .number(e.a as u64);
        }
        event::RNDIS_FAILED => {
            out.text(t(["RNDIS не ответил, шаг ", "RNDIS failed at step ", "RNDIS не відповів, крок "]))
                .number(e.a as u64)
                .text(t([", код ", ", code ", ", код "]))
                .hex(e.b as u64, 8);
        }
        event::LINK_UP => {
            let [a, b, c, d] = e.a.to_be_bytes();
            let [_, _, f, g] = e.b.to_be_bytes();
            out.text(t(["Связь есть, MAC ", "Link up, MAC ", "Зв'язок є, MAC "]))
                .mac([a, b, c, d, f, g]);
        }
        event::DHCP_DISCOVER => {
            out.text(t(["Просим адрес (DHCP)...", "Asking for an address (DHCP)...", "Просимо адресу (DHCP)..."]));
        }
        event::LEASE => {
            out.text(t(["Адрес ", "Address ", "Адреса "]))
                .ip(unpack(e.a))
                .text(t([", шлюз ", ", gateway ", ", шлюз "]))
                .ip(unpack(e.b))
                .text(", DNS ")
                .ip(unpack(e.c));
        }
        event::DHCP_TIMEOUT => {
            out.text(t(["DHCP молчит, повтор через 5 с", "No DHCP answer, retrying in 5 s", "DHCP мовчить, повтор за 5 с"]));
        }
        event::RESOLVED => {
            out.text("DNS: ").ip(unpack(e.a));
        }
        event::CONNECTING => {
            out.text(t(["Соединение с ", "Connecting to ", "З'єднання з "]))
                .ip(unpack(e.a))
                .text(":")
                .number(e.b as u64);
        }
        event::RESPONSE => {
            out.text(t(["Ответ ", "Response ", "Відповідь "]))
                .number(e.a as u64)
                .text(", ")
                .number(e.b as u64)
                .text(t([" байт", " bytes", " байт"]));
        }
        event::FETCH_FAILED => {
            out.text(t(["Ошибка: ", "Failed: ", "Помилка: "])).text(error_text(lang, e.a));
        }
        event::PONG => {
            out.text(t(["Ответ от ", "Reply from ", "Відповідь від "]))
                .ip(unpack(e.a))
                .text(": ")
                .number(e.b as u64)
                .text(t([" мс", " ms", " мс"]));
        }
        event::PING_LOST => {
            out.text(t(["Нет ответа от ", "No reply from ", "Немає відповіді від "]))
                .ip(unpack(e.a));
        }
        event::DISCONNECTED => {
            out.text(t(["Телефон отключён", "Phone unplugged", "Телефон від'єднано"]));
        }
        event::USB_ERROR => {
            out.text(t(["Ошибок передачи USB: ", "USB transfer errors: ", "Помилок передачі USB: "]))
                .number(e.a as u64);
        }
        _ => {
            out.text("?");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    extern crate std;
    #[test]
    fn typed_addresses_and_searches() {
        let mut out = [0u8; 256];
        let mut url = |text: &str| {
            let n = address_to_url(text, &mut out).unwrap();
            std::string::String::from_utf8(out[..n].to_vec()).unwrap()
        };
        assert_eq!(url("example.com"), "http://example.com");
        assert_eq!(url("http://a.b/c"), "http://a.b/c");
        assert_eq!(url("https://ya.ru"), "https://ya.ru");
        assert_eq!(url("котики смешные"), "http://frogfind.com/?q=%D0%BA%D0%BE%D1%82%D0%B8%D0%BA%D0%B8+%D1%81%D0%BC%D0%B5%D1%88%D0%BD%D1%8B%D0%B5");
        assert_eq!(url("rust"), "http://frogfind.com/?q=rust");
        assert_eq!(address_to_url("   ", &mut out), None);
    }
    #[test]
    fn history_and_lines() {
        let mut b = Browser::new();
        assert!(b.back().is_none());
        b.current = Url::new("http://a/");
        b.remember();
        b.current = Url::new("http://b/");
        assert_eq!(b.back().unwrap().as_str(), "http://a/");
        assert!(!b.can_go_back());
        let mut line = Line::new();
        describe(
            Lang::En,
            &net::Event {
                code: event::LEASE,
                a: u32::from_be_bytes([192, 168, 42, 10]),
                b: u32::from_be_bytes([192, 168, 42, 129]),
                c: u32::from_be_bytes([8, 8, 8, 8]),
            },
            &mut line,
        );
        assert_eq!(line.as_str(), "Address 192.168.42.10, gateway 192.168.42.129, DNS 8.8.8.8");
        let mut line = Line::new();
        describe(Lang::Ru, &net::Event { code: event::DEVICE, a: 0x12d1, b: 0x108a, c: 0 }, &mut line);
        assert_eq!(line.as_str(), "Устройство 12d1:108a");
    }
}
