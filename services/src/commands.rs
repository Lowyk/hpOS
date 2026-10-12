//! The terminal's command interpreter and Izzop, running inside the desktop
//! service next to the file-system client.
use crate::{
    storage::{self, Client},
    web::Net,
};
use core::fmt::Write;
use hpos_desktop::{
    Shell,
    files::{Entry, MAX_ENTRIES, Name},
    lang::{Lang, pick},
    model::{BROWSER, EDITOR, NETWORK, Request},
    terminal::{Mode, Tone},
    web::{self, Url},
};
use hpos_fs::Error;
use hpos_izzop::{Model, Rng};
use hpos_service_protocol::{DESKTOP_HEAP, fs, heap, net, status};

/// The model lives in the upper half of the desktop heap; the shell uses the lower.
const MODEL_AT: usize = DESKTOP_HEAP as usize + heap::MODEL;
const MODEL_MAX: usize = heap::MODEL_BYTES;
const MODEL_PATH: &str = "/Система/izzop.bin";

/// Fixed-size text buffer for formatted terminal output.
pub struct Line {
    bytes: [u8; 512],
    len: usize,
}
impl Line {
    pub fn new() -> Self {
        Self {
            bytes: [0; 512],
            len: 0,
        }
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
impl Write for Line {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for ch in s.chars() {
            let mut buffer = [0; 4];
            let encoded = ch.encode_utf8(&mut buffer).as_bytes();
            if self.len + encoded.len() > self.bytes.len() {
                return Err(core::fmt::Error);
            }
            self.bytes[self.len..self.len + encoded.len()].copy_from_slice(encoded);
            self.len += encoded.len();
        }
        Ok(())
    }
}

/// Izzop starts in visible steps, one per presented frame, so a hang shows where it is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Load,
    Greet,
}

pub struct Session {
    cwd: [u8; 512],
    cwd_len: usize,
    rng: Rng,
    model: Option<Model<'static>>,
    stage: Stage,
    /// Frame counter when the current stage was announced.
    stage_frame: u64,
    /// Ping replies still expected in the terminal.
    pinging: u8,
}

