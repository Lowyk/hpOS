use hpos_desktop::{
    FILES, Key, Shell,
    browser::{self, Page},
    canvas::Canvas,
    files::{Entry, Name, Storage},
    view,
};
use std::{fs, io::Write, path::PathBuf};
fn main() {
    let output = PathBuf::from(std::env::args_os().nth(1).expect("output directory"));
    fs::create_dir_all(&output).unwrap();
    let status = [
        9310,
        23839,
        1509,
        1509,
        3018,
        1,
        5,
        1009,
        0,
        0,
        12,
        466,
        0,
        0,
        9310,
        1,
        0,
        120,
        3,
        1,
        1,
        57344,
        812,
        0,
        (1 << 31) | (21 << 16) | (37 << 8),
        (1 << 31) | (2026 << 16) | (10 << 8) | 12,
    ];
    let mut page = unsafe { Box::<Page>::new_zeroed().assume_init() };
    let sample = "<title>FrogFind!</title><h1>Хуйня Полнейшая ОС</h1>\
<p>Микроядро на Rust для <a href=\"/laptop\">старых ноутбуков</a>. Без GNU, без GCC и без GPL.\
 Рисует процессор, видеокарта только показывает.</p><h3>Что умеет</h3><ul><li>файлы и редактор\
<li>терминал и <a href=\"/izzop\">Иззоп</a><li>интернет через USB-модем телефона</ul><hr>\
<blockquote>Сделано двумя долбаёбами. С любовью.</blockquote><pre>hpOS:/$ ping ya.ru\n\
Ответ от 77.88.55.242: 41 мс</pre><p>Plain HTTP pages, FrogFind search &amp; reader mode.</p>";
    let entries = [
        ("Документы", true, 0),
        ("Система", true, 0),
        ("Картинки", true, 0),
        ("Привет.txt", false, 131),
        ("заметки про ратки.txt", false, 5321),
        ("план hpOS.txt", false, 72_000),
    ]
    .map(|(name, directory, size)| Entry {
        id: 0,
        directory,
        size,
        name: Name::new(name),
    });
    for (step, name, status) in [
        (2, "boot", "Память в порядке, запуск служб..."),
        (3, "boot-error", "INVALID BIOS MEMORY MAP"),
    ] {
        let (width, height) = (1024, 768);
        let mut pixels = vec![0u32; width * height];
        let canvas = unsafe { Canvas::new(pixels.as_mut_ptr(), width, height, width).unwrap() };
        view::boot_screen(&canvas, step, status);
        let mut file = fs::File::create(output.join(format!("{name}.ppm"))).unwrap();
        write!(file, "P6\n{width} {height}\n255\n").unwrap();
        let bytes: Vec<u8> = pixels
            .iter()
            .flat_map(|p| [(*p >> 16) as u8, (*p >> 8) as u8, *p as u8])
            .collect();
        file.write_all(&bytes).unwrap();
    }
    for (width, height) in [(800, 600), (1024, 768), (1440, 900), (1920, 1080)] {
        for light in [false, true] {
            for name in [
                "system", "settings", "windows", "menu", "files", "rename", "about", "desktop",
                "editor", "terminal", "english", "banner", "browser", "network",
            ] {
                let mut pixels = vec![0u32; width * height];
                let canvas =
                    unsafe { Canvas::new(pixels.as_mut_ptr(), width, height, width).unwrap() };
                let mut shell = Shell::new(width as i32, height as i32);
                shell.light = light;
                match name {
                    "settings" => shell.open(1),
                    "windows" => {
                        shell.open(1);
                        shell.open(2);
                    }
                    "menu" => {
                        shell.key(Key::Menu);
                        shell.selected = 1;
                    }
                    "about" => shell.open(hpos_desktop::model::ABOUT),
                    "editor" => {
                        shell.editor.load(
                            Some((4, Name::new("план hpOS.txt"))),
                            "План hpOS\n\n1. Файлы - готово\n2. Редактор - вот он\n3. Терминал и Иззоп\n4. Интернет через RNDIS".as_bytes(),
                        );
                        for _ in 0..3 {
                            shell.key(Key::Down);
                        }
                        shell.open(hpos_desktop::model::EDITOR);
                        for ch in " ура".chars() {
                            shell.key(Key::End);
                            shell.key(Key::Text {
                                ch,
                                scan: 0x10,
                                repeat: false,
                            });
                        }
                    }
                    "terminal" => {
                        use hpos_desktop::terminal::Tone;
                        shell.open(hpos_desktop::model::TERMINAL);
                        let t = &mut shell.terminal;
                        t.print("Хуйня Полнейшая ОС", Tone::Prompt);
                        t.print("Наберите help, чтобы увидеть команды.", Tone::Normal);
                        t.print("hpOS:/$ ls", Tone::Prompt);
                        t.print("  Документы/\n  Система/", Tone::Normal);
                        t.print("hpOS:/$ izzop", Tone::Prompt);
                        t.print(
                            "Иззоп на связи. Пишите что угодно, exit - выйти.",
                            Tone::Prompt,
                        );
                        t.print(
                            "Иззоп: пельмень был назначен временным директором дождя",
                            Tone::Izzop,
                        );
                        t.print("ты> как дела", Tone::Prompt);
                        t.print("Иззоп: чайник подозрительно изучил вторник", Tone::Izzop);
                        t.set_prompt("ты> ");
                        t.mode = hpos_desktop::terminal::Mode::Izzop;
                        for ch in "ты кто вообще".chars() {
                            t.type_char(ch);
                        }
                    }
                    "english" => {
                        shell.set_lang(hpos_desktop::lang::Lang::En);
                        shell.open(hpos_desktop::model::SETTINGS);
                        shell.mode_count = 4;
                        for (n, (w, h)) in [(1280, 800), (1024, 768), (800, 600), (640, 480)]
                            .iter()
                            .enumerate()
                        {
                            shell.modes[n] = hpos_desktop::model::DisplayMode {
                                width: *w,
                                height: *h,
                            };
                        }
                        shell.mode_choice = 1;
                    }
                    "banner" => {
                        shell.open(hpos_desktop::model::SETTINGS);
                        shell.mode_deadline = Some(10_000 + 1200);
                    }
                    "desktop" => {
                        shell.key(Key::Escape);
                    }
                    "browser" => {
                        shell.open(hpos_desktop::model::BROWSER);
                        shell.browser.editing = false;
                        shell.browser.has_page = true;
                        shell.browser.current =
                            hpos_desktop::web::Url::new("http://frogfind.com/read.php?a=hpos");
                        browser::load(&mut page, sample.as_bytes(), browser::charset::UTF8, true);
                        browser::layout(&mut page, shell.page_width());
                        shell.browser.page_height = page.height;
                        shell.browser.hover_link = Some(0);
                    }
                    "network" => {
                        use hpos_service_protocol::net::{Event, event, stage};
                        shell.open(hpos_desktop::model::NETWORK);
                        let n = &mut shell.net;
                        n.stage = stage::ONLINE;
                        n.vendor = 0x12d1;
                        n.product = 0x108a;
                        n.mac = [0x02, 0x1e, 0x10, 0x1f, 0x00, 0x00];
                        n.address = [192, 168, 42, 23];
                        n.gateway = [192, 168, 42, 129];
                        n.dns = [192, 168, 42, 129];
                        n.received = 1832;
                        n.sent = 940;
                        let ip = |a: [u8; 4]| u32::from_be_bytes(a);
                        let list = [
                            Event { code: event::CONTROLLER, a: 0x27cc_8086, b: 8, c: 0 },
                            Event { code: event::HANDOFF, a: 1, b: 0, c: 0 },
                            Event { code: event::PORT, a: 3, b: 0, c: 0 },
                            Event { code: event::DEVICE, a: 0x12d1, b: 0x108a, c: 0 },
                            Event { code: event::RNDIS, a: 1, b: 0, c: 1 },
                            Event { code: event::LINK_UP, a: 0x021e_101f, b: 0, c: 0 },
                            Event { code: event::DHCP_DISCOVER, a: 0, b: 0, c: 0 },
                            Event {
                                code: event::LEASE,
                                a: ip([192, 168, 42, 23]),
                                b: ip([192, 168, 42, 129]),
                                c: ip([192, 168, 42, 129]),
                            },
                            Event { code: event::PONG, a: ip([77, 88, 55, 242]), b: 41, c: 1 },
                        ];
                        n.events[..list.len()].copy_from_slice(&list);
                        n.event_count = list.len();
                    }
                    "files" | "rename" => {
                        let mut list = entries;
                        for (i, e) in list.iter_mut().enumerate() {
                            e.id = i as u16 + 1;
                        }
                        shell.files.storage = Storage::Ready {
                            free_kib: 27_400,
                            total_kib: 28_672,
                        };
                        shell.open(FILES);
                        shell.files.enter(1, Name::new("Документы"));
                        shell.files.set_entries(&list, false, Some(4));
                        if name == "rename" {
                            shell.files.message = "";
                            let r = shell.tool(3);
                            shell.cursor_x = r.x + 10;
                            shell.cursor_y = r.y + 10;
                            for buttons in [1, 0] {
                                shell.pointer(
                                    hpos_desktop::pointer::Pointer {
                                        dx: 0,
                                        dy: 0,
                                        wheel: 0,
                                        buttons,
                                    },
                                    1,
                                );
                            }
                            for ch in " v2".chars() {
                                shell.key(Key::Text {
                                    ch,
                                    scan: 0x10,
                                    repeat: false,
                                });
                            }
                        } else {
                            shell.files.message = "Файл создан";
                        }
                    }
                    _ => {}
                }
                shell.animate(10_000);
                view::draw(&canvas, &shell, &page, &status);
                view::cursor(&canvas, &shell);
                let name = format!(
                    "{}-{}-{}",
                    width,
                    if light { "light" } else { "dark" },
                    name
                );
                let mut file = fs::File::create(output.join(format!("{name}.ppm"))).unwrap();
                write!(file, "P6\n{width} {height}\n255\n").unwrap();
                let bytes: Vec<u8> = pixels
                    .iter()
                    .flat_map(|p| [(*p >> 16) as u8, (*p >> 8) as u8, *p as u8])
                    .collect();
                file.write_all(&bytes).unwrap();
            }
        }
    }
}
