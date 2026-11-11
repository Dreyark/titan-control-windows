//! System tray on Windows (docs/windows-port.md): `tray-icon` on its own thread with its own Win32 message loop, so the
//! GLib main loop of the window never has to pump tray messages. Same public API as the Linux `tray.rs`, plus the
//! monitor menu and the "stop writing" action.
//!
//! The menu cannot hold sliders: like on Linux each slider is a submenu of ready values.

use crate::app_settings::TrayIconStyle;
use crate::i18n::{AppLang, tr};
use adw::prelude::*;
use gtk4::glib;
use libadwaita as adw;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};

#[derive(Clone)]
enum TrayAction {
    Toggle,
    SelectFavorite(u16),
    SelectShortcut(String),
    SelectOption(String, usize),
    SelectMonitor(usize),
    StopWriting,
    Quit,
}

/// Messages to the tray thread (menu objects live only there).
enum TrayMsg {
    Refresh,
    SetStyle(TrayIconStyle),
    SetMonitors(Vec<(String, bool)>),
    SetWriteLevel(u8),
    /// The app is closing: the icon is removed before the process ends (otherwise it lingers in the tray).
    Quit,
}

type MonitorHandler = Rc<dyn Fn(usize)>;
type StopHandler = Rc<dyn Fn()>;

#[derive(Default)]
struct Handlers {
    monitor: Option<MonitorHandler>,
    stop_writing: Option<StopHandler>,
}

/// Changes the tray while the app runs.
#[derive(Clone)]
pub struct TrayControl {
    thread: mpsc::Sender<TrayMsg>,
    handlers: Rc<RefCell<Handlers>>,
}

impl TrayControl {
    pub fn set_style(&self, style: TrayIconStyle) {
        let _ = self.thread.send(TrayMsg::SetStyle(style));
    }

    /// Rebuilds the menu after a change it shows (window visibility, favorites, active mode).
    pub fn refresh(&self) {
        let _ = self.thread.send(TrayMsg::Refresh);
    }

    /// The monitors for the menu: label and whether it is the shown one.
    pub fn set_monitors(&self, monitors: Vec<(String, bool)>) {
        let _ = self.thread.send(TrayMsg::SetMonitors(monitors));
    }

    pub fn set_write_level(&self, level: u8) {
        let _ = self.thread.send(TrayMsg::SetWriteLevel(level));
    }

    /// What the menu entries "monitor N" and "stop writing" do.
    pub fn set_handlers(&self, on_monitor: impl Fn(usize) + 'static, on_stop_writing: impl Fn() + 'static) {
        let mut handlers = self.handlers.borrow_mut();
        handlers.monitor = Some(Rc::new(on_monitor));
        handlers.stop_writing = Some(Rc::new(on_stop_writing));
    }
}

#[derive(Clone, Debug, Default)]
pub struct TrayFavoriteProfile {
    pub mode: u16,
    pub label: String,
}

pub type TrayFavoritesState = Arc<Mutex<Vec<TrayFavoriteProfile>>>;
pub type TrayActiveModeState = Arc<Mutex<Option<u16>>>;

#[derive(Clone, Debug, Default)]
pub struct TrayShortcut {
    pub id: String,
    pub label: String,
}

pub type TrayShortcutsState = Arc<Mutex<Vec<TrayShortcut>>>;
pub type TrayWindowVisibleState = Arc<Mutex<bool>>;

#[derive(Clone, Debug)]
pub struct TrayChoiceState {
    pub hdr_selected: usize,
    pub hdr_enabled: bool,
    pub gamma_selected: usize,
    pub gamma_enabled: bool,
}

impl Default for TrayChoiceState {
    fn default() -> Self {
        Self { hdr_selected: 0, hdr_enabled: true, gamma_selected: 2, gamma_enabled: true }
    }
}

pub type TrayChoiceStateHandle = Arc<Mutex<TrayChoiceState>>;
type SelectFavoriteCallback = Rc<dyn Fn(u16)>;
type SelectShortcutCallback = Rc<dyn Fn(&str)>;
type SelectOptionCallback = Rc<dyn Fn(&str, usize)>;

