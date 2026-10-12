use crate::{
    Key,
    canvas::Canvas,
    editor::Editor,
    files::{Edit, EditMode, FileOp, Files, Name, Storage},
    lang::{LANGS, Lang, pick},
    pointer::Pointer,
    terminal::Terminal,
    web::{Browser, NetView, Url},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}
impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(self) -> i32 {
        self.y + self.h
    }
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersect(self, other: Self) -> Self {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        Self::new(
            x,
            y,
            (self.right().min(other.right()) - x).max(0),
            (self.bottom().min(other.bottom()) - y).max(0),
        )
    }
    pub fn union(self, other: Self) -> Self {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Self::new(
            x,
            y,
            self.right().max(other.right()) - x,
            self.bottom().max(other.bottom()) - y,
        )
    }
    pub fn expanded(self, n: i32) -> Self {
        Self::new(self.x - n, self.y - n, self.w + 2 * n, self.h + 2 * n)
    }
}
/// `value` limited to `low..=high`, preferring `low` when the range is empty.
fn bounded(value: i32, low: i32, high: i32) -> i32 {
    value.min(high).max(low)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Window {
    System = 0,
    Settings = 1,
    Help = 2,
    Files = 3,
    About = 4,
    Editor = 5,
    Terminal = 6,
    Browser = 7,
    Network = 8,
}
impl Window {
    pub fn title(self, lang: Lang) -> &'static str {
        pick(
            lang,
            match self {
                Self::System => ["Система", "System", "Система"],
                Self::Settings => ["Настройки", "Settings", "Налаштування"],
                Self::Help => ["Помощь", "Help", "Довідка"],
                Self::Files => ["Файлы", "Files", "Файли"],
                Self::About => ["О системе", "About", "Про систему"],
                Self::Editor => ["Редактор", "Editor", "Редактор"],
                Self::Terminal => ["Терминал", "Terminal", "Термінал"],
                Self::Browser => ["Браузер", "Browser", "Браузер"],
                Self::Network => ["Сеть", "Network", "Мережа"],
            },
        )
    }
    fn size(self) -> (i32, i32) {
        match self {
            Self::System => (620, 456),
            Self::Settings => (660, 470),
            Self::Help => (560, 400),
            Self::Files => (640, 440),
            Self::About => (560, 440),
            Self::Editor => (680, 460),
            Self::Terminal => (700, 440),
            Self::Browser => (780, 540),
            Self::Network => (620, 500),
        }
    }
    fn minimum(self) -> (i32, i32) {
        match self {
            Self::System => (580, 456),
            Self::Settings => (640, 460),
            Self::Help => (520, 380),
            Self::Files => (560, 400),
            Self::About => (540, 420),
            Self::Editor => (480, 320),
            Self::Terminal => (480, 300),
            Self::Browser => (520, 360),
            Self::Network => (560, 460),
        }
    }
}
pub const COUNT: usize = 9;
pub const WINDOWS: [Window; COUNT] = [
    Window::System,
    Window::Settings,
    Window::Help,
    Window::Files,
    Window::About,
    Window::Editor,
    Window::Terminal,
    Window::Browser,
    Window::Network,
];
pub const SYSTEM: usize = Window::System as usize;
pub const SETTINGS: usize = Window::Settings as usize;
pub const HELP: usize = Window::Help as usize;
pub const FILES: usize = Window::Files as usize;
pub const ABOUT: usize = Window::About as usize;
pub const EDITOR: usize = Window::Editor as usize;
pub const TERMINAL: usize = Window::Terminal as usize;
pub const BROWSER: usize = Window::Browser as usize;
pub const NETWORK: usize = Window::Network as usize;
/// Desktop icons, in grid order.
pub const DESKTOP: [usize; 8] = [FILES, BROWSER, TERMINAL, EDITOR, NETWORK, SETTINGS, SYSTEM, HELP];
/// Always on the taskbar; other windows appear while open.
pub const PINNED: [usize; 5] = [FILES, BROWSER, TERMINAL, EDITOR, SETTINGS];
/// Start menu entries, top to bottom.
pub const MENU: [usize; COUNT] = [
    FILES, BROWSER, TERMINAL, EDITOR, NETWORK, SETTINGS, SYSTEM, HELP, ABOUT,
];
pub const TASKBAR: i32 = 52;
const CLOCK_WIDTH: i32 = 150;
/// Animation lengths in 10 ms timer ticks.
pub const WINDOW_TICKS: u64 = 16;
pub const MENU_TICKS: u64 = 14;
pub const MORPH_TICKS: u64 = 14;
pub const LIST_TICKS: u64 = 18;
const MENU_ROW: i32 = 44;
const MENU_TOP: i32 = 92;
pub const MENU_HEIGHT: i32 = MENU_TOP + COUNT as i32 * MENU_ROW + 40;
const TOOLBAR: [i32; 5] = [88, 96, 88, 80, 100];
pub const MAX_MODES: usize = 12;
const MODE_ROWS: usize = 8;
/// Seconds-equivalent ticks before an unconfirmed resolution is reverted.
pub const MODE_CONFIRM_TICKS: u64 = 1500;
pub const ROW: i32 = 30;
pub const TEXT_ROW: i32 = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Opening,
    Closing,
    Minimizing,
    /// Maximize or restore: the frame glides from the old rectangle.
    Morph(Rect),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Animation {
    pub motion: Motion,
    pub start: u64,
}
/// Ease-out cubic on 0..=1000: fast start, soft landing.
pub fn ease_out(permille: u64) -> i32 {
    let t = permille.min(1000) as i64;
    let rest = 1000 - t;
    (1000 - rest * rest * rest / 1_000_000) as i32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DisplayMode {
    pub width: u16,
    pub height: u16,
}

/// Work the desktop service performs for the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    File(FileOp),
    SaveDocument,
    SetMode(usize),
    KeepMode,
    RevertMode,
    /// Load what the address field holds.
    Navigate,
    /// A click on the page at page coordinates.
    Follow { x: i32, y: i32 },
    Back,
    Reload,
    Stop,
    /// Bring up the USB modem.
    Connect,
}

#[derive(Clone, Copy)]
pub struct WindowState {
    pub normal: Rect,
    pub visible: bool,
    pub minimized: bool,
    pub maximized: bool,
}
#[derive(Clone, Copy)]
enum Drag {
    None,
    Move(usize, i32, i32),
    Resize(usize, i32, i32, Rect),
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Arrow,
    Resize,
    Move,
    Text,
}

