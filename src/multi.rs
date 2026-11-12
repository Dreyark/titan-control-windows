//! Several monitors in one window (docs/windows-port.md): the monitor switch, aliases, "identify monitors", per-monitor
//! state and errors, and the read-only mode of the controls. Every session reports with its `MonitorId`; the window
//! shows one monitor at a time (the active one) and keeps the latest state of the others.

use crate::monitor::{self, MonitorId, MonitorInfo, SessionEvent, UiCmd, WorkerCmd};
use crate::ui::bindings;
use crate::{App, i18n::tr};
use adw::prelude::*;
use gtk4::{gdk, glib};
use libadwaita as adw;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

/// What the window knows about one monitor.
pub struct MonitorView {
    pub id: MonitorId,
    pub info: Option<MonitorInfo>,
    /// Latest settings (cached start values, reads and corrections), shown again when the monitor is selected.
    pub settings: HashMap<u8, u16>,
    /// The session could not connect.
    pub failed: bool,
    /// Last error of the session (shown next to the monitor in the list).
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Monitors {
    views: RefCell<Vec<MonitorView>>,
    active: RefCell<Option<MonitorId>>,
    /// The monitor shown last time (from the settings); chosen again if it connects.
    preferred: RefCell<Option<String>>,
    /// Number of monitors announced by the sessions (0 = not announced, i.e. Linux with its single monitor).
    expected: Cell<usize>,
    /// "Monitor not found" reports of sessions that have no view (e.g. nothing was detected at all).
    failed_without_view: Cell<usize>,
    /// Controls greyed out because of the write level, so they can be released again.
    disabled: RefCell<Vec<Disabled>>,
    write_rows: RefCell<Option<WriteRows>>,
    /// Windows: the HDR switch of the display group.
    hdr_row: RefCell<Option<adw::SwitchRow>>,
    /// Windows: the "AI Dual Mode" status row of the display group.
    dual_row: RefCell<Option<adw::ActionRow>>,
    /// Rows created in code (see [`setup`]).
    rows: RefCell<Option<Rows>>,
}

struct WriteRows {
    switch: adw::SwitchRow,
    level: adw::ComboRow,
}

struct Rows {
    alias: adw::EntryRow,
    screen: adw::ActionRow,
    serial: adw::ActionRow,
    firmware: adw::ActionRow,
}

impl Monitors {
    #[allow(dead_code)]
    pub fn is_read_only(&self) -> bool {
        monitor::write_policy_active() && monitor::user_write_level() == 0
    }

