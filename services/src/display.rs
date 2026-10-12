#![no_std]
#![no_main]
mod commands;
mod runtime;
mod storage;
mod web;
use commands::Session;
use hpos_desktop::{
    FILES, Key, Shell,
    browser::{self, Page},
    canvas::Canvas,
    files::{Entry, FileOp, MAX_ENTRIES, Name, Storage},
    lang::{Lang, pick},
    model::{DisplayMode, EDITOR, MAX_MODES, MODE_CONFIRM_TICKS, Request, TERMINAL},
    web::address_to_url,
    pointer::Pointer,
    view,
};
use hpos_fs::Error;
use hpos_service_protocol::{
    DESKTOP_HEAP, STARTUP_ADDRESS, Startup, fs, heap, net, status,
};
use storage::Client;

const SETTINGS_DIR: &str = "Система";
const SETTINGS_FILE: &str = "settings.ini";
/// Settings are written this many ticks after the last change.
const SAVE_DELAY: u64 = 60;
// The shell lives in the lower half of the desktop heap, Izzop's model in the upper.
const _: () = assert!(core::mem::size_of::<Shell>() <= heap::SHELL_BYTES);
const _: () = assert!(core::mem::size_of::<Page>() <= heap::PAGE_BYTES);

fn status() -> [u64; status::COUNT] {
    let mut values = [0; status::COUNT];
    for (n, v) in values.iter_mut().enumerate() {
        let r = runtime::call(5, n as u64, 0);
        if r.status != 0 {
            runtime::fail();
        }
        *v = r.word;
    }
    values
}

fn refresh_storage(client: &mut Client, shell: &mut Shell) {
    shell.files.storage = match client.call(fs::STAT, [0; 4], "", &[]) {
        Ok(r) => Storage::Ready {
            free_kib: r[1] * 4,
            total_kib: r[0] * 4,
        },
        Err(_) => Storage::Unavailable,
    };
}

fn join<'a>(buffer: &'a mut [u8; 64], dir: &str, file: &str) -> &'a str {
    let total = dir.len() + 1 + file.len();
    buffer[..dir.len()].copy_from_slice(dir.as_bytes());
    buffer[dir.len()] = b'/';
    buffer[dir.len() + 1..total].copy_from_slice(file.as_bytes());
    core::str::from_utf8(&buffer[..total]).unwrap_or("")
}

/// Kernel display modes: the shell lists only the switchable ones.
struct Modes {
    kernel: [usize; MAX_MODES],
    previous: usize,
}
impl Modes {
    fn query(shell: &mut Shell) -> Self {
        let mut modes = Self {
            kernel: [0; MAX_MODES],
            previous: 0,
        };
        let info = runtime::call(10, 0, 0);
        if info.status != 0 {
            return modes;
        }
        let (count, current) = ((info.word & 255) as usize, (info.word >> 8 & 255) as usize);
        let mut listed = 0;
        for index in 0..count.min(MAX_MODES) {
            let r = runtime::call(10, 1, index as u64);
            if r.status != 0 || r.word >> 32 & 1 == 0 {
                continue;
            }
            shell.modes[listed] = DisplayMode {
                width: r.word as u16,
                height: (r.word >> 16) as u16,
            };
            modes.kernel[listed] = index;
            if index == current {
                shell.mode_current = listed;
                shell.mode_choice = listed;
            }
            listed += 1;
        }
        if listed > 0 {
            shell.mode_count = listed;
        }
        modes
    }
}

/// Front and back canvases for the current mode.
struct Screen {
    frame: Option<Canvas>,
    back: Option<Canvas>,
    framebuffer: u64,
    backbuffer: u64,
}
impl Screen {
    fn build(&mut self, width: u32, height: u32, stride: u32) {
        self.frame = unsafe {
            Canvas::new(
                self.framebuffer as *mut u32,
                width as usize,
                height as usize,
                stride as usize,
            )
        };
        self.back = unsafe {
            Canvas::new(
                self.backbuffer as *mut u32,
                width as usize,
                height as usize,
                width as usize,
            )
        };
        if self.frame.is_some() != self.back.is_some() {
            runtime::fail();
        }
    }
}