pub struct Shell {
    pub width: i32,
    pub height: i32,
    pub light: bool,
    pub accent: usize,
    pub speed: i32,
    pub lang: Lang,
    pub windows: [WindowState; COUNT],
    pub order: [usize; COUNT],
    pub active: usize,
    pub menu: bool,
    /// Index into [`MENU`].
    pub selected: usize,
    pub menu_x: i32,
    pub menu_y: i32,
    pub cursor_x: i32,
    pub cursor_y: i32,
    pub buttons: u8,
    pub hover: u16,
    pub help_scroll: i32,
    /// Index into [`DESKTOP`].
    pub selected_icon: Option<usize>,
    pub russian: bool,
    /// Theme, accent, speed, language or resolution changed since the last save.
    pub settings_dirty: bool,
    pub files: Files,
    pub editor: Editor,
    pub terminal: Terminal,
    pub browser: Browser,
    pub net: NetView,
    pub modes: [DisplayMode; MAX_MODES],
    pub mode_count: usize,
    pub mode_current: usize,
    pub mode_choice: usize,
    /// Tick by which an unconfirmed resolution change is reverted.
    pub mode_deadline: Option<u64>,
    /// Current timer tick, advanced by `animate`.
    pub now: u64,
    pub animations: [Option<Animation>; COUNT],
    pub menu_opened: u64,
    pub list_shown: u64,
    drag: Drag,
    last_icon: Option<(usize, u64)>,
    last_row: Option<(usize, u64)>,
    requests: [Option<Request>; 8],
    damage: Option<Rect>,
}

