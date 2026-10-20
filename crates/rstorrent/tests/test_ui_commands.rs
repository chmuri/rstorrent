use std::sync::{Arc, Mutex, RwLock};

use lt_common::Object;
use rstorrent::ui_commands::register_ui_commands;
use rt_core::ViewManager;
use rt_rpc::{CommandMap, Target};
use rt_tui::display::{color_map::parse_color_definition, Canvas, ColorKind};
use rt_tui::{RootUI, Window};

struct UiHarness {
    map: CommandMap,
    ui: Arc<Mutex<RootUI>>,
    view_manager: Arc<RwLock<ViewManager>>,
}

fn setup() -> UiHarness {
    let mut view_manager = ViewManager::new();
    view_manager.insert("main");
    view_manager.insert("started");

    let ui = Arc::new(Mutex::new(RootUI::new()));
    let view_manager = Arc::new(RwLock::new(view_manager));

    let mut map = CommandMap::new();
    register_ui_commands(&mut map, Arc::clone(&ui), Arc::clone(&view_manager)).unwrap();
    UiHarness {
        map,
        ui,
        view_manager,
    }
}

fn call(map: &CommandMap, name: &str, args: Vec<Object>) -> Result<Object, rt_rpc::RpcError> {
    let args = Object::create_list_with(args);
    map.call_command(name, &args, &Target::None, true)
}

fn clear_dirty(ui: &Arc<Mutex<RootUI>>) {
    let ui = ui.lock().unwrap();
    let mut canvas = Canvas::new(0, 0, 80, 24);
    ui.window_title.lock().unwrap().redraw(&mut canvas);
    ui.window_download_list.lock().unwrap().redraw(&mut canvas);
    ui.window_statusbar.lock().unwrap().redraw(&mut canvas);
    ui.window_input.lock().unwrap().redraw(&mut canvas);
}