fn switch_mode(shell: &mut Shell, screen: &mut Screen, modes: &Modes, listed: usize) -> bool {
    if listed >= shell.mode_count {
        return false;
    }
    let r = runtime::call(10, 2, modes.kernel[listed] as u64);
    if r.status != 0 {
        return false;
    }
    let (width, height, stride) = (
        r.word as u32 & 0xffff,
        (r.word >> 16) as u32 & 0xffff,
        (r.word >> 32) as u32,
    );
    screen.build(width, height, stride);
    shell.resize(width as i32, height as i32);
    shell.mode_current = listed;
    shell.mode_choice = listed;
    true
}

fn load_settings(client: &mut Client, shell: &mut Shell) -> Option<(u16, u16)> {
    let mut path = [0u8; 64];
    let file = client
        .resolve(join(&mut path, SETTINGS_DIR, SETTINGS_FILE))
        .ok()?;
    let mut buffer = [0u8; 512];
    let n = client.read(file, &mut buffer).ok()?;
    let text = core::str::from_utf8(&buffer[..n]).unwrap_or("");
    let mut resolution = None;
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (key.trim(), value.trim()) {
            ("theme", "light") => shell.light = true,
            ("theme", "dark") => shell.light = false,
            ("accent", v) => {
                if let Ok(n @ 0..=3) = v.parse::<usize>() {
                    shell.accent = n;
                }
            }
            ("speed", v) => {
                if let Ok(n @ 1..=3) = v.parse::<i32>() {
                    shell.speed = n;
                }
            }
            ("lang", v) => {
                if let Some(lang) = Lang::from_code(v) {
                    shell.lang = lang;
                }
            }
            ("resolution", v) => {
                resolution = v
                    .split_once('x')
                    .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)));
            }
            _ => {}
        }
    }
    shell.full();
    resolution
}

fn save_settings(client: &mut Client, shell: &Shell) -> Result<(), u32> {
    let dir = match client.resolve(SETTINGS_DIR) {
        Ok(dir) => dir,
        Err(code) if code == Error::NotFound.code() => client.create(0, SETTINGS_DIR, true)?,
        Err(code) => return Err(code),
    };
    let mut path = [0u8; 64];
    let file = match client.resolve(join(&mut path, SETTINGS_DIR, SETTINGS_FILE)) {
        Ok(file) => file,
        Err(code) if code == Error::NotFound.code() => client.create(dir, SETTINGS_FILE, false)?,
        Err(code) => return Err(code),
    };
    let mut line = commands::Line::new();
    let mode = shell.modes[shell.mode_current];
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "theme={}\naccent={}\nspeed={}\nlang={}\nresolution={}x{}\n",
            if shell.light { "light" } else { "dark" },
            shell.accent,
            shell.speed,
            shell.lang.code(),
            mode.width,
            mode.height
        ),
    );
    client.write(file, line.as_str().as_bytes())
}

/// Lists `dir` into the files window; `keep` selects an entry by id afterwards.
fn show(client: &mut Client, shell: &mut Shell, dir: u16, keep: Option<u16>) -> Result<(), u32> {
    let mut entries = [Entry {
        id: 0,
        directory: false,
        size: 0,
        name: Name::EMPTY,
    }; MAX_ENTRIES];
    let (count, more) = client.list(dir, &mut entries)?;
    shell.files.set_entries(&entries[..count], more, keep);
    shell.listing_shown();
    Ok(())
}

/// Loads a text file into the editor and reports problems in its status line.
pub fn open_in_editor(client: &mut Client, shell: &mut Shell, id: u16, name: Name) {
    let lang = shell.lang;
    let mut buffer = [0u8; fs::DATA_BYTES];
    match client.read_at(id, 0, &mut buffer) {
        Ok((n, size)) if size > n => {
            shell.editor.message = pick(
                lang,
                [
                    "Файл больше 60 КБ: открыть нельзя",
                    "Files over 60 KB cannot be opened",
                    "Файл понад 60 КБ не відкрити",
                ],
            );
        }
        Ok((n, _)) => {
            if !shell.editor.load(Some((id, name)), &buffer[..n]) {
                shell.editor.message = pick(
                    lang,
                    [
                        "Это не текстовый файл",
                        "Not a text file",
                        "Це не текстовий файл",
                    ],
                );
            }
        }
        Err(code) => shell.editor.message = storage::message(code, lang),
    }
    shell.editor_changed();
}