// ---- what the menu is made of ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ShortcutKind {
    Slider,
    Choice,
    Toggle,
}

fn shortcut_kind(id: &str) -> ShortcutKind {
    match id {
        "brightness" | "contrast" | "sharpness" | "color_enhance" | "cr_enhance" | "shadow_balance" | "super_res"
        | "halo_control" | "low_blue_light" | "color_temp_red" | "color_temp_green" | "color_temp_blue" | "hue_red"
        | "hue_green" | "hue_blue" | "hue_cyan" | "hue_magenta" | "hue_yellow" | "saturation_red"
        | "saturation_green" | "saturation_blue" | "saturation_cyan" | "saturation_magenta" | "saturation_yellow"
        | "volume" => ShortcutKind::Slider,
        "color_temp_profile" | "source" | "input_source" | "output_range" | "power_saving" | "power_led" | "hdr"
        | "gamma" | "local_dimming" | "dynamic_od" | "dyds" | "screen_size" | "night_vision" => ShortcutKind::Choice,
        _ => ShortcutKind::Toggle,
    }
}

/// Largest value and step of the values offered for a slider in the tray menu.
fn slider_range(id: &str) -> (u16, u16) {
    match id {
        "sharpness" | "cr_enhance" | "super_res" => (5, 1),
        "low_blue_light" => (4, 1),
        "color_enhance" => (10, 1),
        _ => (100, 10),
    }
}

fn choices_for(id: &str) -> Option<&'static [&'static str]> {
    match id {
        "color_temp_profile" => Some(&["Warm", "Natural", "Cold", "User 1", "User 2", "User 3"]),
        "screen_size" => Some(&["Wide", "25\""]),
        "night_vision" => Some(&["Off", "Lvl 1", "Lvl 2", "Auto 1", "Auto 2"]),
        "hdr" => Some(&["Off", "Auto", "Game", "Movie"]),
        "gamma" => Some(&["1.8", "2.0", "2.2", "2.4", "2.6", "S Curve"]),
        "local_dimming" => Some(&["Off", "Low", "Smooth", "Medium", "High"]),
        "dynamic_od" => Some(&["Off", "Lvl 1", "Lvl 2", "Lvl 3", "Top Speed"]),
        "dyds" => Some(&["Off", "Low", "Med", "High", "ULL-1", "ULL-2", "ULL-3"]),
        "output_range" => Some(&["Auto", "Limited", "Full"]),
        "power_saving" => Some(&["Off", "Lvl 1", "Lvl 2"]),
        "power_led" => Some(&["Off", "Lvl 1", "Lvl 2", "Lvl 3"]),
        "input_source" | "source" => Some(&["DisplayPort", "USB-C", "HDMI 1", "HDMI 2"]),
        _ => None,
    }
}

fn selected_choice(id: &str, choices: &TrayChoiceState) -> Option<usize> {
    match id {
        "hdr" => Some(choices.hdr_selected),
        "gamma" => Some(choices.gamma_selected),
        _ => None,
    }
}

fn choice_enabled(id: &str, choices: &TrayChoiceState) -> bool {
    match id {
        "hdr" => choices.hdr_enabled,
        "gamma" => choices.gamma_enabled,
        _ => true,
    }
}

/// Everything the menu shows; shared with the window thread.
struct Shared {
    lang: AppLang,
    favorites: TrayFavoritesState,
    active_mode: TrayActiveModeState,
    shortcuts: TrayShortcutsState,
    window_visible: TrayWindowVisibleState,
    choices: TrayChoiceStateHandle,
    monitors: Vec<(String, bool)>,
    write_level: u8,
}

/// Menu plus the action behind every item (by menu id).
struct BuiltMenu {
    menu: Menu,
    actions: HashMap<String, TrayAction>,
}