#[test]
fn current_view_switching_is_validated_and_marks_ui_dirty() {
    let harness = setup();

    assert_eq!(
        call(&harness.map, "ui.current_view", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "main"
    );

    clear_dirty(&harness.ui);
    assert!(call(
        &harness.map,
        "ui.current_view.set",
        vec![Object::from("started")]
    )
    .is_ok());
    assert_eq!(harness.ui.lock().unwrap().current_view_name, "started");
    {
        let ui = harness.ui.lock().unwrap();
        assert!(ui.window_download_list.lock().unwrap().is_dirty());
        assert!(ui.window_title.lock().unwrap().is_dirty());
    }

    clear_dirty(&harness.ui);
    assert!(call(
        &harness.map,
        "ui.current_view.set",
        vec![Object::from("missing")]
    )
    .is_err());
    assert_eq!(harness.ui.lock().unwrap().current_view_name, "started");
    // A failed switch must not dirty the UI.
    assert!(!harness
        .ui
        .lock()
        .unwrap()
        .window_download_list
        .lock()
        .unwrap()
        .is_dirty());

    // The external view manager and the UI stay in sync.
    assert!(harness
        .view_manager
        .read()
        .unwrap()
        .find("started")
        .is_some());
}

#[test]
fn unfocus_download_clears_the_current_selection() {
    let harness = setup();

    assert!(call(&harness.map, "ui.unfocus_download", vec![]).is_ok());

    // The registered view has no downloads, so focus remains at zero but the
    // command must still dirty the list so clients observe the state change.
    assert!(harness
        .ui
        .lock()
        .unwrap()
        .window_download_list
        .lock()
        .unwrap()
        .is_dirty());
}

#[test]
fn color_variables_round_trip_all_kinds_and_validate() {
    let harness = setup();
    let mut registered = 0;

    for kind in ColorKind::ALL {
        let Some(name) = kind.rpc_variable_name() else {
            continue;
        };
        registered += 1;

        assert!(harness.map.has(name), "missing getter {name}");
        assert!(
            harness.map.has(&format!("{name}.set")),
            "missing setter {name}"
        );

        let value = format!("bold {}", if registered % 2 == 0 { "red" } else { "green" });
        assert!(call(
            &harness.map,
            &format!("{name}.set"),
            vec![Object::from(value.as_str())]
        )
        .is_ok());
        assert_eq!(
            call(&harness.map, name, vec![]).unwrap().as_str().unwrap(),
            value
        );
    }

    assert_eq!(registered, 14);
}

#[test]
fn invalid_color_leaves_previous_value_and_color_map_intact() {
    let harness = setup();

    assert!(call(
        &harness.map,
        "ui.color.title.set",
        vec![Object::from("bold white on blue")]
    )
    .is_ok());
    let before = harness
        .ui
        .lock()
        .unwrap()
        .color_map
        .read()
        .unwrap()
        .get_style(ColorKind::Title, None);

    assert!(call(
        &harness.map,
        "ui.color.title.set",
        vec![Object::from("definitely-not-a-color")]
    )
    .is_err());

    {
        let ui = harness.ui.lock().unwrap();
        assert_eq!(ui.ui_color(ColorKind::Title), "bold white on blue");
        assert_eq!(
            ui.color_map
                .read()
                .unwrap()
                .get_style(ColorKind::Title, None),
            before
        );
    }
}

#[test]
fn setting_colors_marks_windows_dirty_and_affects_rendering() {
    let harness = setup();

    assert!(call(
        &harness.map,
        "ui.color.title.set",
        vec![Object::from("bold white on blue")]
    )
    .is_ok());
    assert!(call(
        &harness.map,
        "ui.color.footer.set",
        vec![Object::from("black on yellow")]
    )
    .is_ok());

    {
        let mut ui = harness.ui.lock().unwrap();
        assert!(ui.window_title.lock().unwrap().is_dirty());
        assert!(ui.window_download_list.lock().unwrap().is_dirty());
        assert!(ui.window_statusbar.lock().unwrap().is_dirty());

        let mut canvas = Canvas::new(0, 0, 80, 24);
        ui.balance_and_redraw(&mut canvas);

        let expected_title = parse_color_definition("bold white on blue").unwrap();
        let title = canvas.get_raw_cell(0, 0).unwrap();
        assert_eq!(title.style.fg, expected_title.fg);
        assert_eq!(title.style.bg, expected_title.bg);
        assert!(title
            .style
            .add_modifier
            .contains(expected_title.add_modifier));

        let expected_footer = parse_color_definition("black on yellow").unwrap();
        let footer = canvas.get_raw_cell(0, 23).unwrap();
        assert_eq!(footer.style.fg, expected_footer.fg);
        assert_eq!(footer.style.bg, expected_footer.bg);
    }
}

#[test]
fn history_size_is_bounded_and_clear_works() {
    let harness = setup();

    assert!(call(
        &harness.map,
        "ui.input.history.size.set",
        vec![Object::from(2i64)]
    )
    .is_ok());
    {
        let mut ui = harness.ui.lock().unwrap();
        ui.push_input_history("one");
        ui.push_input_history("two");
        ui.push_input_history("three");
        assert_eq!(
            ui.input_history.entries().to_vec(),
            vec!["two".to_string(), "three".to_string()]
        );
    }

    // 0 disables history storage.
    assert!(call(
        &harness.map,
        "ui.input.history.size.set",
        vec![Object::from(0i64)]
    )
    .is_ok());
    {
        let mut ui = harness.ui.lock().unwrap();
        ui.push_input_history("ignored");
        assert!(ui.input_history.is_empty());
    }

    // Values above the legacy maximum are clamped.
    assert!(call(
        &harness.map,
        "ui.input.history.size.set",
        vec![Object::from(10_000i64)]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.input.history.size", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        rt_tui::input::MAX_INPUT_HISTORY_SIZE as i64
    );

    {
        let mut ui = harness.ui.lock().unwrap();
        ui.push_input_history("clear me");
        assert!(!ui.input_history.is_empty());
    }
    assert!(call(&harness.map, "ui.input.history.clear", vec![]).is_ok());
    assert!(harness.ui.lock().unwrap().input_history.is_empty());

    // Negative sizes are coerced to the "disabled" state rather than failing.
    assert!(call(
        &harness.map,
        "ui.input.history.size.set",
        vec![Object::from(-5i64)]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.input.history.size", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        0
    );
}

#[test]
fn ui_scalar_variables_round_trip_and_apply() {
    let harness = setup();

    assert!(call(
        &harness.map,
        "ui.input.history.size.set",
        vec![Object::from(10i64)]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.input.history.size", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        10
    );

    assert!(call(
        &harness.map,
        "ui.keymap.style.set",
        vec![Object::from("vi")]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.keymap.style", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "vi"
    );

    assert!(call(
        &harness.map,
        "ui.torrent_list.layout.set",
        vec![Object::from("compact")]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.torrent_list.layout", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "compact"
    );
    {
        let ui = harness.ui.lock().unwrap();
        assert_eq!(ui.torrent_list_layout, "compact");
        assert_eq!(ui.window_download_list.lock().unwrap().layout, "compact");
        assert!(ui.window_download_list.lock().unwrap().is_dirty());
    }

    assert!(call(
        &harness.map,
        "ui.focus.page_size.set",
        vec![Object::from(25i64)]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.focus.page_size", vec![])
            .unwrap()
            .as_value()
            .unwrap(),
        25
    );

    for (name, expected) in [("small", 7i64), ("medium", 70), ("large", 700)] {
        assert!(call(
            &harness.map,
            &format!("ui.throttle.global.step.{name}.set"),
            vec![Object::from(expected)]
        )
        .is_ok());
        assert_eq!(
            call(
                &harness.map,
                &format!("ui.throttle.global.step.{name}"),
                vec![]
            )
            .unwrap()
            .as_value()
            .unwrap(),
            expected
        );
    }

    assert!(call(
        &harness.map,
        "ui.status.throttle.up.set",
        vec![Object::from("upstream")]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.status.throttle.up", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "upstream"
    );

    // The legacy setter also accepts a list of throttle names.
    assert!(call(
        &harness.map,
        "ui.status.throttle.down.set",
        vec![Object::create_list_with(vec![
            Object::from("down-a"),
            Object::from("down-b"),
        ])]
    )
    .is_ok());
    assert_eq!(
        call(&harness.map, "ui.status.throttle.down", vec![])
            .unwrap()
            .as_str()
            .unwrap(),
        "down-a,down-b"
    );
}
