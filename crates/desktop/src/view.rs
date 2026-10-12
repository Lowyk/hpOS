use crate::{
    Cursor, FILES, Rect, Shell, WINDOWS,
    browser::{self, Page},
    canvas::Canvas,
    files::{EditMode, Storage},
    font::Font,
    lang::{LANGS, Lang},
    model::{
        ABOUT, BROWSER, DESKTOP, EDITOR, MENU, NETWORK, ROW, SETTINGS, SYSTEM, TASKBAR, TERMINAL,
        TEXT_ROW,
    },
    terminal::{Mode, Tone},
    web::{self, Line},
};
use hpos_service_protocol::{net::stage, status};
pub const NAME: &str = "Хуйня Полнейшая ОС";
pub const VERSION: &str = "0.10";
pub const ACCENTS: [u32; 4] = [0x8b5cf6, 0x3b82f6, 0x10b981, 0xf43f5e];
struct Palette {
    top: u32,
    bottom: u32,
    panel: u32,
    bar: u32,
    text: u32,
    muted: u32,
    line: u32,
    taskbar: u32,
    shadow: u32,
    accent: u32,
}
fn palette(s: &Shell) -> Palette {
    let accent = ACCENTS[s.accent];
    if s.light {
        Palette {
            top: 0xdce6f4,
            bottom: 0xf0e8f7,
            panel: 0xffffff,
            bar: 0xf1f3f8,
            text: 0x1b2131,
            muted: 0x5d6880,
            line: 0xdbe0ea,
            taskbar: 0xeef1f6,
            shadow: 0x1b2440,
            accent,
        }
    } else {
        Palette {
            top: 0x0a1020,
            bottom: 0x1c1436,
            panel: 0x1e2536,
            bar: 0x283146,
            text: 0xeef2fb,
            muted: 0x9ba8c3,
            line: 0x364158,
            taskbar: 0x101625,
            shadow: 0x000000,
            accent,
        }
    }
}
fn text(c: &Canvas, x: i32, y: i32, s: &str, color: u32) {
    c.text(x as usize, y as usize, s, color, 2);
}
fn number(c: &Canvas, x: i32, y: i32, value: u64, color: u32) {
    c.number(x as usize, y as usize, value, color);
}
/// Maps a rectangle from the 54-unit icon grid onto `r`.
fn unit(r: Rect, x: i32, y: i32, w: i32, h: i32) -> Rect {
    let k = r.w;
    Rect::new(
        r.x + x * k / 54,
        r.y + y * k / 54,
        (w * k / 54).max(1),
        (h * k / 54).max(1),
    )
}
fn icon(c: &Canvas, r: Rect, id: usize, color: u32) {
    match id {
        0 => {
            c.fill(unit(r, 12, 12, 30, 4), color);
            for i in 0..3 {
                c.fill(unit(r, 12, 22 + i * 9, 8 + i * 11, 4), color);
            }
        }
        1 => {
            for i in 0..3 {
                c.fill(unit(r, 12, 15 + i * 11, 30, 3), color);
                c.fill(unit(r, 17 + (i % 2) * 14, 12 + i * 11, 6, 9), color);
            }
        }
        2 => {
            c.fill(unit(r, 24, 11, 6, 21), color);
            c.fill(unit(r, 24, 37, 6, 6), color);
        }
        3 => folder(c, unit(r, 11, 15, 32, 25), color, color),
        5 => {
            for i in 0..4 {
                c.fill(
                    unit(r, 12, 13 + i * 8, if i == 3 { 16 } else { 30 }, 3),
                    color,
                );
            }
            c.fill(unit(r, 32, 34, 12, 4), color);
            c.fill(unit(r, 40, 30, 4, 12), color);
        }
        6 => {
            for i in 0..6 {
                c.fill(unit(r, 12 + i * 2, 15 + i * 2, 4, 4), color);
                c.fill(unit(r, 12 + i * 2, 33 - i * 2, 4, 4), color);
            }
            c.fill(unit(r, 27, 36, 15, 4), color);
        }
        7 => {
            // A globe: rim, equator and meridian.
            for (x, y, w, h) in [
                (19, 10, 16, 3),
                (19, 41, 16, 3),
                (11, 19, 3, 16),
                (40, 19, 3, 16),
                (14, 13, 5, 6),
                (35, 13, 5, 6),
                (14, 35, 5, 6),
                (35, 35, 5, 6),
                (12, 26, 30, 2),
                (26, 12, 2, 30),
                (19, 18, 3, 18),
                (32, 18, 3, 18),
            ] {
                c.fill(unit(r, x, y, w, h), color);
            }
        }
        8 => {
            // Signal bars.
            for i in 0..4 {
                c.fill(unit(r, 12 + i * 8, 36 - i * 7, 5, 6 + i * 7), color);
            }
        }
        _ => {
            c.fill(unit(r, 24, 11, 6, 6), color);
            c.fill(unit(r, 24, 21, 6, 22), color);
            c.fill(unit(r, 19, 21, 8, 4), color);
            c.fill(unit(r, 19, 39, 16, 4), color);
        }
    }
}
/// The hpOS mark: «хп» in white on an accent square.
fn logo(c: &Canvas, r: Rect, accent: u32) {
    c.fill(r, accent);
    c.fill(
        Rect::new(r.x, r.bottom() - (r.h / 12).max(2), r.w, (r.h / 12).max(2)),
        Canvas::mix(accent, 0x000000, 60),
    );
    let font = if r.w >= 56 {
        Font::Display
    } else if r.w >= 30 {
        Font::Title
    } else {
        Font::Interface
    };
    let (_, cell) = font.cell();
    let w = Canvas::measure_with(font, "хп");
    // Lowercase letters sit low in the cell; lift them to the optical centre.
    let lift = cell as i32 * 3 / 20;
    c.clipped(r).label(
        font,
        r.x + (r.w - w) / 2,
        r.y + (r.h - cell as i32) / 2 - lift,
        "хп",
        0xffffff,
    );
}
/// Soft drop shadow: stacked translucent rings around `r`, never under it.
fn shadow(c: &Canvas, r: Rect, color: u32, depth: i32) {
    for d in 1..=depth {
        let outer = Rect::new(r.x - d, r.y - d + depth / 2, r.w + 2 * d, r.h + 2 * d);
        for edge in [
            Rect::new(outer.x, outer.y, outer.w, r.y - outer.y),
            Rect::new(outer.x, r.bottom(), outer.w, outer.bottom() - r.bottom()),
            Rect::new(
                outer.x,
                r.y.max(outer.y),
                r.x - outer.x,
                r.bottom() - r.y.max(outer.y),
            ),
            Rect::new(
                r.right(),
                r.y.max(outer.y),
                outer.right() - r.right(),
                r.bottom() - r.y.max(outer.y),
            ),
        ] {
            c.shade(edge, color, 1);
        }
    }
}
fn folder(c: &Canvas, r: Rect, tab: u32, body: u32) {
    c.fill(Rect::new(r.x, r.y, r.w * 2 / 5, 4), tab);
    c.fill(Rect::new(r.x, r.y + 4, r.w, r.h - 4), body);
}
fn document(c: &Canvas, r: Rect, paper: u32, ink: u32) {
    c.fill(r, paper);
    for i in 0..3 {
        c.fill(Rect::new(r.x + 3, r.y + 5 + i * 5, r.w - 6, 2), ink);
    }
}
fn button(c: &Canvas, r: Rect, label: &str, selected: bool, hover: bool, p: &Palette) {
    c.fill(
        r,
        if selected {
            p.accent
        } else if hover {
            p.line
        } else {
            p.bar
        },
    );
    text(
        c,
        r.x + 12,
        r.y + 6,
        label,
        if selected { 0xffffff } else { p.text },
    );
}
fn disk_status(shell: &Shell, s: &[u64; status::COUNT]) -> &'static str {
    shell.t(match s[status::DISK_STATE] {
        1 => ["ДИСК OK", "DISK OK", "ДИСК OK"],
        2 => ["ОШИБКА ДИСКА", "DISK ERROR", "ПОМИЛКА ДИСКА"],
        _ => ["ДИСК НЕДОСТУПЕН", "NO DISK", "ДИСК НЕДОСТУПНИЙ"],
    })
}
fn system(c: &Canvas, shell: &Shell, p: &Palette, s: &[u64; status::COUNT], r: Rect) {
    let t = |text| shell.t(text);
    c.label(
        Font::Title,
        r.x + 24,
        r.y + 48,
        t([
            "Микроядро и службы",
            "Microkernel and services",
            "Мікроядро і служби",
        ]),
        p.text,
    );
    text(
        c,
        r.x + 24,
        r.y + 84,
        if s[9] != 0 {
            "SERVICE ERROR"
        } else if s[2] > 0 && s[3] > 0 {
            "IPC OK"
        } else {
            "STARTING"
        },
        if s[9] != 0 { 0xf17989 } else { p.accent },
    );
    text(
        c,
        r.x + r.w / 2,
        r.y + 84,
        if s[5] != 0 { "ISOLATION OK" } else { "PROBING" },
        p.accent,
    );
    let half = (r.w - 60) / 2;
    for (i, label, value) in [
        (0, ["Ping", "Ping", "Ping"], s[2]),
        (1, ["Pong", "Pong", "Pong"], s[3]),
        (2, ["Сообщения", "Messages", "Повідомлення"], s[4]),
        (3, ["Переключения", "Switches", "Перемикання"], s[1]),
        (4, ["Свободно, МиБ", "Free, MiB", "Вільно, МіБ"], s[7]),
        (
            5,
            ["Проверки памяти", "Memory checks", "Перевірки пам'яті"],
            s[6],
        ),
        (6, ["Клавиши", "Keys", "Клавіші"], s[10]),
        (7, ["Пакеты мыши", "Mouse packets", "Пакети миші"], s[17]),
    ] {
        let x = r.x + 24 + (i % 2) * (half + 12);
        let y = r.y + 118 + (i / 2) * 48;
        c.fill(Rect::new(x, y, half, 42), p.bar);
        text(c, x + 12, y + 2, t(label), p.muted);
        number(c, x + 12, y + 20, value, p.text);
    }
    text(
        c,
        r.x + 24,
        r.y + 322,
        if s[6] > 0 {
            "MEMORY OK"
        } else {
            "CHECK PENDING"
        },
        p.muted,
    );
    text(
        c,
        r.x + half + 36,
        r.y + 322,
        if s[8] != 0 {
            "NX AVAILABLE"
        } else {
            "NX NOT AVAILABLE"
        },
        p.muted,
    );
    text(
        c,
        r.x + 24,
        r.y + 352,
        if s[12] != 0 || s[16] != 0 {
            "INPUT QUEUE OVERFLOW"
        } else if s[13] != 0 {
            "PROCESS FAULT"
        } else if s[14] == 0 {
            "INPUT STARTING"
        } else {
            "INPUT OK / DISPLAY IN RING 3"
        },
        p.muted,
    );
    let frames = t(["Кадры", "Frames", "Кадри"]);
    text(c, r.x + 24, r.y + 382, frames, p.muted);
    number(
        c,
        r.x + 34 + Canvas::measure(frames),
        r.y + 382,
        s[11],
        p.text,
    );
    let draw = t(["Отрисовка, мс", "Draw, ms", "Малювання, мс"]);
    text(c, r.x + 200, r.y + 382, draw, p.muted);
    number(
        c,
        r.x + 210 + Canvas::measure(draw),
        r.y + 382,
        s[19] * 10,
        p.text,
    );
    text(c, r.x + 24, r.y + 412, disk_status(shell, s), p.muted);
    if s[status::DISK_STATE] != 0 {
        number(
            c,
            r.x + 200,
            r.y + 412,
            s[status::DISK_SECTORS] / 2048,
            p.text,
        );
        text(
            c,
            r.x + 236,
            r.y + 412,
            t(["МиБ, раздел HPFS", "MiB, HPFS volume", "МіБ, розділ HPFS"]),
            p.muted,
        );
    }
}
fn settings(c: &Canvas, shell: &Shell, p: &Palette, s: &[u64; status::COUNT], r: Rect) {
    let t = |text| shell.t(text);
    let heading = |y: i32, label| c.label(Font::Title, r.x + 24, y, label, p.text);
    heading(r.y + 46, t(["Оформление", "Appearance", "Вигляд"]));
    button(
        c,
        shell.setting(0),
        t(["Светлая", "Light", "Світла"]),
        shell.light,
        shell.hover == 500,
        p,
    );
    button(
        c,
        shell.setting(1),
        t(["Тёмная", "Dark", "Темна"]),
        !shell.light,
        shell.hover == 501,
        p,
    );
    heading(r.y + 122, t(["Акцент", "Accent", "Акцент"]));
    for (n, color) in ACCENTS.iter().copied().enumerate() {
        let b = shell.setting(n + 2);
        if shell.accent == n {
            c.fill(b.expanded(3), p.text);
        }
        c.fill(b, color);
    }
    heading(r.y + 194, t(["Указатель", "Pointer", "Вказівник"]));
    for (n, label) in ["1x", "2x", "3x"].iter().enumerate() {
        button(
            c,
            shell.setting(n + 6),
            label,
            shell.speed == n as i32 + 1,
            shell.hover == 506 + n as u16,
            p,
        );
    }
    heading(r.y + 266, t(["Язык", "Language", "Мова"]));
    for (n, lang) in LANGS.iter().enumerate() {
        button(
            c,
            shell.setting(n + 10),
            lang.short(),
            shell.lang == *lang,
            shell.hover == 510 + n as u16,
            p,
        );
    }
    button(
        c,
        shell.setting(9),
        t(["Проверить память", "Check memory", "Перевірити пам'ять"]),
        false,
        shell.hover == 509,
        p,
    );
    let checks = t(["Проверки:", "Checks:", "Перевірки:"]);
    text(c, r.x + 260, r.y + 362, checks, p.muted);
    number(
        c,
        r.x + 268 + Canvas::measure(checks),
        r.y + 362,
        s[6],
        p.text,
    );
    // Resolution column.
    let right = r.x + 334;
    c.label(
        Font::Title,
        right,
        r.y + 46,
        t(["Экран", "Display", "Екран"]),
        p.text,
    );
    let mut size = [0u8; 12];
    let current = mode_text(shell.width as u16, shell.height as u16, &mut size);
    text(c, right + 90, r.y + 50, current, p.muted);
    for n in 0..shell.mode_count.min(8) {
        let row = shell.setting(20 + n);
        let mode = shell.modes[n];
        let chosen = shell.mode_choice == n;
        c.fill(
            row,
            if chosen {
                Canvas::mix(p.panel, p.accent, 70)
            } else if shell.hover == 520 + n as u16 {
                p.bar
            } else {
                p.panel
            },
        );
        let mut buffer = [0u8; 12];
        text(
            c,
            row.x + 12,
            row.y + 3,
            mode_text(mode.width, mode.height, &mut buffer),
            p.text,
        );
        if n == shell.mode_current {
            let now = t(["сейчас", "current", "зараз"]);
            text(
                c,
                row.right() - 12 - Canvas::measure(now),
                row.y + 3,
                now,
                p.accent,
            );
        }
    }
    let apply = shell.setting(32);
    let enabled = shell.mode_choice != shell.mode_current;
    c.fill(apply, if enabled { p.accent } else { p.bar });
    text(
        c,
        apply.x + 14,
        apply.y + 6,
        t(["Применить", "Apply", "Застосувати"]),
        if enabled { 0xffffff } else { p.muted },
    );
    text(
        c,
        r.x + 24,
        r.bottom() - 36,
        match shell.files.storage {
            Storage::Ready { .. } => t([
                "Настройки сохраняются на диск",
                "Settings are saved to disk",
                "Налаштування зберігаються на диск",
            ]),
            Storage::Unavailable => t([
                "Диск недоступен: настройки не сохранятся",
                "No disk: settings will not be kept",
                "Немає диска: налаштування не збережуться",
            ]),
            Storage::Unknown => t([
                "Проверка диска...",
                "Checking disk...",
                "Перевірка диска...",
            ]),
        },
        p.muted,
    );
}
fn mode_text(width: u16, height: u16, out: &mut [u8; 12]) -> &str {
    let mut len = 0;
    for (i, value) in [width, height].iter().enumerate() {
        if i == 1 {
            out[len] = b'x';
            len += 1;
        }
        let mut digits = [0u8; 5];
        let mut n = 0;
        let mut v = *value;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
            if v == 0 {
                break;
            }
        }
        for d in digits[..n].iter().rev() {
            out[len] = *d;
            len += 1;
        }
    }
    core::str::from_utf8(&out[..len]).unwrap_or("")
}
pub const HELP_LINES: usize = 24;
const HELP: [[&str; HELP_LINES]; 3] = [
    [
        "Добро пожаловать в Хуйню Полнейшую ОС!",
        "Двойной щелчок открывает значок.",
        "Заголовок окна можно перетащить.",
        "Правый нижний угол меняет размер.",
        "Кнопки: свернуть, развернуть, закрыть.",
        "Правая кнопка на рабочем столе - меню.",
        "F1 система, F2 помощь, F3 настройки,",
        "F4 файлы, F5 терминал, F6 редактор,",
        "F7 браузер, F8 сеть.",
        "Tab: следующее окно, Esc: закрыть.",
        "T: тема, M: проверка памяти, Space: меню.",
        "Alt+Shift: раскладка EN / RU.",
        "Файлы: Enter открывает, Backspace - вверх.",
        "Delete дважды удаляет выбранное.",
        "Редактор: Ctrl+S сохраняет файл.",
        "Терминал: help - список команд.",
        "В терминале: izzop - поболтать с Иззопом.",
        "Разрешение меняется в Настройках.",
        "Shift при загрузке: запуск без диска.",
        "Колесо мыши прокручивает списки.",
        "Меню hpOS, О системе: версия и авторы.",
        "Интернет: телефон по USB + «USB-модем».",
        "Браузер: Backspace назад, Ctrl+L адрес.",
        "Текст без точки ищется через FrogFind.",
    ],
    [
        "Welcome to Huynya Polneyshaya OS!",
        "Double-click an icon to open it.",
        "Drag a window by its title bar.",
        "The bottom-right corner resizes.",
        "Buttons: minimize, maximize, close.",
        "Right-click the desktop for the menu.",
        "F1 system, F2 help, F3 settings,",
        "F4 files, F5 terminal, F6 editor,",
        "F7 browser, F8 network.",
        "Tab: next window, Esc: close.",
        "T: theme, M: memory check, Space: menu.",
        "Alt+Shift: keyboard layout EN / RU.",
        "Files: Enter opens, Backspace goes up.",
        "Delete twice removes the selection.",
        "Editor: Ctrl+S saves the file.",
        "Terminal: help lists the commands.",
        "In the terminal, izzop starts a chat.",
        "Change the resolution in Settings.",
        "Hold Shift at boot to skip the disk.",
        "The mouse wheel scrolls lists.",
        "hpOS menu, About: version and authors.",
        "Internet: phone over USB + tethering.",
        "Browser: Backspace back, Ctrl+L address.",
        "Text without a dot searches FrogFind.",
    ],
    [
        "Ласкаво просимо: Хуйня Полнейшая ОС!",
        "Подвійне клацання відкриває значок.",
        "Вікно можна тягнути за заголовок.",
        "Правий нижній кут змінює розмір.",
        "Кнопки: згорнути, розгорнути, закрити.",
        "Права кнопка на робочому столі - меню.",
        "F1 система, F2 довідка, F3 налаштування,",
        "F4 файли, F5 термінал, F6 редактор,",
        "F7 браузер, F8 мережа.",
        "Tab: наступне вікно, Esc: закрити.",
        "T: тема, M: перевірка пам'яті, Space: меню.",
        "Alt+Shift: розкладка EN / RU.",
        "Файли: Enter відкриває, Backspace - вгору.",
        "Delete двічі видаляє вибране.",
        "Редактор: Ctrl+S зберігає файл.",
        "Термінал: help - список команд.",
        "У терміналі izzop - балачки з Іззопом.",
        "Роздільність змінюється в Налаштуваннях.",
        "Shift під час завантаження: без диска.",
        "Коліщатко миші гортає списки.",
        "Меню hpOS, Про систему: версія й автори.",
        "Інтернет: телефон по USB + «USB-модем».",
        "Браузер: Backspace назад, Ctrl+L адреса.",
        "Текст без крапки шукається у FrogFind.",
    ],
];
fn help(c: &Canvas, shell: &Shell, p: &Palette, r: Rect) {
    let clip = c.clipped(Rect::new(r.x + 18, r.y + 46, r.w - 36, r.h - 66));
    for (line, s) in HELP[shell.lang as usize]
        .iter()
        .skip(shell.help_scroll as usize)
        .take(((r.h - 74) / 30) as usize)
        .enumerate()
    {
        text(
            &clip,
            r.x + 24,
            r.y + 52 + line as i32 * 30,
            s,
            if line == 0 && shell.help_scroll == 0 {
                p.accent
            } else {
                p.text
            },
        );
    }
    let h = r.h - 76;
    c.fill(Rect::new(r.right() - 12, r.y + 46, 4, h), p.bar);
    c.fill(
        Rect::new(
            r.right() - 12,
            r.y + 46 + shell.help_scroll * h / HELP_LINES as i32,
            4,
            (h * 8 / HELP_LINES as i32).max(18),
        ),
        p.accent,
    );
}
fn size_text<'a>(bytes: u32, units: [&str; 3], out: &'a mut [u8; 24]) -> &'a str {
    let (value, unit) = if bytes < 1024 {
        (bytes, units[0])
    } else if bytes < 1024 * 1024 {
        (bytes.div_ceil(1024), units[1])
    } else {
        (bytes.div_ceil(1024 * 1024), units[2])
    };
    let mut digits = [0u8; 10];
    let mut len = 0;
    let mut v = value;
    loop {
        digits[len] = b'0' + (v % 10) as u8;
        v /= 10;
        len += 1;
        if v == 0 {
            break;
        }
    }
    for i in 0..len {
        out[i] = digits[len - 1 - i];
    }
    out[len] = b' ';
    out[len + 1..len + 1 + unit.len()].copy_from_slice(unit.as_bytes());
    core::str::from_utf8(&out[..len + 1 + unit.len()]).unwrap_or("")
}
fn units(shell: &Shell) -> [&'static str; 3] {
    match shell.lang {
        Lang::En => ["B", "KB", "MB"],
        _ => ["Б", "КБ", "МБ"],
    }
}
fn files(c: &Canvas, shell: &Shell, p: &Palette, r: Rect) {
    let t = |text| shell.t(text);
    let f = &shell.files;
    let ready = matches!(f.storage, Storage::Ready { .. });
    let labels = [
        t(["Вверх", "Up", "Вгору"]),
        t(["+ Папка", "+ Folder", "+ Тека"]),
        t(["+ Файл", "+ File", "+ Файл"]),
        t(["Имя", "Rename", "Ім'я"]),
        t(["Удалить", "Delete", "Видалити"]),
    ];
    for (n, label) in labels.iter().enumerate() {
        let enabled = ready
            && match n {
                0 => f.parent().is_some(),
                3 | 4 => f.selected_entry().is_some(),
                _ => true,
            };
        let b = shell.tool(n);
        let armed = n == 4 && f.confirm_delete.is_some();
        c.fill(
            b,
            if armed {
                0xca5269
            } else if shell.hover == 600 + n as u16 && enabled {
                p.line
            } else {
                p.bar
            },
        );
        let ink = if armed {
            0xffffff
        } else if enabled {
            p.text
        } else {
            p.muted
        };
        c.clipped(b).label(
            Font::Interface,
            b.x + (b.w - Canvas::measure(label)) / 2,
            b.y + 6,
            label,
            ink,
        );
    }
    // Breadcrumb path; the deepest components win when space runs out.
    let path_clip = c.clipped(Rect::new(r.x + 16, r.y + 86, r.w - 32, 26));
    let disk = t(["Диск", "Disk", "Диск"]);
    let mut x = r.x + 16;
    text(&path_clip, x, r.y + 88, disk, p.accent);
    x += Canvas::measure(disk);
    for part in f.components() {
        text(&path_clip, x + 6, r.y + 88, "/", p.muted);
        x += 6 + Canvas::measure("/") + 6;
        text(&path_clip, x, r.y + 88, part, p.text);
        x += Canvas::measure(part);
    }
    if let Some(edit) = &f.edit {
        let e = shell.edit_rect();
        let label = match edit.mode {
            EditMode::NewFolder => t(["Папка:", "Folder:", "Тека:"]),
            EditMode::NewFile => t(["Файл:", "File:", "Файл:"]),
            EditMode::Rename(_) => t(["Имя:", "Name:", "Ім'я:"]),
        };
        text(c, e.x, e.y + 6, label, p.muted);
        let field = Rect::new(e.x + 70, e.y, e.w - 70, e.h);
        c.fill(field, p.accent);
        c.fill(
            Rect::new(field.x + 1, field.y + 1, field.w - 2, field.h - 2),
            p.panel,
        );
        let inner = c.clipped(Rect::new(field.x + 4, field.y, field.w - 8, field.h));
        let width = Canvas::measure(edit.text.as_str());
        let tx = field.x + 10 - (width - (field.w - 28)).max(0);
        text(&inner, tx, field.y + 6, edit.text.as_str(), p.text);
        c.fill(
            Rect::new((tx + width + 1).min(field.right() - 6), field.y + 7, 2, 20),
            p.accent,
        );
    }
    let list = shell.list_rect();
    c.fill(Rect::new(list.x, list.y - 4, list.w, 1), p.line);
    let rows = shell.visible_rows();
    let clip = c.clipped(list);
    if !ready || f.count == 0 {
        text(
            &clip,
            list.x + 16,
            list.y + 12,
            match f.storage {
                Storage::Unavailable => t([
                    "Диск hpOS не найден. Файлы недоступны.",
                    "No hpOS disk. Files are unavailable.",
                    "Диск hpOS не знайдено. Файли недоступні.",
                ]),
                Storage::Unknown => t(["Загрузка...", "Loading...", "Завантаження..."]),
                Storage::Ready { .. } => t(["Папка пуста", "Empty folder", "Тека порожня"]),
            },
            p.muted,
        );
    }
    for row in 0..rows {
        let index = f.scroll + row;
        if !ready || index >= f.count {
            break;
        }
        let entry = &f.entries[index];
        let y = list.y + row as i32 * ROW;
        let slide = shell.row_offset(row);
        let line = Rect::new(list.x, y, list.w, ROW);
        let selected = f.selected == Some(index);
        if selected {
            clip.fill(line, p.accent);
        } else if shell.hover == 700 + row as u16 {
            clip.fill(line, p.bar);
        }
        let ink = if selected { 0xffffff } else { p.text };
        let x = line.x + slide;
        if entry.directory {
            folder(&clip, Rect::new(x + 10, y + 7, 20, 16), 0xe0a83a, 0xf2c45a);
        } else {
            let (paper, ink) = if shell.light {
                (0xd9e0ec, 0x6f7b92)
            } else {
                (0xf4f6fa, 0x8d98ad)
            };
            document(&clip, Rect::new(x + 12, y + 5, 16, 20), paper, ink);
        }
        let name_clip = clip.clipped(Rect::new(line.x + 40, y, line.w - 150, ROW));
        text(&name_clip, x + 40, y + 4, entry.name.as_str(), ink);
        if !entry.directory {
            let mut buffer = [0u8; 24];
            let size = size_text(entry.size, units(shell), &mut buffer);
            text(
                &clip,
                line.right() - 12 - Canvas::measure(size) + slide,
                y + 4,
                size,
                if selected { 0xffffff } else { p.muted },
            );
        }
    }
    if f.count > rows {
        let h = list.h;
        c.fill(Rect::new(list.right() - 4, list.y, 4, h), p.bar);
        let thumb = (h * rows as i32 / f.count as i32).max(18);
        let top = list.y + (h - thumb) * f.scroll as i32 / (f.count - rows) as i32;
        c.fill(Rect::new(list.right() - 4, top, 4, thumb), p.accent);
    }
    let footer = r.bottom() - 34;
    c.fill(Rect::new(r.x + 12, footer - 6, r.w - 24, 1), p.line);
    let message = if !f.message.is_empty() {
        f.message
    } else if f.truncated {
        t([
            "Показаны первые 128 элементов",
            "Showing the first 128 items",
            "Показано перші 128 елементів",
        ])
    } else {
        ""
    };
    let footer_clip = c.clipped(Rect::new(r.x + 16, footer, r.w - 200, 26));
    text(&footer_clip, r.x + 16, footer, message, p.muted);
    if let Storage::Ready { free_kib, .. } = f.storage {
        let mut buffer = [0u8; 24];
        let free = size_text(
            (free_kib.min(u32::MAX as u64 / 1024) * 1024) as u32,
            units(shell),
            &mut buffer,
        );
        let label = t(["Свободно ", "Free ", "Вільно "]);
        let x = r.right() - 16 - Canvas::measure(free);
        text(c, x - Canvas::measure(label), footer, label, p.muted);
        text(c, x, footer, free, p.text);
    }
}
fn editor(c: &Canvas, shell: &Shell, p: &Palette, r: Rect) {
    let t = |text| shell.t(text);
    let e = &shell.editor;
    let save = shell.save_button();
    let enabled = e.file.is_some();
    c.fill(
        save,
        if enabled && e.dirty {
            p.accent
        } else if shell.hover == 800 {
            p.line
        } else {
            p.bar
        },
    );
    let label = t(["Сохранить", "Save", "Зберегти"]);
    text(
        c,
        save.x + (save.w - Canvas::measure(label)) / 2,
        save.y + 6,
        label,
        if enabled && e.dirty {
            0xffffff
        } else if enabled {
            p.text
        } else {
            p.muted
        },
    );
    let title = c.clipped(Rect::new(
        save.right() + 16,
        r.y + 40,
        r.right() - save.right() - 32,
        44,
    ));
    match &e.file {
        Some((_, name)) => {
            title.label(
                Font::Title,
                save.right() + 16,
                r.y + 49,
                name.as_str(),
                p.text,
            );
            if e.dirty {
                let w = Canvas::measure_with(Font::Title, name.as_str());
                title.label(Font::Title, save.right() + 22 + w, r.y + 49, "*", p.accent);
            }
        }
        None => text(
            &title,
            save.right() + 16,
            r.y + 52,
            t([
                "Нет открытого файла",
                "No file open",
                "Немає відкритого файлу",
            ]),
            p.muted,
        ),
    }
    let area = shell.text_rect();
    let paper = if shell.light { 0xfbfbfd } else { 0x171d2b };
    c.fill(area, paper);
    c.fill(Rect::new(area.x, area.y, area.w, 1), p.line);
    let clip = c.clipped(Rect::new(area.x + 4, area.y, area.w - 8, area.h));
    if e.file.is_none() {
        text(
            &clip,
            area.x + 16,
            area.y + 16,
            t([
                "Откройте текстовый файл в «Файлах» (Enter или двойной щелчок)",
                "Open a text file from Files (Enter or double-click)",
                "Відкрийте текстовий файл у «Файлах» (Enter або подвійне клацання)",
            ]),
            p.muted,
        );
    }
    let shift = shell.editor_shift(0);
    let (cursor_line, cursor_column) = e.position();
    for row in 0..shell.text_rows() {
        let line = e.top + row;
        let Some(content) = e.line(line) else {
            break;
        };
        let y = area.y + 2 + row as i32 * TEXT_ROW;
        if line == cursor_line && e.file.is_some() {
            clip.fill(
                Rect::new(area.x, y, area.w, TEXT_ROW),
                Canvas::mix(paper, p.accent, 18),
            );
            let end = content
                .char_indices()
                .nth(cursor_column)
                .map_or(content.len(), |(i, _)| i);
            let cx = area.x + 8 - shift + Canvas::measure(&content[..end]);
            clip.fill(Rect::new(cx, y + 2, 2, TEXT_ROW - 4), p.accent);
        }
        text(&clip, area.x + 8 - shift, y, content, p.text);
    }
    let footer = r.bottom() - 32;
    let mut position = [0u8; 48];
    let mut len = 0;
    for (label, value) in [
        (t(["Строка ", "Line ", "Рядок "]), cursor_line + 1),
        (
            t([", столбец ", ", column ", ", стовпець "]),
            cursor_column + 1,
        ),
    ] {
        position[len..len + label.len()].copy_from_slice(label.as_bytes());
        len += label.len();
        let mut digits = [0u8; 8];
        let mut n = 0;
        let mut v = value;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
            if v == 0 {
                break;
            }
        }
        for d in digits[..n].iter().rev() {
            position[len] = *d;
            len += 1;
        }
    }
    if enabled {
        text(
            c,
            r.x + 16,
            footer,
            core::str::from_utf8(&position[..len]).unwrap_or(""),
            p.muted,
        );
    }
    let message = c.clipped(Rect::new(r.x + 240, footer, r.w - 256, 26));
    text(
        &message,
        r.x + 240,
        footer,
        e.message,
        if e.confirm_close { 0xf43f5e } else { p.muted },
    );
}
fn console(c: &Canvas, shell: &Shell, p: &Palette, _r: Rect) {
    let area = shell.console_rect();
    let background = 0x0b0e16;
    c.fill(area, background);
    let term = &shell.terminal;
    let clip = c.clipped(Rect::new(area.x + 8, area.y + 4, area.w - 16, area.h - 8));
    let rows = shell.console_rows();
    let first = term.count().saturating_sub(rows + term.scroll);
    for row in 0..rows {
        let Some((line, tone)) = term.line(first + row) else {
            break;
        };
        let color = match tone {
            Tone::Normal => 0xd8dee9,
            Tone::Prompt => Canvas::mix(0xd8dee9, p.accent, 150),
            Tone::Error => 0xf87171,
            Tone::Izzop => 0xf2c45a,
        };
        text(
            &clip,
            area.x + 10,
            area.y + 6 + row as i32 * 22,
            line,
            color,
        );
    }
    let y = area.y + 6 + rows as i32 * 22;
    let prompt = term.prompt();
    let prompt_color = if term.mode == Mode::Izzop {
        0xf2c45a
    } else {
        Canvas::mix(0xd8dee9, p.accent, 150)
    };
    text(&clip, area.x + 10, y, prompt, prompt_color);
    let x = area.x + 10 + Canvas::measure(prompt);
    let input = term.input();
    let width = Canvas::measure(input);
    let shift = (x + width - (area.right() - 30)).max(0);
    let line = clip.clipped(Rect::new(x, y, area.right() - x, 22));
    text(&line, x - shift, y, input, 0xffffff);
    if shell.active == TERMINAL {
        line.fill(Rect::new(x - shift + width + 1, y + 3, 9, 17), p.accent);
    }
    if term.scroll > 0 {
        let label = shell.t(["PgDn: вниз", "PgDn: down", "PgDn: вниз"]);
        text(
            c,
            area.right() - 20 - Canvas::measure(label),
            area.y + 6,
            label,
            p.muted,
        );
    }
}
fn about(c: &Canvas, shell: &Shell, p: &Palette, r: Rect) {
    let t = |text| shell.t(text);
    logo(c, Rect::new(r.x + 28, r.y + 60, 88, 88), p.accent);
    c.label(Font::Display, r.x + 140, r.y + 58, NAME, p.text);
    c.label(Font::Title, r.x + 142, r.y + 108, "hpOS", p.accent);
    c.label(
        Font::Title,
        r.x + 142 + Canvas::measure_with(Font::Title, "hpOS "),
        r.y + 108,
        VERSION,
        p.muted,
    );
    for (n, line) in [
        t([
            "Авторы: Lowyk и tixcon",
            "Authors: Lowyk and tixcon",
            "Автори: Lowyk і tixcon",
        ]),
        t([
            "Микроядро на Rust, x86_64, загрузка Legacy BIOS",
            "Rust microkernel, x86_64, Legacy BIOS boot",
            "Мікроядро на Rust, x86_64, Legacy BIOS",
        ]),
        t([
            "Ни GNU, ни GCC, ни GPL. Вообще.",
            "No GNU, no GCC, no GPL. At all.",
            "Ні GNU, ні GCC, ні GPL. Взагалі.",
        ]),
        t([
            "Рисует процессор, видеокарта только показывает",
            "The CPU draws, the GPU only displays",
            "Малює процесор, відеокарта лише показує",
        ]),
        t([
            "Шрифт Noto Sans, лицензия SIL OFL 1.1",
            "Noto Sans font, SIL OFL 1.1 licence",
            "Шрифт Noto Sans, ліцензія SIL OFL 1.1",
        ]),
    ]
    .iter()
    .enumerate()
    {
        text(
            c,
            r.x + 28,
            r.y + 172 + n as i32 * 30,
            line,
            if n == 0 { p.text } else { p.muted },
        );
    }
    let band = Rect::new(r.x + 4, r.bottom() - 60, r.w - 8, 56);
    c.fill(band, Canvas::mix(p.panel, p.accent, 40));
    c.fill(Rect::new(band.x, band.y, 4, band.h), p.accent);
    text(
        c,
        band.x + 24,
        band.y + 16,
        t([
            "Сделано двумя долбаёбами. С любовью.",
            "Made by two absolute idiots. With love.",
            "Зроблено двома дурнями. З любов'ю.",
        ]),
        p.text,
    );
}
fn tool_button(c: &Canvas, r: Rect, label: &str, fill: u32, ink: u32) {
    c.fill(r, fill);
    c.clipped(r).label(
        Font::Interface,
        r.x + (r.w - Canvas::measure(label)) / 2,
        r.y + 6,
        label,
        ink,
    );
}
fn web_page(c: &Canvas, shell: &Shell, page: &Page, p: &Palette, r: Rect) {
    let t = |text| shell.t(text);
    let b = &shell.browser;
    let back = shell.back_button();
    let can_go_back = b.can_go_back();
    tool_button(
        c,
        back,
        t(["Назад", "Back", "Назад"]),
        if shell.hover == 1000 && can_go_back { p.line } else { p.bar },
        if can_go_back { p.text } else { p.muted },
    );
    let reload = shell.reload_button();
    tool_button(
        c,
        reload,
        if b.loading {
            t(["Стоп", "Stop", "Стоп"])
        } else {
            t(["Обновить", "Reload", "Оновити"])
        },
        if b.loading {
            Canvas::mix(p.bar, 0xf43f5e, 70)
        } else if shell.hover == 1001 {
            p.line
        } else {
            p.bar
        },
        p.text,
    );
    let field = shell.address_rect();
    c.fill(field, if b.editing { p.accent } else { p.line });
    let inner = Rect::new(field.x + 1, field.y + 1, field.w - 2, field.h - 2);
    c.fill(inner, p.panel);
    let clip = c.clipped(Rect::new(inner.x + 4, inner.y, inner.w - 8, inner.h));
    let (shown, ink) = if b.editing || !b.address.is_empty() {
        (b.address.as_str(), p.text)
    } else if !b.current.is_empty() {
        (b.current.as_str(), p.text)
    } else {
        (
            t([
                "Адрес сайта или поисковый запрос",
                "Address or search",
                "Адреса сайту або пошуковий запит",
            ]),
            p.muted,
        )
    };
    let width = Canvas::measure(shown);
    let shift = if b.editing { (width - (inner.w - 28)).max(0) } else { 0 };
    text(&clip, inner.x + 10 - shift, inner.y + 5, shown, ink);
    if b.editing {
        c.fill(
            Rect::new((inner.x + 11 - shift + width).min(inner.right() - 6), inner.y + 6, 2, 20),
            p.accent,
        );
    }
    let area = shell.page_rect();
    let paper = if shell.light { 0xffffff } else { 0x151a27 };
    c.fill(area, paper);
    c.fill(Rect::new(area.x, area.y, area.w, 1), p.line);
    if b.loading {
        // A sliding highlight on the top edge while bytes arrive.
        let w = area.w / 4;
        let x = area.x + ((shell.now as i32 * 9) % (area.w + w)) - w;
        c.clipped(area).fill(Rect::new(x, area.y, w, 3), p.accent);
    }
    let view = c.clipped(Rect::new(area.x, area.y + 1, area.w - 10, area.h - 1));
    if !b.has_page {
        let lines = [
            t(["Браузер hpOS", "hpOS Browser", "Браузер hpOS"]),
            t([
                "Введите адрес или запрос и нажмите Enter.",
                "Type an address or a query and press Enter.",
                "Введіть адресу або запит і натисніть Enter.",
            ]),
            t([
                "Запросы ищутся через FrogFind,",
                "Queries are searched with FrogFind,",
                "Запити шукаються через FrogFind,",
            ]),
            t([
                "HTTPS-сайты открываются через его читалку.",
                "HTTPS sites open through its reader.",
                "HTTPS-сайти відкриваються через його читалку.",
            ]),
            t([
                "Интернет включается в окне «Сеть» (F8).",
                "Turn the internet on in Network (F8).",
                "Інтернет вмикається у вікні «Мережа» (F8).",
            ]),
        ];
        view.label(Font::Title, area.x + 24, area.y + 24, lines[0], p.text);
        for (n, line) in lines[1..].iter().enumerate() {
            text(&view, area.x + 24, area.y + 66 + n as i32 * 28, line, p.muted);
        }
        let state = web::stage_text(shell.lang, shell.net.stage);
        text(
            &view,
            area.x + 24,
            area.y + 66 + 4 * 28 + 12,
            state,
            if shell.net.stage == stage::ONLINE { 0x10b981 } else { p.accent },
        );
    } else {
        let top = b.scroll;
        for f in browser::visible(page, top - 8, top + area.h) {
            let x = area.x + f.x;
            let y = area.y + f.y - top;
            match f.style {
                browser::style::RULE => {
                    view.fill(Rect::new(x, y + 4, browser::fragment_width(page, f), 1), p.line);
                }
                browser::style::BULLET => view.fill(Rect::new(x, y, 6, 6), p.accent),
                style => {
                    let font = if style == browser::style::TITLE { Font::Title } else { Font::Interface };
                    let link = f.link != 0;
                    let hovered = link && b.hover_link == Some(f.link as usize - 1);
                    let color = if link {
                        if hovered { Canvas::mix(p.accent, p.text, 110) } else { p.accent }
                    } else {
                        match style {
                            browser::style::SUBTITLE => Canvas::mix(p.text, p.accent, 120),
                            browser::style::QUOTE => p.muted,
                            browser::style::PRE => Canvas::mix(p.text, 0x10b981, 60),
                            _ => p.text,
                        }
                    };
                    let words = page.text_of(f.start, f.len);
                    view.label(font, x, y, words, color);
                    if link {
                        let (_, h) = font.cell();
                        let w = browser::fragment_width(page, f);
                        view.fill(Rect::new(x, y + h as i32 - 2, w, 1), color);
                    }
                }
            }
        }
        if b.page_height > area.h {
            let track = Rect::new(area.right() - 7, area.y + 4, 4, area.h - 8);
            c.fill(track, p.bar);
            let thumb = (track.h * area.h / b.page_height).max(24);
            let y = track.y + (track.h - thumb) * b.scroll / (b.page_height - area.h).max(1);
            c.fill(Rect::new(track.x, y, 4, thumb), p.accent);
        }
    }
    let footer = r.bottom() - 28;
    let footer_clip = c.clipped(Rect::new(r.x + 16, footer, r.w - 32, 26));
    let mut line = Line::new();
    let mut ink = p.muted;
    if b.loading {
        line.text(t(["Загрузка... ", "Loading... ", "Завантаження... "]))
            .number(b.progress as u64 / 1024)
            .text(t([" КБ", " KB", " КБ"]));
    } else if let Some(link) = b.hover_link {
        line.text(page.href(link));
        ink = p.accent;
    } else if b.error != 0 {
        line.text(t(["Ошибка: ", "Error: ", "Помилка: "])).text(web::error_text(shell.lang, b.error));
        ink = 0xf43f5e;
    } else if b.has_page {
        if b.status >= 400 {
            line.text("HTTP ").number(b.status as u64).text(" - ");
        }
        let title = page.title().trim();
        line.text(if title.is_empty() { b.current.as_str() } else { title });
        if page.truncated {
            line.text(t([" (страница обрезана)", " (page cut short)", " (сторінку обрізано)"]));
        }
    }
    text(&footer_clip, r.x + 16, footer, line.as_str(), ink);
}
fn network(c: &Canvas, shell: &Shell, p: &Palette, r: Rect) {
    let t = |text| shell.t(text);
    let n = &shell.net;
    let color = match n.stage {
        stage::ONLINE => 0x10b981,
        stage::FAILED | stage::NO_CONTROLLER => 0xf43f5e,
        stage::OFF => p.muted,
        _ => 0xf2c45a,
    };
    c.fill(Rect::new(r.x + 24, r.y + 62, 14, 14), color);
    c.label(Font::Title, r.x + 48, r.y + 54, web::stage_text(shell.lang, n.stage), p.text);
    let hint = c.clipped(Rect::new(r.x + 24, r.y + 92, r.w - 48, 30));
    text(&hint, r.x + 24, r.y + 94, web::stage_hint(shell.lang, n.stage), p.muted);
    let button = shell.connect_button();
    let idle = matches!(n.stage, stage::OFF | stage::FAILED | stage::NO_CONTROLLER);
    tool_button(
        c,
        button,
        if idle {
            t(["Подключить", "Connect", "Під'єднати"])
        } else {
            t(["Искать снова", "Search again", "Шукати знову"])
        },
        if shell.hover == 1100 { Canvas::mix(p.accent, 0xffffff, 40) } else { p.accent },
        0xffffff,
    );
    text(
        c,
        button.right() + 16,
        button.y + 7,
        t(["или Enter", "or Enter", "або Enter"]),
        p.muted,
    );
    let dash = "-";
    let mut rows: [(&str, Line); 6] = [
        (t(["Устройство", "Device", "Пристрій"]), Line::new()),
        ("MAC", Line::new()),
        (t(["Адрес", "Address", "Адреса"]), Line::new()),
        (t(["Шлюз", "Gateway", "Шлюз"]), Line::new()),
        ("DNS", Line::new()),
        (t(["Пакеты", "Packets", "Пакети"]), Line::new()),
    ];
    if n.vendor != 0 {
        rows[0].1.hex(n.vendor as u64, 4).text(":").hex(n.product as u64, 4);
    }
    if n.mac != [0; 6] {
        rows[1].1.mac(n.mac);
    }
    let online = n.address != [0; 4];
    if online {
        rows[2].1.ip(n.address);
        rows[3].1.ip(n.gateway);
        rows[4].1.ip(n.dns);
    }
    if n.received != 0 || n.sent != 0 {
        rows[5].1.number(n.received as u64).text(" / ").number(n.sent as u64);
    }
    let half = (r.w - 48) / 2;
    for (i, (label, value)) in rows.iter().enumerate() {
        let x = r.x + 24 + (i as i32 % 2) * half;
        let y = r.y + 184 + (i as i32 / 2) * 28;
        text(c, x, y, label, p.muted);
        let shown = if value.as_str().is_empty() { dash } else { value.as_str() };
        let cell = c.clipped(Rect::new(x + 110, y, half - 116, 26));
        text(&cell, x + 110, y, shown, p.text);
    }
    c.label(Font::Title, r.x + 24, r.y + 270, t(["Журнал", "Log", "Журнал"]), p.text);
    let log = shell.net_log_rect();
    c.fill(log, if shell.light { 0xf6f7fb } else { 0x151a27 });
    let clip = c.clipped(Rect::new(log.x + 8, log.y + 4, log.w - 16, log.h - 8));
    let rows = ((log.h - 8) / 24).max(1) as usize;
    let events = n.events();
    let first = events.len().saturating_sub(rows);
    if events.is_empty() {
        text(&clip, log.x + 12, log.y + 6, t(["Пока пусто", "Nothing yet", "Поки порожньо"]), p.muted);
    }
    for (row, e) in events[first..].iter().enumerate() {
        let mut line = Line::new();
        web::describe(shell.lang, e, &mut line);
        let failed = matches!(
            e.code,
            hpos_service_protocol::net::event::USB_FAILED
                | hpos_service_protocol::net::event::RNDIS_FAILED
                | hpos_service_protocol::net::event::CONTROLLER_FAILED
                | hpos_service_protocol::net::event::FETCH_FAILED
                | hpos_service_protocol::net::event::NOT_RNDIS
        );
        text(
            &clip,
            log.x + 12,
            log.y + 6 + row as i32 * 24,
            line.as_str(),
            if failed { 0xf87171 } else { p.text },
        );
    }
}
fn wallpaper(c: &Canvas, shell: &Shell, p: &Palette) {
    let area = c
        .clip()
        .intersect(Rect::new(0, 0, shell.width, shell.height - TASKBAR));
    let h = (shell.height - TASKBAR).max(1);
    for y in area.y..area.bottom() {
        let row = Canvas::mix(p.top, p.bottom, (y * 255 / h) as u32);
        c.fill(Rect::new(area.x, y, area.w, 1), row);
        // Light beams in the accent colour, Windows 10 hero style.
        for (start, width, alpha) in [(52, 90, 26), (66, 36, 44), (78, 140, 16)] {
            let x = shell.width * start / 100 - y * 40 / 100 + shell.height * 20 / 100;
            c.fill(Rect::new(x, y, width, 1), Canvas::mix(row, p.accent, alpha));
        }
    }
    let name = Canvas::measure_with(Font::Display, NAME);
    let tile = 64;
    let right = shell.width - 40;
    let x = right - name - 20 - tile;
    let y = shell.height - TASKBAR - 128;
    if x > 250 && y > 120 {
        logo(c, Rect::new(x, y, tile, tile), p.accent);
        c.label(
            Font::Display,
            x + tile + 20,
            y - 2,
            NAME,
            if shell.light { 0x283048 } else { 0xf4f1ff },
        );
        text(c, x + tile + 22, y + 44, "hpOS ", p.accent);
        let after = x + tile + 22 + Canvas::measure("hpOS ");
        text(c, after, y + 44, VERSION, p.muted);
        text(
            c,
            after + Canvas::measure(VERSION) + 14,
            y + 44,
            "Lowyk & tixcon",
            p.muted,
        );
    }
}
fn chrome(c: &Canvas, shell: &Shell, p: &Palette, id: usize, r: Rect) {
    let active = shell.active == id && shell.interactive(id);
    c.fill(r, p.panel);
    c.fill(
        Rect::new(r.x, r.y, r.w, 36),
        if active { p.bar } else { p.panel },
    );
    let tile = Rect::new(r.x + 12, r.y + 9, 18, 18);
    c.fill(tile, if active { p.accent } else { p.muted });
    icon(c, tile, id, 0xffffff);
    let title = c.clipped(Rect::new(r.x + 38, r.y, (r.w - 166).max(0), 36));
    title.label(
        Font::Title,
        r.x + 40,
        r.y + 4,
        WINDOWS[id].title(shell.lang),
        if active { p.text } else { p.muted },
    );
    for n in 0..3 {
        let b = Rect::new(r.right() - 120 + n * 40, r.y, 40, 36);
        let hover = shell.hover == 200 + id as u16 * 3 + n as u16;
        if hover {
            c.fill(b, if n == 2 { 0xe81123 } else { p.line });
        }
        let ink = if hover && n == 2 { 0xffffff } else { p.text };
        let x = b.x + 14;
        let y = b.y + 12;
        match n {
            0 => c.fill(Rect::new(x, y + 6, 12, 1), ink),
            1 if shell.windows[id].maximized => {
                c.fill(Rect::new(x + 3, y, 9, 1), ink);
                c.fill(Rect::new(x + 11, y, 1, 9), ink);
                for (dx, dy) in [(0, 3), (0, 11)] {
                    c.fill(Rect::new(x + dx, y + dy, 9, 1), ink);
                }
                c.fill(Rect::new(x, y + 3, 1, 9), ink);
                c.fill(Rect::new(x + 8, y + 3, 1, 9), ink);
            }
            1 => {
                c.fill(Rect::new(x, y, 12, 1), ink);
                c.fill(Rect::new(x, y + 11, 12, 1), ink);
                c.fill(Rect::new(x, y, 1, 12), ink);
                c.fill(Rect::new(x + 11, y, 1, 12), ink);
            }
            _ => {
                for k in 0..12 {
                    c.fill(Rect::new(x + k, y + k, 1, 1), ink);
                    c.fill(Rect::new(x + 11 - k, y + k, 1, 1), ink);
                }
            }
        }
    }
    let frame = if active { p.accent } else { p.line };
    c.fill(Rect::new(r.x, r.y, r.w, 1), frame);
    c.fill(Rect::new(r.x, r.bottom() - 1, r.w, 1), frame);
    c.fill(Rect::new(r.x, r.y, 1, r.h), frame);
    c.fill(Rect::new(r.right() - 1, r.y, 1, r.h), frame);
}
fn two_digits(value: u64, out: &mut [u8], at: usize) {
    out[at] = b'0' + (value / 10 % 10) as u8;
    out[at + 1] = b'0' + (value % 10) as u8;
}
fn taskbar(c: &Canvas, shell: &Shell, p: &Palette, s: &[u64; status::COUNT]) {
    let bar = shell.taskbar();
    c.fill(bar, p.taskbar);
    c.fill(Rect::new(0, bar.y, shell.width, 1), p.line);
    let start = shell.start_button();
    if shell.menu || shell.hover == 1 {
        c.fill(start, p.bar);
    }
    logo(c, Rect::new(start.x + 7, start.y + 4, 32, 32), p.accent);
    let (list, count) = shell.task_order();
    for id in list[..count].iter().copied() {
        let r = shell.task(id);
        let focused = shell.interactive(id) && shell.active == id;
        if focused {
            c.fill(r, p.bar);
        } else if shell.hover == 10 + id as u16 {
            c.fill(r, Canvas::mix(p.taskbar, p.bar, 160));
        }
        let compact = r.w < 100;
        let tile = Rect::new(
            if compact {
                r.x + (r.w - 20) / 2
            } else {
                r.x + 10
            },
            r.y + 10,
            20,
            20,
        );
        c.fill(
            tile,
            if shell.windows[id].visible {
                p.accent
            } else {
                p.muted
            },
        );
        icon(c, tile, id, 0xffffff);
        if !compact {
            let label = c.clipped(Rect::new(r.x + 36, r.y, r.w - 40, r.h));
            text(
                &label,
                r.x + 38,
                r.y + 8,
                WINDOWS[id].title(shell.lang),
                if focused { p.text } else { p.muted },
            );
        }
        if shell.windows[id].visible {
            let w = if focused { r.w } else { (r.w - 24).max(12) };
            c.fill(
                Rect::new(r.x + (r.w - w) / 2, r.bottom() - 3, w, 3),
                p.accent,
            );
        }
    }
    let clock = shell.clock_rect();
    let time = s[status::TIME];
    let date = s[status::DATE];
    let mut digits = *b"--:--";
    if time >> 31 & 1 == 1 {
        two_digits(time >> 16 & 255, &mut digits, 0);
        two_digits(time >> 8 & 255, &mut digits, 3);
    }
    c.label(
        Font::Title,
        clock.x + 52,
        clock.y + 2,
        core::str::from_utf8(&digits).unwrap_or("--:--"),
        p.text,
    );
    if date >> 31 & 1 == 1 {
        let mut d = *b"00.00.0000";
        two_digits(date & 255, &mut d, 0);
        two_digits(date >> 8 & 255, &mut d, 3);
        two_digits((date >> 16 & 0x7fff) / 100, &mut d, 6);
        two_digits((date >> 16 & 0x7fff) % 100, &mut d, 8);
        text(
            c,
            clock.x + 52,
            clock.y + 28,
            core::str::from_utf8(&d).unwrap_or(""),
            p.muted,
        );
    } else {
        text(
            c,
            clock.x + 52,
            clock.y + 28,
            shell.t(["офлайн", "offline", "офлайн"]),
            p.muted,
        );
    }
    let pill = Rect::new(clock.x + 8, clock.y + 14, 34, 24);
    c.fill(pill, p.bar);
    text(
        c,
        pill.x + 6,
        pill.y + 1,
        if shell.russian { "RU" } else { "EN" },
        p.accent,
    );
}
fn start_menu(c: &Canvas, shell: &Shell, p: &Palette) {
    let final_rect = shell.menu_rect();
    let offset = shell.menu_offset(0);
    let r = Rect::new(
        final_rect.x,
        final_rect.y + offset,
        final_rect.w,
        final_rect.h,
    );
    // Slides up from behind its own bottom edge; rows follow one tick apart.
    let c = c.clipped(Rect::new(
        final_rect.x - 12,
        final_rect.y - 12,
        final_rect.w + 24,
        final_rect.h + 12,
    ));
    shadow(&c, r, p.shadow, 10);
    c.fill(r, p.panel);
    c.fill(Rect::new(r.x, r.y, 4, r.h), p.accent);
    logo(&c, Rect::new(r.x + 20, r.y + 18, 52, 52), p.accent);
    let title = c.clipped(Rect::new(r.x + 84, r.y, r.w - 92, 80));
    title.label(Font::Title, r.x + 84, r.y + 16, NAME, p.text);
    text(&title, r.x + 85, r.y + 46, "Lowyk & tixcon", p.muted);
    c.fill(Rect::new(r.x + 16, r.y + 84, r.w - 32, 1), p.line);
    let rows = c.clipped(final_rect);
    for (n, id) in MENU.iter().copied().enumerate() {
        let row = shell.menu_row(n);
        let row = Rect::new(row.x, row.y + shell.menu_offset(n + 1), row.w, row.h);
        let hot = shell.hover == 400 + n as u16 || shell.selected == n;
        if hot {
            rows.fill(
                row,
                if shell.selected == n {
                    Canvas::mix(p.panel, p.accent, 60)
                } else {
                    p.bar
                },
            );
        }
        let tile = Rect::new(row.x + 10, row.y + 6, 28, 28);
        rows.fill(tile, p.accent);
        icon(&rows, tile, id, 0xffffff);
        rows.label(
            Font::Title,
            row.x + 50,
            row.y + 6,
            WINDOWS[id].title(shell.lang),
            p.text,
        );
    }
    text(
        &c,
        r.x + 20,
        r.bottom() - 32,
        shell.t([
            "Space / Esc: закрыть",
            "Space / Esc: close",
            "Space / Esc: закрити",
        ]),
        p.muted,
    );
}
fn banner(c: &Canvas, shell: &Shell, p: &Palette) {
    let Some(deadline) = shell.mode_deadline else {
        return;
    };
    let r = shell.banner_rect();
    shadow(c, r, p.shadow, 12);
    c.fill(r, p.panel);
    c.fill(Rect::new(r.x, r.y, r.w, 3), p.accent);
    c.label(
        Font::Title,
        r.x + 24,
        r.y + 16,
        shell.t([
            "Оставить это разрешение?",
            "Keep this resolution?",
            "Залишити цю роздільність?",
        ]),
        p.text,
    );
    let seconds = deadline.saturating_sub(shell.now).div_ceil(100);
    let label = shell.t(["Откат через", "Reverting in", "Відкат через"]);
    text(c, r.x + 24, r.y + 44, label, p.muted);
    let x = r.x + 32 + Canvas::measure(label);
    number(c, x, r.y + 44, seconds, p.text);
    let digits = if seconds >= 10 { "00" } else { "0" };
    text(
        c,
        x + Canvas::measure(digits) + 4,
        r.y + 44,
        shell.t(["с", "s", "с"]),
        p.muted,
    );
    for keep in [true, false] {
        let b = shell.banner_button(keep);
        let hover = shell.hover == if keep { 950 } else { 951 };
        c.fill(
            b,
            if keep {
                p.accent
            } else if hover {
                p.line
            } else {
                p.bar
            },
        );
        let label = if keep {
            shell.t(["Оставить (Enter)", "Keep (Enter)", "Залишити (Enter)"])
        } else {
            shell.t(["Вернуть (Esc)", "Revert (Esc)", "Повернути (Esc)"])
        };
        text(
            c,
            b.x + (b.w - Canvas::measure(label)) / 2,
            b.y + 6,
            label,
            if keep { 0xffffff } else { p.text },
        );
    }
}
pub fn draw(c: &Canvas, shell: &Shell, page: &Page, s: &[u64; status::COUNT]) {
    let p = palette(shell);
    wallpaper(c, shell, &p);
    for (slot, id) in DESKTOP.iter().copied().enumerate() {
        let r = shell.icon(slot);
        if shell.selected_icon == Some(slot) {
            c.fill(r, Canvas::mix(p.top, p.accent, 70));
        } else if shell.hover == 20 + slot as u16 {
            c.shade(r, 0xffffff, 2);
        }
        let tile = Rect::new(r.x + 27, r.y + 6, 52, 52);
        c.fill(tile, p.accent);
        icon(c, tile, id, 0xffffff);
        let label = WINDOWS[id].title(shell.lang);
        let w = Canvas::measure(label);
        let ink = if shell.light { 0x1b2131 } else { 0xf2f4fa };
        c.clipped(r).label(
            Font::Interface,
            r.x + (r.w - w).max(0) / 2,
            r.y + 64,
            label,
            ink,
        );
    }
    for id in shell.order {
        if !shell.shown(id) {
            continue;
        }
        let v = shell.visual(id);
        // While maximizing the frame and content follow the gliding rectangle.
        let r = if shell.morphing(id) {
            v
        } else {
            shell.rect(id)
        };
        let reveal = c.clipped(v.expanded(12));
        shadow(
            &reveal,
            v,
            p.shadow,
            if matches!(shell.cursor(), Cursor::Move) && shell.active == id {
                14
            } else {
                10
            },
        );
        let window = c.clipped(v);
        chrome(&window, shell, &p, id, r);
        let body = window.clipped(Rect::new(r.x + 4, r.y + 36, r.w - 8, r.h - 40));
        match id {
            SYSTEM => system(&body, shell, &p, s, r),
            SETTINGS => settings(&body, shell, &p, s, r),
            FILES => files(&body, shell, &p, r),
            ABOUT => about(&body, shell, &p, r),
            EDITOR => editor(&body, shell, &p, r),
            TERMINAL => console(&body, shell, &p, r),
            BROWSER => web_page(&body, shell, page, &p, r),
            NETWORK => network(&body, shell, &p, r),
            _ => help(&body, shell, &p, r),
        }
        if !shell.windows[id].maximized {
            for i in 0..3 {
                window.fill(
                    Rect::new(r.right() - 7 - i * 4, r.bottom() - 4, 2, 2),
                    p.muted,
                );
            }
        }
    }
    taskbar(c, shell, &p, s);
    if shell.menu {
        start_menu(c, shell, &p);
    }
    banner(c, shell, &p);
}
/// Kernel boot screen. `step` 1–2 shows progress, 3 shows `status` as a fatal error.
pub fn boot_screen(c: &Canvas, step: i32, status: &str) {
    let (w, h) = (c.width as i32, c.height as i32);
    let (accent, muted, ink, red) = (ACCENTS[0], 0x9ba8c3, 0xf4f1ff, 0xf43f5e);
    for y in 0..h {
        c.fill(
            Rect::new(0, y, w, 1),
            Canvas::mix(0x0a1020, 0x1c1436, (y * 255 / h) as u32),
        );
    }
    let center = w / 2;
    let top = h / 2 - 130;
    let tile = Rect::new(center - 40, top, 80, 80);
    c.fill(tile, accent);
    c.fill(
        Rect::new(tile.x, tile.bottom() - 6, tile.w, 6),
        Canvas::mix(accent, 0, 60),
    );
    let centered = |font: Font, y: i32, line: &str, color: u32| {
        c.label(
            font,
            center - Canvas::measure_with(font, line) / 2,
            y,
            line,
            color,
        );
    };
    centered(Font::Title, top + 20, "хп", 0xffffff);
    centered(Font::Title, top + 100, NAME, ink);
    let mut line = Line::new();
    line.text("hpOS ").text(VERSION).text(" / Lowyk & tixcon");
    centered(Font::Interface, top + 134, line.as_str(), muted);
    let failed = step >= 3;
    let track = Rect::new(center - 130, top + 190, 260, 4);
    c.fill(track, 0x283146);
    c.fill(
        Rect::new(track.x, track.y, track.w * step.clamp(0, 3) / 3, track.h),
        if failed { red } else { accent },
    );
    centered(
        Font::Interface,
        top + 206,
        status,
        if failed { red } else { muted },
    );
    if failed {
        centered(
            Font::Interface,
            top + 234,
            "Система остановлена. Подробности в COM1.",
            muted,
        );
    }
}
pub fn cursor_rect(shell: &Shell) -> Rect {
    Rect::new(shell.cursor_x, shell.cursor_y, 18, 24)
}
pub fn cursor(c: &Canvas, shell: &Shell) {
    let x = shell.cursor_x;
    let y = shell.cursor_y;
    match shell.cursor() {
        Cursor::Arrow => {
            for row in 0..19 {
                for col in 0..12 {
                    let arrow = row < 14 && col <= row / 2;
                    let stem = (12..19).contains(&row) && (4..=7).contains(&col);
                    if arrow || stem {
                        let edge = col == 0
                            || col == row / 2
                            || row == 13
                            || stem && (col == 4 || col == 7 || row == 18);
                        c.blend(x + col + 1, y + row + 2, 0, 5);
                        c.fill(
                            Rect::new(x + col, y + row, 1, 1),
                            if edge { 0x152033 } else { 0xffffff },
                        );
                    }
                }
            }
        }
        Cursor::Resize => {
            for n in 1..15 {
                for offset in -1..=1 {
                    c.fill(
                        Rect::new(x + n + offset, y + n, 1, 1),
                        if offset == 0 { 0xffffff } else { 0x152033 },
                    );
                }
            }
            for n in 0..5 {
                c.fill(Rect::new(x + 1 + n, y + 1, 1, 1), 0xffffff);
                c.fill(Rect::new(x + 1, y + 1 + n, 1, 1), 0xffffff);
                c.fill(Rect::new(x + 15 - n, y + 15, 1, 1), 0xffffff);
                c.fill(Rect::new(x + 15, y + 15 - n, 1, 1), 0xffffff);
            }
        }
        Cursor::Move => {
            c.fill(Rect::new(x + 7, y, 3, 18), 0x152033);
            c.fill(Rect::new(x, y + 7, 18, 3), 0x152033);
            c.fill(Rect::new(x + 8, y + 1, 1, 16), 0xffffff);
            c.fill(Rect::new(x + 1, y + 8, 16, 1), 0xffffff);
        }
        Cursor::Text => {
            // I-beam, outlined so it shows on light and dark text areas.
            c.fill(Rect::new(x + 2, y, 7, 3), 0x152033);
            c.fill(Rect::new(x + 2, y + 17, 7, 3), 0x152033);
            c.fill(Rect::new(x + 4, y, 3, 20), 0x152033);
            c.fill(Rect::new(x + 3, y + 1, 5, 1), 0xffffff);
            c.fill(Rect::new(x + 3, y + 18, 5, 1), 0xffffff);
            c.fill(Rect::new(x + 5, y + 1, 1, 18), 0xffffff);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::{Key, pointer::Pointer};
    use std::vec;
    #[test]
    fn damaged_region_composition_matches_full_scene_after_window_and_menu_changes() {
        let mut shell = Shell::new(800, 600);
        let status = [0u64; status::COUNT];
        let page = unsafe { std::boxed::Box::<Page>::new_zeroed().assume_init() };
        let mut partial = vec![0u32; 800 * 600];
        let mut full = vec![0u32; 800 * 600];
        let a = unsafe { Canvas::new(partial.as_mut_ptr(), 800, 600, 800).unwrap() };
        let b = unsafe { Canvas::new(full.as_mut_ptr(), 800, 600, 800).unwrap() };
        for step in 0..15 {
            match step {
                11 => {
                    shell.key(Key::Browser);
                }
                12 => {
                    shell.key(Key::Text {
                        ch: 'x',
                        scan: 0x2d,
                        repeat: false,
                    });
                }
                13 => {
                    shell.key(Key::Network);
                }
                1 => {
                    shell.open(1);
                }
                2 => {
                    shell.open(2);
                }
                3 => {
                    shell.key(Key::Menu);
                }
                4 => {
                    shell.key(Key::Down);
                }
                5 => {
                    shell.key(Key::Enter);
                }
                6 => {
                    let r = shell.rect(shell.active);
                    shell.cursor_x = r.x + 30;
                    shell.cursor_y = r.y + 15;
                    shell.pointer(
                        Pointer {
                            dx: 0,
                            dy: 0,
                            wheel: 0,
                            buttons: 1,
                        },
                        1,
                    );
                }
                7 => {
                    shell.pointer(
                        Pointer {
                            dx: 20,
                            dy: 14,
                            wheel: 0,
                            buttons: 1,
                        },
                        2,
                    );
                }
                8 => {
                    shell.pointer(
                        Pointer {
                            dx: 0,
                            dy: 0,
                            wheel: 0,
                            buttons: 0,
                        },
                        3,
                    );
                }
                9 => {
                    shell.key(Key::Text {
                        ch: 't',
                        scan: 0x14,
                        repeat: false,
                    });
                }
                10 => {
                    shell.key(Key::Escape);
                }
                _ => {}
            }
            if let Some(damage) = shell.take_damage() {
                draw(&a.clipped(damage), &shell, &page, &status);
            }
            draw(&b, &shell, &page, &status);
            if let Some(index) = partial.iter().zip(&full).position(|(a, b)| a != b) {
                panic!(
                    "partial scene diverged at step {step}, pixel ({},{}): {:x} != {:x}",
                    index % 800,
                    index / 800,
                    partial[index],
                    full[index]
                );
            }
        }
    }
}