    pub fn active(&self) -> Option<MonitorId> {
        self.active.borrow().clone()
    }
}

fn view_index(a: &App, id: &MonitorId) -> usize {
    let mut views = a.monitors.views.borrow_mut();
    if let Some(index) = views.iter().position(|view| &view.id == id) {
        return index;
    }
    views.push(MonitorView { id: id.clone(), info: None, settings: HashMap::new(), failed: false, error: None });
    views.len() - 1
}

/// Name shown for a monitor: its alias, else model and the tail of its device path.
pub fn display_name(a: &App, id: &MonitorId) -> String {
    a.cfg
        .borrow()
        .monitor_aliases
        .get(&id.cache_key())
        .filter(|alias| !alias.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| id.label())
}

fn list_entry(a: &App, view: &MonitorView) -> String {
    let lang = a.lang();
    let mut text = display_name(a, &view.id);
    if view.error.as_deref() == Some("disconnected") {
        text.push_str(&format!(" · {}", tr(&lang, "monitor_disconnected")));
    } else if view.failed || view.error.is_some() {
        text.push_str(&format!(" · {}", tr(&lang, "monitor_offline")));
    } else if view.info.is_none() {
        text.push_str(&format!(" · {}", tr(&lang, "monitor_connecting")));
    }
    text
}

/// Rebuilds the monitor list of the sidebar; it is visible only when there is a choice. Done from an idle callback:
/// it is often requested from inside the handler of the list itself, and a list must not change under its handler.
pub fn refresh_list(a: &Rc<App>) {
    let a = a.clone();
    glib::idle_add_local_once(move || refresh_list_now(&a));
}

fn refresh_list_now(a: &Rc<App>) {
    let select: gtk4::DropDown = a.ui.get("monitor_select");
    let views = a.monitors.views.borrow();
    let entries: Vec<String> = views.iter().map(|view| list_entry(a, view)).collect();
    let refs: Vec<&str> = entries.iter().map(String::as_str).collect();
    let active = a.monitors.active.borrow().clone();
    let selected = active.and_then(|id| views.iter().position(|view| view.id == id));
    bindings::suppress(|| {
        match select.model().and_downcast::<gtk4::StringList>() {
            Some(model) => model.splice(0, model.n_items(), &refs),
            None => select.set_model(Some(&gtk4::StringList::new(&refs))),
        }
        if let Some(index) = selected {
            select.set_selected(index as u32);
        }
    });
    select.set_visible(views.len() > 1);
    drop(views);
    #[cfg(windows)]
    sync_tray(a);
}

/// Shows the data of the active monitor in the rows that describe it.
fn show_identity(a: &Rc<App>, view: &MonitorView) {
    let rows = a.monitors.rows.borrow();
    let Some(rows) = rows.as_ref() else { return };
    let name = display_name(a, &view.id);
    bindings::suppress(|| {
        let alias = a.cfg.borrow().monitor_aliases.get(&view.id.cache_key()).cloned().unwrap_or_default();
        rows.alias.set_text(&alias);
    });
    let info = view.info.as_ref();
    rows.screen.set_subtitle(info.map(|i| i.connector.as_str()).filter(|c| !c.is_empty()).unwrap_or("—"));
    rows.serial.set_subtitle(&Some(view.id.serial.as_str()).filter(|s| !s.is_empty()).map_or("—".to_string(), |s| s.to_string()));
    rows.firmware.set_subtitle(info.map(|i| i.firm.as_str()).unwrap_or("—"));
    a.ui.get::<gtk4::Label>("ov_monitor_title").set_label(&name);
}

/// Makes `id` the monitor the window shows and sends its commands to. `first` = the first activation (closes the
/// splash window and presents the main window).
fn activate(a: &Rc<App>, id: &MonitorId, first: bool, splash: Option<&adw::Window>) {
    let (info, settings) = {
        let views = a.monitors.views.borrow();
        let Some(view) = views.iter().find(|view| &view.id == id) else { return };
        let Some(info) = view.info.clone() else { return };
        (info, view.settings.clone())
    };
    *a.monitors.active.borrow_mut() = Some(id.clone());
    log!("[UI] shown monitor: {}", id.label());
    let _ = a.tx().send(WorkerCmd::SelectMonitor(id.clone()));
    *a.connector.borrow_mut() = info.connector.clone();
    crate::show_monitor_info(a, &info);
    // A switch always shows the stored values; the first activation shows them as ordinary reads.
    a.bindings.apply(&settings, first);
    *a.info.borrow_mut() = Some(info);
    if let Some(view) = a.monitors.views.borrow().iter().find(|view| &view.id == id) {
        show_identity(a, view);
    }
    crate::refresh_system_display(a);
    crate::sync_state(a);
    refresh_list(a);
    if first {
        if let Some(splash) = splash {
            splash.close();
        }
        if !a.cfg.borrow().start_minimized {
            a.ui.window.present();
        }
    }
}

/// Handles one event of a session.
pub fn handle_event(a: &Rc<App>, event: SessionEvent, splash: &(adw::Window, gtk4::Label), app: &adw::Application) {
    let (splash_window, splash_label) = splash;
    let SessionEvent { monitor: id, cmd } = event;
    match cmd {
        UiCmd::Detected(ids) => {
            // The authoritative list of the monitors that exist now: a monitor that is not in it has been unplugged.
            a.monitors.expected.set(ids.len());
            let preferred = a.cfg.borrow().last_selected_monitor.clone();
            *a.monitors.preferred.borrow_mut() = preferred;
            a.monitors.views.borrow_mut().retain(|view| ids.contains(&view.id));
            for id in &ids {
                view_index(a, id);
            }
            let active_gone = a.monitors.active.borrow().as_ref().is_some_and(|active| !ids.contains(active));
            if active_gone {
                *a.monitors.active.borrow_mut() = None;
                let fallback = a
                    .monitors
                    .views
                    .borrow()
                    .iter()
                    .find(|view| view.info.is_some() && view.error.is_none())
                    .map(|view| view.id.clone());
                match fallback {
                    Some(other) => activate(a, &other, false, None),
                    None => {
                        *a.info.borrow_mut() = None;
                        a.ui.get::<gtk4::Label>("ov_monitor_subtitle").set_label(&tr(&a.lang(), "monitor_disconnected"));
                    }
                }
            }
            refresh_list(a);
        }
        UiCmd::MonitorFound(info) => {
            let index = view_index(a, &id);
            {
                let mut views = a.monitors.views.borrow_mut();
                let view = &mut views[index];
                view.settings.extend(info.settings.clone());
                view.info = Some(info);
                view.failed = false;
                view.error = None;
            }
            if a.monitors.active.borrow().is_none() && wants_to_be_shown(a, &id) {
                activate(a, &id, true, Some(splash_window));
            } else if is_active(a, &id) {
                // The shown monitor came back (cable, sleep): show its fresh state.
                activate(a, &id, false, None);
            } else {
                refresh_list(a);
            }
        }
        UiCmd::Settings(values) | UiCmd::Corrected(values) if !is_known(a, &id) => drop(values),
        UiCmd::Settings(values) => {
            let index = view_index(a, &id);
            a.monitors.views.borrow_mut()[index].settings.extend(values.clone());
            if is_active(a, &id) {
                a.bindings.apply(&values, true);
            }
        }
        UiCmd::Corrected(values) => {
            let index = view_index(a, &id);
            a.monitors.views.borrow_mut()[index].settings.extend(values.clone());
            if is_active(a, &id) {
                a.bindings.apply(&values, false);
            }
        }
        UiCmd::Responding(responding) => {
            let index = view_index(a, &id);
            a.monitors.views.borrow_mut()[index].error = (!responding).then(|| "disconnected".to_string());
            refresh_list(a);
            if is_active(a, &id) {
                let subtitle: gtk4::Label = a.ui.get("ov_monitor_subtitle");
                if responding {
                    if let Some(info) = a.info.borrow().as_ref() {
                        crate::show_monitor_info(a, info);
                    }
                } else {
                    subtitle.set_label(&tr(&a.lang(), "monitor_disconnected"));
                }
            }
        }
        UiCmd::ProfileLoaded => {
            if is_active(a, &id) {
                crate::finish_profile_load(a);
            }
        }
        UiCmd::Progress(message) => {
            if a.monitors.active.borrow().is_none() {
                splash_label.set_label(&crate::translate_startup_message(&a.lang(), &message));
            }
        }
        UiCmd::Error(error) if error == "Monitor not found" => {
            match a.monitors.views.borrow_mut().iter_mut().find(|view| view.id == id) {
                Some(view) => view.failed = true,
                None => a.monitors.failed_without_view.set(a.monitors.failed_without_view.get() + 1),
            }
            if a.monitors.active.borrow().is_none() {
                // The shown monitor failed: show another one that is already connected.
                let fallback = a
                    .monitors
                    .views
                    .borrow()
                    .iter()
                    .find(|view| view.info.is_some() && !view.failed)
                    .map(|view| view.id.clone());
                if let Some(other) = fallback {
                    activate(a, &other, true, Some(splash_window));
                    return;
                }
            }
            refresh_list(a);
            let failed = a.monitors.views.borrow().iter().filter(|view| view.failed).count()
                + a.monitors.failed_without_view.get();
            if a.monitors.active.borrow().is_none() && failed >= a.monitors.expected.get().max(1) {
                splash_label.set_label(&crate::translate_startup_message(&a.lang(), &error));
                if !splash_window.is_visible() {
                    splash_window.present();
                }
                let app = app.clone();
                glib::timeout_add_local_once(Duration::from_secs(3), move || app.quit());
            }
        }
        // Writes are disabled in this build; the controls are greyed out, so this only logs a stray write.
        UiCmd::Error(error) if error.starts_with("Write") => {
            log!("[UI] {}: {error}", id.label());
        }
        UiCmd::Error(error) => {
            let index = view_index(a, &id);
            a.monitors.views.borrow_mut()[index].error = Some(error.clone());
            refresh_list(a);
            if is_active(a, &id) {
                splash_label.set_label(&crate::translate_startup_message(&a.lang(), &error));
                if !splash_window.is_visible() {
                    splash_window.present();
                }
            }
        }
        UiCmd::UsageTimeUpdated(usage_mins) => {
            let index = view_index(a, &id);
            if let Some(info) = a.monitors.views.borrow_mut()[index].info.as_mut() {
                info.usage_mins = usage_mins;
            }
            if is_active(a, &id) {
                a.ui.get::<adw::ActionRow>("info_usage").set_subtitle(&crate::format_usage_minutes(usage_mins));
            }
        }
        UiCmd::InfoRefreshed { hz, usage_mins } => {
            let index = view_index(a, &id);
            {
                let mut views = a.monitors.views.borrow_mut();
                let Some(info) = views[index].info.as_mut() else { return };
                if let Some(hz) = hz {
                    info.hz = hz;
                }
                if let Some(usage_mins) = usage_mins {
                    info.usage_mins = usage_mins;
                }
            }
            if is_active(a, &id) {
                let mut info = a.info.borrow_mut();
                let Some(info) = info.as_mut() else { return };
                if let Some(hz) = hz {
                    info.hz = hz;
                }
                if let Some(usage_mins) = usage_mins {
                    info.usage_mins = usage_mins;
                }
                if let Some(state) = a.system_display.borrow().as_ref() {
                    info.resolution = state.current_resolution.clone();
                }
                crate::show_monitor_info(a, info);
                let views = a.monitors.views.borrow();
                if let Some(view) = views.iter().find(|view| view.id == id) {
                    show_identity(a, view);
                }
            }
        }
    }
}

fn is_known(a: &App, id: &MonitorId) -> bool {
    a.monitors.views.borrow().iter().any(|view| &view.id == id)
}

fn is_active(a: &App, id: &MonitorId) -> bool {
    a.monitors.active.borrow().as_ref() == Some(id)
}

/// The first monitor to show: the one shown last time, or any if that one did not connect (or is not known).
fn wants_to_be_shown(a: &App, id: &MonitorId) -> bool {
    let preferred = a.monitors.preferred.borrow().clone();
    match preferred {
        None => true,
        Some(key) => {
            let views = a.monitors.views.borrow();
            id.cache_key() == key
                || !views.iter().any(|view| view.id.cache_key() == key)
                || views.iter().any(|view| view.id.cache_key() == key && view.failed)
        }
    }
}

/// Builds the rows and handlers of the multi-monitor UI. Called once, after the window exists.
pub fn setup(a: &Rc<App>) {
    #[cfg(windows)]
    setup_display_extras(a);
    #[cfg(windows)]
    setup_font_size(a);
    let lang = a.lang();
    let model_row: adw::ActionRow = a.ui.get("info_model");
    let group = model_row.ancestor(adw::PreferencesGroup::static_type()).and_downcast::<adw::PreferencesGroup>();
    let property_row = |title: &str| {
        let row = adw::ActionRow::builder().title(title).subtitle("—").build();
        row.add_css_class("property");
        row.add_css_class("keep-enabled");
        row
    };

    let alias = adw::EntryRow::builder().title(tr(&lang, "monitor_alias")).show_apply_button(true).build();
    alias.add_css_class("keep-enabled");
    let screen = property_row(&tr(&lang, "info_screen"));
    let serial = property_row(&tr(&lang, "info_serial"));
    let firmware = property_row(&tr(&lang, "info_firmware"));

    let identify_button = gtk4::Button::builder().label(tr(&lang, "identify_button")).valign(gtk4::Align::Center).build();
    let identify_row = adw::ActionRow::builder()
        .title(tr(&lang, "identify_title"))
        .subtitle(tr(&lang, "identify_subtitle"))
        .build();
    identify_row.add_suffix(&identify_button);
    identify_row.set_activatable_widget(Some(&identify_button));
    identify_row.add_css_class("keep-enabled");

    if let Some(group) = &group {
        group.add(&alias);
        group.add(&screen);
        group.add(&serial);
        group.add(&firmware);
        group.add(&identify_row);
    }

    // Writing: off at every start; switched on (and raised, one level per consent) in the window.
    if monitor::write_policy_active() {
        let switch = adw::SwitchRow::builder()
            .title(tr(&lang, "write_enable_title"))
            .subtitle(tr(&lang, "write_enable_subtitle_off"))
            .build();
        switch.add_css_class("keep-enabled");
        let levels = [write_level_label(a, 1), write_level_label(a, 2), write_level_label(a, 3)];
        let level_refs: Vec<&str> = levels.iter().map(String::as_str).collect();
        let level = adw::ComboRow::builder()
            .title(tr(&lang, "write_level_title"))
            .model(&gtk4::StringList::new(&level_refs))
            .sensitive(false)
            .build();
        level.add_css_class("keep-enabled");
        if let Some(group) = &group {
            group.add(&switch);
            group.add(&level);
        }
        *a.monitors.write_rows.borrow_mut() = Some(WriteRows { switch: switch.clone(), level: level.clone() });
        a.bindings.set_write_filter(Some(Rc::new(|code| monitor::write_allowed(code, 0))));

        {
            let a2 = a.clone();
            switch.connect_active_notify(move |row| {
                if bindings::suppressed() {
                    return;
                }
                if row.is_active() {
                    // Switching on needs a consent; until it is given the switch goes back.
                    let a3 = a2.clone();
                    confirm_write_level(&a2, 1, move |accepted| {
                        set_write_level(&a3, if accepted { 1 } else { 0 });
                    });
                } else {
                    // The emergency stop needs no question.
                    set_write_level(&a2, 0);
                }
            });
        }
        {
            let a2 = a.clone();
            level.connect_selected_notify(move |row| {
                if bindings::suppressed() {
                    return;
                }
                let wanted = row.selected() as u8 + 1;
                let current = monitor::user_write_level();
                if wanted <= current {
                    set_write_level(&a2, wanted);
                } else {
                    // One level per consent.
                    let a3 = a2.clone();
                    confirm_write_level(&a2, wanted, move |accepted| {
                        set_write_level(&a3, if accepted { wanted } else { current });
                    });
                }
            });
        }
        set_write_level(a, 0);
    }
    *a.monitors.rows.borrow_mut() = Some(Rows { alias: alias.clone(), screen, serial, firmware });

    // Alias: saved under the monitor's key and shown in the list and the title.
    {
        let a2 = a.clone();
        alias.connect_apply(move |row| {
            if bindings::suppressed() {
                return;
            }
            let Some(id) = a2.monitors.active() else { return };
            let text = row.text().trim().to_string();
            persist(&a2, |cfg| {
                if text.is_empty() {
                    cfg.monitor_aliases.remove(&id.cache_key());
                } else {
                    cfg.monitor_aliases.insert(id.cache_key(), text.clone());
                }
            });
            refresh_list(&a2);
            a2.ui.get::<gtk4::Label>("ov_monitor_title").set_label(&display_name(&a2, &id));
        });
    }

    // Monitor switch.
    {
        let a2 = a.clone();
        let select: gtk4::DropDown = a.ui.get("monitor_select");
        select.connect_selected_notify(move |select| {
            if bindings::suppressed() {
                return;
            }
            select_monitor_index(&a2, select.selected() as usize);
        });
    }

    // Identify: no DDC traffic, only windows on the screens.
    {
        let a2 = a.clone();
        identify_button.connect_clicked(move |_| identify(&a2));
    }
}

/// A control that was greyed out by [`enforce_read_only`], with the tooltip it had.
struct Disabled {
    widget: glib::WeakRef<gtk4::Widget>,
    tooltip: Option<glib::GString>,
}

fn write_level_label(a: &App, level: u8) -> String {
    tr(&a.lang(), &format!("write_level_{level}"))
}

/// Greys out the controls whose settings may not be written at the current write level (Windows) and says why.
/// Called again after every state update, because dependencies between controls enable rows now and then.
pub fn enforce_read_only(a: &Rc<App>) {
    if !monitor::write_policy_active() {
        return;
    }
    let level = monitor::user_write_level();
    let lang = a.lang();
    let tooltip = if level == 0 {
        tr(&lang, "read_only_tooltip")
    } else {
        tr(&lang, "write_level_tooltip")
    };
    // Rows whose setting may be written now stay as the dependencies between settings left them.
    let mut allowed = a.bindings.rows_where(|code| monitor::write_allowed(code, level));
    if level >= 2 {
        for id in ["picture_mode", "row_profile_variant"] {
            allowed.push(a.ui.get::<gtk4::Widget>(id));
        }
    }
    // A container of an allowed row (an expander around a slider) must stay usable as well.
    let mut keep: Vec<gtk4::Widget> = Vec::new();
    for row in &allowed {
        let mut parent = row.parent();
        while let Some(p) = parent {
            keep.push(p.clone());
            parent = p.parent();
        }
    }
    let stack: gtk4::Widget = a.ui.get::<gtk4::Stack>("stack").upcast();
    let mut widget = stack.first_child();
    while let Some(page) = widget {
        disable_writers(&page, &tooltip, &allowed, &keep, &a.monitors.disabled);
        widget = page.next_sibling();
    }
}

/// Undoes [`enforce_read_only`] (before the next state update decides again).
fn release_disabled(a: &App) {
    for item in a.monitors.disabled.borrow_mut().drain(..) {
        if let Some(widget) = item.widget.upgrade() {
            widget.set_sensitive(true);
            widget.set_tooltip_text(item.tooltip.as_deref());
        }
    }
}

/// A new write level: shared with every session at once, controls follow.
pub fn set_write_level(a: &Rc<App>, level: u8) {
    monitor::set_user_write_level(level);
    let level = monitor::user_write_level();
    a.bindings.set_write_filter(Some(Rc::new(move |code| monitor::write_allowed(code, level))));
    release_disabled(a);
    crate::sync_state(a);
    if let Some(rows) = a.monitors.write_rows.borrow().as_ref() {
        bindings::suppress(|| {
            rows.switch.set_active(level > 0);
            rows.level.set_sensitive(level > 0);
            if level > 0 {
                rows.level.set_selected(level as u32 - 1);
            }
        });
        #[cfg(windows)]
        sync_tray(a);
        rows.switch.set_subtitle(&if level == 0 {
            tr(&a.lang(), "write_enable_subtitle_off")
        } else {
            format!("{}: {}", tr(&a.lang(), "write_enable_subtitle_on"), write_level_label(a, level))
        });
    }
}

fn is_exempt(widget: &gtk4::Widget) -> bool {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if w.has_css_class("keep-enabled") || w.is::<gtk4::MenuButton>() || w.is::<adw::EntryRow>() {
            return true;
        }
        current = w.parent();
    }
    false
}