fn say(shell: &mut Shell, text: &str) {
    shell.terminal.print(text, Tone::Normal);
}
fn fail(shell: &mut Shell, text: &str) {
    shell.terminal.print(text, Tone::Error);
}
fn t(shell: &Shell, text: [&'static str; 3]) -> &'static str {
    pick(shell.lang, text)
}

impl Session {
    pub fn new(seed: u64) -> Self {
        let mut session = Self {
            cwd: [0; 512],
            cwd_len: 1,
            rng: Rng::new(seed),
            model: None,
            stage: Stage::Idle,
            stage_frame: 0,
            pinging: 0,
        };
        session.cwd[0] = b'/';
        session
    }
    pub fn cwd(&self) -> &str {
        core::str::from_utf8(&self.cwd[..self.cwd_len]).unwrap_or("/")
    }
    pub fn prompt(&self, shell: &mut Shell) {
        if shell.terminal.mode == Mode::Izzop {
            shell
                .terminal
                .set_prompt(t(shell, ["ты> ", "you> ", "ти> "]));
            return;
        }
        let mut line = Line::new();
        let _ = write!(line, "hpOS:{}$ ", self.cwd());
        shell.terminal.set_prompt(line.as_str());
    }
    pub fn greet(&mut self, shell: &mut Shell) {
        if shell.terminal.started {
            return;
        }
        shell.terminal.started = true;
        shell.terminal.print(hpos_desktop::view::NAME, Tone::Prompt);
        say(
            shell,
            t(
                shell,
                [
                    "Наберите help, чтобы увидеть команды. izzop - поговорить с Иззопом.",
                    "Type help for commands. izzop starts a chat with Izzop.",
                    "Наберіть help для списку команд. izzop - розмова з Іззопом.",
                ],
            ),
        );
        self.prompt(shell);
    }

    /// Absolute, normalized path for `arg` relative to the working directory.
    fn path<'a>(&self, arg: &str, out: &'a mut [u8; 512]) -> Option<&'a str> {
        let mut parts: [&str; 32] = [""; 32];
        let mut depth = 0usize;
        let base = if arg.starts_with('/') { "" } else { self.cwd() };
        for part in base.split('/').chain(arg.split('/')) {
            match part {
                "" | "." => {}
                ".." => depth = depth.saturating_sub(1),
                name => {
                    if depth == parts.len() {
                        return None;
                    }
                    parts[depth] = name;
                    depth += 1;
                }
            }
        }
        let mut len = 0;
        for part in &parts[..depth] {
            if len + 1 + part.len() > out.len() {
                return None;
            }
            out[len] = b'/';
            out[len + 1..len + 1 + part.len()].copy_from_slice(part.as_bytes());
            len += 1 + part.len();
        }
        if len == 0 {
            out[0] = b'/';
            len = 1;
        }
        core::str::from_utf8(&out[..len]).ok()
    }
    /// Splits a path into (parent directory path, final name).
    fn split<'a>(path: &'a str) -> (&'a str, &'a str) {
        match path.rfind('/') {
            Some(0) => ("/", &path[1..]),
            Some(i) => (&path[..i], &path[i + 1..]),
            None => ("/", path),
        }
    }

    pub fn run(
        &mut self,
        client: &mut Client,
        net: &mut Net,
        shell: &mut Shell,
        line: &str,
        values: &[u64; status::COUNT],
        frame: u64,
    ) {
        if shell.terminal.mode == Mode::Izzop {
            self.izzop_line(client, shell, line);
            self.prompt(shell);
            return;
        }
        let mut words = line.split_whitespace();
        let Some(command) = words.next() else {
            self.prompt(shell);
            return;
        };
        let arg = words.next().unwrap_or("");
        let rest = line
            .trim_start()
            .get(command.len()..)
            .unwrap_or("")
            .trim_start()
            .get(arg.len()..)
            .unwrap_or("")
            .trim_start();
        match command {
            "help" | "помощь" | "допомога" => self.help(shell),
            "clear" | "cls" => shell.terminal.clear(),
            "pwd" => {
                let mut line = Line::new();
                let _ = write!(line, "{}", self.cwd());
                say(shell, line.as_str());
            }
            "ls" | "dir" => self.ls(client, shell, arg),
            "cd" => self.cd(client, shell, arg),
            "cat" => self.cat(client, shell, arg),
            "mkdir" => self.create(client, shell, arg, true),
            "touch" => self.create(client, shell, arg, false),
            "rm" | "del" => self.remove(client, shell, arg),
            "mv" | "ren" => self.rename(client, shell, arg, rest),
            "write" => self.write(client, shell, arg, rest),
            "edit" => self.edit(client, shell, arg),
            "date" | "time" => self.date(shell, values),
            "uptime" => {
                let mut line = Line::new();
                let _ = write!(line, "{} s", values[0] / 100);
                say(shell, line.as_str());
            }
            "mem" | "free" => {
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "{} {}",
                    values[7],
                    t(shell, ["МиБ свободно", "MiB free", "МіБ вільно"])
                );
                say(shell, line.as_str());
            }
            "ver" | "about" => {
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "{} {}, Lowyk & tixcon",
                    hpos_desktop::view::NAME,
                    hpos_desktop::view::VERSION
                );
                say(shell, line.as_str());
            }
            "theme" => match arg {
                "light" | "dark" => {
                    if shell.light != (arg == "light") {
                        shell.light = arg == "light";
                        shell.settings_dirty = true;
                        shell.full();
                    }
                }
                _ => fail(shell, "theme light | dark"),
            },
            "lang" => match Lang::from_code(arg) {
                Some(lang) => shell.set_lang(lang),
                None => fail(shell, "lang ru | en | uk"),
            },
            "res" => self.resolution(shell, arg),
            "izzop" | "иззоп" => self.izzop_start(client, shell, frame),
            "net" | "сеть" | "мережа" => self.net_status(shell),
            "connect" => {
                net.send(net::CONNECT, "");
                say(
                    shell,
                    t(
                        shell,
                        [
                            "Запускаю USB. Включите на телефоне «USB-модем»; ход дел в окне «Сеть» (F8)",
                            "Starting USB. Turn on USB tethering; progress is in Network (F8)",
                            "Запускаю USB. Увімкніть «USB-модем»; хід справ у вікні «Мережа» (F8)",
                        ],
                    ),
                );
            }
            "ping" => {
                if arg.is_empty() {
                    fail(shell, "ping host");
                } else {
                    net.send(net::PING, arg);
                    self.pinging = 3;
                }
            }
            "open" | "browse" => {
                if arg.is_empty() {
                    fail(shell, "open url");
                } else {
                    let target = if rest.is_empty() { arg } else { line.trim_start()[command.len()..].trim() };
                    shell.browser.address = Url::new(target);
                    shell.open(BROWSER);
                    shell.push_request(Request::Navigate);
                }
            }
            "exit" => {
                shell.key(hpos_desktop::Key::Escape);
            }
            _ => {
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "{}: {}",
                    command,
                    t(
                        shell,
                        [
                            "нет такой команды (help)",
                            "unknown command (help)",
                            "немає такої команди (help)"
                        ]
                    )
                );
                fail(shell, line.as_str());
            }
        }
        self.prompt(shell);
    }

    fn help(&self, shell: &mut Shell) {
        let lines: &[[&str; 3]] = &[
            [
                "ls [путь]  cd путь  pwd",
                "ls [path]  cd path  pwd",
                "ls [шлях]  cd шлях  pwd",
            ],
            [
                "cat файл  edit файл  write файл текст",
                "cat file  edit file  write file text",
                "cat файл  edit файл  write файл текст",
            ],
            [
                "mkdir имя  touch имя  rm имя  mv старое новое",
                "mkdir name  touch name  rm name  mv old new",
                "mkdir ім'я  touch ім'я  rm ім'я  mv старе нове",
            ],
            [
                "date  uptime  mem  ver  clear  exit",
                "date  uptime  mem  ver  clear  exit",
                "date  uptime  mem  ver  clear  exit",
            ],
            [
                "theme light|dark  lang ru|en|uk  res [ШxВ]",
                "theme light|dark  lang ru|en|uk  res [WxH]",
                "theme light|dark  lang ru|en|uk  res [ШxВ]",
            ],
            [
                "izzop - поболтать с Иззопом (выход: exit)",
                "izzop - chat with Izzop (leave with exit)",
                "izzop - балачки з Іззопом (вихід: exit)",
            ],
            [
                "connect  net  ping адрес  open сайт|запрос",
                "connect  net  ping host  open site|query",
                "connect  net  ping адреса  open сайт|запит",
            ],
            [
                "Ctrl+L очищает, PgUp/PgDn листают",
                "Ctrl+L clears, PgUp/PgDn scroll",
                "Ctrl+L очищає, PgUp/PgDn гортають",
            ],
        ];
        for line in lines {
            say(shell, pick(shell.lang, *line));
        }
    }
    fn net_status(&self, shell: &mut Shell) {
        let view = shell.net;
        let mut line = web::Line::new();
        line.text(web::stage_text(shell.lang, view.stage));
        if view.address != [0; 4] {
            line.text(": ").ip(view.address);
        }
        say(shell, line.as_str());
        for e in view.events() {
            let mut line = web::Line::new();
            line.text("  ");
            web::describe(shell.lang, e, &mut line);
            say(shell, line.as_str());
        }
        if !shell.windows[NETWORK].visible && view.stage == net::stage::OFF {
            say(shell, t(shell, ["connect - включить", "connect - turn on", "connect - увімкнути"]));
        }
    }
    /// Prints ping results that arrived from the network service.
    pub fn network_events(&mut self, shell: &mut Shell, events: &[net::Event]) {
        for e in events {
            if self.pinging == 0 {
                return;
            }
            match e.code {
                net::event::PONG | net::event::PING_LOST => self.pinging -= 1,
                net::event::FETCH_FAILED => self.pinging = 0,
                _ => continue,
            }
            let mut line = web::Line::new();
            web::describe(shell.lang, e, &mut line);
            let tone = if e.code == net::event::PONG { Tone::Normal } else { Tone::Error };
            shell.terminal.print(line.as_str(), tone);
            shell.terminal_changed();
        }
    }
    fn report(&self, shell: &mut Shell, code: u32) {
        fail(shell, storage::message(code, shell.lang));
    }
    fn ls(&self, client: &mut Client, shell: &mut Shell, arg: &str) {
        let mut buffer = [0u8; 512];
        let Some(path) = self.path(arg, &mut buffer) else {
            return fail(
                shell,
                t(
                    shell,
                    ["Слишком длинный путь", "Path too long", "Задовгий шлях"],
                ),
            );
        };
        let dir = match client.resolve(path) {
            Ok(dir) => dir,
            Err(code) => return self.report(shell, code),
        };
        let mut entries = [Entry {
            id: 0,
            directory: false,
            size: 0,
            name: Name::EMPTY,
        }; MAX_ENTRIES];
        match client.list(dir, &mut entries) {
            Ok((count, more)) => {
                if count == 0 {
                    say(shell, t(shell, ["(пусто)", "(empty)", "(порожньо)"]));
                }
                for entry in &entries[..count] {
                    let mut line = Line::new();
                    if entry.directory {
                        let _ = write!(line, "  {}/", entry.name.as_str());
                    } else {
                        let _ = write!(line, "  {}  ({} B)", entry.name.as_str(), entry.size);
                    }
                    say(shell, line.as_str());
                }
                if more {
                    say(shell, "  ...");
                }
            }
            Err(code) => self.report(shell, code),
        }
    }
    fn cd(&mut self, client: &mut Client, shell: &mut Shell, arg: &str) {
        let mut buffer = [0u8; 512];
        let Some(path) = self.path(if arg.is_empty() { "/" } else { arg }, &mut buffer) else {
            return fail(
                shell,
                t(
                    shell,
                    ["Слишком длинный путь", "Path too long", "Задовгий шлях"],
                ),
            );
        };
        let checked = client
            .resolve(path)
            .and_then(|dir| client.call(fs::LIST, [dir as u64, fs::LIST_START, 0, 0], "", &[]));
        match checked {
            Ok(_) => {
                self.cwd[..path.len()].copy_from_slice(path.as_bytes());
                self.cwd_len = path.len();
            }
            Err(code) => self.report(shell, code),
        }
    }
    fn file(&self, client: &mut Client, shell: &mut Shell, arg: &str) -> Option<(u16, Name)> {
        if arg.is_empty() {
            fail(
                shell,
                t(shell, ["Укажите файл", "Name a file", "Вкажіть файл"]),
            );
            return None;
        }
        let mut buffer = [0u8; 512];
        let path = self.path(arg, &mut buffer)?;
        match client.resolve(path) {
            Ok(id) => Some((id, Name::new(Self::split(path).1))),
            Err(code) => {
                self.report(shell, code);
                None
            }
        }
    }
    fn cat(&self, client: &mut Client, shell: &mut Shell, arg: &str) {
        let Some((id, _)) = self.file(client, shell, arg) else {
            return;
        };
        let mut offset = 0;
        let mut buffer = [0u8; 4096];
        loop {
            match client.read_at(id, offset, &mut buffer) {
                Ok((0, _)) => break,
                Ok((n, size)) => {
                    // Print complete characters only; a split one continues next round.
                    let valid = match core::str::from_utf8(&buffer[..n]) {
                        Ok(text) => text.len(),
                        Err(e) if e.error_len().is_none() => e.valid_up_to(),
                        Err(_) => {
                            return fail(
                                shell,
                                t(
                                    shell,
                                    [
                                        "Это не текстовый файл",
                                        "Not a text file",
                                        "Це не текстовий файл",
                                    ],
                                ),
                            );
                        }
                    };
                    let text = core::str::from_utf8(&buffer[..valid]).unwrap_or("");
                    shell
                        .terminal
                        .print(text.strip_suffix('\n').unwrap_or(text), Tone::Normal);
                    offset += valid.max(1);
                    if offset >= size || offset > 64 * 1024 {
                        break;
                    }
                }
                Err(code) => return self.report(shell, code),
            }
        }
    }
    fn create(&self, client: &mut Client, shell: &mut Shell, arg: &str, directory: bool) {
        let mut buffer = [0u8; 512];
        let Some(path) = self.path(arg, &mut buffer).filter(|_| !arg.is_empty()) else {
            return fail(
                shell,
                t(shell, ["Укажите имя", "Give a name", "Вкажіть ім'я"]),
            );
        };
        let (parent, name) = Self::split(path);
        let result = client
            .resolve(parent)
            .and_then(|dir| client.create(dir, name, directory));
        match result {
            Ok(_) => {}
            Err(code) if !directory && code == Error::Exists.code() => {}
            Err(code) => self.report(shell, code),
        }
    }
    fn remove(&self, client: &mut Client, shell: &mut Shell, arg: &str) {
        if let Some((id, _)) = self.file(client, shell, arg) {
            if let Err(code) = client.call(fs::REMOVE, [id as u64, 0, 0, 0], "", &[]) {
                self.report(shell, code);
            }
        }
    }
    fn rename(&self, client: &mut Client, shell: &mut Shell, arg: &str, to: &str) {
        let Some((id, _)) = self.file(client, shell, arg) else {
            return;
        };
        let mut buffer = [0u8; 512];
        let Some(path) = self
            .path(to.trim(), &mut buffer)
            .filter(|_| !to.trim().is_empty())
        else {
            return fail(shell, "mv old new");
        };
        let (parent, name) = Self::split(path);
        let result = client
            .resolve(parent)
            .and_then(|dir| client.call(fs::RENAME, [id as u64, dir as u64, 0, 0], name, &[]));
        if let Err(code) = result {
            self.report(shell, code);
        }
    }
    fn write(&self, client: &mut Client, shell: &mut Shell, arg: &str, text: &str) {
        self.create(client, shell, arg, false);
        let Some((id, _)) = self.file(client, shell, arg) else {
            return;
        };
        let mut data = [0u8; 512];
        let n = text.len().min(data.len() - 1);
        let n = (0..=n)
            .rev()
            .find(|i| text.is_char_boundary(*i))
            .unwrap_or(0);
        data[..n].copy_from_slice(&text.as_bytes()[..n]);
        data[n] = b'\n';
        if let Err(code) = client.write(id, &data[..n + 1]) {
            self.report(shell, code);
        }
    }
    fn edit(&self, client: &mut Client, shell: &mut Shell, arg: &str) {
        self.create(client, shell, arg, false);
        if let Some((id, name)) = self.file(client, shell, arg) {
            crate::open_in_editor(client, shell, id, name);
            shell.open(EDITOR);
        }
    }
    fn date(&self, shell: &mut Shell, values: &[u64; status::COUNT]) {
        let (time, date) = (values[status::TIME], values[status::DATE]);
        if time >> 31 & 1 == 0 || date >> 31 & 1 == 0 {
            return fail(
                shell,
                t(
                    shell,
                    [
                        "Часы недоступны",
                        "Clock unavailable",
                        "Годинник недоступний",
                    ],
                ),
            );
        }
        let mut line = Line::new();
        let _ = write!(
            line,
            "{:02}.{:02}.{} {:02}:{:02}:{:02}",
            date & 255,
            date >> 8 & 255,
            date >> 16 & 0x7fff,
            time >> 16 & 255,
            time >> 8 & 255,
            time & 255
        );
        say(shell, line.as_str());
    }
    fn resolution(&self, shell: &mut Shell, arg: &str) {
        if arg.is_empty() {
            for n in 0..shell.mode_count {
                let mode = shell.modes[n];
                let mut line = Line::new();
                let _ = write!(
                    line,
                    "  {}x{}{}",
                    mode.width,
                    mode.height,
                    if n == shell.mode_current { "  *" } else { "" }
                );
                say(shell, line.as_str());
            }
            return;
        }
        let wanted = arg
            .split_once(['x', 'х'])
            .and_then(|(w, h)| Some((w.parse::<u16>().ok()?, h.parse::<u16>().ok()?)));
        match wanted.and_then(|(w, h)| {
            (0..shell.mode_count)
                .find(|n| shell.modes[*n].width == w && shell.modes[*n].height == h)
        }) {
            Some(n) if n == shell.mode_current => {}
            Some(n) => {
                shell.mode_choice = n;
                shell.push_request(Request::SetMode(n));
            }
            None => fail(
                shell,
                t(
                    shell,
                    [
                        "Нет такого режима (res - список)",
                        "No such mode (res lists them)",
                        "Немає такого режиму (res - список)",
                    ],
                ),
            ),
        }
    }

    fn load_model(&mut self, client: &mut Client) -> Result<(), &'static [&'static str; 3]> {
        if self.model.is_some() {
            return Ok(());
        }
        static MISSING: [&str; 3] = [
            "Иззоп спит: модели нет. Положите корпус в private/izzop/corpus.txt и пересоберите образ.",
            "Izzop is asleep: no model. Put a corpus in private/izzop/corpus.txt and rebuild the image.",
            "Іззоп спить: моделі немає. Покладіть корпус у private/izzop/corpus.txt і перезберіть образ.",
        ];
        static BROKEN: [&str; 3] = [
            "Модель Иззопа повреждена",
            "The Izzop model is damaged",
            "Модель Іззопа пошкоджена",
        ];
        let id = client.resolve(MODEL_PATH).map_err(|_| &MISSING)?;
        let heap = unsafe { core::slice::from_raw_parts_mut(MODEL_AT as *mut u8, MODEL_MAX) };
        let mut offset = 0;
        loop {
            let chunk = (MODEL_MAX - offset).min(fs::DATA_BYTES);
            let (n, size) = client
                .read_at(id, offset, &mut heap[offset..offset + chunk])
                .map_err(|_| &BROKEN)?;
            if size > MODEL_MAX {
                return Err(&BROKEN);
            }
            offset += n;
            if n == 0 || offset >= size {
                break;
            }
        }
        let bytes: &'static [u8] =
            unsafe { core::slice::from_raw_parts(MODEL_AT as *const u8, offset) };
        self.model = Some(Model::parse(bytes).ok_or(&BROKEN)?);
        Ok(())
    }
    fn izzop_start(&mut self, _client: &mut Client, shell: &mut Shell, frame: u64) {
        shell.terminal.print(
            t(
                shell,
                [
                    "Иззоп просыпается...",
                    "Izzop is waking up...",
                    "Іззоп прокидається...",
                ],
            ),
            Tone::Normal,
        );
        self.stage = Stage::Load;
        self.stage_frame = frame;
    }
    /// Advances a pending Izzop start once the previous step reached the screen.
    pub fn step(&mut self, client: &mut Client, shell: &mut Shell, frame: u64) {
        if self.stage == Stage::Idle || frame <= self.stage_frame + 1 {
            return;
        }
        match self.stage {
            Stage::Load => {
                if let Err(text) = self.load_model(client) {
                    self.stage = Stage::Idle;
                    fail(shell, pick(shell.lang, *text));
                    self.prompt(shell);
                    return;
                }
                let mut line = Line::new();
                let words = self.model.as_ref().map_or(0, |m| m.len());
                let _ = write!(
                    line,
                    "{} {}",
                    t(
                        shell,
                        [
                            "Модель загружена, слов:",
                            "Model loaded, words:",
                            "Модель завантажено, слів:"
                        ]
                    ),
                    words
                );
                say(shell, line.as_str());
                self.stage = Stage::Greet;
                self.stage_frame = frame;
            }
            Stage::Greet => {
                self.stage = Stage::Idle;
                shell.terminal.mode = Mode::Izzop;
                shell.terminal.print(
                    t(
                        shell,
                        [
                            "Иззоп на связи. Пишите что угодно, exit - выйти.",
                            "Izzop is here. Say anything, exit to leave.",
                            "Іззоп на зв'язку. Пишіть що завгодно, exit - вийти.",
                        ],
                    ),
                    Tone::Prompt,
                );
                self.izzop_reply(shell, "");
                self.prompt(shell);
            }
            Stage::Idle => {}
        }
        shell.terminal_changed();
    }
    fn izzop_line(&mut self, _client: &mut Client, shell: &mut Shell, line: &str) {
        if matches!(line.trim(), "exit" | "выход" | "пока" | "вихід" | "quit") {
            shell.terminal.mode = Mode::Shell;
            shell.terminal.print(
                t(
                    shell,
                    [
                        "Иззоп обиделся и ушёл.",
                        "Izzop sulked off.",
                        "Іззоп образився й пішов.",
                    ],
                ),
                Tone::Izzop,
            );
            return;
        }
        self.izzop_reply(shell, line);
    }
    fn izzop_reply(&mut self, shell: &mut Shell, prompt: &str) {
        let Some(model) = &self.model else {
            return;
        };
        let mut out = [0u8; 300];
        let n = model.reply(prompt, &mut self.rng, &mut out);
        let mut line = Line::new();
        let _ = write!(
            line,
            "{}: {}",
            t(shell, ["Иззоп", "Izzop", "Іззоп"]),
            core::str::from_utf8(&out[..n]).unwrap_or("...")
        );
        shell.terminal.print(line.as_str(), Tone::Izzop);
    }
}
