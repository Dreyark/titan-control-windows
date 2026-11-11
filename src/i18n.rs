
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AppLang {
    EN,
    PL,
    DE,
    ES,
    FR,
    UA,
}

pub fn detect_system_lang() -> AppLang {
    let lang = std::env::var("LANG").unwrap_or_default();
    if lang.starts_with("pl") {
        AppLang::PL
    } else if lang.starts_with("de") {
        AppLang::DE
    } else if lang.starts_with("es") {
        AppLang::ES
    } else if lang.starts_with("fr") {
        AppLang::FR
    } else if lang.starts_with("uk") {
        AppLang::UA
    } else {
        AppLang::EN
    }
}

pub fn lang_index(lang: &AppLang) -> u32 {
    match lang {
        AppLang::EN => 0,
        AppLang::PL => 1,
        AppLang::DE => 2,
        AppLang::ES => 3,
        AppLang::FR => 4,
        AppLang::UA => 5,
    }
}

pub fn lang_from_index(idx: u32) -> AppLang {
    match idx {
        1 => AppLang::PL,
        2 => AppLang::DE,
        3 => AppLang::ES,
        4 => AppLang::FR,
        5 => AppLang::UA,
        _ => AppLang::EN,
    }
}

pub fn tr(lang: &AppLang, key: &str) -> String {
    match lang {
        AppLang::PL => tr_pl(key),
        _ => tr_en(key),
    }
}

