//! `ui.*` RPC command namespace.

use std::sync::{Arc, Mutex, RwLock};

use lt_common::Object;
use rt_core::ViewManager;
use rt_rpc::command_flags::*;
use rt_rpc::{convert_to_string, convert_to_value, CommandMap, RpcError};
use rt_tui::display::ColorKind;
use rt_tui::RootUI;

fn to_str(obj: &Object) -> Result<String, RpcError> {
    convert_to_string(obj)
}

fn to_value(obj: &Object) -> Result<i64, RpcError> {
    convert_to_value(obj, 10, 1)
}

/// `ui.status.throttle.*.set` accepts either a single name or, like the
/// legacy command, a list of names.
fn to_name_list(obj: &Object) -> Result<String, RpcError> {
    match obj.as_list() {
        Ok(list) => {
            // Commands are normally called with a single argument list, e.g.
            // `[[name, name]]`.  Unwrap that outer list so the legacy
            // multi-name form works while single names keep working.
            let list = if list.len() == 1 && list[0].is_list() {
                list[0].as_list().unwrap_or(list)
            } else {
                list
            };
            let mut names = Vec::with_capacity(list.len());
            for item in list {
                names.push(convert_to_string(item)?);
            }
            Ok(names.join(","))
        }
        Err(_) => convert_to_string(obj),
    }
}

fn current_view(ui: &RootUI) -> String {
    ui.current_view_name.clone()
}

