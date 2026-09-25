//! The native menu bar. Its layout comes from the UI (built from the command
//! registry, see src/menu/menuModel.ts); this module only materializes it and
//! routes clicks back as `menu-command` events, so a menu item runs exactly
//! the same command as its shortcut or Search Everywhere entry.

use serde::Deserialize;
use tauri::menu::{AboutMetadata, IsMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};

const COMMAND_PREFIX: &str = "cmd:";
const LINK_PREFIX: &str = "link:";

#[derive(Deserialize)]
pub struct MenuSpec {
    title: String,
    items: Vec<MenuEntry>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MenuEntry {
    #[serde(rename_all = "camelCase")]
    Command { command_id: String, title: String, accelerator: Option<String> },
    Predefined { name: Predefined },
    Link { title: String, url: String },
    Separator,
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum Predefined {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Minimize,
    Zoom,
    Fullscreen,
}

/// Replaces the app menu. Called again whenever the command registry changes.
#[tauri::command]
pub fn menu_set(app: AppHandle, menus: Vec<MenuSpec>) -> Result<(), String> {
    build(&app, &menus).and_then(|menu| app.set_menu(menu)).map(|_| ()).map_err(|e| e.to_string())
}

/// Routes menu clicks: commands go back to the UI, links open in the browser.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    let id = event.id().as_ref();
    if let Some(command) = id.strip_prefix(COMMAND_PREFIX) {
        let _ = app.emit("menu-command", command);
    } else if let Some(url) = id.strip_prefix(LINK_PREFIX) {
        open_url(url);
    }
}

fn build<R: Runtime>(app: &AppHandle<R>, menus: &[MenuSpec]) -> tauri::Result<Menu<R>> {
    let bar = Menu::new(app)?;
    for spec in menus {
        let submenu = Submenu::new(app, &spec.title, true)?;
        for entry in &spec.items {
            submenu.append(item(app, entry)?.as_ref())?;
        }
        bar.append(&submenu)?;
    }
    Ok(bar)
}

fn item<R: Runtime>(app: &AppHandle<R>, entry: &MenuEntry) -> tauri::Result<Box<dyn IsMenuItem<R>>> {
    Ok(match entry {
        MenuEntry::Command { command_id, title, accelerator } => {
            let id = format!("{COMMAND_PREFIX}{command_id}");
            // An accelerator the platform cannot parse must not cost the item.
            match MenuItem::with_id(app, &id, title, true, accelerator.as_deref()) {
                Ok(item) => Box::new(item),
                Err(_) => Box::new(MenuItem::with_id(app, &id, title, true, None::<&str>)?),
            }
        }
        MenuEntry::Link { title, url } => {
            Box::new(MenuItem::with_id(app, format!("{LINK_PREFIX}{url}"), title, true, None::<&str>)?)
        }
        MenuEntry::Separator => Box::new(PredefinedMenuItem::separator(app)?),
        MenuEntry::Predefined { name } => Box::new(predefined(app, *name)?),
    })
}

fn predefined<R: Runtime>(app: &AppHandle<R>, name: Predefined) -> tauri::Result<PredefinedMenuItem<R>> {
    match name {
        Predefined::About => {
            let info = app.package_info();
            let metadata = AboutMetadata {
                name: Some(info.name.clone()),
                version: Some(info.version.to_string()),
                website: Some("https://github.com/naitsric/IdeDB".into()),
                icon: app.default_window_icon().cloned(),
                ..Default::default()
            };
            PredefinedMenuItem::about(app, Some(&format!("About {}", info.name)), Some(metadata))
        }
        Predefined::Services => PredefinedMenuItem::services(app, None),
        Predefined::Hide => PredefinedMenuItem::hide(app, None),
        Predefined::HideOthers => PredefinedMenuItem::hide_others(app, None),
        Predefined::ShowAll => PredefinedMenuItem::show_all(app, None),
        Predefined::Quit => PredefinedMenuItem::quit(app, None),
        Predefined::Undo => PredefinedMenuItem::undo(app, None),
        Predefined::Redo => PredefinedMenuItem::redo(app, None),
        Predefined::Cut => PredefinedMenuItem::cut(app, None),
        Predefined::Copy => PredefinedMenuItem::copy(app, None),
        Predefined::Paste => PredefinedMenuItem::paste(app, None),
        Predefined::SelectAll => PredefinedMenuItem::select_all(app, None),
        Predefined::Minimize => PredefinedMenuItem::minimize(app, None),
        Predefined::Zoom => PredefinedMenuItem::maximize(app, Some("Zoom")),
        Predefined::Fullscreen => PredefinedMenuItem::fullscreen(app, None),
    }
}

/// Only web links, and only from menu items we built.
fn open_url(url: &str) {
    if !url.starts_with("https://") {
        return;
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(url).spawn();
}