fn perform(client: &mut Client, shell: &mut Shell, op: FileOp) {
    let lang = shell.lang;
    if !matches!(shell.files.storage, Storage::Ready { .. }) {
        refresh_storage(client, shell);
        if !matches!(shell.files.storage, Storage::Ready { .. }) {
            shell.files.message = pick(
                lang,
                [
                    "Диск hpOS не найден",
                    "No hpOS disk found",
                    "Диск hpOS не знайдено",
                ],
            );
            shell.files_changed();
            return;
        }
    }
    let dir = shell.files.directory();
    let result = match op {
        FileOp::Refresh => show(client, shell, dir, None).or_else(|code| {
            // The folder vanished: fall back to the root.
            shell.files.reset();
            show(client, shell, 0, None).and(Err(code))
        }),
        FileOp::Open(id, name) => {
            let mut listed = Ok(());
            if shell.files.can_enter() {
                shell.files.enter(id, name);
                shell.files.selected = None;
                shell.files.scroll = 0;
                listed = show(client, shell, id, None);
                if listed.is_err() {
                    shell.files.leave();
                }
            }
            listed
        }
        FileOp::Edit(id, name) => {
            open_in_editor(client, shell, id, name);
            shell.open(EDITOR);
            Ok(())
        }
        FileOp::Up => match shell.files.parent() {
            Some(parent) => {
                shell.files.leave();
                shell.files.scroll = 0;
                show(client, shell, parent, Some(dir))
            }
            None => Ok(()),
        },
        FileOp::CreateFolder(name) | FileOp::CreateFile(name) => {
            let folder = matches!(op, FileOp::CreateFolder(_));
            client.create(dir, name.as_str(), folder).and_then(|id| {
                shell.files.message = if folder {
                    pick(lang, ["Папка создана", "Folder created", "Теку створено"])
                } else {
                    pick(lang, ["Файл создан", "File created", "Файл створено"])
                };
                show(client, shell, dir, Some(id))
            })
        }
        FileOp::Rename(id, name) => client
            .call(
                fs::RENAME,
                [id as u64, dir as u64, 0, 0],
                name.as_str(),
                &[],
            )
            .and_then(|_| {
                shell.files.message = pick(lang, ["Переименовано", "Renamed", "Перейменовано"]);
                show(client, shell, dir, Some(id))
            }),
        FileOp::Delete(id) => client
            .call(fs::REMOVE, [id as u64, 0, 0, 0], "", &[])
            .and_then(|_| {
                shell.files.message = pick(lang, ["Удалено", "Deleted", "Видалено"]);
                if shell.editor.file.is_some_and(|(open, _)| open == id) {
                    shell.editor.load(None, &[]);
                    shell.editor_changed();
                }
                show(client, shell, dir, None)
            }),
    };
    match result {
        Ok(()) => {
            if matches!(op, FileOp::Refresh | FileOp::Open(..) | FileOp::Up) {
                shell.files.message = "";
            }
        }
        Err(code) => shell.files.message = storage::message(code, lang),
    }
    if !matches!(
        op,
        FileOp::Refresh | FileOp::Open(..) | FileOp::Up | FileOp::Edit(..)
    ) {
        refresh_storage(client, shell);
    }
    shell.files_changed();
}

fn save_document(client: &mut Client, shell: &mut Shell) {
    let lang = shell.lang;
    let Some((id, _)) = shell.editor.file else {
        return;
    };
    match client.write(id, shell.editor.bytes()) {
        Ok(()) => {
            shell.editor.saved();
            shell.editor.message = pick(lang, ["Сохранено", "Saved", "Збережено"]);
            if shell.windows[FILES].visible {
                shell.push_request(Request::File(FileOp::Refresh));
            }
            refresh_storage(client, shell);
        }
        Err(code) => shell.editor.message = storage::message(code, lang),
    }
    shell.editor_changed();
}

