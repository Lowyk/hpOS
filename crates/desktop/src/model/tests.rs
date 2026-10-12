use super::*;

fn click(s: &mut Shell, x: i32, y: i32, t: u64) {
    s.cursor_x = x;
    s.cursor_y = y;
    for buttons in [1, 0] {
        s.pointer(
            Pointer {
                dx: 0,
                dy: 0,
                wheel: 0,
                buttons,
            },
            t,
        );
    }
}
fn typed(ch: char) -> Key {
    Key::Text {
        ch,
        scan: match ch {
            't' => 0x14,
            'm' => 0x32,
            ' ' => 0x39,
            _ => 0x10,
        },
        repeat: false,
    }
}
#[test]
fn focus_drag_minimize_restore_close_preserve_other_windows() {
    let mut s = Shell::new(1024, 768);
    s.open(SETTINGS);
    assert_eq!(s.order[COUNT - 1], SETTINGS);
    let r = s.rect(SETTINGS);
    s.cursor_x = r.x + 30;
    s.cursor_y = r.y + 15;
    for (dx, dy, buttons) in [(0, 0, 1), (20, 30, 1), (0, 0, 0)] {
        s.pointer(
            Pointer {
                dx,
                dy,
                wheel: 0,
                buttons,
            },
            2,
        );
    }
    assert_eq!(
        (s.rect(SETTINGS).x, s.rect(SETTINGS).y),
        (r.x + 20, r.y + 30)
    );
    let task = s.task(SETTINGS);
    click(&mut s, task.x + 20, task.y + 10, 4);
    assert!(
        !s.interactive(SETTINGS),
        "minimizing windows stop taking input at once"
    );
    assert!(s.animate(5));
    assert!(!s.animate(5 + WINDOW_TICKS));
    assert!(!s.shown(SETTINGS));
    click(&mut s, task.x + 20, task.y + 10, 30);
    assert!(s.shown(SETTINGS));
    let r = s.rect(SETTINGS);
    click(&mut s, r.right() - 20, r.y + 15, 31);
    assert!(s.windows[SETTINGS].visible);
    s.animate(100);
    assert!(!s.windows[SETTINGS].visible);
    assert!(s.shown(SYSTEM));
}
#[test]
fn menu_keys_and_extreme_pointer_stay_bounded() {
    let mut s = Shell::new(800, 600);
    s.key(Key::Menu);
    s.key(Key::Down);
    s.key(Key::Enter);
    assert!(s.shown(MENU[1]));
    s.key(Key::Escape);
    s.pointer(
        Pointer {
            dx: i16::MAX,
            dy: i16::MIN,
            wheel: 0,
            buttons: 0,
        },
        0,
    );
    assert_eq!((s.cursor_x, s.cursor_y), (799, 0));
    s.animate(1000);
    s.open(SYSTEM);
    assert!(s.key(typed('m')));
    s.key(typed('t'));
    assert!(s.light);
    assert!(s.settings_dirty);
}
#[test]
fn animations_ease_out_and_reveal_the_window_from_its_centre() {
    assert_eq!(ease_out(0), 0);
    assert_eq!(ease_out(1000), 1000);
    assert!(ease_out(500) > 800, "most of the motion happens early");
    let mut s = Shell::new(1024, 768);
    s.animate(100);
    s.open(ABOUT);
    let full = s.rect(ABOUT);
    let first = s.visual(ABOUT);
    assert!(first.w < full.w && first.h < full.h);
    assert_eq!(first.x + first.w / 2, full.x + full.w / 2);
    s.animate(100 + WINDOW_TICKS / 2);
    assert!(s.visual(ABOUT).w > first.w);
    s.animate(100 + WINDOW_TICKS);
    assert_eq!(s.visual(ABOUT), full);
    assert!(
        s.task(ABOUT).w > 0,
        "About gets a taskbar button while open"
    );
    s.key(Key::Menu);
    assert_eq!(s.menu_offset(0), 30);
    s.animate(200 + MENU_TICKS + COUNT as u64);
    assert_eq!(s.menu_offset(COUNT), 0);
}
#[test]
fn maximize_glides_between_rectangles() {
    let mut s = Shell::new(1024, 768);
    s.animate(10);
    let before = s.rect(SYSTEM);
    let r = s.rect(SYSTEM);
    click(&mut s, r.right() - 60, r.y + 15, 10);
    assert!(s.windows[SYSTEM].maximized);
    assert!(s.morphing(SYSTEM));
    assert_eq!(s.visual(SYSTEM), before);
    s.animate(10 + MORPH_TICKS / 2);
    let middle = s.visual(SYSTEM);
    assert!(middle.w > before.w && middle.w < 1024);
    s.animate(10 + MORPH_TICKS);
    assert_eq!(s.visual(SYSTEM), Rect::new(0, 0, 1024, 768 - TASKBAR));
}
fn ready_files() -> Shell {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Files);
    assert_eq!(s.take_file_op(), Some(FileOp::Refresh));
    s.animate(100);
    s.files.storage = Storage::Ready {
        free_kib: 1000,
        total_kib: 2000,
    };
    let entries = [("docs", true), ("a.txt", false)].map(|(name, directory)| crate::files::Entry {
        id: if directory { 1 } else { 2 },
        directory,
        size: 3,
        name: Name::new(name),
    });
    s.files.set_entries(&entries, false, None);
    s
}
#[test]
fn typing_a_new_folder_name_does_not_trigger_hotkeys() {
    let mut s = ready_files();
    let r = s.tool(1);
    click(&mut s, r.x + 5, r.y + 5, 10);
    assert_eq!(s.files.edit.unwrap().text.as_str(), "Новая папка");
    for _ in 0.."Новая папка".chars().count() {
        s.key(Key::Backspace);
    }
    for ch in "tt".chars() {
        s.key(typed(ch));
    }
    assert!(!s.light, "typing must not toggle the theme");
    s.key(Key::Enter);
    assert_eq!(
        s.take_file_op(),
        Some(FileOp::CreateFolder(Name::new("tt")))
    );
    assert!(s.files.edit.is_none());
    s.key(typed('t'));
    assert!(s.light);
}
#[test]
fn delete_needs_confirmation_and_enter_opens_folders_and_files() {
    let mut s = ready_files();
    s.key(Key::Down);
    assert_eq!(s.files.selected_entry().unwrap().id, 1);
    s.key(Key::Delete);
    assert_eq!(s.take_file_op(), None);
    assert_eq!(s.files.confirm_delete, Some(1));
    s.key(Key::Delete);
    assert_eq!(s.take_file_op(), Some(FileOp::Delete(1)));
    s.key(Key::Enter);
    assert_eq!(s.take_file_op(), Some(FileOp::Open(1, Name::new("docs"))));
    s.key(Key::Down);
    s.key(Key::Enter);
    assert_eq!(s.take_file_op(), Some(FileOp::Edit(2, Name::new("a.txt"))));
    let row = s.list_rect();
    click(&mut s, row.x + 50, row.y + 5, 40);
    click(&mut s, row.x + 50, row.y + 5, 45);
    assert_eq!(s.take_file_op(), Some(FileOp::Open(1, Name::new("docs"))));
}
#[test]
fn rename_starts_from_the_current_name_and_escape_cancels() {
    let mut s = ready_files();
    s.files.selected = Some(1);
    let r = s.tool(3);
    click(&mut s, r.x + 5, r.y + 5, 10);
    s.key(typed('x'));
    s.key(Key::Enter);
    assert_eq!(
        s.take_file_op(),
        Some(FileOp::Rename(2, Name::new("a.txtx")))
    );
    click(&mut s, r.x + 5, r.y + 5, 90);
    s.key(Key::Escape);
    assert!(s.files.edit.is_none());
    assert!(
        s.shown(FILES),
        "escape cancels editing before closing the window"
    );
}
#[test]
fn editor_keys_edit_save_and_guard_unsaved_close() {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Editor);
    s.animate(100);
    s.key(typed('x'));
    assert_eq!(s.editor.text(), "", "no file loaded yet");
    assert!(
        s.editor
            .load(Some((5, Name::new("a.txt"))), "ok".as_bytes())
    );
    s.key(Key::End);
    for key in [typed(' '), typed('t'), Key::Enter, Key::Tab] {
        s.key(key);
    }
    assert_eq!(s.editor.text(), "ok t\n    ");
    assert!(!s.light, "letters in the editor are text, not hotkeys");
    s.key(Key::Ctrl { scan: 0x1f });
    assert_eq!(s.take_request(), Some(Request::SaveDocument));
    s.key(Key::Escape);
    assert!(s.interactive(EDITOR), "first close only warns");
    s.key(Key::Escape);
    assert!(!s.interactive(EDITOR));
}
#[test]
fn editor_click_moves_the_cursor_to_the_clicked_line() {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Editor);
    s.animate(100);
    assert!(s.editor.load(
        Some((5, Name::new("a"))),
        "первая\nвторая строка".as_bytes()
    ));
    let area = s.text_rect();
    click(&mut s, area.right() - 5, area.y + TEXT_ROW + 4, 200);
    assert_eq!(s.editor.position(), (1, 13));
    click(&mut s, area.x + 1, area.y + 2, 300);
    assert_eq!(s.editor.position(), (0, 0));
}
#[test]
fn terminal_lines_are_queued_for_the_service() {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Terminal);
    s.animate(100);
    for ch in "ls".chars() {
        s.key(typed(ch));
    }
    s.key(Key::Enter);
    assert_eq!(
        s.terminal.take_command(),
        Some((crate::terminal::Mode::Shell, "ls"))
    );
    s.key(typed(' '));
    assert!(!s.menu, "space is text in the terminal");
}
#[test]
fn language_resolution_and_confirmation() {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Settings);
    s.animate(100);
    let r = s.setting(11);
    click(&mut s, r.x + 2, r.y + 2, 101);
    assert_eq!(s.lang, Lang::En);
    assert!(s.settings_dirty);
    assert_eq!(WINDOWS[FILES].title(s.lang), "Files");
    s.mode_count = 3;
    s.modes[1] = DisplayMode {
        width: 800,
        height: 600,
    };
    let r = s.setting(21);
    click(&mut s, r.x + 2, r.y + 2, 102);
    let r = s.setting(32);
    click(&mut s, r.x + 2, r.y + 2, 103);
    assert_eq!(s.take_request(), Some(Request::SetMode(1)));
    s.resize(800, 600);
    for id in 0..COUNT {
        let r = s.rect(id);
        assert!(
            r.right() <= 800 && r.bottom() <= 600 - TASKBAR,
            "window {id} fits"
        );
    }
    s.mode_deadline = Some(s.now + MODE_CONFIRM_TICKS);
    s.key(typed('t'));
    assert!(!s.light, "the confirmation banner takes the keyboard");
    s.key(Key::Enter);
    assert_eq!(s.take_request(), Some(Request::KeepMode));
    s.mode_deadline = Some(s.now + 5);
    s.animate(s.now + 5);
    assert_eq!(s.take_request(), Some(Request::RevertMode));
}
#[test]
fn browser_address_typing_links_and_history_requests() {
    let mut s = Shell::new(1024, 768);
    s.key(Key::Browser);
    assert!(s.browser.editing, "a fresh browser waits for an address");
    let light = s.light;
    for ch in "test.ru".chars() {
        s.key(typed(ch));
    }
    assert_eq!(s.light, light, "typing 't' must not switch the theme");
    assert_eq!(s.browser.address.as_str(), "test.ru");
    s.key(Key::Enter);
    assert!(!s.browser.editing);
    assert_eq!(s.take_request(), Some(Request::Navigate));
    // A page is up: clicks report page coordinates, keys scroll.
    s.browser.has_page = true;
    s.browser.page_height = 5000;
    s.key(Key::PageDown);
    let scroll = s.browser.scroll;
    assert!(scroll > 0);
    let page = s.page_rect();
    click(&mut s, page.x + 30, page.y + 40, 1);
    assert_eq!(
        s.take_request(),
        Some(Request::Follow {
            x: 30,
            y: 40 + scroll
        })
    );
    s.key(Key::Backspace);
    assert_eq!(s.take_request(), Some(Request::Back));
    s.key(Key::End);
    assert_eq!(s.browser.scroll, 5000 - page.h);
    // Typing on the page starts a new address; Escape restores the old one.
    s.browser.current = Url::new("http://a.ru/");
    s.key(typed('x'));
    assert!(s.browser.editing && s.browser.address.as_str() == "x");
    s.key(Key::Escape);
    assert_eq!(s.browser.address.as_str(), "http://a.ru/");
    assert!(s.shown(BROWSER), "Escape in the address field keeps the window");
    // The network window connects on Enter and on its button.
    s.key(Key::Network);
    s.key(Key::Enter);
    assert_eq!(s.take_request(), Some(Request::Connect));
    let b = s.connect_button();
    click(&mut s, b.x + 5, b.y + 5, 2);
    assert_eq!(s.take_request(), Some(Request::Connect));
    // Tab only cycles through open windows.
    let open: std::vec::Vec<usize> = (0..COUNT).filter(|id| s.windows[*id].visible).collect();
    for _ in 0..COUNT {
        s.key(Key::Tab);
        assert!(open.contains(&s.active));
    }
}