fn is_inside(widget: &gtk4::Widget, rows: &[gtk4::Widget]) -> bool {
    let mut current = Some(widget.clone());
    while let Some(w) = current {
        if rows.contains(&w) {
            return true;
        }
        current = w.parent();
    }
    false
}

fn disable_writers(
    widget: &gtk4::Widget,
    tooltip: &str,
    allowed: &[gtk4::Widget],
    keep: &[gtk4::Widget],
    disabled: &RefCell<Vec<Disabled>>,
) {
    let writes = widget.is::<gtk4::Scale>()
        || widget.is::<gtk4::Switch>()
        || widget.is::<adw::SwitchRow>()
        || widget.is::<adw::ComboRow>()
        || widget.is::<adw::SpinRow>()
        || widget.is::<adw::ToggleGroup>()
        || widget.is::<adw::ButtonRow>()
        || widget.is::<adw::ExpanderRow>()
        || widget.is::<gtk4::DropDown>()
        || widget.is::<gtk4::ToggleButton>()
        || widget.is::<gtk4::Button>();
    let star = widget.downcast_ref::<gtk4::ToggleButton>().is_some_and(|b| b.icon_name().as_deref() == Some("starred-symbolic"));
    if writes && !star && !is_exempt(widget) && !is_inside(widget, allowed) && !keep.contains(widget) && widget.is_sensitive() {
        disabled.borrow_mut().push(Disabled { widget: widget.downgrade(), tooltip: widget.tooltip_text() });
        widget.set_sensitive(false);
        widget.set_tooltip_text(Some(tooltip));
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        disable_writers(&c, tooltip, allowed, keep, disabled);
        child = c.next_sibling();
    }
}

