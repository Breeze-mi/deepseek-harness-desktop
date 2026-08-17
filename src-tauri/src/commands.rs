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

/// 打开设置窗口。
///
/// **必须是 async。** Tauri v2 的同步命令在主线程（事件循环分发现场）执行，
/// 而本命令首次调用要 `WebviewWindowBuilder::build()` —— 在分发现场建窗口
/// 正是 `windows::create_close_confirm` 文档里那个死锁：wry 重入事件循环，
/// 主线程停摆，标题栏「设置」点下去整个应用假死。
/// async 命令跑在异步运行时线程，与托盘菜单（spawn 后调用）同一条安全路径。
#[tauri::command]
pub async fn open_settings(app: AppHandle) {
    windows::open_settings(&app);
}

/// 设置页用：读回当前的关闭行为偏好
#[tauri::command]
pub fn get_close_action(app: AppHandle) -> Option<String> {
    settings::close_action(&app)
}

/// 设置页用：直接指定关闭行为。
/// `"ask"` = 恢复每次询问（删掉记住的选择），其余原样存下。
#[tauri::command]
pub fn set_close_action(app: AppHandle, action: String) {
    if action == "ask" {
        settings::clear_close_action(&app);
    } else {
        settings::set_close_action(&app, &action);
    }
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

/// 重启 dsh 服务。顶栏按钮用，和托盘菜单走同一条路径。
///
/// 返回新地址；前端不必用它 —— 重启成功会广播 `bootstrap:ready`，
/// 外壳听到就把 DSH 页面换过去。
#[tauri::command]
pub async fn restart_dsh(app: AppHandle) -> Result<String, String> {
    crate::runtime::restart(app).await
}

/// 当前应跟随的界面主题（"light" | "dark"）。每个窗口挂载前读一次，
/// 之后的变化走 `app:theme` 事件。
#[tauri::command]
pub fn get_theme(app: AppHandle) -> String {
    crate::theme::current(&app)
}

/// 标题栏主题按钮的模式："follow" | "light" | "dark"
#[tauri::command]
pub fn get_theme_mode(app: AppHandle) -> String {
    settings::theme_mode(&app)
}

/// - 显式亮/暗：把选择经 `settings.update` 写进 DSH —— 页面实时翻转、落盘
///   settings.yaml，随后模式回到 "follow"，两边同源、永不分叉；
///   写不进去（服务未起、上游协议变了）才降级成外壳独立模式。
/// - "follow"：恢复跟随（降级态的手动出口）。
#[tauri::command]
pub async fn set_theme_mode(app: AppHandle, value: String) -> String {
    use tauri::Emitter;

    let emit = |theme: String| {
        let _ = app.emit(
            crate::theme::EVENT_THEME,
            crate::theme::ThemePayload { theme },
        );
    };

    match value.as_str() {
        v @ ("light" | "dark") => {
            // 乐观广播在前、网络在后：按钮点下去不能等 push 的往返
            emit(v.to_string());
            let mode = if crate::theme::push_to_dsh(&app, v).await {
                "follow"
            } else {
                v
            };
            settings::set_theme_mode(&app, mode);
            mode.to_string()
        }
        _ => {
            // 恢复跟随没有网络环节。**必须先落模式再取值**：current() 按模式
            // 分派，先取值会拿旧的显式模式算出错值，真值要等轮询兜回来。
            settings::set_theme_mode(&app, "follow");
            emit(crate::theme::current(&app));
            "follow".to_string()
        }
    }
}