#[unsafe(no_mangle)]
extern "C" fn service_main() -> ! {
    if runtime::call(6, 0, 0).status != 3 || runtime::call(7, 0, 0).status != 3 {
        runtime::fail();
    }
    let info = unsafe { core::ptr::read_volatile(STARTUP_ADDRESS as *const Startup) };
    let mut screen = Screen {
        frame: None,
        back: None,
        framebuffer: info.framebuffer,
        backbuffer: info.backbuffer,
    };
    screen.build(info.width, info.height, info.stride);
    // The shell is large (editor, terminal, folder listing): it lives in the heap window.
    let shell = unsafe { &mut *(DESKTOP_HEAP as *mut Shell) };
    unsafe {
        core::ptr::write(
            shell,
            Shell::new(info.width.max(800) as i32, info.height.max(600) as i32),
        );
    }
    // The page is zeroed heap memory, which is an empty page.
    let page = unsafe { &mut *((DESKTOP_HEAP as usize + heap::PAGE) as *mut Page) };
    let mut net = web::Net::new();
    let mut client = Client::new();
    let mut modes = Modes::query(shell);
    refresh_storage(&mut client, shell);
    if matches!(shell.files.storage, Storage::Ready { .. }) {
        if let Some((w, h)) = load_settings(&mut client, shell) {
            let wanted = (0..shell.mode_count)
                .find(|n| shell.modes[*n].width == w && shell.modes[*n].height == h);
            if let Some(n) = wanted.filter(|n| *n != shell.mode_current) {
                switch_mode(shell, &mut screen, &modes, n);
            }
        }
    }
    shell.settings_dirty = false;
    let seed = unsafe { core::arch::x86_64::_rdtsc() };
    let mut session = Session::new(seed);
    let mut sequence = 1;
    let mut last_status = 0;
    let mut last_present = 0;
    let mut dirty_since = None;
    let mut values = status();
    let mut old_cursor = view::cursor_rect(shell);
    let mut old_style = shell.cursor();
    loop {
        let clock = runtime::call(5, 0, 0).word;
        let animating = shell.animate(clock);
        for _ in 0..64 {
            let event = runtime::call(2, 1, 0);
            if event.status == 1 {
                break;
            }
            if event.status != 0 || event.sender != 3 {
                runtime::fail();
            }
            let check = if event.word >> 63 != 0 {
                let Some(packet) = Pointer::decode(event.word) else {
                    runtime::fail();
                };
                shell.pointer(packet, clock)
            } else {
                let Some(key) = Key::decode(event.word) else {
                    runtime::fail();
                };
                shell.key(key)
            };
            if check && runtime::call(8, 0, 0).status != 0 {
                runtime::fail();
            }
        }
        if shell.shown(TERMINAL) && !shell.terminal.started {
            session.greet(shell);
            shell.terminal_changed();
        }
        let mut command = [0u8; hpos_desktop::terminal::INPUT_BYTES];
        let mut command_len = None;
        if let Some((_, line)) = shell.terminal.take_command() {
            command[..line.len()].copy_from_slice(line.as_bytes());
            command_len = Some(line.len());
        }
        if let Some(len) = command_len {
            let line = core::str::from_utf8(&command[..len]).unwrap_or("");
            values = status();
            session.run(&mut client, &mut net, shell, line, &values, sequence);
            shell.terminal_changed();
        }
        while let Some(request) = shell.take_request() {
            match request {
                Request::File(op) => perform(&mut client, shell, op),
                Request::SaveDocument => save_document(&mut client, shell),
                Request::SetMode(n) => {
                    let previous = shell.mode_current;
                    if switch_mode(shell, &mut screen, &modes, n) {
                        modes.previous = previous;
                        shell.mode_deadline = Some(clock + MODE_CONFIRM_TICKS);
                    }
                }
                Request::KeepMode => shell.settings_dirty = true,
                Request::Navigate => {
                    let mut url = [0u8; net::URL_BYTES];
                    let typed = shell.browser.address;
                    if let Some(n) = address_to_url(typed.as_str(), &mut url) {
                        let url = core::str::from_utf8(&url[..n]).unwrap_or("");
                        net.navigate(shell, url, true);
                    }
                }
                Request::Follow { x, y } => {
                    if let Some(link) = browser::link_at(page, x, y) {
                        let mut url = [0u8; net::URL_BYTES];
                        let base = shell.browser.current;
                        if let Some(n) = hpos_net::http::resolve(base.as_str(), page.href(link), &mut url) {
                            let url = core::str::from_utf8(&url[..n]).unwrap_or("");
                            net.navigate(shell, url, true);
                        }
                    }
                }
                Request::Back => {
                    if let Some(url) = shell.browser.back() {
                        net.navigate(shell, url.as_str(), false);
                    }
                }
                Request::Reload => {
                    let url = shell.browser.current;
                    if !url.is_empty() {
                        net.navigate(shell, url.as_str(), false);
                    }
                }
                Request::Stop => net.stop(),
                Request::Connect => {
                    net.send(net::CONNECT, "");
                }
                Request::RevertMode => {
                    let previous = modes.previous;
                    switch_mode(shell, &mut screen, &modes, previous);
                }
            }
        }
        let fresh = net.poll(shell, page);
        session.network_events(shell, &fresh.events[..fresh.count]);
        // The page follows the window width; the pointer shows link targets.
        if shell.browser.has_page && page.width != shell.page_width() {
            browser::layout(page, shell.page_width());
            shell.browser.page_height = page.height;
            let visible = shell.page_rect().h;
            shell.browser.scroll_by(0, visible);
            shell.browser_changed();
        }
        let link = if shell.hover == 1050 && shell.browser.has_page {
            let area = shell.page_rect();
            browser::link_at(page, shell.cursor_x - area.x, shell.cursor_y - area.y + shell.browser.scroll)
        } else {
            None
        };
        if link != shell.browser.hover_link {
            shell.browser.hover_link = link;
            shell.browser_changed();
        }
        if shell.browser.loading && shell.shown(hpos_desktop::model::BROWSER) {
            let area = shell.page_rect();
            shell.invalidate(hpos_desktop::Rect::new(area.x, area.y, area.w, 3));
        }
        if shell.settings_dirty {
            shell.settings_dirty = false;
            dirty_since = Some(clock);
        }
        if dirty_since.is_some_and(|t| clock.wrapping_sub(t) >= SAVE_DELAY) {
            dirty_since = None;
            if matches!(shell.files.storage, Storage::Ready { .. })
                && save_settings(&mut client, shell).is_err()
            {
                shell.files.message = pick(
                    shell.lang,
                    [
                        "Не удалось сохранить настройки",
                        "Could not save settings",
                        "Не вдалося зберегти налаштування",
                    ],
                );
                shell.files_changed();
            }
        }
        if clock.wrapping_sub(last_status) >= 20 {
            values = status();
            last_status = clock;
            if shell.shown(0) {
                shell.invalidate(shell.rect(0));
            }
            shell.invalidate(shell.clock_rect());
        }
        // Animations present every tick; otherwise at most every other tick.
        let animating = animating || shell.browser.loading;
        if sequence == 1 || clock.wrapping_sub(last_present) >= if animating { 1 } else { 2 } {
            let damage = shell.take_damage();
            let cursor = view::cursor_rect(shell);
            let style = shell.cursor();
            if damage.is_some() || cursor != old_cursor || style != old_style {
                if let (Some(frame), Some(back)) = (&screen.frame, &screen.back) {
                    frame.copy_from(back, old_cursor);
                    if let Some(rect) = damage {
                        view::draw(&back.clipped(rect), shell, page, &values);
                        frame.copy_from(back, rect);
                    }
                    view::cursor(frame, shell);
                }
                let end = runtime::call(5, 0, 0).word;
                runtime::report(sequence, end.wrapping_sub(clock));
                sequence = sequence.wrapping_add(1);
                last_present = end;
                old_cursor = cursor;
                old_style = style;
            }
        }
        session.step(&mut client, shell, sequence);
        runtime::sleep(1);
    }
}