/// Asks for the user's consent before a higher write level (or any writing) is switched on.
fn confirm_write_level(a: &Rc<App>, level: u8, on_decision: impl Fn(bool) + 'static) {
    let lang = a.lang();
    let (heading, body, accept) = if monitor::user_write_level() == 0 {
        (tr(&lang, "write_confirm_heading"), tr(&lang, "write_confirm_body"), tr(&lang, "write_confirm_enable"))
    } else {
        (
            tr(&lang, "write_raise_heading").replace("{level}", &level.to_string()),
            tr(&lang, "write_raise_body").replace("{level}", &write_level_label(a, level)),
            tr(&lang, "write_raise_accept"),
        )
    };
    let dialog = adw::AlertDialog::builder().heading(heading).body(body).build();
    dialog.add_response("cancel", &tr(&lang, "write_confirm_cancel"));
    dialog.add_response("accept", &accept);
    dialog.set_response_appearance("accept", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| on_decision(response == "accept"));
    dialog.present(Some(&a.ui.window));
}

/// Shows the number and name of every monitor on its own screen for two seconds (no DDC traffic).
fn identify(a: &Rc<App>) {
    let Some(display) = gdk::Display::default() else { return };
    let monitors = display.monitors();
    let provider = gtk4::CssProvider::new();
    provider.load_from_string(
        ".identify-overlay { background: rgba(0, 0, 0, 0.82); } \
         .identify-number { color: white; font-size: 220px; font-weight: 800; } \
         .identify-name { color: white; font-size: 36px; font-weight: 600; } \
         .identify-device { color: rgba(255, 255, 255, 0.7); font-size: 22px; }",
    );
    gtk4::style_context_add_provider_for_display(&display, &provider, gtk4::STYLE_PROVIDER_PRIORITY_APPLICATION);

    let views = a.monitors.views.borrow();
    for (index, view) in views.iter().enumerate() {
        let connector = view.info.as_ref().map(|info| info.connector.clone()).unwrap_or_default();
        let screen = (0..monitors.n_items())
            .filter_map(|i| monitors.item(i).and_downcast::<gdk::Monitor>())
            .find(|m| !connector.is_empty() && m.connector().is_some_and(|c| c.eq_ignore_ascii_case(&connector)))
            .or_else(|| monitors.item(index as u32).and_downcast::<gdk::Monitor>());
        let Some(screen) = screen else { continue };

        let content = gtk4::Box::builder().orientation(gtk4::Orientation::Vertical).valign(gtk4::Align::Center).build();
        content.append(&gtk4::Label::builder().label((index + 1).to_string()).css_classes(["identify-number"]).build());
        content.append(&gtk4::Label::builder().label(display_name(a, &view.id)).css_classes(["identify-name"]).build());
        content.append(&gtk4::Label::builder().label(&connector).css_classes(["identify-device"]).build());
        let overlay = gtk4::Window::builder().decorated(false).child(&content).css_classes(["identify-overlay"]).build();
        overlay.set_application(a.ui.window.application().as_ref());
        overlay.fullscreen_on_monitor(&screen);
        overlay.present();
        let click = gtk4::GestureClick::new();
        let closing = overlay.clone();
        click.connect_pressed(move |_, _, _, _| closing.close());
        overlay.add_controller(click);
        let closing = overlay.clone();
        glib::timeout_add_local_once(Duration::from_secs(2), move || closing.close());
    }
}