fn build_menu(shared: &Shared) -> BuiltMenu {
    let menu = Menu::new();
    let mut actions: HashMap<String, TrayAction> = HashMap::new();
    let mut counter = 0usize;
    let mut next_id = |action: TrayAction, actions: &mut HashMap<String, TrayAction>| {
        counter += 1;
        let id = format!("item{counter}");
        actions.insert(id.clone(), action);
        id
    };
    let none = None::<tray_icon::menu::accelerator::Accelerator>;

    let favorites = shared.favorites.lock().map(|f| f.clone()).unwrap_or_default();
    let active_mode = shared.active_mode.lock().map(|m| *m).unwrap_or(None);
    let shortcuts = shared.shortcuts.lock().map(|s| s.clone()).unwrap_or_default();
    let window_visible = shared.window_visible.lock().map(|v| *v).unwrap_or(false);
    let choices = shared.choices.lock().map(|c| c.clone()).unwrap_or_default();
    let lang = shared.lang;
    let mut any = false;

    // Monitors.
    if shared.monitors.len() > 1 {
        let submenu = Submenu::new(tr(&lang, "tray_monitors"), true);
        for (index, (label, selected)) in shared.monitors.iter().enumerate() {
            let id = next_id(TrayAction::SelectMonitor(index), &mut actions);
            let _ = submenu.append(&CheckMenuItem::with_id(id, label, true, *selected, none));
        }
        let _ = menu.append(&submenu);
        any = true;
    }

    // Picture modes.
    for favorite in favorites {
        let id = next_id(TrayAction::SelectFavorite(favorite.mode), &mut actions);
        let _ = menu.append(&CheckMenuItem::with_id(id, &favorite.label, true, active_mode == Some(favorite.mode), none));
        any = true;
    }

    // Sliders: submenus of ready values.
    let sliders: Vec<_> = shortcuts.iter().filter(|s| shortcut_kind(&s.id) == ShortcutKind::Slider).collect();
    if !sliders.is_empty() {
        let outer = Submenu::new(tr(&lang, "tray_sliders"), true);
        for shortcut in sliders {
            let inner = Submenu::new(&shortcut.label, true);
            let (max, step) = slider_range(&shortcut.id);
            for value in (0..=max).step_by(step as usize) {
                let id = next_id(TrayAction::SelectOption(shortcut.id.clone(), value as usize), &mut actions);
                let _ = inner.append(&MenuItem::with_id(id, value.to_string(), true, none));
            }
            let _ = outer.append(&inner);
        }
        let _ = menu.append(&outer);
        any = true;
    }

    // Choices.
    let choice_shortcuts: Vec<_> = shortcuts.iter().filter(|s| shortcut_kind(&s.id) == ShortcutKind::Choice).collect();
    if !choice_shortcuts.is_empty() {
        let outer = Submenu::new(tr(&lang, "tray_options"), true);
        for shortcut in choice_shortcuts {
            match choices_for(&shortcut.id) {
                Some(options) => {
                    let enabled = choice_enabled(&shortcut.id, &choices);
                    let selected = selected_choice(&shortcut.id, &choices);
                    let inner = Submenu::new(&shortcut.label, enabled);
                    for (index, option) in options.iter().enumerate() {
                        let id = next_id(TrayAction::SelectOption(shortcut.id.clone(), index), &mut actions);
                        let _ = inner.append(&CheckMenuItem::with_id(id, option, enabled, selected == Some(index), none));
                    }
                    let _ = outer.append(&inner);
                }
                None => {
                    let id = next_id(TrayAction::SelectShortcut(shortcut.id.clone()), &mut actions);
                    let _ = outer.append(&MenuItem::with_id(id, &shortcut.label, true, none));
                }
            }
        }
        let _ = menu.append(&outer);
        any = true;
    }

    // Toggles.
    let toggles: Vec<_> = shortcuts.iter().filter(|s| shortcut_kind(&s.id) == ShortcutKind::Toggle).collect();
    if !toggles.is_empty() {
        let outer = Submenu::new(tr(&lang, "tray_toggles"), true);
        for shortcut in toggles {
            let id = next_id(TrayAction::SelectShortcut(shortcut.id.clone()), &mut actions);
            let _ = outer.append(&MenuItem::with_id(id, &shortcut.label, true, none));
        }
        let _ = menu.append(&outer);
        any = true;
    }

    // Writing: while it is on, a one-click way back to read-only (the emergency stop).
    if shared.write_level > 0 {
        let _ = menu.append(&PredefinedMenuItem::separator());
        let state = tr(&lang, "tray_write_state").replace("{level}", &shared.write_level.to_string());
        let _ = menu.append(&MenuItem::new(state, false, none));
        let id = next_id(TrayAction::StopWriting, &mut actions);
        let _ = menu.append(&MenuItem::with_id(id, tr(&lang, "tray_stop_writing"), true, none));
        any = false;
    }

    if any {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let id = next_id(TrayAction::Toggle, &mut actions);
    let _ = menu.append(&MenuItem::with_id(id, tr(&lang, if window_visible { "tray_hide" } else { "tray_show" }), true, none));
    let _ = menu.append(&PredefinedMenuItem::separator());
    let id = next_id(TrayAction::Quit, &mut actions);
    let _ = menu.append(&MenuItem::with_id(id, tr(&lang, "tray_quit"), true, none));
    BuiltMenu { menu, actions }
}

// ---- icon -----------------------------------------------------------------------------------------------------

/// A small monitor drawn in one color (white for dark taskbars, black for light ones).
pub fn draw_icon(white: bool) -> (Vec<u8>, u32, u32) {
    const SIZE: u32 = 32;
    let color = if white { 255u8 } else { 0u8 };
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let mut put = |x: u32, y: u32, alpha: u8| {
        let i = ((y * SIZE + x) * 4) as usize;
        rgba[i..i + 4].copy_from_slice(&[color, color, color, alpha]);
    };
    for y in 4..23u32 {
        for x in 2..30u32 {
            // Rounded corners: the four corner pixels stay empty.
            if (x == 2 || x == 29) && (y == 4 || y == 22) {
                continue;
            }
            let screen = (4..28).contains(&x) && (6..21).contains(&y);
            put(x, y, if screen { 70 } else { 255 });
        }
    }
    for y in 23..27u32 {
        for x in 14..18u32 {
            put(x, y, 255);
        }
    }
    for y in 27..29u32 {
        for x in 8..24u32 {
            put(x, y, 255);
        }
    }
    (rgba, SIZE, SIZE)
}

fn icon_for(style: TrayIconStyle) -> Option<Icon> {
    let white = match style {
        TrayIconStyle::Light => true,
        TrayIconStyle::Dark => false,
        // Follow the taskbar: a light taskbar needs a dark icon.
        TrayIconStyle::Theme => !crate::platform::windows::system_uses_light_theme().unwrap_or(false),
    };
    let (rgba, w, h) = draw_icon(white);
    Icon::from_rgba(rgba, w, h).ok()
}

// ---- thread ---------------------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn setup_tray(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    hold_guard: gtk4::gio::ApplicationHoldGuard,
    lang: AppLang,
    favorites: TrayFavoritesState,
    active_mode: TrayActiveModeState,
    shortcuts: TrayShortcutsState,
    window_visible: TrayWindowVisibleState,
    choices: TrayChoiceStateHandle,
    icon_style: TrayIconStyle,
    on_select_favorite: SelectFavoriteCallback,
    on_select_shortcut: SelectShortcutCallback,
    on_select_option: SelectOptionCallback,
) -> TrayControl {
    let (action_tx, action_rx) = async_channel::unbounded::<TrayAction>();
    let (thread_tx, thread_rx) = mpsc::channel::<TrayMsg>();
    let shared = Shared {
        lang,
        favorites,
        active_mode,
        shortcuts,
        window_visible,
        choices,
        monitors: Vec::new(),
        write_level: 0,
    };

    std::thread::spawn(move || run_tray_thread(shared, icon_style, thread_rx, action_tx));

    let handlers: Rc<RefCell<Handlers>> = Rc::default();
    let quit_tx = thread_tx.clone();
    app.connect_shutdown(move |_| {
        let _ = quit_tx.send(TrayMsg::Quit);
        // The tray thread polls every 25 ms; give it a moment to remove the icon.
        std::thread::sleep(Duration::from_millis(250));
    });
    let control = TrayControl { thread: thread_tx, handlers: handlers.clone() };

    let window = window.clone();
    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        let _hold = hold_guard;
        while let Ok(action) = action_rx.recv().await {
            match action {
                TrayAction::Toggle => {
                    if !window.is_visible() {
                        window.set_visible(true);
                        window.present();
                    } else {
                        window.set_visible(false);
                    }
                }
                TrayAction::SelectFavorite(mode) => on_select_favorite(mode),
                TrayAction::SelectShortcut(id) => on_select_shortcut(&id),
                TrayAction::SelectOption(id, index) => on_select_option(&id, index),
                TrayAction::SelectMonitor(index) => {
                    let handler = handlers.borrow().monitor.clone();
                    if let Some(handler) = handler {
                        handler(index);
                    }
                }
                TrayAction::StopWriting => {
                    let handler = handlers.borrow().stop_writing.clone();
                    if let Some(handler) = handler {
                        handler();
                    }
                }
                TrayAction::Quit => {
                    // Leaving: writing is switched off first; the monitor handles are released with the process.
                    crate::monitor::set_user_write_level(0);
                    app.quit();
                }
            }
        }
    });
    control
}

