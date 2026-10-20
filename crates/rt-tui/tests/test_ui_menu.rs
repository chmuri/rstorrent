use lt_common::{HashString, Object};
use ratatui::style::Style;
use rt_rpc::command_flags::FLAG_PUBLIC_RPC;
use rt_rpc::{CommandMap, RpcError, Target};
use rt_tui::display::{Canvas, Window};
use rt_tui::windows::menu::{
    context_menu_for_target, download_context_menu, download_context_menu_with_state,
    file_context_menu, file_context_menu_with_priority, peer_context_menu,
    peer_context_menu_with_state, Menu, MenuElement, MenuEntry, MenuLevel, MenuOutcome, KEY_DOWN,
    KEY_ENTER, KEY_ESCAPE, KEY_LEFT, KEY_PAGE_DOWN, KEY_PAGE_UP, KEY_RIGHT, KEY_UP,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn hash(byte: u8) -> HashString {
    HashString::new([byte; 20])
}

fn items(count: usize) -> Vec<MenuEntry> {
    (0..count)
        .map(|index| MenuEntry::new(format!("Item {index}")))
        .collect()
}

fn command_map_with_hit() -> (CommandMap, Arc<AtomicUsize>) {
    let mut map = CommandMap::new();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    map.insert_func(
        "test.hit",
        move |_target, _obj| {
            counter.fetch_add(1, Ordering::SeqCst);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Increment the test counter.",
    )
    .unwrap();
    (map, hits)
}

#[test]
fn test_navigation_wraps_and_pages() {
    let mut menu = Menu::from_entries("Test", items(5));

    assert_eq!(menu.selected_index(), 0);
    menu.move_up();
    assert_eq!(menu.selected_index(), 4, "up wraps to the last entry");
    menu.move_down();
    assert_eq!(menu.selected_index(), 0, "down wraps to the first entry");

    menu.move_down();
    assert_eq!(menu.selected_index(), 1);

    menu.set_page_size(2);
    menu.page_down();
    assert_eq!(menu.selected_index(), 3);
    menu.page_down();
    assert_eq!(menu.selected_index(), 4, "page down clamps at the end");
    menu.page_up();
    assert_eq!(menu.selected_index(), 2);

    menu.select_first();
    assert_eq!(menu.selected_index(), 0);
    menu.select_last();
    assert_eq!(menu.selected_index(), 4);

    assert!(menu.select_index(3));
    assert_eq!(menu.selected_index(), 3);
    assert!(!menu.select_index(5));

    // Raw keys take the same path as the explicit navigation methods.
    assert_eq!(
        menu.handle_key(KEY_UP, &CommandMap::new()),
        Some(MenuOutcome::Navigated)
    );
    assert_eq!(menu.selected_index(), 2);
    assert_eq!(
        menu.handle_key(KEY_PAGE_UP, &CommandMap::new()),
        Some(MenuOutcome::Navigated)
    );
    assert_eq!(menu.selected_index(), 0);
    assert_eq!(
        menu.handle_key(KEY_PAGE_DOWN, &CommandMap::new()),
        Some(MenuOutcome::Navigated)
    );
    assert_eq!(menu.selected_index(), 2);
    assert_eq!(menu.handle_key(b'x' as i32, &CommandMap::new()), None);
}

#[test]
fn test_nested_submenus_and_back_closes_one_level() {
    let priority = MenuLevel::with_entries(
        "Priority",
        vec![
            MenuEntry::command("Normal", "d.priority.set=1"),
            MenuEntry::command("High", "d.priority.set=2"),
        ],
    );
    let mut menu = Menu::from_entries(
        "Main",
        vec![
            MenuEntry::new("First"),
            MenuEntry::submenu("More", priority),
        ],
    );
    let map = CommandMap::new();

    assert_eq!(menu.depth(), 0);
    assert!(menu.is_root());

    assert!(menu.select_index(1));
    assert_eq!(menu.activate(&map), MenuOutcome::SubmenuOpened);
    assert_eq!(menu.depth(), 1);
    assert!(!menu.is_root());
    assert_eq!(menu.current_title(), "Priority");
    assert_eq!(menu.selected_index(), 0);
    assert_eq!(menu.title_line(10), "[Main > Priority]");

    // Escape pops exactly one level and preserves the parent's selection.
    assert_eq!(
        menu.handle_key(KEY_ESCAPE, &map),
        Some(MenuOutcome::LevelClosed)
    );
    assert_eq!(menu.depth(), 0);
    assert_eq!(menu.selected_index(), 1);
    assert_eq!(menu.title(), "Main");

    // Right arrow opens, left arrow closes one level.
    assert_eq!(
        menu.handle_key(KEY_RIGHT, &map),
        Some(MenuOutcome::SubmenuOpened)
    );
    assert_eq!(menu.depth(), 1);
    assert_eq!(
        menu.handle_key(KEY_LEFT, &map),
        Some(MenuOutcome::LevelClosed)
    );
    assert_eq!(menu.depth(), 0);

    // At the root the caller is asked to close the overlay.
    assert_eq!(
        menu.handle_key(KEY_ESCAPE, &map),
        Some(MenuOutcome::CloseRequested)
    );
    assert_eq!(menu.depth(), 0, "the root level is never popped");
}

#[test]
fn test_disabled_entries_are_visible_but_not_executable() {
    let (map, hits) = command_map_with_hit();
    let mut menu = Menu::from_entries(
        "Main",
        vec![
            MenuEntry::command("Enabled", "test.hit="),
            MenuEntry::command("Disabled", "test.hit=").disabled(),
        ],
    );

    assert!(menu.select_index(1));
    assert_eq!(
        menu.activate(&map),
        MenuOutcome::Disabled {
            label: "Disabled".to_string()
        }
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "disabled entry must not run"
    );
    assert!(menu.last_error().is_none());

    let mut canvas = Canvas::new(0, 0, 32, 4);
    menu.render(&mut canvas);
    let rows = canvas.to_string_rows();
    assert!(rows[2].contains("Disabled"));
    assert!(rows[2].contains("(disabled)"));
    assert!(rows[2].starts_with("* "));
    assert!(rows[1].contains("Enabled"));
    // Disabled rows are dimmed rather than hidden.
    assert_ne!(canvas.get_cell(0, 2).unwrap().style, Style::default());

    assert!(menu.select_index(0));
    assert_eq!(
        menu.activate(&map),
        MenuOutcome::Executed("test.hit=".to_string())
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

#[test]
fn test_command_errors_are_propagated_and_logged() {
    let mut map = CommandMap::new();
    map.insert_func(
        "test.fail",
        |_target, _obj| Err(RpcError::InputError("boom".to_string())),
        FLAG_PUBLIC_RPC,
        "",
        "Always fails.",
    )
    .unwrap();
    map.insert_func(
        "test.ok",
        |_target, _obj| Ok(Object::create_empty()),
        FLAG_PUBLIC_RPC,
        "",
        "Always succeeds.",
    )
    .unwrap();

    let mut menu = Menu::from_entries("Main", vec![MenuEntry::command("Fail", "test.fail=")]);

    let outcome = menu.activate(&map);
    match &outcome {
        MenuOutcome::CommandFailed { command, error } => {
            assert_eq!(command, "test.fail=");
            assert!(error.contains("boom"));
        }
        other => panic!("expected CommandFailed, got {other:?}"),
    }
    assert!(outcome.is_error());
    assert!(outcome.message().unwrap().contains("boom"));

    let last_error = menu.last_error().expect("error is retained on the menu");
    assert!(last_error.contains("test.fail="));
    assert!(last_error.contains("boom"));

    let messages = menu.drain_messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].contains("Command failed"));
    assert!(messages[0].contains("boom"));
    assert!(menu.messages().is_empty());

    // A successful command clears the previous error.
    menu.set_entries(vec![MenuEntry::command("Ok", "test.ok=")]);
    assert_eq!(
        menu.activate(&map),
        MenuOutcome::Executed("test.ok=".to_string())
    );
    assert!(menu.last_error().is_none());
    assert!(menu.drain_messages()[0].contains("Executed"));
}

#[test]
fn test_commands_use_the_captured_target() {
    let mut map = CommandMap::new();
    let captured = Arc::new(Mutex::new(Vec::<Target>::new()));
    let seen = Arc::clone(&captured);
    map.insert_func(
        "test.capture",
        move |target, _obj| {
            seen.lock().unwrap().push(target.clone());
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Capture the dispatched target.",
    )
    .unwrap();

    let download = hash(7);
    let mut menu = Menu::from_entries(
        "Download",
        vec![MenuEntry::command("Capture", "test.capture")],
    )
    .with_target(Target::Download(download));

    assert_eq!(menu.target(), &Target::Download(download));
    assert_eq!(
        menu.activate(&map),
        MenuOutcome::Executed("test.capture".to_string())
    );
    assert_eq!(
        *captured.lock().unwrap(),
        vec![Target::Download(download)],
        "the target captured when the menu was created must be used"
    );
}

#[test]
fn test_command_arguments_are_parsed() {
    let mut map = CommandMap::new();
    let seen = Arc::new(Mutex::new(None));
    let value = Arc::clone(&seen);
    map.insert_func(
        "test.echo",
        move |_target, obj| {
            *value.lock().unwrap() = Some(obj.as_str().unwrap().to_string());
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Record the first argument.",
    )
    .unwrap();

    let mut menu = Menu::from_entries("Args", vec![MenuEntry::command("Echo", "test.echo=42")]);
    assert_eq!(
        menu.activate(&map),
        MenuOutcome::Executed("test.echo=42".to_string())
    );
    assert_eq!(seen.lock().unwrap().as_deref(), Some("42"));
}

#[test]
fn test_rendering_selection_scrolling_and_small_terminals() {
    let mut menu = Menu::from_entries("Test", items(10));
    menu.set_page_size(3);
    for _ in 0..4 {
        menu.move_down();
    }

    assert_eq!(menu.selected_index(), 4);
    assert_eq!(menu.scroll_offset(), 2);

    let mut canvas = Canvas::new(0, 0, 24, 4);
    menu.render(&mut canvas);
    let rows = canvas.to_string_rows();
    assert!(rows[0].contains("[Test]"));
    assert!(rows[0].contains("[3-5/10]"));
    assert!(rows[1].contains("Item 2"));
    assert!(rows[2].contains("Item 3"));
    assert!(rows[3].contains("Item 4"));
    assert!(rows[3].starts_with("* "), "selected entry is marked");
    assert_ne!(
        canvas.get_cell(0, 3).unwrap().style,
        Style::default(),
        "selected entry is highlighted"
    );
    assert_eq!(canvas.get_cell(0, 1).unwrap().style, Style::default());

    // Zero-sized and one-line terminals must not panic.
    let mut empty = Canvas::new(0, 0, 0, 0);
    menu.render(&mut empty);
    assert_eq!(empty.to_string_rows().len(), 0);

    let mut tiny = Canvas::new(0, 0, 1, 1);
    menu.render(&mut tiny);
    assert_eq!(tiny.get_line(0), "[");

    let mut single_line = Canvas::new(0, 0, 10, 1);
    menu.render(&mut single_line);
    assert!(single_line.get_line(0).starts_with("[Test]"));

    // An empty menu still renders its title and an explicit empty state.
    let mut empty_menu = Menu::from_entries("Empty", Vec::new());
    let mut empty_canvas = Canvas::new(0, 0, 20, 3);
    empty_menu.render(&mut empty_canvas);
    let empty_rows = empty_canvas.to_string_rows();
    assert!(empty_rows[0].contains("[Empty]"));
    assert!(empty_rows[1].contains("[No entries]"));
}

#[test]
fn test_menu_element_window_and_key_routing() {
    let (map, hits) = command_map_with_hit();
    let mut menu = Menu::from_entries(
        "Element",
        vec![
            MenuEntry::command("Hit", "test.hit="),
            MenuEntry::command("Sub", "test.hit="),
        ],
    );
    menu.set_page_size(1);
    let mut element = MenuElement::new(menu).with_flags(rt_tui::display::FLAG_ACTIVE);

    assert_eq!(element.min_width(), 6);
    assert_eq!(element.min_height(), 1);
    assert!(element.is_dirty());

    element.resize(0, 0, 20, 3);
    let mut canvas = Canvas::new(0, 0, 20, 3);
    element.redraw(&mut canvas);
    assert!(!element.is_dirty());
    assert!(canvas.get_line(0).contains("[Element]"));

    assert_eq!(
        element.handle_key(KEY_DOWN, &map),
        Some(MenuOutcome::Navigated)
    );
    assert!(element.is_dirty());
    assert_eq!(element.menu().selected_index(), 1);

    assert_eq!(
        element.handle_key(KEY_ENTER, &map),
        Some(MenuOutcome::Executed("test.hit=".to_string()))
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    assert_eq!(element.handle_key(b'x' as i32, &map), None);
}

#[test]
fn test_download_context_menu_builder() {
    let download = hash(3);
    let menu = download_context_menu(&download);

    assert_eq!(menu.title(), "Download");
    assert_eq!(menu.target(), &Target::Download(download));
    assert!(menu
        .entries()
        .iter()
        .any(|entry| entry.command.as_deref() == Some("d.start=")));
    assert!(menu
        .entries()
        .iter()
        .any(|entry| entry.command.as_deref() == Some("d.check_hash=")));
    assert!(menu
        .entries()
        .iter()
        .any(|entry| entry.command.as_deref() == Some("d.erase=")));

    let priority = menu
        .entries()
        .iter()
        .find(|entry| entry.label == "Priority")
        .expect("priority submenu");
    let submenu = priority.submenu.as_ref().expect("priority level");
    assert_eq!(submenu.title, "Priority");
    assert_eq!(submenu.entries.len(), 3);
    assert!(submenu
        .entries
        .iter()
        .any(|entry| entry.command.as_deref() == Some("d.priority.set=2")));

    let active = download_context_menu_with_state(&download, true, false);
    let entry = |label: &str| {
        active
            .entries()
            .iter()
            .find(|entry| entry.label == label)
            .unwrap()
            .clone()
    };
    assert!(!entry("Start").enabled);
    assert!(entry("Stop").enabled);
    assert!(entry("Pause").enabled);
    assert!(!entry("Resume").enabled);
    assert!(entry("Close").enabled);
}

#[test]
fn test_file_context_menu_builder() {
    let download = hash(4);
    let menu = file_context_menu(&download, 3);

    assert_eq!(menu.title(), "File");
    assert_eq!(menu.target(), &Target::File(download, 3));
    assert!(menu
        .entries()
        .iter()
        .any(|entry| entry.command.as_deref() == Some("f.prioritize_first.enable=")));

    let priority = menu
        .entries()
        .iter()
        .find(|entry| entry.label == "Priority")
        .and_then(|entry| entry.submenu.as_ref())
        .expect("priority submenu");
    assert!(priority
        .entries
        .iter()
        .any(|entry| entry.command.as_deref() == Some("f.priority.set=1")));

    let high = file_context_menu_with_priority(&download, 3, 2);
    let priority = high
        .entries()
        .iter()
        .find(|entry| entry.label == "Priority")
        .and_then(|entry| entry.submenu.as_ref())
        .expect("priority submenu");
    let entry = |command: &str| {
        priority
            .entries
            .iter()
            .find(|entry| entry.command.as_deref() == Some(command))
            .unwrap()
            .clone()
    };
    assert!(!entry("f.priority.set=2").enabled);
    assert!(entry("f.priority.set=1").enabled);
    assert!(entry("f.priority.set=0").enabled);
}

#[test]
fn test_peer_context_menu_builder() {
    let download = hash(5);
    let address: std::net::SocketAddr = "127.0.0.1:51413".parse().unwrap();
    let target = Target::Peer(download, address.into());

    let menu = peer_context_menu(target.clone());
    assert_eq!(menu.title(), "Peer");
    assert_eq!(menu.target(), &target);
    assert!(menu
        .entries()
        .iter()
        .any(|entry| entry.command.as_deref() == Some("p.disconnect=")));

    let state = peer_context_menu_with_state(target, true, false, true);
    let entry = |label: &str| {
        state
            .entries()
            .iter()
            .find(|entry| entry.label == label)
            .unwrap()
            .clone()
    };
    assert!(!entry("Snub").enabled);
    assert!(entry("Unsnub").enabled);
    assert!(entry("Prefer").enabled);
    assert!(!entry("Unprefer").enabled);
    assert!(!entry("Ban").enabled);
    assert!(entry("Unban").enabled);
}

#[test]
fn test_context_menu_for_target_dispatch() {
    let download = hash(6);
    let file_target = Target::File(download, 2);
    let address: std::net::SocketAddr = "10.0.0.2:6881".parse().unwrap();
    let peer_target = Target::Peer(download, address.into());

    assert_eq!(
        context_menu_for_target(Target::Download(download)).map(|menu| menu.title().to_string()),
        Some("Download".to_string())
    );
    assert_eq!(
        context_menu_for_target(file_target.clone()).map(|menu| menu.title().to_string()),
        Some("File".to_string())
    );
    assert_eq!(
        context_menu_for_target(peer_target.clone()).map(|menu| menu.title().to_string()),
        Some("Peer".to_string())
    );
    assert!(context_menu_for_target(Target::None).is_none());

    assert_eq!(
        context_menu_for_target(file_target).map(|menu| (*menu.target()).clone()),
        Some(Target::File(download, 2))
    );
}