/// Development aid (debug builds, with `TITAN_FAKE_MONITORS`): switches between the fake monitors, checks that the
/// controls follow, that writes are greyed out, runs "identify" and quits. Results go to the log (`[SELFTEST]`).
#[cfg(debug_assertions)]
pub fn selftest(a: &Rc<App>, app: &adw::Application) {
    let a = a.clone();
    let app = app.clone();
    glib::timeout_add_local_once(Duration::from_millis(2500), move || {
        let check = |name: &str, ok: bool| log!("[SELFTEST] {name}: {}", if ok { "ok" } else { "FAILED" });
        let views = a.monitors.views.borrow().len();
        check("two monitors listed", views == 2);
        let select: gtk4::DropDown = a.ui.get("monitor_select");
        check("switch visible", select.is_visible());
        let first = a.monitors.active();
        check("first monitor shown", first.is_some());
        let before = a.bindings.get(monitor::VCP_BRIGHTNESS);
        check("brightness of the first monitor (30)", before == Some(30));
        let other = if select.selected() == 0 { 1 } else { 0 };
        select.set_selected(other);
        let a2 = a.clone();
        let app2 = app.clone();
        glib::timeout_add_local_once(Duration::from_millis(400), move || {
            let check = |name: &str, ok: bool| log!("[SELFTEST] {name}: {}", if ok { "ok" } else { "FAILED" });
            check("active monitor changed", a2.monitors.active() != first_clone(&first));
            check("controls show the other monitor (brightness 80)", a2.bindings.get(monitor::VCP_BRIGHTNESS) == Some(80));
            check("title shows the monitor", !a2.ui.get::<gtk4::Label>("ov_monitor_title").label().is_empty());
            if monitor::write_policy_active() {
                let scale_ok = a2.ui.scales.first().is_some_and(|scale| scale.is_sensitive() != a2.monitors.is_read_only());
                check("sliders greyed out in read-only mode", scale_ok || !a2.monitors.is_read_only());
            } else {
                // Writing is on from the start: a slider of a Custom profile is usable.
                check("sliders are not greyed out", a2.bindings.is_sensitive(monitor::VCP_BRIGHTNESS));
            }
            // Alias.
            if let Some(id) = a2.monitors.active() {
                a2.cfg.borrow_mut().monitor_aliases.insert(id.cache_key(), "Prawy".into());
                refresh_list(&a2);
                check("alias used in the list", display_name(&a2, &id) == "Prawy");
            }
            // Write levels: one level opens only its own settings; 0 closes everything again.
            if monitor::write_policy_active() {
                set_write_level(&a2, 1);
                check("level 1: brightness may be written", a2.bindings.is_sensitive(monitor::VCP_BRIGHTNESS));
                check("level 1: volume may be written", a2.bindings.is_sensitive(monitor::VCP_VOLUME));
                check("level 1: sharpness stays greyed out", !a2.bindings.is_sensitive(monitor::VCP_SHARPNESS));
                set_write_level(&a2, 2);
                check("level 2: sharpness may be written", a2.bindings.is_sensitive(monitor::VCP_SHARPNESS));
                set_write_level(&a2, 0);
                check("level 0: brightness greyed out again", !a2.bindings.is_sensitive(monitor::VCP_BRIGHTNESS));
            }
            identify(&a2);
            let app3 = app2.clone();
            glib::timeout_add_local_once(Duration::from_millis(2600), move || {
                log!("[SELFTEST] finished");
                app3.quit();
            });
        });
    });
}

