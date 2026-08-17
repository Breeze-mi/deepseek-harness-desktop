//! 轻量配置持久化（tauri-plugin-store）。
//!
//! 只放「应用外壳」自己的偏好；DSH 的配置一律归 `~/.dsh`，我们不碰。

use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

const FILE: &str = "settings.json";

/// 关闭主窗口的默认动作："quit" | "tray"；未设置时弹三选框询问
const KEY_CLOSE_ACTION: &str = "closeAction";

pub fn close_action(app: &AppHandle) -> Option<String> {
    let store = app.store(FILE).ok()?;
    let value = store.get(KEY_CLOSE_ACTION)?;
    value.as_str().map(str::to_string)
}

pub fn set_close_action(app: &AppHandle, action: &str) {
    let Ok(store) = app.store(FILE) else { return };
    store.set(KEY_CLOSE_ACTION, serde_json::Value::from(action));
    let _ = store.save();
}

pub fn clear_close_action(app: &AppHandle) {
    let Ok(store) = app.store(FILE) else { return };
    store.delete(KEY_CLOSE_ACTION);
    let _ = store.save();
}

/// 缓存 dsh 的 JS 入口路径。
///
/// 发现它要跑 `npm root -g`，而 npm 冷启动就要 1-2 秒 —— 每次开应用都付这个
/// 代价没道理。缓存后只需一次 `is_file()`（微秒级）就能确认还在，
/// 文件没了再回退到完整探测。
const KEY_DSH_ENTRY: &str = "dshEntry";

pub fn dsh_entry(app: &AppHandle) -> Option<std::path::PathBuf> {
    let store = app.store(FILE).ok()?;
    let raw = store.get(KEY_DSH_ENTRY)?;
    let path = std::path::PathBuf::from(raw.as_str()?);
    // 只信任仍然存在的路径。用户可能卸载或重装过 dsh。
    path.is_file().then_some(path)
}

pub fn set_dsh_entry(app: &AppHandle, path: &std::path::Path) {
    let Ok(store) = app.store(FILE) else { return };
    store.set(
        KEY_DSH_ENTRY,
        serde_json::Value::from(path.to_string_lossy().as_ref()),
    );
    let _ = store.save();
}

/// 呼出主窗口的全局快捷键。
///
/// 默认 `Alt+Space`，但**必须可改可关** —— Alt+Space 在 Windows 上是
/// 用了几十年的系统菜单快捷键，抢占它是高频抱怨点。
/// 空字符串表示关闭该功能。
const KEY_GLOBAL_SHORTCUT: &str = "globalShortcut";
pub const DEFAULT_GLOBAL_SHORTCUT: &str = "Alt+Space";

pub fn global_shortcut(app: &AppHandle) -> String {
    let Ok(store) = app.store(FILE) else {
        return DEFAULT_GLOBAL_SHORTCUT.to_string();
    };
    match store.get(KEY_GLOBAL_SHORTCUT) {
        // 显式存过空串 = 用户主动关了，要尊重，不能回退默认值
        Some(v) => v.as_str().unwrap_or(DEFAULT_GLOBAL_SHORTCUT).to_string(),
        None => DEFAULT_GLOBAL_SHORTCUT.to_string(),
    }
}

pub fn set_global_shortcut(app: &AppHandle, value: &str) {
    let Ok(store) = app.store(FILE) else { return };
    store.set(KEY_GLOBAL_SHORTCUT, serde_json::Value::from(value));
    let _ = store.save();
}

/// 任务完成时是否发系统通知。默认开 —— agent 任务动辄几分钟，
/// 用户多半已经切走干别的了，不提醒等于白等。
const KEY_NOTIFY_ON_DONE: &str = "notifyOnDone";

pub fn notify_on_done(app: &AppHandle) -> bool {
    let Ok(store) = app.store(FILE) else {
        return true;
    };
    store
        .get(KEY_NOTIFY_ON_DONE)
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
}

pub fn set_notify_on_done(app: &AppHandle, value: bool) {
    let Ok(store) = app.store(FILE) else { return };
    store.set(KEY_NOTIFY_ON_DONE, serde_json::Value::from(value));
    let _ = store.save();
}

/// 外壳主题模式："follow"（跟随 DSH，默认）| "light" | "dark"。
///
/// 显式模式存在的理由：跟随模式的延迟 = DSH 把偏好落盘 + 我们轮询发现，
/// 最坏要一两秒；标题栏按钮直切时必须立即生效，等不起这一趟。
const KEY_THEME_MODE: &str = "themeMode";

pub fn theme_mode(app: &AppHandle) -> String {
    let Ok(store) = app.store(FILE) else {
        return "follow".into();
    };
    store
        .get(KEY_THEME_MODE)
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "follow".into())
}

pub fn set_theme_mode(app: &AppHandle, value: &str) {
    let Ok(store) = app.store(FILE) else { return };
    store.set(KEY_THEME_MODE, serde_json::Value::from(value));
    let _ = store.save();
}
