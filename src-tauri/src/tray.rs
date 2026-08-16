//! 系统托盘。关闭主窗口后应用驻留托盘，dsh 子进程保持运行。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle,
};

use crate::windows;

const TRAY_ID: &str = "main-tray";

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

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("DeepSeek Harness")
        .menu(&menu)
        .show_menu_on_left_click(false);

    // 复用窗口图标，避免 include_bytes! 带来的生命周期纠缠
    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| {
            // 菜单回调也在事件循环分发现场，open_settings 首次会建窗口 ——
            // 全部 spawn 出去（死锁机理见 windows::create_close_confirm）
            let app = app.clone();
            let id = event.id.as_ref().to_string();
            tauri::async_runtime::spawn(async move {
                match id.as_str() {
                    "show" => windows::show_main(&app),
                    "settings" => windows::open_settings(&app),
                    "restart_dsh" => restart_dsh(app.clone()),
                    // 版本检查在设置页的「版本与更新」卡片里，页面挂载时会自动查一次
                    "check_update" => windows::open_settings(&app),
                    "quit" => crate::quit(&app),
                    _ => {}
                }
            });
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

/// 重启 dsh 服务。
///
/// 整个过程要几秒到几十秒（停服务 → 起新进程 → 等就绪 → 导航），
/// 菜单回调里不能干等，所以扔到异步运行时。
///
/// 失败时发系统通知
fn restart_dsh(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(e) = crate::runtime::restart(app.clone()).await {
            dlog!("[tray] 重启 dsh 失败：{e}");

            use tauri_plugin_notification::NotificationExt;
            let _ = app
                .notification()
                .builder()
                .title("重启 dsh 失败")
                .body(e)
                .show();
        }
    });
}

/// 托盘图标是否正在闪烁。
///
/// 用全局原子量而不是给托盘挂状态：开始闪烁来自监视器线程，停止来自
/// 窗口事件线程，两边都要能改，原子量是最省事且不会死锁的同步方式。
static BLINKING: AtomicBool = AtomicBool::new(false);

/// 闪烁任务的代次。
///
/// **只有 `BLINKING` 一个标志是不够的**，会漏掉这条时序：
/// 任务 A 卡在 sleep → 用户看了窗口，标志置 false → A 还没醒，
/// 新任务完成又把标志置回 true 并启动任务 B → A 醒来读到 true，继续跑。
/// 结果两个任务同时切图标，互相覆盖。
///
/// 每个任务记住自己启动时的代次，发现当前代次已经变了就退出，
/// 保证任何时刻只有最新那个任务在动图标。
static BLINK_GEN: AtomicU64 = AtomicU64::new(0);

const BLINK_INTERVAL: Duration = Duration::from_millis(600);

/// 开始闪烁托盘图标（QQ 收到消息那种效果）。
///
/// 做法是在「有图标」和「无图标」之间交替 —— `set_icon(None)` 会清空图标，
/// 正好得到一闪一闪的效果，不需要额外准备一张空白图片当第二帧。
///
/// 重复调用是安全的：已经在闪就直接返回。
pub fn start_blink(app: &AppHandle) {
    if BLINKING.swap(true, Ordering::SeqCst) {
        return;
    }
    let generation = BLINK_GEN.fetch_add(1, Ordering::SeqCst) + 1;

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let icon = app.default_window_icon().cloned();
        let mut visible = false;

        while BLINKING.load(Ordering::SeqCst) && BLINK_GEN.load(Ordering::SeqCst) == generation {
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                let _ = tray.set_icon(if visible { icon.clone() } else { None });
            }
            visible = !visible;
            tokio::time::sleep(BLINK_INTERVAL).await;
        }

        // 只有「最后一个」任务负责恢复图标。被换代的旧任务直接走人，
        // 否则它的收尾会把新任务刚设好的空白帧覆盖掉。
        if BLINK_GEN.load(Ordering::SeqCst) == generation {
            if let Some(tray) = app.tray_by_id(TRAY_ID) {
                let _ = tray.set_icon(app.default_window_icon().cloned());
            }
        }
    });
}

/// 停止闪烁并恢复图标。用户看过主窗口就该停。
pub fn stop_blink() {
    BLINKING.store(false, Ordering::SeqCst);
}