impl Shell {
    pub fn new(width: i32, height: i32) -> Self {
        let mut windows = [WindowState {
            normal: Rect::new(0, 0, 0, 0),
            visible: false,
            minimized: false,
            maximized: false,
        }; COUNT];
        for (id, window) in WINDOWS.iter().enumerate() {
            let (w, h) = window.size();
            // Cascade from the centre so several open windows stay visible.
            let shift = [0, 36, 70, 50, 0, 24, 60, 12, 44][id];
            let x = bounded((width - w) / 2 + shift - 30, 140, width - w - 8);
            let y = bounded(
                (height - TASKBAR - h) / 2 + shift / 2 - 10,
                8,
                height - TASKBAR - h - 8,
            );
            windows[id].normal = Rect::new(x, y, w, h);
        }
        windows[SYSTEM].visible = true;
        let mut shell = Self {
            width,
            height,
            light: false,
            accent: 0,
            speed: 1,
            lang: Lang::Ru,
            windows,
            order: [8, 7, 6, 5, 4, 3, 2, 1, 0],
            active: SYSTEM,
            menu: false,
            selected: 0,
            menu_x: 8,
            menu_y: height - TASKBAR - MENU_HEIGHT - 8,
            cursor_x: width / 2,
            cursor_y: height / 2,
            buttons: 0,
            hover: 0,
            help_scroll: 0,
            selected_icon: None,
            russian: false,
            settings_dirty: false,
            files: Files::new(),
            editor: Editor::new(),
            terminal: Terminal::new(),
            browser: Browser::new(),
            net: NetView::new(),
            modes: [DisplayMode {
                width: width as u16,
                height: height as u16,
            }; MAX_MODES],
            mode_count: 1,
            mode_current: 0,
            mode_choice: 0,
            mode_deadline: None,
            now: 0,
            animations: [None; COUNT],
            menu_opened: 0,
            list_shown: 0,
            drag: Drag::None,
            last_icon: None,
            last_row: None,
            requests: [None; 8],
            damage: None,
        };
        shell.full();
        shell
    }
    pub fn t(&self, text: [&'static str; 3]) -> &'static str {
        pick(self.lang, text)
    }
    pub fn viewport(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }
    pub fn rect(&self, id: usize) -> Rect {
        if self.windows[id].maximized {
            Rect::new(0, 0, self.width, self.height - TASKBAR)
        } else {
            self.windows[id].normal
        }
    }
    /// Everything a window can paint: frame, shadow and animation overshoot.
    pub fn area(&self, id: usize) -> Rect {
        let r = self.rect(id);
        Rect::new(r.x - 18, r.y - 18, r.w + 36, r.h + 66)
    }
    pub fn shown(&self, id: usize) -> bool {
        self.windows[id].visible && !self.windows[id].minimized
    }
    /// Shown and not on its way out; only these receive input.
    pub fn interactive(&self, id: usize) -> bool {
        self.shown(id)
            && !matches!(
                self.animations[id],
                Some(Animation {
                    motion: Motion::Closing | Motion::Minimizing,
                    ..
                })
            )
    }
    fn progress(&self, start: u64, ticks: u64) -> i32 {
        ease_out(self.now.saturating_sub(start) * 1000 / ticks)
    }
    /// Where the window frame is drawn this frame.
    pub fn visual(&self, id: usize) -> Rect {
        let r = self.rect(id);
        let Some(a) = self.animations[id] else {
            return r;
        };
        if let Motion::Morph(from) = a.motion {
            let e = self.progress(a.start, MORPH_TICKS);
            let mix = |a: i32, b: i32| a + (b - a) * e / 1000;
            return Rect::new(
                mix(from.x, r.x),
                mix(from.y, r.y),
                mix(from.w, r.w),
                mix(from.h, r.h),
            );
        }
        let e = self.progress(a.start, WINDOW_TICKS);
        let scale = match a.motion {
            Motion::Opening => 880 + 120 * e / 1000,
            _ => 1000 - 140 * e / 1000,
        };
        let w = r.w * scale / 1000;
        let h = r.h * scale / 1000;
        let lift = if a.motion == Motion::Minimizing {
            40 * e / 1000
        } else {
            0
        };
        Rect::new(r.x + (r.w - w) / 2, r.y + (r.h - h) / 2 + lift, w, h)
    }
    /// True while the frame itself changes size, so content follows the frame.
    pub fn morphing(&self, id: usize) -> bool {
        matches!(
            self.animations[id],
            Some(Animation {
                motion: Motion::Morph(_),
                ..
            })
        )
    }
    /// Vertical offset of start menu row `n` (the header is row 0) while it slides in.
    pub fn menu_offset(&self, n: usize) -> i32 {
        let start = self.menu_opened + n as u64;
        30 - 30 * self.progress(start, MENU_TICKS) / 1000
    }
    /// Horizontal offset of list row `n` right after a folder is shown.
    pub fn row_offset(&self, n: usize) -> i32 {
        let start = self.list_shown + n.min(12) as u64;
        24 - 24 * self.progress(start, LIST_TICKS) / 1000
    }
    /// Advances animations to `now`; returns true while any is running.
    pub fn animate(&mut self, now: u64) -> bool {
        self.now = now;
        let mut running = false;
        for id in 0..COUNT {
            let Some(a) = self.animations[id] else {
                continue;
            };
            let length = if matches!(a.motion, Motion::Morph(_)) {
                MORPH_TICKS
            } else {
                WINDOW_TICKS
            };
            let mut area = self.area(id);
            if let Motion::Morph(from) = a.motion {
                area = area.union(Rect::new(
                    from.x - 18,
                    from.y - 18,
                    from.w + 36,
                    from.h + 66,
                ));
            }
            self.invalidate(area);
            if now.saturating_sub(a.start) < length {
                running = true;
                continue;
            }
            self.animations[id] = None;
            match a.motion {
                Motion::Closing => self.windows[id].visible = false,
                Motion::Minimizing => self.windows[id].minimized = true,
                _ => {}
            }
            if matches!(a.motion, Motion::Closing | Motion::Minimizing) {
                self.choose_active();
            }
        }
        if self.menu && now.saturating_sub(self.menu_opened) <= MENU_TICKS + COUNT as u64 {
            self.invalidate(self.menu_rect().expanded(34));
            running = true;
        }
        if self.shown(FILES) && now.saturating_sub(self.list_shown) <= LIST_TICKS + 12 {
            self.invalidate(self.list_rect());
            running = true;
        }
        if let Some(deadline) = self.mode_deadline {
            if now >= deadline {
                self.mode_deadline = None;
                self.request(Request::RevertMode);
            }
            self.invalidate(self.banner_rect());
        }
        running
    }
    fn begin(&mut self, id: usize, motion: Motion) {
        self.animations[id] = Some(Animation {
            motion,
            start: self.now,
        });
        self.invalidate(self.area(id));
    }
    pub fn invalidate(&mut self, r: Rect) {
        let r = r.intersect(self.viewport());
        if r.w > 0 && r.h > 0 {
            self.damage = Some(self.damage.map_or(r, |old| old.union(r)));
        }
    }
    pub fn full(&mut self) {
        self.invalidate(self.viewport());
    }
    pub fn take_damage(&mut self) -> Option<Rect> {
        self.damage.take()
    }
    fn request(&mut self, request: Request) {
        if let Some(slot) = self.requests.iter_mut().find(|r| r.is_none()) {
            *slot = Some(request);
        }
    }
    pub fn push_request(&mut self, request: Request) {
        self.request(request);
    }
    pub fn take_request(&mut self) -> Option<Request> {
        let first = self.requests[0].take();
        self.requests.rotate_left(1);
        first
    }
    /// Re-lays out everything for a new screen size.
    pub fn resize(&mut self, width: i32, height: i32) {
        self.width = width;
        self.height = height;
        for (id, window) in self.windows.iter_mut().enumerate() {
            let (min_w, min_h) = WINDOWS[id].minimum();
            let r = &mut window.normal;
            r.w = bounded(r.w, min_w, width - 16);
            r.h = bounded(r.h, min_h, height - TASKBAR - 16);
            r.x = bounded(r.x, 8, width - r.w - 8);
            r.y = bounded(r.y, 8, height - TASKBAR - r.h - 8);
        }
        self.animations = [None; COUNT];
        self.menu = false;
        self.menu_y = height - TASKBAR - MENU_HEIGHT - 8;
        self.cursor_x = self.cursor_x.min(width - 1);
        self.cursor_y = self.cursor_y.min(height - 1);
        self.drag = Drag::None;
        self.damage = None;
        self.full();
    }

    pub fn menu_rect(&self) -> Rect {
        Rect::new(self.menu_x, self.menu_y, 340, MENU_HEIGHT)
    }
    pub fn menu_row(&self, n: usize) -> Rect {
        let r = self.menu_rect();
        Rect::new(
            r.x + 8,
            r.y + MENU_TOP + n as i32 * MENU_ROW,
            r.w - 16,
            MENU_ROW - 4,
        )
    }
    /// Desktop icon `slot`, laid out in columns that fit above the taskbar.
    pub fn icon(&self, slot: usize) -> Rect {
        let per = ((self.height - TASKBAR - 24) / 104).max(1) as usize;
        let (column, row) = (slot / per, slot % per);
        Rect::new(20 + column as i32 * 112, 24 + row as i32 * 104, 106, 96)
    }
    pub fn taskbar(&self) -> Rect {
        Rect::new(0, self.height - TASKBAR, self.width, TASKBAR)
    }
    pub fn start_button(&self) -> Rect {
        Rect::new(6, self.height - 46, 46, 40)
    }
    pub fn clock_rect(&self) -> Rect {
        Rect::new(
            self.width - CLOCK_WIDTH,
            self.height - TASKBAR,
            CLOCK_WIDTH,
            TASKBAR,
        )
    }
    /// Windows on the taskbar, in order: pinned first, then other open ones.
    pub fn task_order(&self) -> ([usize; COUNT], usize) {
        let mut list = [0; COUNT];
        let mut n = 0;
        for id in PINNED {
            list[n] = id;
            n += 1;
        }
        for id in 0..COUNT {
            if !PINNED.contains(&id) && self.windows[id].visible {
                list[n] = id;
                n += 1;
            }
        }
        (list, n)
    }
    pub fn task(&self, id: usize) -> Rect {
        let (list, n) = self.task_order();
        let Some(index) = list[..n].iter().position(|v| *v == id) else {
            return Rect::new(0, 0, 0, 0);
        };
        let w = ((self.width - 64 - CLOCK_WIDTH) / n as i32 - 4).clamp(44, 132);
        Rect::new(60 + index as i32 * (w + 4), self.height - 46, w, 40)
    }
    pub fn tool(&self, n: usize) -> Rect {
        let r = self.rect(FILES);
        let x = r.x + 16 + TOOLBAR[..n].iter().map(|w| w + 8).sum::<i32>();
        Rect::new(x, r.y + 46, TOOLBAR[n], 34)
    }
    pub fn edit_rect(&self) -> Rect {
        let r = self.rect(FILES);
        Rect::new(r.x + 16, r.y + 116, r.w - 32, 34)
    }
    /// List rows of the files window, below the toolbar, path and optional name field.
    pub fn list_rect(&self) -> Rect {
        let r = self.rect(FILES);
        let top = r.y + 118 + if self.files.edit.is_some() { 44 } else { 0 };
        Rect::new(r.x + 12, top, r.w - 24, (r.bottom() - 44 - top).max(0))
    }
    pub const ROW: i32 = ROW;
    pub fn visible_rows(&self) -> usize {
        (self.list_rect().h / ROW).max(1) as usize
    }
    pub fn save_button(&self) -> Rect {
        let r = self.rect(EDITOR);
        Rect::new(r.x + 16, r.y + 46, 128, 34)
    }
    pub fn text_rect(&self) -> Rect {
        let r = self.rect(EDITOR);
        Rect::new(r.x + 12, r.y + 90, r.w - 24, (r.h - 90 - 40).max(TEXT_ROW))
    }
    pub fn text_rows(&self) -> usize {
        (self.text_rect().h / TEXT_ROW).max(1) as usize
    }
    pub fn console_rect(&self) -> Rect {
        let r = self.rect(TERMINAL);
        Rect::new(r.x + 4, r.y + 40, r.w - 8, (r.h - 44).max(TEXT_ROW))
    }
    /// Scrollback rows above the input line.
    pub fn console_rows(&self) -> usize {
        ((self.console_rect().h - 12) / 22 - 1).max(1) as usize
    }
    /// Takes the oldest queued file operation, leaving other requests queued.
    pub fn take_file_op(&mut self) -> Option<FileOp> {
        let index = self
            .requests
            .iter()
            .position(|r| matches!(r, Some(Request::File(_))))?;
        let Some(Request::File(op)) = self.requests[index].take() else {
            return None;
        };
        self.requests[index..].rotate_left(1);
        Some(op)
    }
    pub fn files_changed(&mut self) {
        if self.windows[FILES].visible {
            self.invalidate(self.rect(FILES));
        }
    }
    pub fn editor_changed(&mut self) {
        if self.windows[EDITOR].visible {
            self.invalidate(self.rect(EDITOR));
        }
    }
    pub fn terminal_changed(&mut self) {
        if self.windows[TERMINAL].visible {
            self.invalidate(self.rect(TERMINAL));
        }
    }
    /// A new folder listing arrived: rows slide in.
    pub fn listing_shown(&mut self) {
        self.list_shown = self.now;
        self.files_changed();
    }
    pub fn setting(&self, n: usize) -> Rect {
        let r = self.rect(SETTINGS);
        let right = r.x + 334;
        match n {
            0 | 1 => Rect::new(r.x + 24 + n as i32 * 112, r.y + 80, 104, 34),
            2..=5 => Rect::new(r.x + 24 + (n - 2) as i32 * 46, r.y + 156, 34, 30),
            6..=8 => Rect::new(r.x + 24 + (n - 6) as i32 * 72, r.y + 228, 64, 34),
            10..=12 => Rect::new(r.x + 24 + (n - 10) as i32 * 72, r.y + 300, 64, 34),
            9 => Rect::new(r.x + 24, r.y + 356, 224, 34),
            20..=31 => Rect::new(right, r.y + 80 + (n - 20) as i32 * 30, r.w - 358, 28),
            _ => Rect::new(right, r.y + 84 + MODE_ROWS as i32 * 30, 150, 34),
        }
    }
    pub fn back_button(&self) -> Rect {
        let r = self.rect(BROWSER);
        Rect::new(r.x + 12, r.y + 44, 84, 34)
    }
    pub fn reload_button(&self) -> Rect {
        let r = self.rect(BROWSER);
        Rect::new(r.x + 102, r.y + 44, 110, 34)
    }
    pub fn address_rect(&self) -> Rect {
        let r = self.rect(BROWSER);
        Rect::new(r.x + 218, r.y + 44, r.w - 230, 34)
    }
    /// The page viewport of the browser.
    pub fn page_rect(&self) -> Rect {
        let r = self.rect(BROWSER);
        Rect::new(r.x + 4, r.y + 86, r.w - 8, (r.h - 86 - 32).max(TEXT_ROW))
    }
    /// Width the page is laid out for: the viewport minus the scroll bar.
    pub fn page_width(&self) -> i32 {
        self.page_rect().w - 10
    }
    pub fn connect_button(&self) -> Rect {
        let r = self.rect(NETWORK);
        Rect::new(r.x + 24, r.y + 132, 200, 36)
    }
    pub fn net_log_rect(&self) -> Rect {
        let r = self.rect(NETWORK);
        Rect::new(r.x + 16, r.y + 300, r.w - 32, (r.h - 300 - 12).max(TEXT_ROW))
    }
    pub fn browser_changed(&mut self) {
        if self.windows[BROWSER].visible {
            self.invalidate(self.rect(BROWSER));
        }
    }
    pub fn network_changed(&mut self) {
        if self.windows[NETWORK].visible {
            self.invalidate(self.rect(NETWORK));
        }
    }
    pub fn banner_rect(&self) -> Rect {
        Rect::new((self.width - 520) / 2, 40, 520, 120)
    }
    pub fn banner_button(&self, keep: bool) -> Rect {
        let b = self.banner_rect();
        Rect::new(b.x + if keep { 24 } else { 196 }, b.y + 70, 160, 34)
    }

    pub fn open(&mut self, id: usize) {
        let old = self.active;
        let first_show = !self.windows[id].visible;
        if !self.interactive(id) {
            self.begin(id, Motion::Opening);
        }
        let pos = self.order.iter().position(|v| *v == id).unwrap();
        for i in pos..COUNT - 1 {
            self.order[i] = self.order[i + 1];
        }
        self.order[COUNT - 1] = id;
        if id == FILES && first_show {
            self.request(Request::File(FileOp::Refresh));
        }
        if id == BROWSER && first_show && !self.browser.has_page {
            self.browser.editing = true;
        }
        self.active = id;
        self.windows[id].visible = true;
        self.windows[id].minimized = false;
        self.invalidate(self.area(old));
        self.invalidate(self.area(id));
        self.invalidate(self.taskbar());
    }
    fn close(&mut self, id: usize) {
        if !self.interactive(id) {
            return;
        }
        if id == EDITOR && self.editor.dirty && !self.editor.confirm_close {
            self.editor.confirm_close = true;
            self.editor.message = self.t([
                "Не сохранено! Закройте ещё раз, чтобы выбросить",
                "Not saved! Close again to discard",
                "Не збережено! Закрийте ще раз, щоб відкинути",
            ]);
            self.editor_changed();
            return;
        }
        self.begin(id, Motion::Closing);
        if id == FILES {
            self.files.edit = None;
        }
        if id == EDITOR {
            self.editor.confirm_close = false;
        }
        if id == BROWSER {
            self.browser.editing = false;
        }
        self.choose_active();
    }
    fn minimize(&mut self, id: usize) {
        if self.interactive(id) {
            self.begin(id, Motion::Minimizing);
            self.choose_active();
        }
    }
    fn choose_active(&mut self) {
        if let Some(id) = self
            .order
            .iter()
            .rev()
            .copied()
            .find(|id| self.interactive(*id))
        {
            self.active = id;
            self.invalidate(self.rect(id));
        }
        self.invalidate(self.taskbar());
    }
    fn maximize(&mut self, id: usize) {
        let from = self.rect(id);
        self.windows[id].maximized = !self.windows[id].maximized;
        self.begin(id, Motion::Morph(from));
        self.invalidate(Rect::new(
            from.x - 18,
            from.y - 18,
            from.w + 36,
            from.h + 66,
        ));
    }
    fn close_menu(&mut self) {
        if self.menu {
            self.invalidate(self.menu_rect().expanded(34));
            self.invalidate(self.taskbar());
            self.menu = false;
        }
    }
    fn open_menu(&mut self, x: i32, y: i32) {
        self.close_menu();
        self.menu = true;
        self.menu_opened = self.now;
        self.menu_x = bounded(x, 0, self.width - 340);
        self.menu_y = bounded(y, 0, self.height - TASKBAR - MENU_HEIGHT - 8);
        self.invalidate(self.menu_rect().expanded(34));
        self.invalidate(self.taskbar());
    }
    fn toggle_theme(&mut self) {
        self.light = !self.light;
        self.settings_dirty = true;
        self.full();
    }
    pub fn set_lang(&mut self, lang: Lang) {
        if self.lang != lang {
            self.lang = lang;
            self.settings_dirty = true;
            self.full();
        }
    }
    fn focused(&self, id: usize) -> bool {
        self.active == id && self.interactive(id) && !self.menu
    }
    /// The active window takes typed text, so letter hotkeys are off.
    pub fn text_focus(&self) -> bool {
        self.focused(EDITOR)
            || self.focused(TERMINAL)
            || (self.focused(FILES) && self.files.edit.is_some())
            || (self.focused(BROWSER) && self.browser.editing)
    }

    /// Returns true when the user asked for a kernel memory check.
    pub fn key(&mut self, key: Key) -> bool {
        if let Key::Layout { russian } = key {
            self.russian = russian;
            self.invalidate(self.clock_rect());
            return false;
        }
        if self.mode_deadline.is_some() {
            match key {
                Key::Enter => {
                    self.mode_deadline = None;
                    self.request(Request::KeepMode);
                }
                Key::Escape => {
                    self.mode_deadline = None;
                    self.request(Request::RevertMode);
                }
                _ => {}
            }
            self.invalidate(self.banner_rect());
            return false;
        }
        let window = match key {
            Key::System => Some(SYSTEM),
            Key::Help => Some(HELP),
            Key::Settings => Some(SETTINGS),
            Key::Files => Some(FILES),
            Key::Terminal => Some(TERMINAL),
            Key::Editor => Some(EDITOR),
            Key::Browser => Some(BROWSER),
            Key::Network => Some(NETWORK),
            _ => None,
        };
        if let Some(id) = window {
            self.close_menu();
            self.open(id);
            return false;
        }
        if self.menu {
            match key {
                Key::Up | Key::Down => {
                    self.selected =
                        (self.selected + if key == Key::Up { COUNT - 1 } else { 1 }) % COUNT;
                    self.invalidate(self.menu_rect());
                }
                Key::Enter => {
                    let id = MENU[self.selected];
                    self.close_menu();
                    self.open(id);
                }
                Key::Escape | Key::Menu | Key::Text { scan: 0x39, .. } => self.close_menu(),
                _ => {}
            }
            return false;
        }
        if self.focused(FILES) && self.files.edit.is_some() && self.edit_key(key) {
            return false;
        }
        if self.focused(FILES) && self.files_key(key) {
            return false;
        }
        if self.focused(EDITOR) && self.editor_key(key) {
            return false;
        }
        if self.focused(TERMINAL) && self.terminal_key(key) {
            return false;
        }
        if self.focused(BROWSER) && self.browser_key(key) {
            return false;
        }
        if self.focused(NETWORK) && key == Key::Enter {
            self.request(Request::Connect);
            return false;
        }
        match key {
            Key::Text {
                scan: 0x14,
                repeat: false,
                ..
            } if !self.text_focus() => self.toggle_theme(),
            Key::Text {
                scan: 0x32,
                repeat: false,
                ..
            } if !self.text_focus() => return true,
            Key::Menu
            | Key::Text {
                scan: 0x39,
                repeat: false,
                ..
            } if !self.text_focus() => self.open_menu(8, self.height),
            Key::Escape => self.close(self.active),
            Key::Tab => {
                // The next open window; closed ones stay closed.
                let next = (1..COUNT)
                    .map(|k| (self.active + k) % COUNT)
                    .find(|id| self.windows[*id].visible);
                if let Some(id) = next {
                    self.open(id);
                }
            }
            _ => {}
        }
        false
    }
    fn editor_key(&mut self, key: Key) -> bool {
        let rows = self.text_rows();
        let e = &mut self.editor;
        match key {
            Key::Text { ch, .. } => {
                if e.file.is_none() {
                    e.message = pick(
                        self.lang,
                        [
                            "Откройте файл в «Файлах» или создайте его там",
                            "Open or create a file in Files first",
                            "Відкрийте або створіть файл у «Файлах»",
                        ],
                    );
                } else if !e.insert(ch) {
                    e.message = pick(
                        self.lang,
                        ["Файл слишком большой", "File too large", "Файл завеликий"],
                    );
                }
            }
            Key::Enter if e.file.is_some() => {
                e.insert('\n');
            }
            Key::Tab if e.file.is_some() => {
                for _ in 0..4 {
                    e.insert(' ');
                }
            }
            Key::Backspace => e.backspace(),
            Key::Delete => e.delete(),
            Key::Left => e.left(),
            Key::Right => e.right(),
            Key::Up => e.up(),
            Key::Down => e.down(),
            Key::Home => e.home(),
            Key::End => e.end(),
            Key::PageUp => e.page(rows, false),
            Key::PageDown => e.page(rows, true),
            Key::Ctrl { scan: 0x1f } => self.request(Request::SaveDocument),
            _ => return false,
        }
        self.editor.follow(rows);
        self.editor_changed();
        true
    }
    fn browser_key(&mut self, key: Key) -> bool {
        let visible = self.page_rect().h;
        let b = &mut self.browser;
        if b.editing {
            match key {
                Key::Text { ch, .. } => {
                    b.address.push(ch);
                }
                Key::Backspace => b.address.pop(),
                Key::Ctrl { scan: 0x26 } => b.address = Url::EMPTY,
                Key::Enter => {
                    b.editing = false;
                    self.request(Request::Navigate);
                }
                Key::Escape => {
                    b.editing = false;
                    b.address = b.current;
                }
                _ => return false,
            }
            self.invalidate(self.address_rect().expanded(2));
            return true;
        }
        let step = 48;
        let page = (visible - step).max(step);
        match key {
            Key::Up => b.scroll_by(-step, visible),
            Key::Down => b.scroll_by(step, visible),
            Key::PageUp => b.scroll_by(-page, visible),
            Key::PageDown | Key::Text { scan: 0x39, .. } => b.scroll_by(page, visible),
            Key::Home => b.scroll = 0,
            Key::End => b.scroll_by(i32::MAX / 2, visible),
            Key::Backspace => self.request(Request::Back),
            Key::Ctrl { scan: 0x13 } => self.request(Request::Reload),
            Key::Escape if b.loading => self.request(Request::Stop),
            Key::Ctrl { scan: 0x26 } => {
                b.editing = true;
                b.address = Url::EMPTY;
            }
            Key::Text { ch, .. } => {
                // Typing on the page starts a new address or search.
                b.editing = true;
                b.address = Url::EMPTY;
                b.address.push(ch);
            }
            _ => return false,
        }
        self.browser_changed();
        true
    }
    fn terminal_key(&mut self, key: Key) -> bool {
        let rows = self.console_rows();
        let t = &mut self.terminal;
        match key {
            Key::Text { ch, .. } => t.type_char(ch),
            Key::Backspace => t.backspace(),
            Key::Enter => t.enter(),
            Key::Up => t.history(true),
            Key::Down => t.history(false),
            Key::PageUp => t.scroll_by(rows as i32 - 1, rows),
            Key::PageDown => t.scroll_by(1 - rows as i32, rows),
            Key::Ctrl { scan: 0x26 } => t.clear(),
            _ => return false,
        }
        self.terminal_changed();
        true
    }
    fn edit_key(&mut self, key: Key) -> bool {
        let Some(mut edit) = self.files.edit else {
            return false;
        };
        match key {
            Key::Text { ch, .. } => {
                edit.text.push(ch);
            }
            Key::Backspace => edit.text.pop(),
            Key::Escape => {
                self.files.edit = None;
                self.files.message = "";
                self.files_changed();
                return true;
            }
            Key::Enter => {
                self.submit_edit(edit);
                return true;
            }
            _ => return false,
        }
        self.files.edit = Some(edit);
        self.invalidate(self.edit_rect());
        true
    }
    fn submit_edit(&mut self, edit: Edit) {
        let name = edit.text;
        let trimmed = name.as_str().trim();
        if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
            self.files.message = self.t(["Введите имя", "Type a name", "Введіть ім'я"]);
        } else {
            let name = Name::new(trimmed);
            self.request(Request::File(match edit.mode {
                EditMode::NewFolder => FileOp::CreateFolder(name),
                EditMode::NewFile => FileOp::CreateFile(name),
                EditMode::Rename(id) => FileOp::Rename(id, name),
            }));
            self.files.edit = None;
        }
        self.files_changed();
    }
    fn files_key(&mut self, key: Key) -> bool {
        let count = self.files.count;
        match key {
            Key::Up | Key::Down if count > 0 => {
                let current =
                    self.files
                        .selected
                        .unwrap_or(if key == Key::Up { count } else { usize::MAX });
                let next = if key == Key::Up {
                    current.saturating_sub(1)
                } else {
                    current.wrapping_add(1).min(count - 1)
                };
                self.select_row(next);
            }
            Key::Enter => self.open_selected(),
            Key::Backspace => self.file_tool(0),
            Key::Delete => self.file_tool(4),
            _ => return false,
        }
        true
    }
    fn select_row(&mut self, index: usize) {
        self.files.selected = Some(index);
        self.files.confirm_delete = None;
        let rows = self.visible_rows();
        if index < self.files.scroll {
            self.files.scroll = index;
        } else if index >= self.files.scroll + rows {
            self.files.scroll = index + 1 - rows;
        }
        self.files_changed();
    }
    fn open_selected(&mut self) {
        if let Some(entry) = self.files.selected_entry().copied() {
            if entry.directory {
                if self.files.can_enter() {
                    self.request(Request::File(FileOp::Open(entry.id, entry.name)));
                } else {
                    self.files.message = self.t([
                        "Слишком глубокая вложенность",
                        "Folders nested too deep",
                        "Завелика вкладеність",
                    ]);
                }
            } else {
                self.request(Request::File(FileOp::Edit(entry.id, entry.name)));
            }
            self.files_changed();
        }
    }
    fn file_tool(&mut self, n: usize) {
        if self.files.storage == Storage::Unavailable {
            self.files.message =
                self.t(["Диск недоступен", "Disk unavailable", "Диск недоступний"]);
            self.files_changed();
            return;
        }
        let selected = self.files.selected_entry().copied();
        if n != 4 {
            self.files.confirm_delete = None;
        }
        let choose = self.t([
            "Сначала выберите файл или папку",
            "Select a file or folder first",
            "Спершу виберіть файл або теку",
        ]);
        match n {
            0 => {
                if self.files.parent().is_some() {
                    self.request(Request::File(FileOp::Up));
                }
            }
            1 | 2 => {
                let text = if n == 1 {
                    self.files
                        .unique_name(self.t(["Новая папка", "New folder", "Нова тека"]), "")
                } else {
                    self.files
                        .unique_name(self.t(["Новый файл", "New file", "Новий файл"]), ".txt")
                };
                self.files.edit = Some(Edit {
                    mode: if n == 1 {
                        EditMode::NewFolder
                    } else {
                        EditMode::NewFile
                    },
                    text,
                });
                self.files.message = self.t([
                    "Введите имя и нажмите Enter",
                    "Type a name and press Enter",
                    "Введіть ім'я та Enter",
                ]);
            }
            3 => match selected {
                Some(entry) => {
                    self.files.edit = Some(Edit {
                        mode: EditMode::Rename(entry.id),
                        text: entry.name,
                    });
                    self.files.message = self.t([
                        "Новое имя и Enter",
                        "New name, then Enter",
                        "Нове ім'я та Enter",
                    ]);
                }
                None => self.files.message = choose,
            },
            _ => match selected {
                Some(entry) if self.files.confirm_delete == Some(entry.id) => {
                    self.request(Request::File(FileOp::Delete(entry.id)));
                    self.files.confirm_delete = None;
                }
                Some(entry) => {
                    self.files.confirm_delete = Some(entry.id);
                    self.files.message = if entry.directory {
                        self.t([
                            "Удалить папку? Нажмите ещё раз",
                            "Delete folder? Press again",
                            "Видалити теку? Ще раз",
                        ])
                    } else {
                        self.t([
                            "Удалить файл? Нажмите ещё раз",
                            "Delete file? Press again",
                            "Видалити файл? Ще раз",
                        ])
                    };
                }
                None => self.files.message = choose,
            },
        }
        self.files_changed();
    }
    pub fn cursor(&self) -> Cursor {
        match self.drag {
            Drag::Move(..) => Cursor::Move,
            Drag::Resize(..) => Cursor::Resize,
            _ if (300..300 + COUNT as u16).contains(&self.hover) => Cursor::Resize,
            _ if self.hover == 850 || self.hover == 1002 => Cursor::Text,
            _ => Cursor::Arrow,
        }
    }
    fn hit(&self, x: i32, y: i32) -> u16 {
        if self.mode_deadline.is_some() && self.banner_rect().contains(x, y) {
            return if self.banner_button(true).contains(x, y) {
                950
            } else if self.banner_button(false).contains(x, y) {
                951
            } else {
                949
            };
        }
        if self.menu && self.menu_rect().contains(x, y) {
            return (0..COUNT)
                .find(|n| self.menu_row(*n).contains(x, y))
                .map_or(499, |n| 400 + n as u16);
        }
        if y >= self.height - TASKBAR {
            if self.start_button().contains(x, y) {
                return 1;
            }
            for id in 0..COUNT {
                if self.task(id).contains(x, y) {
                    return 10 + id as u16;
                }
            }
            return 0;
        }
        for id in self.order.iter().rev().copied() {
            if !self.interactive(id) {
                continue;
            }
            let r = self.rect(id);
            if !r.contains(x, y) {
                continue;
            }
            if y < r.y + 36 {
                if x >= r.right() - 120 {
                    return 200 + id as u16 * 3 + ((x - r.right() + 120) / 40) as u16;
                }
                return 100 + id as u16;
            }
            if !self.windows[id].maximized && x >= r.right() - 18 && y >= r.bottom() - 18 {
                return 300 + id as u16;
            }
            match id {
                SETTINGS => {
                    let modes = 20..20 + self.mode_count.min(MODE_ROWS);
                    for n in (0..13).chain(modes).chain([32]) {
                        if self.setting(n).contains(x, y) {
                            return 500 + n as u16;
                        }
                    }
                }
                FILES => {
                    for n in 0..TOOLBAR.len() {
                        if self.tool(n).contains(x, y) {
                            return 600 + n as u16;
                        }
                    }
                    let list = self.list_rect();
                    if list.contains(x, y) {
                        let row = ((y - list.y) / ROW) as usize;
                        if self.files.scroll + row < self.files.count {
                            return 700 + row as u16;
                        }
                    }
                }
                EDITOR => {
                    if self.save_button().contains(x, y) {
                        return 800;
                    }
                    if self.text_rect().contains(x, y) {
                        return 850;
                    }
                }
                TERMINAL if self.console_rect().contains(x, y) => return 900,
                BROWSER => {
                    for (code, rect) in [
                        (1000, self.back_button()),
                        (1001, self.reload_button()),
                        (1002, self.address_rect()),
                        (1050, self.page_rect()),
                    ] {
                        if rect.contains(x, y) {
                            return code;
                        }
                    }
                }
                NETWORK if self.connect_button().contains(x, y) => return 1100,
                _ => {}
            }
            return 150 + id as u16;
        }
        for slot in 0..DESKTOP.len() {
            if self.icon(slot).contains(x, y) {
                return 20 + slot as u16;
            }
        }
        0
    }
    /// Places the editor cursor under a click inside the text area.
    fn click_text(&mut self, x: i32, y: i32) {
        let area = self.text_rect();
        let line = self.editor.top + ((y - area.y) / TEXT_ROW).max(0) as usize;
        let line = line.min(self.editor.lines() - 1);
        let text = self.editor.line(line).unwrap_or("");
        let mut column = 0;
        let mut position = area.x + 8 - self.editor_shift(line);
        for ch in text.chars() {
            let mut buffer = [0u8; 4];
            let w = Canvas::measure(ch.encode_utf8(&mut buffer));
            if position + w / 2 > x {
                break;
            }
            position += w;
            column += 1;
        }
        self.editor.go_to(line, column);
        self.editor_changed();
    }
    /// Horizontal scroll of the editor so the cursor stays visible.
    pub fn editor_shift(&self, _line: usize) -> i32 {
        let (line, column) = self.editor.position();
        let text = self.editor.line(line).unwrap_or("");
        let end = text
            .char_indices()
            .nth(column)
            .map_or(text.len(), |(i, _)| i);
        let x = Canvas::measure(&text[..end]);
        (x - (self.text_rect().w - 40)).max(0)
    }
    pub fn pointer(&mut self, p: Pointer, clock: u64) -> bool {
        self.cursor_x = (self.cursor_x + p.dx as i32 * self.speed).clamp(0, self.width - 1);
        self.cursor_y = (self.cursor_y + p.dy as i32 * self.speed).clamp(0, self.height - 1);
        let x = self.cursor_x;
        let y = self.cursor_y;
        let pressed = p.buttons & !self.buttons;
        self.buttons = p.buttons;
        if p.buttons & 1 == 0 {
            if let Drag::Move(id, ..) = self.drag {
                // The lifted shadow settles back.
                self.invalidate(self.area(id));
            }
            self.drag = Drag::None;
        }
        match self.drag {
            Drag::Move(id, ox, oy) => {
                let old = self.rect(id);
                let (width, height) = (self.width, self.height);
                let r = &mut self.windows[id].normal;
                r.x = bounded(x - ox, 8 - r.w + 80, width - 80);
                r.y = bounded(y - oy, 0, height - TASKBAR - 36);
                self.invalidate(Rect::new(old.x - 18, old.y - 18, old.w + 36, old.h + 66));
                self.invalidate(self.area(id));
            }
            Drag::Resize(id, sx, sy, old) => {
                let before = self.rect(id);
                let (width, height) = (self.width, self.height);
                let r = &mut self.windows[id].normal;
                let (min_w, min_h) = WINDOWS[id].minimum();
                r.w = bounded(old.w + x - sx, min_w, width - r.x - 8);
                r.h = bounded(old.h + y - sy, min_h, height - r.y - TASKBAR - 8);
                self.invalidate(Rect::new(
                    before.x - 18,
                    before.y - 18,
                    before.w + 36,
                    before.h + 66,
                ));
                self.invalidate(self.area(id));
                if id == EDITOR {
                    let rows = self.text_rows();
                    self.editor.follow(rows);
                }
            }
            Drag::None => {}
        }
        let hit = self.hit(x, y);
        if hit != self.hover {
            self.hover = hit;
            // Hover changes affect chrome, icons and menus, but ordinary motion does not redraw the scene.
            if self.menu {
                self.invalidate(self.menu_rect());
            }
            if self.mode_deadline.is_some() {
                self.invalidate(self.banner_rect());
            }
            self.invalidate(self.taskbar());
            for slot in 0..DESKTOP.len() {
                self.invalidate(self.icon(slot));
            }
            for id in 0..COUNT {
                if self.shown(id) {
                    self.invalidate(self.rect(id));
                }
            }
        }
        if p.wheel != 0 {
            match hit {
                152 => {
                    self.help_scroll = (self.help_scroll + p.wheel as i32).clamp(
                        0,
                        (crate::view::HELP_LINES as i32 - (self.rect(HELP).h - 74) / 30).max(0),
                    );
                    self.invalidate(self.rect(HELP));
                }
                153 | 700..=799 => {
                    let max = self.files.count.saturating_sub(self.visible_rows());
                    self.files.scroll =
                        bounded(self.files.scroll as i32 + p.wheel as i32, 0, max as i32) as usize;
                    self.files_changed();
                }
                850 | 155 => {
                    let max = self.editor.lines().saturating_sub(1);
                    self.editor.top =
                        bounded(self.editor.top as i32 + p.wheel as i32 * 3, 0, max as i32)
                            as usize;
                    self.editor_changed();
                }
                900 | 156 => {
                    let rows = self.console_rows();
                    self.terminal.scroll_by(-(p.wheel as i32) * 3, rows);
                    self.terminal_changed();
                }
                1000..=1050 | 157 => {
                    let visible = self.page_rect().h;
                    self.browser.scroll_by(p.wheel as i32 * 72, visible);
                    self.browser_changed();
                }
                _ => {}
            }
        }
        if pressed & 2 != 0 && hit == 0 {
            self.open_menu(x, y);
        }
        if pressed & 1 == 0 {
            return false;
        }
        if !(20..20 + DESKTOP.len() as u16).contains(&hit) {
            self.last_icon = None;
        }
        if !(700..800).contains(&hit) {
            self.last_row = None;
        }
        if self.browser.editing && hit != 1002 {
            self.browser.editing = false;
            self.browser_changed();
        }
        if self.menu && !(400..=499).contains(&hit) && hit != 1 {
            self.close_menu();
        }
        let window =
            |base: u16| (hit >= base && hit < base + COUNT as u16).then(|| (hit - base) as usize);
        match hit {
            1 => {
                if self.menu {
                    self.close_menu();
                } else {
                    self.open_menu(8, self.height);
                }
            }
            950 => {
                self.mode_deadline = None;
                self.request(Request::KeepMode);
            }
            951 => {
                self.mode_deadline = None;
                self.request(Request::RevertMode);
            }
            _ if window(10).is_some() => {
                let id = window(10).unwrap();
                if self.interactive(id) && self.active == id {
                    self.minimize(id);
                } else {
                    self.open(id);
                }
            }
            _ if (20..20 + DESKTOP.len() as u16).contains(&hit) => {
                let slot = (hit - 20) as usize;
                self.selected_icon = Some(slot);
                self.invalidate(self.icon(slot));
                if self
                    .last_icon
                    .is_some_and(|(prior, t)| prior == slot && clock.wrapping_sub(t) < 35)
                {
                    self.open(DESKTOP[slot]);
                    self.last_icon = None;
                } else {
                    self.last_icon = Some((slot, clock));
                }
            }
            _ if window(100).is_some() => {
                let id = window(100).unwrap();
                self.open(id);
                if !self.windows[id].maximized {
                    let r = self.rect(id);
                    self.drag = Drag::Move(id, x - r.x, y - r.y);
                    self.invalidate(self.area(id));
                }
            }
            _ if window(150).is_some() => self.open(window(150).unwrap()),
            _ if (200..200 + 3 * COUNT as u16).contains(&hit) => {
                let id = ((hit - 200) / 3) as usize;
                let button = (hit - 200) % 3;
                self.open(id);
                match button {
                    0 => self.minimize(id),
                    1 => self.maximize(id),
                    _ => self.close(id),
                }
            }
            _ if window(300).is_some() => {
                let id = window(300).unwrap();
                self.open(id);
                self.drag = Drag::Resize(id, x, y, self.rect(id));
            }
            _ if window(400).is_some() => {
                let id = MENU[window(400).unwrap()];
                self.close_menu();
                self.open(id);
            }
            500..=532 => {
                self.open(SETTINGS);
                let n = (hit - 500) as usize;
                match n {
                    0 | 1 => {
                        if self.light != (n == 0) {
                            self.toggle_theme();
                        }
                    }
                    2..=5 => {
                        self.accent = n - 2;
                        self.settings_dirty = true;
                        self.full();
                    }
                    6..=8 => {
                        self.speed = n as i32 - 5;
                        self.settings_dirty = true;
                        self.invalidate(self.rect(SETTINGS));
                    }
                    9 => return true,
                    10..=12 => self.set_lang(LANGS[n - 10]),
                    20..=31 => {
                        self.mode_choice = n - 20;
                        self.invalidate(self.rect(SETTINGS));
                    }
                    32 => {
                        if self.mode_choice != self.mode_current {
                            self.request(Request::SetMode(self.mode_choice));
                        }
                    }
                    _ => {}
                }
            }
            600..=604 => {
                self.open(FILES);
                self.file_tool((hit - 600) as usize);
            }
            700..=799 => {
                self.open(FILES);
                let index = self.files.scroll + (hit - 700) as usize;
                let double = self
                    .last_row
                    .is_some_and(|(prior, t)| prior == index && clock.wrapping_sub(t) < 35);
                self.select_row(index);
                if double {
                    self.last_row = None;
                    self.open_selected();
                } else {
                    self.last_row = Some((index, clock));
                }
            }
            800 => {
                self.open(EDITOR);
                self.request(Request::SaveDocument);
            }
            850 => {
                self.open(EDITOR);
                self.click_text(x, y);
            }
            900 => self.open(TERMINAL),
            1000..=1050 => {
                self.open(BROWSER);
                let was_editing = self.browser.editing;
                self.browser.editing = hit == 1002;
                if hit == 1002 && !was_editing && self.browser.address.is_empty() {
                    self.browser.address = self.browser.current;
                }
                match hit {
                    1000 => self.request(Request::Back),
                    1001 if self.browser.loading => self.request(Request::Stop),
                    1001 => self.request(Request::Reload),
                    1050 => {
                        let page = self.page_rect();
                        self.request(Request::Follow {
                            x: x - page.x,
                            y: y - page.y + self.browser.scroll,
                        });
                    }
                    _ => {}
                }
                self.browser_changed();
            }
            1100 => {
                self.open(NETWORK);
                self.request(Request::Connect);
            }
            _ => {
                self.selected_icon = None;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests;