#[cfg(debug_assertions)]
fn first_clone(id: &Option<MonitorId>) -> Option<MonitorId> {
    id.clone()
}

/// Saves a settings change; with fake monitors (a development aid) it stays in memory so the real config is untouched.
fn persist(a: &App, change: impl FnOnce(&mut crate::app_settings::AppSettings)) {
    if cfg!(debug_assertions) && std::env::var_os("TITAN_FAKE_MONITORS").is_some() {
        change(&mut a.cfg.borrow_mut());
    } else {
        a.save_cfg(change);
    }
}

/// Selects the monitor at `index` of the list (the sidebar list and the tray menu both come here).
pub fn select_monitor_index(a: &Rc<App>, index: usize) {
    let id = a.monitors.views.borrow().get(index).map(|view| view.id.clone());
    let Some(id) = id else { return };
    if is_active(a, &id) {
        return;
    }
    let ready = a
        .monitors
        .views
        .borrow()
        .iter()
        .any(|view| view.id == id && view.info.is_some() && view.error.as_deref() != Some("disconnected"));
    if !ready {
        // Not connected (yet): stay on the shown monitor.
        refresh_list(a);
        return;
    }
    persist(a, |cfg| cfg.last_selected_monitor = Some(id.cache_key()));
    activate(a, &id, false, None);
}

