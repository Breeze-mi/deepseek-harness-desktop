//! 窗口创建与管理。
//!
//! 主窗口在引导完成后会导航到 DSH 的 Web UI —— 那之后我们自己的 Vue 应用
//! 就不在主窗口里了。所以设置、关闭确认这类自有 UI 必须开独立窗口承载，
//! 不能指望在主窗口里弹层。

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub const MAIN: &str = "main";
pub const SETTINGS: &str = "settings";
pub const CLOSE_CONFIRM: &str = "close-confirm";

/// 所有自有窗口都加载同一个 index.html，前端按 window label 决定渲染哪个视图。
/// 比在 URL 里拼 hash 路由稳，也不用担心打包后路径变化。
fn app_url() -> WebviewUrl {
    WebviewUrl::App("index.html".into())
}

/// 把主窗口调到前台。
///
/// 三步缺一不可：
/// - `show()` 处理「被隐藏到托盘」
/// - `unminimize()` 处理「被最小化」—— 只 show 不 unminimize，窗口仍在任务栏躺着
/// - `set_focus()` 才真正抢前台
pub fn show_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
}

/// 全局快捷键的目标动作：在前台就收起，否则调出来。
///
/// Windows 默认禁止后台进程抢前台（只会闪任务栏图标），但 `RegisterHotKey`
/// 触发的场景属于官方豁免之一 —— 按键事件归属于注册热键的进程，
/// 因此这里的 `set_focus()` 是能生效的。
///
/// 判据用「可见**且**有焦点」而不是只看可见：窗口露在后面时按快捷键，
/// 用户想要的是拉到眼前，而不是把它藏起来。
pub fn toggle_main(app: &AppHandle) {
    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };

    let visible = w.is_visible().unwrap_or(false);
    let focused = w.is_focused().unwrap_or(false);

    if visible && focused {
        let _ = w.hide();
    } else {
        show_main(app);
    }
}

pub fn open_settings(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(SETTINGS) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }

    let _ = WebviewWindowBuilder::new(app, SETTINGS, app_url())
        .title("设置")
        .inner_size(820.0, 640.0)
        .min_inner_size(680.0, 480.0)
        .center()
        .resizable(true)
        .build();
}

/// 关闭主窗口时的三选确认框。
/// 无边框 + 置顶，视觉上接近原生对话框。
pub fn open_close_confirm(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(CLOSE_CONFIRM) {
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }

    let _ = WebviewWindowBuilder::new(app, CLOSE_CONFIRM, app_url())
        .title("关闭")
        .inner_size(380.0, 190.0)
        .center()
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .build();
}

pub fn close_window(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
}

pub fn hide_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MAIN) {
        let _ = w.hide();
    }
}

/// 让主窗口的任务栏按钮闪烁，直到用户看向它。
///
/// **这是系统通知的补充，不是替代。** 两条限制让 toast 单独用不够可靠：
/// - Windows 的 toast 必须由带 AppUserModelID 的应用发出，而这个 ID 来自
///   安装时创建的开始菜单快捷方式。未安装时 Tauri 回落到
///   `tauri-winrt-notification` 的 `POWERSHELL_APP_ID`，通知会被错误地
///   署名成 PowerShell/资源管理器 —— `tauri dev` 下必然如此。
/// - 专注助手（勿扰）开启时 toast 会被直接吞掉，不留痕迹。
///
/// 任务栏闪烁不受这两条限制，应用装没装都一样能用。
///
/// `FLASHW_TIMERNOFG` = 一直闪到窗口进入前台为止 —— 正好是「你回来看一眼就停」，
/// 不用我们自己计时或清理。
#[cfg(windows)]
pub fn flash_main(app: &AppHandle) {
    // 必须写 `::windows`：本模块自身就叫 windows，不加前导 `::` 会与 crate 名歧义
    use ::windows::Win32::Foundation::HWND;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        FlashWindowEx, FLASHWINFO, FLASHW_ALL, FLASHW_TIMERNOFG,
    };

    let Some(w) = app.get_webview_window(MAIN) else {
        return;
    };
    let Ok(handle) = w.hwnd() else {
        return;
    };

    let info = FLASHWINFO {
        cbSize: std::mem::size_of::<FLASHWINFO>() as u32,
        // 取原始指针再包一层：tauri 依赖的 windows crate 版本可能与我们直接
        // 依赖的不同，那样两个 HWND 会是不同类型，直接传会编译不过
        hwnd: HWND(handle.0),
        dwFlags: FLASHW_ALL | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };

    // 安全性：info 是栈上完整初始化的结构体，cbSize 按实际类型填写；
    // FlashWindowEx 只读该结构体，调用期间引用始终有效。
    unsafe {
        let _ = FlashWindowEx(&info);
    }
}

#[cfg(not(windows))]
pub fn flash_main(_app: &AppHandle) {}