fn tr_en(key: &str) -> String {
    match key {
        // Power
        "power_off" => "Turn off",

        // Reset
        "reset_btn" => "Reset",
        "cancel_btn" => "Cancel",
        "confirm_reset_colors_title" => "Reset colors?",
        "confirm_reset_colors_body" => "The colors return to their factory values (color temperature: Warm).",
        "confirm_reset_settings_title" => "Reset brightness and contrast?",
        "confirm_reset_settings_body" => "Brightness and contrast return to their factory values (90 / 50).",
        "confirm_reset_factory_title" => "Restore factory settings?",
        "confirm_reset_factory_body" => "All monitor settings return to their factory values.",
        "confirm_power_off_title" => "Turn the monitor off?",
        "confirm_power_off_body" => "Only the monitor's power button turns it on again.",

        // Profiles
        "brightness" => "Brightness",
        "contrast" => "Contrast",
        "sharpness" => "Sharpness",
        "shadow_balance" => "Shadow Balance",
        "cr_enhance" => "CR Enhance",
        "color_enhance" => "Color Enhance",
        "super_res" => "Super Res",
        "dcr" => "DCR",
        "low_blue_light" => "Low Blue Light",
        "color_temp_profile" => "Color temperature",
        "rgb_gain" => "RGB",
        "red" => "Red",
        "green" => "Green",
        "blue" => "Blue",
        "axis_red" => "Red",
        "axis_green" => "Green",
        "axis_blue" => "Blue",
        "axis_cyan" => "Cyan",
        "axis_magenta" => "Magenta",
        "axis_yellow" => "Yellow",
        "hdr" => "HDR",
        "gamma" => "Gamma",
        "night_vision" => "Night Vision",
        "dynamic_od" => "Dynamic OD",
        "hue" => "Hue",
        "saturation" => "Saturation",

        // Gaming - Game Aid
        "screen_size" => "Screen size (DualMode)",
        "fps_counter" => "FPS/Hz counter",
        "crosshair" => "Crosshair",
        "stopwatch" => "Stopwatch",
        "game_time" => "Game time",
        "magnifier" => "Magnifier",
        "hawkeye" => "Hawkeye Vision",

        // Gaming - Picture Enhance
        "adaptive_sync" => "Adaptive-Sync",
        "local_dimming" => "Local dimming",
        "dyds" => "DyDs",
        "halo_control" => "Halo Control",

        // Splash / Tray
        "splash_searching" => "Searching for monitor...",
        "splash_detecting_monitors" => "Detecting monitors...",
        "splash_searching_monitors" => "Searching for monitors...",
        "splash_loading_cached" => "Loading cached settings...",
        "monitor_alias" => "Monitor alias",
        "info_screen" => "Screen",
        "info_serial" => "Serial number",
        "info_firmware" => "Firmware version",
        "identify_title" => "Identify monitors",
        "identify_subtitle" => "Shows a number and the name on every screen for 2 s (no communication with the monitors)",
        "identify_button" => "Show",
        "read_only_title" => "Read-only mode",
        "read_only_subtitle" => "Writing to the monitors is disabled in this version; settings can only be read",
        "read_only_tooltip" => "Writing is disabled (read-only mode)",
        "monitor_connecting" => "connecting…",
        "monitor_offline" => "not responding",
        "sys_dual_title" => "AI Dual Mode (1080p / 320 Hz)",
        "sys_dual_on" => "On",
        "sys_dual_off" => "Off",
        "ui_font_size_title" => "Interface size",
        "ui_font_size_subtitle" => "Font size (11 pt is what the window is designed for; Windows offers 9 pt, which makes everything look small)",
        "sys_read_only" => "Change the resolution and refresh rate here (the new mode has to be confirmed, otherwise it goes back after 15 s). Change the scaling and the screen layout in the Windows settings (button below)",
        "display_keep_heading" => "Keep the new display mode?",
        "display_keep_body" => "The mode goes back to the previous one in {s} s unless you confirm.",
        "display_keep" => "Keep",
        "display_revert" => "Revert",
        "sys_hdr_title" => "HDR in Windows (this display)",
        "sys_hdr_subtitle" => "Turns HDR of this display in Windows on and off; it is not the monitor HDR mode",
        "sys_hdr_unsupported" => "This display does not report HDR support in Windows",
        "sys_hdr_on_heading" => "Turn HDR on in Windows?",
        "sys_hdr_off_heading" => "Turn HDR off in Windows?",
        "sys_hdr_body" => "The screen may go dark for a moment and colors and brightness change. Only this display is affected.",
        "sys_hdr_apply" => "Change",
        "sys_hdr_failed" => "Could not change HDR",
        "sys_open_title" => "Scaling and screen layout",
        "sys_open_subtitle" => "Windows does not let apps change them: open the display settings",
        "sys_open_button" => "Open",
        "write_enable_title" => "Writing to the monitors",
        "write_enable_subtitle_off" => "Off: the app only reads (this is the state at every start)",
        "write_enable_subtitle_on" => "On",
        "write_level_title" => "Write level",
        "write_level_1" => "1 – brightness, contrast, volume",
        "write_level_2" => "2 – other picture settings",
        "write_level_3" => "3 – game aids, PIP, LED, OSD, inputs, system",
        "write_level_tooltip" => "Needs a higher write level",
        "write_confirm_heading" => "Enable writing to the monitors?",
        "write_confirm_body" => "Settings are saved in the monitor memory. A wrong write can lock monitor functions or need a reset. Start with level 1 (brightness, contrast, volume) on one monitor. The app reads only again at the next start.",
        "write_confirm_enable" => "Enable level 1",
        "write_confirm_cancel" => "Cancel",
        "write_raise_heading" => "Raise the write level to {level}?",
        "write_raise_body" => "Adds: {level}. Raise the level only after checking the previous one on your monitors.",
        "write_raise_accept" => "Raise the level",
        "monitor_disconnected" => "disconnected (restart the app after reconnecting)",
        "splash_reading_profile" => "Reading the picture profile...",
        "splash_monitor_not_found" => "Monitor not found",
        "tray_show" => "Show",
        "tray_hide" => "Hide",
        "tray_quit" => "Quit",
        "tray_monitors" => "Monitors",
        "tray_write_state" => "Writing: level {level}",
        "tray_stop_writing" => "Stop writing (read-only)",
        "tray_sliders" => "Sliders",
        "tray_options" => "Options",
        "tray_toggles" => "Toggles",

        _ => key,
    }
    .into()
}

