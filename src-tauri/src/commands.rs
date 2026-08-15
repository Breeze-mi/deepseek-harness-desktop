//! 前端可调用的命令。

use tauri::AppHandle;

use crate::{bootstrap, settings, windows};

/// 启动引导流程。前端在引导页挂载后调用。
/// 立刻返回，进度通过 `bootstrap:*` 事件推送。
#[tauri::command]
pub fn start_bootstrap(app: AppHandle) {
    tauri::async_runtime::spawn(bootstrap::run(app));
}

/// 关闭确认框的三个选项：quit / tray / cancel
#[tauri::command]
pub fn resolve_close(app: AppHandle, action: String, remember: bool) {
    // cancel 不该被记住 —— 否则用户下次永远关不掉窗口
    if remember && action != "cancel" {
        settings::set_close_action(&app, &action);
    }

    windows::close_window(&app, windows::CLOSE_CONFIRM);

    match action.as_str() {
        "quit" => crate::quit(&app),
        "tray" => windows::hide_main(&app),
        _ => {}
    }
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    windows::open_settings(&app);
}

/// 设置页用：读回当前的关闭行为偏好
#[tauri::command]
pub fn get_close_action(app: AppHandle) -> Option<String> {
    settings::close_action(&app)
}

/// 设置页用：恢复成「每次询问」
#[tauri::command]
pub fn reset_close_action(app: AppHandle) {
    settings::clear_close_action(&app);
}

/// 全局快捷键的目标动作：调出/收起主窗口。托盘菜单与设置页也用它。
#[tauri::command]
pub fn toggle_main(app: AppHandle) {
    windows::toggle_main(&app);
}

#[tauri::command]
pub fn get_global_shortcut(app: AppHandle) -> String {
    settings::global_shortcut(&app)
}

/// 改快捷键。空字符串 = 关闭该功能。
#[tauri::command]
pub fn set_global_shortcut(app: AppHandle, value: String) {
    #[cfg(desktop)]
    {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;
        // 先全撤，否则旧键会继续生效，用户会遇到「两个键都能唤起」的怪状
        let _ = app.global_shortcut().unregister_all();
    }

    settings::set_global_shortcut(&app, value.trim());

    #[cfg(desktop)]
    crate::register_global_shortcut(&app);
}

/// dsh 服务是否已就绪。设置页用来显示运行状态。
#[tauri::command]
pub fn service_ready(app: AppHandle) -> bool {
    use tauri::Manager;
    app.state::<crate::runtime::DshState>().url().is_some()
}

#[tauri::command]
pub fn get_notify_on_done(app: AppHandle) -> bool {
    settings::notify_on_done(&app)
}

/// 任务完成通知开关。改完立刻生效，不需要重启监视器。
#[tauri::command]
pub fn set_notify_on_done(app: AppHandle, value: bool) {
    use tauri::Manager;
    settings::set_notify_on_done(&app, value);
    app.state::<crate::runtime::watcher::NotifyEnabled>()
        .set(value);
}

/// 设置页用：查询 dsh 与界面插件的版本状态。会走网络，可能要几秒。
#[tauri::command]
pub async fn check_upgrades(app: AppHandle) -> bootstrap::upgrade::UpgradeReport {
    let entry = settings::dsh_entry(&app);
    bootstrap::upgrade::check(entry.as_deref()).await
}

/// 设置页用：把 dsh 升到 registry 最新版。
///
/// **必须先停掉 dsh 子进程。** dsh 依赖 koffi、node-addon-* 这些原生模块，
/// `.node` 文件被加载期间在 Windows 上是锁死的，npm 覆盖会撞 EPERM。
///
/// 停掉之后主窗口那个页面就死了，所以这个操作天然是「升级 + 重启应用」，
/// 调用方必须在点之前跟用户讲清楚。
#[tauri::command]
pub async fn upgrade_dsh(app: AppHandle) -> Result<String, String> {
    use tauri::Manager;
    app.state::<crate::runtime::DshState>().shutdown();

    let entry = tokio::task::spawn_blocking(bootstrap::dsh::upgrade)
        .await
        .map_err(|e| format!("升级任务异常退出：{e}"))?
        .map_err(|e| e.to_string())?;

    settings::set_dsh_entry(&app, &entry);

    Ok(bootstrap::upgrade::installed_dsh(&entry).unwrap_or_else(|| "未知".into()))
}

/// 重启整个应用。升级完 dsh 后由设置页调用。
///
/// 用 `restart()` 而不是 `request_restart()`：后者会走完整的退出事件链，
/// 而我们在 `on_window_event` 里拦了主窗口的 CloseRequested 去弹关闭确认框 ——
/// 万一被那条路径接住，重启会卡在一个莫名其妙的对话框上。
///
/// 跳过退出事件不影响清理：升级前已经显式 shutdown 过 dsh，
/// 而 Job Object 本来就兜着「进程没了子进程一起走」。
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}
