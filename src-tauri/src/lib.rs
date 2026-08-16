// **必须排在所有模块前面。** `#[macro_use]` 只对声明之后的模块生效，
// 放在中间的话 bootstrap / commands / error 里就用不了 `dlog!`。
#[macro_use]
mod logging;

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
/// 先关掉 dsh 子进程再退出。Job Object 会兜住「主进程被强杀」这类异常，
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
        dlog!("[shortcut] 无法解析快捷键：{configured}");
        return;
    };

    if let Err(e) = app.global_shortcut().register(shortcut) {
        dlog!("[shortcut] 注册 {configured} 失败（多半已被其他程序占用）：{e}");
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 起点登记 + 轮转上一轮日志。真正的写入由下面注册的 log 插件承担，
    // 但轮转必须发生在插件打开文件**之前**，所以这一行的位置不能动。
    logging::init();
    let log_dir = logging::path()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let mut builder = tauri::Builder::default();

    // single-instance 必须最先注册，否则第二个实例可能已经跑完一部分初始化
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_main(app);
        }));
    }

    builder
        .plugin(
            tauri_plugin_log::Builder::new()
                // 默认 target 的 LogDir 在 %LocalAppData%\<identifier>\logs，
                // 和我们既有的日志位置不同。全清掉自己指：文件继续落在
                // %APPDATA%\deepseek-harness-desktop\app.log ——
                // 「打开日志」按钮与用户已有的习惯零变化。
                .clear_targets()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                        path: log_dir,
                        file_name: Some("app".into()),
                    }),
                ])
                // 上限很小。轮转靠 logging::init() 的每启动 rename；
                // 这里把上限放大并选 KeepAll，纯作兜底，正常永远不该触发。
                .max_file_size(50_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll)
                .level(log::LevelFilter::Info)
                // 依赖库的闲话压到 Warn
                .level_for("hyper", log::LevelFilter::Warn)
                .level_for("reqwest", log::LevelFilter::Warn)
                .level_for("tao", log::LevelFilter::Warn)
                .level_for("wry", log::LevelFilter::Warn)
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "[{}][{:>7.1}s][{}][{}] {}",
                        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                        crate::logging::rel_secs(),
                        record.level(),
                        record.target(),
                        message
                    ))
                })
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .manage(DshState::default())
        .manage(runtime::watcher::NotifyEnabled::default())
        .setup(|app| {
            // 横幅放 setup 而不是 run() 开头：log 插件在上面注册后才接管门面，
            // 更早的日志会被静默丢掉
            dlog!("[app] 启动 v{}", env!("CARGO_PKG_VERSION"));

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

                // 关闭确认框预创建（隐藏）。
                // 现场建 —— 那条路偶发死锁整个事件循环，见 create_close_confirm。
                windows::create_close_confirm(app.handle());
                // 防白屏的另一半：主窗口是 visible:false，由前端首帧后 show()。
                // 若前端坏了（构建损坏、资源缺失）永远不喊，这里 4 秒后强制
                // 亮出来 —— 宁可闪白，也不能让用户面对一个「不存在」的窗口。
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
                    if let Some(w) = handle.get_webview_window(windows::MAIN) {
                        if !w.is_visible().unwrap_or(true) {
                            dlog!("[app] 前端 4 秒未就绪，强制显示主窗口");
                            let _ = w.show();
                        }
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // 用户把主窗口切回前台，托盘不用再闪。
            // 这条和 windows::show_main 里那次是互补的：那里管「我们主动调出窗口」，
            // 这里管「用户自己用 Alt+Tab 或点任务栏切回来」。
            if let WindowEvent::Focused(true) = event {
                if window.label() == windows::MAIN {
                    tray::stop_blink();
                }
                return;
            }

            let WindowEvent::CloseRequested { api, .. } = event else {
                return;
            };

            // 关闭确认框藏而不关：它是预创建复用的，被 Alt+F4 真关掉之后，
            // 下一次弹出就得现场重建 —— 又慢，又贴着当初死锁陷阱的边。
            if window.label() == windows::CLOSE_CONFIRM {
                api.prevent_close();
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    windows::hide_close_confirm(&app);
                });
                return;
            }

            // 只拦主窗口。设置窗口该关就关。
            if window.label() != windows::MAIN {
                return;
            }

            api.prevent_close();

            // 回调运行在事件循环分发现场，窗口操作一律 spawn 出去 ——
            // 同步建窗口会偶发死锁整个事件循环（机理见 windows::create_close_confirm）
            let app = window.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                // 用户勾过「记住选择」就直接执行，不再打扰
                match settings::close_action(&app).as_deref() {
                    Some("quit") => quit(&app),
                    Some("tray") => windows::hide_main(&app),
                    _ => windows::open_close_confirm(&app),
                }
            });
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
            commands::upgrade_plugins,
            commands::restart_app,
            commands::open_log,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
