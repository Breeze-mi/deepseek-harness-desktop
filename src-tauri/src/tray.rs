//! 系统托盘。关闭主窗口后应用驻留托盘，dsh 子进程保持运行。

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter,
};

use crate::windows;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "打开主窗口", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let restart = MenuItem::with_id(app, "restart_dsh", "重启 dsh 服务", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "check_update", "检查更新", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[&show, &settings, &sep1, &restart, &update, &sep2, &quit],
    )?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("DeepSeek Harness")
        .menu(&menu)
        .show_menu_on_left_click(false);

    // 复用窗口图标，避免 include_bytes! 带来的生命周期纠缠
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => windows::show_main(app),
            "settings" => windows::open_settings(app),
            "restart_dsh" => {
                let _ = app.emit_to(windows::MAIN, "tray:restart-dsh", ());
            }
            "check_update" => {
                let _ = app.emit_to(windows::SETTINGS, "tray:check-update", ());
                windows::open_settings(app);
            }
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // 左键单击直接显示主窗口，符合 Windows 习惯
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                windows::show_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}