fn run_tray_thread(
    mut shared: Shared,
    style: TrayIconStyle,
    messages: mpsc::Receiver<TrayMsg>,
    actions_out: async_channel::Sender<TrayAction>,
) {
    let actions: Arc<Mutex<HashMap<String, TrayAction>>> = Arc::default();

    let sender = actions_out.clone();
    let lookup = actions.clone();
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        let action = lookup.lock().ok().and_then(|map| map.get(&event.id.0).cloned());
        if let Some(action) = action {
            let _ = sender.send_blocking(action);
        }
    }));
    let sender = actions_out;
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| match event {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. }
        | TrayIconEvent::DoubleClick { button: MouseButton::Left, .. } => {
            let _ = sender.send_blocking(TrayAction::Toggle);
        }
        _ => {}
    }));

    let built = build_menu(&shared);
    if let Ok(mut map) = actions.lock() {
        *map = built.actions;
    }
    let mut builder = TrayIconBuilder::new().with_menu(Box::new(built.menu)).with_tooltip(crate::app_settings::APP_DISPLAY_NAME);
    if let Some(icon) = icon_for(style) {
        builder = builder.with_icon(icon);
    }
    let tray = match builder.build() {
        Ok(tray) => tray,
        Err(error) => {
            log!("[TRAY] cannot create the tray icon: {error}");
            return;
        }
    };
    let rebuild = |shared: &Shared, tray: &tray_icon::TrayIcon| {
        let built = build_menu(shared);
        if let Ok(mut map) = actions.lock() {
            *map = built.actions;
        }
        tray.set_menu(Some(Box::new(built.menu)));
    };

    loop {
        // The message loop of this thread: tray clicks and menu messages arrive here.
        let mut msg = MSG::default();
        // SAFETY: standard message pump on the thread that owns the tray window.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut changed = false;
        while let Ok(message) = messages.try_recv() {
            match message {
                TrayMsg::Refresh => changed = true,
                TrayMsg::SetStyle(style) => {
                    let _ = tray.set_icon(icon_for(style));
                }
                TrayMsg::SetMonitors(monitors) => {
                    shared.monitors = monitors;
                    changed = true;
                }
                TrayMsg::SetWriteLevel(level) => {
                    shared.write_level = level;
                    changed = true;
                }
                TrayMsg::Quit => return,
            }
        }
        if changed {
            rebuild(&shared, &tray);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_square_rgba_with_content() {
        for white in [true, false] {
            let (rgba, w, h) = draw_icon(white);
            assert_eq!((w, h), (32, 32));
            assert_eq!(rgba.len(), (w * h * 4) as usize);
            assert!(rgba.chunks(4).any(|px| px[3] == 255), "the icon has opaque pixels");
            let color = if white { 255 } else { 0 };
            assert!(rgba.chunks(4).filter(|px| px[3] > 0).all(|px| px[0] == color), "one color only");
        }
        assert!(Icon::from_rgba(draw_icon(true).0, 32, 32).is_ok());
    }

    #[test]
    fn every_shortcut_kind_has_a_home() {
        assert_eq!(shortcut_kind("brightness"), ShortcutKind::Slider);
        assert_eq!(shortcut_kind("gamma"), ShortcutKind::Choice);
        assert_eq!(shortcut_kind("fps_counter"), ShortcutKind::Toggle);
        assert!(choices_for("gamma").is_some() && choices_for("fps_counter").is_none());
    }
}

/// The application icon (a blue rounded square with a white monitor), drawn at any size with simple geometry.
#[cfg(test)]
fn draw_app_icon(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    // Inside a rounded rectangle with the given corner radius (all in 0..1 units).
    let in_rounded = |x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32| {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    };
    for py in 0..size {
        for px in 0..size {
            let (x, y) = ((px as f32 + 0.5) / s, (py as f32 + 0.5) / s);
            let i = ((py * size + px) * 4) as usize;
            let pixel: [u8; 4] = if in_rounded(x, y, 0.04, 0.04, 0.96, 0.96, 0.2) {
                // Background: blue, a little lighter at the top.
                let lift = ((1.0 - y) * 30.0) as u8;
                let mut color = [24 + lift / 2, 72 + lift, 170 + lift / 2, 255];
                let outer = in_rounded(x, y, 0.16, 0.2, 0.84, 0.66, 0.05);
                let screen = in_rounded(x, y, 0.21, 0.25, 0.79, 0.61, 0.02);
                let stand = (0.45..=0.55).contains(&x) && (0.66..=0.76).contains(&y);
                let base = in_rounded(x, y, 0.3, 0.76, 0.7, 0.82, 0.03);
                if outer && !screen || stand || base {
                    color = [255, 255, 255, 255];
                } else if screen {
                    color = [14, 40, 110, 255];
                }
                color
            } else {
                [0, 0, 0, 0]
            };
            rgba[i..i + 4].copy_from_slice(&pixel);
        }
    }
    rgba
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    /// Regenerates `assets/windows/titan_control.ico`: `cargo test write_app_icon -- --ignored`.
    #[test]
    #[ignore]
    fn write_app_icon() {
        use image::codecs::ico::{IcoEncoder, IcoFrame};
        let frames: Vec<IcoFrame> = [16u32, 24, 32, 48, 64, 128, 256]
            .iter()
            .map(|&size| IcoFrame::as_png(&draw_app_icon(size), size, size, image::ExtendedColorType::Rgba8).expect("frame"))
            .collect();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/windows/titan_control.ico");
        let file = std::fs::File::create(&path).expect("create the icon file");
        IcoEncoder::new(file).encode_images(&frames).expect("encode");
        println!("written {}", path.display());
        // A preview to look at (in `target/`, not committed).
        let preview = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/app_icon_preview.png");
        image::save_buffer(preview, &draw_app_icon(256), 256, 256, image::ColorType::Rgba8).expect("preview");
    }

    #[test]
    fn app_icon_has_a_shape_and_transparent_corners() {
        let rgba = draw_app_icon(64);
        assert_eq!(rgba.len(), 64 * 64 * 4);
        assert_eq!(rgba[3], 0, "the corner is transparent");
        let mid = (32 * 64 + 32) * 4;
        assert_eq!(rgba[mid + 3], 255, "the middle is opaque");
    }
}