pub fn register_ui_commands(
    map: &mut CommandMap,
    ui: Arc<Mutex<RootUI>>,
    view_manager: Arc<RwLock<ViewManager>>,
) -> Result<(), RpcError> {
    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "ui.current_view",
        move |_target, _obj| {
            let ui = ui_get.lock().unwrap();
            Ok(Object::from(current_view(&ui)))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the current view name.",
    )?;

    let ui_set = Arc::clone(&ui);
    let vm_set = Arc::clone(&view_manager);
    map.insert_func(
        "ui.current_view.set",
        move |_target, obj| {
            let name = to_str(obj)?;
            if vm_set.read().unwrap().find(&name).is_none() {
                return Err(RpcError::InputError(format!("Unknown view '{name}'")));
            }
            let mut ui = ui_set.lock().unwrap();
            // A UI constructed without a view manager (notably in RPC-only
            // tests or embedded clients) can still switch views as long as the
            // command was registered with one.
            if ui.view_manager.is_none() {
                ui.set_view_manager(Arc::clone(&vm_set));
            }
            ui.switch_view(&name);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<view>",
        "Switch the active view.",
    )?;

    let ui_unfocus = Arc::clone(&ui);
    map.insert_func(
        "ui.unfocus_download",
        move |target, _obj| {
            // The legacy command is a download-target command driven by
            // `event.download.erased`.  A target-less call clears the current
            // selection, which is useful for interactive/RPC clients.
            let hash = target.download().cloned();
            ui_unfocus.lock().unwrap().unfocus_download(hash.as_ref());
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Remove focus from the selected (or targeted) download.",
    )?;

    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "ui.input.history.size",
        move |_target, _obj| {
            let size = ui_get.lock().unwrap().input_history_size();
            Ok(Object::from(size as i64))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the input history limit.",
    )?;

    let ui_set = Arc::clone(&ui);
    map.insert_func(
        "ui.input.history.size.set",
        move |_target, obj| {
            let size = to_value(obj)?.max(0) as usize;
            ui_set.lock().unwrap().set_input_history_size(size);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<size>",
        "Set the input history limit.",
    )?;

    let ui_clear = Arc::clone(&ui);
    map.insert_func(
        "ui.input.history.clear",
        move |_target, _obj| {
            ui_clear.lock().unwrap().clear_input_history();
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "",
        "Clear the input history.",
    )?;

    // Throttle step variables.
    for (name, field) in [("small", 0usize), ("medium", 1), ("large", 2)] {
        let ui_get = Arc::clone(&ui);
        map.insert_func(
            &format!("ui.throttle.global.step.{name}"),
            move |_target, _obj| {
                let ui = ui_get.lock().unwrap();
                let value = match field {
                    0 => ui.throttle_step_small,
                    1 => ui.throttle_step_medium,
                    _ => ui.throttle_step_large,
                };
                Ok(Object::from(value as i64))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "Get a global throttle step.",
        )?;

        let ui_set = Arc::clone(&ui);
        map.insert_func(
            &format!("ui.throttle.global.step.{name}.set"),
            move |_target, obj| {
                let value = to_value(obj)?.max(0) as u32;
                ui_set.lock().unwrap().set_throttle_step(field, value);
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<value>",
            "Set a global throttle step.",
        )?;
    }

    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "ui.focus.page_size",
        move |_target, _obj| Ok(Object::from(ui_get.lock().unwrap().focus_page_size as i64)),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the page size used by focus movement.",
    )?;

    let ui_set = Arc::clone(&ui);
    map.insert_func(
        "ui.focus.page_size.set",
        move |_target, obj| {
            let value = to_value(obj)?.max(0) as u32;
            ui_set.lock().unwrap().set_focus_page_size(value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<value>",
        "Set the page size used by focus movement.",
    )?;

    for (name, is_up) in [("up", true), ("down", false)] {
        let ui_get = Arc::clone(&ui);
        map.insert_func(
            &format!("ui.status.throttle.{name}"),
            move |_target, _obj| {
                let ui = ui_get.lock().unwrap();
                let value = if is_up {
                    ui.status_throttle_up.clone()
                } else {
                    ui.status_throttle_down.clone()
                };
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "Get the statusbar throttle name.",
        )?;

        let ui_set = Arc::clone(&ui);
        map.insert_func(
            &format!("ui.status.throttle.{name}.set"),
            move |_target, obj| {
                let value = to_name_list(obj)?;
                let mut ui = ui_set.lock().unwrap();
                if is_up {
                    ui.set_status_throttle_up(value);
                } else {
                    ui.set_status_throttle_down(value);
                }
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<name>",
            "Set the statusbar throttle name.",
        )?;
    }

    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "ui.keymap.style",
        move |_target, _obj| Ok(Object::from(ui_get.lock().unwrap().keymap_style.clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the keymap style.",
    )?;

    let ui_set = Arc::clone(&ui);
    map.insert_func(
        "ui.keymap.style.set",
        move |_target, obj| {
            let value = to_str(obj)?;
            ui_set
                .lock()
                .unwrap()
                .set_keymap_style(&value)
                .map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<style>",
        "Set the keymap style (default, emacs or vi).",
    )?;

    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "keys.layout",
        move |_target, _obj| Ok(Object::from(ui_get.lock().unwrap().throttle_layout.clone())),
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the throttle key layout.",
    )?;

    let ui_set = Arc::clone(&ui);
    map.insert_func(
        "keys.layout.set",
        move |_target, obj| {
            let value = to_str(obj)?;
            ui_set
                .lock()
                .unwrap()
                .set_throttle_layout(&value)
                .map_err(RpcError::InputError)?;
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<layout>",
        "Set the throttle key layout (qwerty, azerty, qwertz or dvorak).",
    )?;

    let ui_get = Arc::clone(&ui);
    map.insert_func(
        "ui.torrent_list.layout",
        move |_target, _obj| {
            Ok(Object::from(
                ui_get.lock().unwrap().torrent_list_layout.clone(),
            ))
        },
        FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
        "",
        "Get the torrent list layout.",
    )?;

    let ui_set = Arc::clone(&ui);
    map.insert_func(
        "ui.torrent_list.layout.set",
        move |_target, obj| {
            let value = to_str(obj)?;
            ui_set.lock().unwrap().set_torrent_list_layout(value);
            Ok(Object::create_empty())
        },
        FLAG_PUBLIC_RPC,
        "<layout>",
        "Set the torrent list layout.",
    )?;

    // Color variables.
    for kind in ColorKind::ALL {
        let Some(name) = kind.rpc_variable_name() else {
            continue;
        };

        let ui_get = Arc::clone(&ui);
        map.insert_func(
            name,
            move |_target, _obj| {
                let value = ui_get.lock().unwrap().ui_color(kind);
                Ok(Object::from(value))
            },
            FLAG_PUBLIC_RPC | FLAG_UNTRUSTED_SAFE,
            "",
            "Get a UI color definition.",
        )?;

        let ui_set = Arc::clone(&ui);
        map.insert_func(
            &format!("{name}.set"),
            move |_target, obj| {
                let value = to_str(obj)?;
                ui_set
                    .lock()
                    .unwrap()
                    .set_ui_color(kind, &value)
                    .map_err(RpcError::InputError)?;
                Ok(Object::create_empty())
            },
            FLAG_PUBLIC_RPC,
            "<color definition>",
            "Set a UI color definition.",
        )?;
    }

    Ok(())
}