fn tr_pl(key: &str) -> String {
    match key {
        // Power
        "power_off" => "Wyłącz",

        // Reset
        "reset_btn" => "Resetuj",
        "cancel_btn" => "Anuluj",
        "confirm_reset_colors_title" => "Zresetować kolory?",
        "confirm_reset_colors_body" => "Kolory wrócą do wartości fabrycznych (temperatura barwowa: Warm).",
        "confirm_reset_settings_title" => "Zresetować jasność i kontrast?",
        "confirm_reset_settings_body" => "Jasność i kontrast wrócą do wartości fabrycznych (90 / 50).",
        "confirm_reset_factory_title" => "Przywrócić ustawienia fabryczne?",
        "confirm_reset_factory_body" => "Wszystkie ustawienia monitora wrócą do wartości fabrycznych.",
        "confirm_power_off_title" => "Wyłączyć monitor?",
        "confirm_power_off_body" => "Włączysz go ponownie tylko przyciskiem zasilania na monitorze.",

        // Profiles
        "brightness" => "Jasność",
        "contrast" => "Kontrast",
        "sharpness" => "Ostrość",
        "shadow_balance" => "Shadow Balance",
        "cr_enhance" => "CR Enhance",
        "color_enhance" => "Color Enhance",
        "super_res" => "Super Res",
        "dcr" => "DCR",
        "low_blue_light" => "Low Blue Light",
        "color_temp_profile" => "Temperatura barwowa",
        "rgb_gain" => "RGB",
        "red" => "Czerwony (R)",
        "green" => "Zielony (G)",
        "blue" => "Niebieski (B)",
        "axis_red" => "Czerwony (R)",
        "axis_green" => "Zielony (G)",
        "axis_blue" => "Niebieski (B)",
        "axis_cyan" => "Cyjan (C)",
        "axis_magenta" => "Magenta (M)",
        "axis_yellow" => "Żółty (Y)",
        "hdr" => "HDR",
        "gamma" => "Gamma",
        "night_vision" => "Night Vision",
        "dynamic_od" => "Dynamic OD",
        "hue" => "Odcień",
        "saturation" => "Nasycenie",

        // Gaming - Game Aid
        "screen_size" => "Rozmiar ekranu (DualMode)",
        "fps_counter" => "Licznik FPS/Hz",
        "crosshair" => "Celownik",
        "stopwatch" => "Stoper",
        "game_time" => "Czas gry",
        "magnifier" => "Lupa",
        "hawkeye" => "Hawkeye Vision",

        // Gaming - Picture Enhance
        "adaptive_sync" => "Adaptive-Sync",
        "local_dimming" => "Lokalne przyciemnianie",
        "dyds" => "DyDs",
        "halo_control" => "Halo Control",

        // Splash / Tray
        "splash_searching" => "Szukam monitora...",
        "splash_detecting_monitors" => "Wykrywam monitory...",
        "splash_searching_monitors" => "Szukam monitorów...",
        "splash_loading_cached" => "Wczytuję zapisane ustawienia...",
        "monitor_alias" => "Alias monitora",
        "info_screen" => "Ekran",
        "info_serial" => "Numer seryjny",
        "info_firmware" => "Wersja firmware",
        "identify_title" => "Zidentyfikuj monitory",
        "identify_subtitle" => "Pokazuje numer i nazwę na każdym ekranie przez 2 s (bez komunikacji z monitorami)",
        "identify_button" => "Pokaż",
        "read_only_title" => "Tryb tylko do odczytu",
        "read_only_subtitle" => "Zapis do monitorów jest w tej wersji wyłączony; ustawienia można tylko odczytywać",
        "read_only_tooltip" => "Zapis wyłączony (tryb tylko do odczytu)",
        "monitor_connecting" => "łączenie…",
        "monitor_offline" => "nie odpowiada",
        "sys_dual_title" => "AI Dual Mode (1080p / 320 Hz)",
        "sys_dual_on" => "Włączony",
        "sys_dual_off" => "Wyłączony",
        "ui_font_size_title" => "Rozmiar interfejsu",
        "ui_font_size_subtitle" => "Wielkość czcionki (11 pt to rozmiar, do którego zaprojektowano okno; Windows podaje 9 pt, przy którym wszystko wygląda na małe)",
        "sys_read_only" => "Rozdzielczość i odświeżanie zmieniasz tutaj (nowy tryb trzeba potwierdzić, inaczej wróci po 15 s). Skalowanie i układ ekranów zmieniasz w ustawieniach Windows (przycisk poniżej)",
        "display_keep_heading" => "Zachować nowy tryb ekranu?",
        "display_keep_body" => "Tryb wróci do poprzedniego za {s} s, jeśli nie potwierdzisz.",
        "display_keep" => "Zachowaj",
        "display_revert" => "Cofnij",
        "sys_hdr_title" => "HDR w Windows (ten ekran)",
        "sys_hdr_subtitle" => "Włącza i wyłącza HDR tego ekranu w Windows; to nie jest tryb HDR monitora",
        "sys_hdr_unsupported" => "Ten ekran nie zgłasza obsługi HDR w Windows",
        "sys_hdr_on_heading" => "Włączyć HDR w Windows?",
        "sys_hdr_off_heading" => "Wyłączyć HDR w Windows?",
        "sys_hdr_body" => "Ekran może na chwilę zgasnąć, a kolory i jasność się zmienią. Dotyczy tylko tego ekranu.",
        "sys_hdr_apply" => "Zmień",
        "sys_hdr_failed" => "Nie udało się zmienić HDR",
        "sys_open_title" => "Skalowanie i układ ekranów",
        "sys_open_subtitle" => "Windows nie pozwala zmienić ich z aplikacji: otwórz ustawienia ekranu",
        "sys_open_button" => "Otwórz",
        "write_enable_title" => "Zapis do monitorów",
        "write_enable_subtitle_off" => "Wyłączony: aplikacja tylko odczytuje (tak jest przy każdym uruchomieniu)",
        "write_enable_subtitle_on" => "Włączony",
        "write_level_title" => "Poziom zapisu",
        "write_level_1" => "1 – jasność, kontrast, głośność",
        "write_level_2" => "2 – pozostałe ustawienia obrazu",
        "write_level_3" => "3 – gry, PIP, LED, OSD, wejścia, system",
        "write_level_tooltip" => "Wymaga wyższego poziomu zapisu",
        "write_confirm_heading" => "Włączyć zapis do monitorów?",
        "write_confirm_body" => "Ustawienia zapisują się w pamięci monitora. Błędny zapis może zablokować funkcje monitora albo wymagać resetu. Zacznij od poziomu 1 (jasność, kontrast, głośność) na jednym monitorze. Przy następnym uruchomieniu aplikacja znów będzie tylko odczytywać.",
        "write_confirm_enable" => "Włącz poziom 1",
        "write_confirm_cancel" => "Anuluj",
        "write_raise_heading" => "Podnieść poziom zapisu do {level}?",
        "write_raise_body" => "Dojdą ustawienia: {level}. Podnoś poziom dopiero po sprawdzeniu poprzedniego na swoich monitorach.",
        "write_raise_accept" => "Podnieś poziom",
        "monitor_disconnected" => "odłączony (po ponownym podłączeniu uruchom aplikację ponownie)",
        "splash_reading_profile" => "Odczytuję profil obrazu...",
        "splash_monitor_not_found" => "Nie znaleziono monitora",
        "tray_show" => "Pokaż",
        "tray_hide" => "Ukryj",
        "tray_quit" => "Wyjdź",
        "tray_monitors" => "Monitory",
        "tray_write_state" => "Zapis: poziom {level}",
        "tray_stop_writing" => "Zatrzymaj zapis (tylko odczyt)",
        "tray_sliders" => "Suwaki",
        "tray_options" => "Opcje",
        "tray_toggles" => "Przełączniki",

        _ => return tr_en(key),
    }
    .into()
}