/// Windows: tells the tray menu which monitors there are, which one is shown and the write level.
#[cfg(windows)]
pub fn sync_tray(a: &Rc<App>) {
    let Some(tray) = a.tray.borrow().clone() else { return };
    let active = a.monitors.active();
    let entries: Vec<(String, bool)> = a
        .monitors
        .views
        .borrow()
        .iter()
        .map(|view| (list_entry(a, view), active.as_ref() == Some(&view.id)))
        .collect();
    tray.set_monitors(entries);
    tray.set_write_level(monitor::user_write_level());
}

/// Windows: the tray menu selects monitors and can stop writing.
#[cfg(windows)]
pub fn attach_tray(a: &Rc<App>, control: &crate::tray::TrayControl) {
    let for_monitor = a.clone();
    let for_stop = a.clone();
    control.set_handlers(
        move |index| select_monitor_index(&for_monitor, index),
        move || set_write_level(&for_stop, 0),
    );
    sync_tray(a);
}

// ---- Windows display settings -------------------------------------------------------------------------------

/// Windows: after a resolution / refresh change the new mode is only tried. The user keeps it, or it goes back by
/// itself after this many seconds (so a mode that gives a black screen is never kept).
#[cfg(windows)]
const DISPLAY_CONFIRM_SECONDS: u32 = 15;

#[cfg(windows)]
pub fn confirm_display_change(a: &Rc<App>, device: String) {
    let lang = a.lang();
    let dialog = adw::AlertDialog::builder()
        .heading(tr(&lang, "display_keep_heading"))
        .body(tr(&lang, "display_keep_body").replace("{s}", &DISPLAY_CONFIRM_SECONDS.to_string()))
        .build();
    dialog.add_response("revert", &tr(&lang, "display_revert"));
    dialog.add_response("keep", &tr(&lang, "display_keep"));
    dialog.set_default_response(Some("revert"));
    dialog.set_close_response("revert");
    dialog.set_response_appearance("keep", adw::ResponseAppearance::Suggested);

    let finished = Rc::new(Cell::new(false));
    let finish = {
        let a = a.clone();
        let device = device.clone();
        let finished = finished.clone();
        move |keep: bool| {
            if finished.replace(true) {
                return;
            }
            let result = if keep {
                monitor::keep_system_display_mode(&device)
            } else {
                monitor::revert_system_display_mode(&device)
            };
            if let Err(error) = result {
                log!("[display] {} failed: {error}", if keep { "keeping the mode" } else { "reverting the mode" });
            }
            let a = a.clone();
            glib::timeout_add_local_once(Duration::from_millis(400), move || crate::refresh_system_display(&a));
        }
    };
    {
        let finish = finish.clone();
        dialog.connect_response(None, move |_, response| finish(response == "keep"));
    }
    // Countdown: when it runs out the dialog closes and the mode goes back.
    let remaining = Rc::new(Cell::new(DISPLAY_CONFIRM_SECONDS));
    {
        let dialog = dialog.clone();
        glib::timeout_add_seconds_local(1, move || {
            if finished.get() {
                return glib::ControlFlow::Break;
            }
            let left = remaining.get().saturating_sub(1);
            remaining.set(left);
            if left == 0 {
                finish(false);
                dialog.force_close();
                return glib::ControlFlow::Break;
            }
            dialog.set_body(&tr(&lang, "display_keep_body").replace("{s}", &left.to_string()));
            glib::ControlFlow::Continue
        });
    }
    dialog.present(Some(&a.ui.window));
}

/// Windows: HDR of the shown monitor (with a question) and a button to the Windows display settings.
#[cfg(windows)]
pub fn setup_display_extras(a: &Rc<App>) {
    let lang = a.lang();
    let group: adw::PreferencesGroup = a.ui.get("sys_group");
    for id in ["sys_resolution", "sys_refresh"] {
        a.ui.get::<adw::ComboRow>(id).add_css_class("keep-enabled");
    }

    let hdr = adw::SwitchRow::builder().title(tr(&lang, "sys_hdr_title")).subtitle(tr(&lang, "sys_hdr_subtitle")).build();
    hdr.add_css_class("keep-enabled");
    group.add(&hdr);
    *a.monitors.hdr_row.borrow_mut() = Some(hdr.clone());
    {
        let a2 = a.clone();
        hdr.connect_active_notify(move |row| {
            if bindings::suppressed() {
                return;
            }
            let wanted = row.is_active();
            let device = a2.connector.borrow().clone();
            let lang = a2.lang();
            let dialog = adw::AlertDialog::builder()
                .heading(tr(&lang, if wanted { "sys_hdr_on_heading" } else { "sys_hdr_off_heading" }))
                .body(tr(&lang, "sys_hdr_body"))
                .build();
            dialog.add_response("cancel", &tr(&lang, "write_confirm_cancel"));
            dialog.add_response("ok", &tr(&lang, "sys_hdr_apply"));
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let a3 = a2.clone();
            dialog.connect_response(None, move |_, response| {
                if response == "ok"
                    && let Err(error) = monitor::set_system_hdr(&device, wanted)
                {
                    log!("[display] HDR change failed: {error}");
                    let failed = adw::AlertDialog::new(Some(&tr(&a3.lang(), "sys_hdr_failed")), Some(&error.to_string()));
                    failed.add_response("ok", "OK");
                    failed.present(Some(&a3.ui.window));
                }
                // The switch shows what Windows says now, whatever happened.
                let a4 = a3.clone();
                glib::timeout_add_local_once(Duration::from_millis(600), move || refresh_display_extras(&a4));
            });
            dialog.present(Some(&a2.ui.window));
        });
    }

    // AI Dual Mode (UHD 160 Hz <-> FHD 320 Hz) has no DDC code: it is switched in the OSD. The row only reports it.
    let dual = adw::ActionRow::builder().title(tr(&lang, "sys_dual_title")).subtitle("—").build();
    dual.add_css_class("property");
    dual.add_css_class("keep-enabled");
    group.add(&dual);
    *a.monitors.dual_row.borrow_mut() = Some(dual);

    let open_button = gtk4::Button::builder().label(tr(&lang, "sys_open_button")).valign(gtk4::Align::Center).build();
    let open_row = adw::ActionRow::builder().title(tr(&lang, "sys_open_title")).subtitle(tr(&lang, "sys_open_subtitle")).build();
    open_row.add_css_class("keep-enabled");
    open_row.add_suffix(&open_button);
    open_row.set_activatable_widget(Some(&open_button));
    group.add(&open_row);
    open_button.connect_clicked(|_| {
        let _ = gtk4::gio::AppInfo::launch_default_for_uri("ms-settings:display", None::<&gtk4::gio::AppLaunchContext>);
    });
    refresh_display_extras(a);
}

