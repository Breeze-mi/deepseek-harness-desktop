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

    // 藏而不关：预创建复用的窗口，销毁即退回现场重建的死锁路径（见 windows）
    windows::hide_close_confirm(&app);

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

/// 设置页用：把 dsh 升到 registry 最新版，然后重启服务。
///
/// **必须先停掉 dsh 子进程。** dsh 依赖 koffi、node-addon-* 这些原生模块，
/// `.node` 文件被加载期间在 Windows 上是锁死的，npm 覆盖会撞 EPERM。
///
/// 升完直接重起 dsh 并把主窗口导到新端口，不需要用户重启整个应用 ——
/// 换掉的只是 dsh 的文件，我们自己的进程没有任何理由跟着重来。
#[tauri::command]
pub async fn upgrade_dsh(app: AppHandle) -> Result<String, String> {
    use tauri::Manager;
    app.state::<crate::runtime::DshState>().shutdown();

    let entry = tokio::task::spawn_blocking(bootstrap::dsh::upgrade)
        .await
        .map_err(|e| format!("升级任务异常退出：{e}"))?
        .map_err(|e| e.to_string())?;

    let version = bootstrap::upgrade::installed_dsh(&entry).unwrap_or_else(|| "未知".into());
    // 缓存要在 restart 之前刷新 —— restart 读的就是这个值
    settings::set_dsh_entry(&app, &entry);

    crate::runtime::restart(app).await?;

    Ok(version)
}

/// 设置页用：把界面插件升到指定版本（通常是上游最新版），然后重启 dsh。
///
/// `BUNDLE_VERSION` 只是**新装时的已知可用版本**，不是上限。用户显式升上去之后
/// 引导流程不会把他降回来 —— `plugins::is_installed` 只在「装的比硬编码的旧」
/// 时才判定需要重装。
///
/// 和升级 dsh 一样要先停服务：插件里的 cloudflared / ssh2 / cpu-features 带原生
/// 模块，运行中的 dsh 加载了它们，pnpm 覆盖会撞同一类 EPERM。
#[tauri::command]
pub async fn upgrade_plugins(app: AppHandle, version: String) -> Result<(), String> {
    use crate::bootstrap::{download, node, plugins};
    use tauri::Manager;

    let entry = settings::dsh_entry(&app).ok_or_else(|| "找不到 dsh 安装位置".to_string())?;
    let node = node::detect_system_node()
        .or_else(download::installed_portable_node)
        .ok_or_else(|| "找不到可用的 Node.js 运行时".to_string())?;

    app.state::<crate::runtime::DshState>().shutdown();

    tokio::task::spawn_blocking(move || plugins::install_version(&node, &entry, &version, None))
        .await
        .map_err(|e| format!("安装任务异常退出：{e}"))?
        .map_err(|e| e.to_string())?;

    // 插件是 dsh 启动时加载的，不重起服务不会生效
    crate::runtime::restart(app).await?;
    Ok(())
}

/// 重启整个应用。dsh 升级失败、服务起不来时的兜底。
///
/// 用 `restart()` 而不是 `request_restart()`：后者会走完整的退出事件链，
/// 而我们在 `on_window_event` 里拦了主窗口的 CloseRequested 去弹关闭确认框
/// 跳过退出事件不影响清理：升级前已经显式 shutdown 过 dsh，
/// 而 Job Object 本来就兜着「进程没了子进程一起走」。
#[tauri::command]
pub fn restart_app(app: AppHandle) {
    app.restart();
}

/// 用系统默认程序打开日志文件。
///
/// 装机环境千奇百怪（执行策略、nvm4w、代理、镜像），远程排查唯一靠得住的
#[tauri::command]
pub fn open_log(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let path = crate::logging::path().ok_or_else(|| "无法定位日志目录".to_string())?;
    if !path.is_file() {
        return Err(format!("日志文件还不存在：{}", path.display()));
    }

    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("打开日志失败：{e}"))
}
