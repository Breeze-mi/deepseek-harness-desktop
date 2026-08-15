mod bootstrap;
mod commands;
mod error;
mod runtime;
mod settings;
mod tray;
mod windows;

use tauri::{AppHandle, Manager, WindowEvent};

use runtime::DshState;

/// 统一的退出入口。
///
/// 先优雅关掉 dsh 子进程再退出。Job Object 会兜住「主进程被强杀」这类异常，
/// 但正常退出走这条路能让 dsh 有机会落盘，不留脏状态。
pub fn quit(app: &AppHandle) {
    app.state::<DshState>().shutdown();
    app.exit(0);
}

/// 按配置注册全局快捷键（调出/收起主窗口）。
///
/// 三种情况都只降级、不中断启动：用户关掉了、字符串写错了、
/// 或者被别的程序占用了（Alt+Space 尤其常见）。
/// 抢不到全局快捷键不该让整个应用起不来 —— 托盘菜单里还有入口。
#[cfg(desktop)]
pub fn register_global_shortcut(app: &AppHandle) {
    use std::str::FromStr;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let configured = settings::global_shortcut(app);
    if configured.trim().is_empty() {
        return;
    }

    let Ok(shortcut) = Shortcut::from_str(&configured) else {
        eprintln!("[shortcut] 无法解析快捷键：{configured}");
        return;
    };

    if let Err(e) = app.global_shortcut().register(shortcut) {
        eprintln!("[shortcut] 注册 {configured} 失败（多半已被其他程序占用）：{e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    // single-instance 必须最先注册，否则第二个实例可能已经跑完一部分初始化
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_main(app);
        }));
    }

    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(DshState::default())
        .manage(runtime::watcher::NotifyEnabled::default())
        .setup(|app| {
            // 开关的初值要在这里从 store 灌进去 —— managed state 的 Default 是 false，
            // 不读一次配置的话，用户上次开着的通知会在重启后静默失效
            app.state::<runtime::watcher::NotifyEnabled>()
                .set(settings::notify_on_done(app.handle()));

            #[cfg(desktop)]
            {
                // TODO(P4)：接自更新。updater 插件要求 tauri.conf.json 里存在
                // `plugins.updater`（endpoints + pubkey），没有配置段直接注册会
                // 在 setup 阶段 panic。等 P4 用 `tauri signer generate` 出密钥
                // 并写好配置后再启用。
                app.handle().plugin(tauri_plugin_process::init())?;

                app.handle().plugin(
                    tauri_plugin_global_shortcut::Builder::new()
                        .with_handler(|app, _shortcut, event| {
                            use tauri_plugin_global_shortcut::ShortcutState;
                            // 只认按下，不然按一次会触发两遍（按下 + 抬起）
                            if event.state == ShortcutState::Pressed {
                                windows::toggle_main(app);
                            }
                        })
                        .build(),
                )?;
                register_global_shortcut(app.handle());

                tray::create(app.handle())?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };

            // 只拦主窗口。设置窗口、关闭确认框该关就关。
            if window.label() != windows::MAIN {
                return;
            }

            api.prevent_close();
            let app = window.app_handle();

            // 用户勾过「记住选择」就直接执行，不再打扰
            match settings::close_action(app).as_deref() {
                Some("quit") => quit(app),
                Some("tray") => windows::hide_main(app),
                _ => windows::open_close_confirm(app),
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::start_bootstrap,
            commands::resolve_close,
            commands::open_settings,
            commands::get_close_action,
            commands::reset_close_action,
            commands::toggle_main,
            commands::get_global_shortcut,
            commands::set_global_shortcut,
            commands::service_ready,
            commands::get_notify_on_done,
            commands::set_notify_on_done,
            commands::check_upgrades,
            commands::upgrade_dsh,
            commands::restart_app,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