/// Windows: shows the HDR state Windows reports for the shown monitor.
#[cfg(windows)]
pub fn refresh_display_extras(a: &Rc<App>) {
    if let Some(row) = a.monitors.dual_row.borrow().as_ref() {
        let lang = a.lang();
        let text = match a.system_display.borrow().as_ref() {
            Some(state) => {
                let hz: f64 = state.current_refresh.parse().unwrap_or(0.0);
                let fhd = state.current_resolution == "1920x1080";
                let key = if fhd && hz >= 300.0 { "sys_dual_on" } else { "sys_dual_off" };
                format!("{} ({} @ {:.0} Hz)", tr(&lang, key), state.current_resolution.replace('x', "×"), hz)
            }
            None => "—".to_string(),
        };
        row.set_subtitle(&text);
    }
    let Some(row) = a.monitors.hdr_row.borrow().clone() else { return };
    let device = a.connector.borrow().clone();
    let state = if device.is_empty() { None } else { monitor::windows_hdr_state(&device) };
    let lang = a.lang();
    bindings::suppress(|| match state {
        Some((supported, enabled)) => {
            row.set_sensitive(supported);
            row.set_active(enabled);
            row.set_subtitle(&tr(&lang, if supported { "sys_hdr_subtitle" } else { "sys_hdr_unsupported" }));
        }
        None => {
            row.set_sensitive(false);
            row.set_subtitle(&tr(&lang, "sys_hdr_unsupported"));
        }
    });
}

// ---- interface size (Windows) -----------------------------------------------------------------------------------

/// Font sizes offered in the preferences, in points.
#[cfg(windows)]
const FONT_SIZES: [u32; 5] = [9, 10, 11, 12, 14];
/// The size the interface is designed for (Adwaita's own default).
#[cfg(windows)]
const DEFAULT_FONT_SIZE: u32 = 11;

/// Windows gives GTK the system font at 9 pt, but libadwaita is designed for 11 pt: at 9 pt every row, button and
/// label is about 20% smaller, which looks like a window that is "too small". The size is a setting.
#[cfg(windows)]
pub fn apply_font_size(size: Option<u32>) {
    let Some(settings) = gtk4::Settings::default() else { return };
    let size = size.unwrap_or(DEFAULT_FONT_SIZE).clamp(8, 20);
    let name = settings.gtk_font_name().map(|name| name.to_string()).unwrap_or_default();
    // Keep the family ("Segoe UI 9" -> "Segoe UI"), change the size.
    let family = name.rsplit_once(' ').filter(|(_, tail)| tail.parse::<f32>().is_ok()).map_or(name.as_str(), |(family, _)| family);
    let family = if family.is_empty() { "Segoe UI" } else { family };
    settings.set_gtk_font_name(Some(&format!("{family} {size}")));
}

/// Windows: the interface size row in the preferences.
#[cfg(windows)]
pub fn setup_font_size(a: &Rc<App>) {
    let theme: adw::ComboRow = a.ui.get("pref_theme");
    let Some(group) = theme.ancestor(adw::PreferencesGroup::static_type()).and_downcast::<adw::PreferencesGroup>() else { return };
    let lang = a.lang();
    let labels: Vec<String> = FONT_SIZES.iter().map(|size| format!("{size} pt")).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let row = adw::ComboRow::builder()
        .title(tr(&lang, "ui_font_size_title"))
        .subtitle(tr(&lang, "ui_font_size_subtitle"))
        .model(&gtk4::StringList::new(&refs))
        .build();
    let current = a.cfg.borrow().ui_font_size.unwrap_or(DEFAULT_FONT_SIZE);
    row.set_selected(FONT_SIZES.iter().position(|&size| size == current).unwrap_or(2) as u32);
    group.add(&row);
    let a2 = a.clone();
    row.connect_selected_notify(move |row| {
        let Some(&size) = FONT_SIZES.get(row.selected() as usize) else { return };
        apply_font_size(Some(size));
        a2.save_cfg(|cfg| cfg.ui_font_size = Some(size));
    });
}
