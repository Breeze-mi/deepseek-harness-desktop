//! 跟随 DSH 的界面主题（亮 / 暗）。

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

pub const EVENT_THEME: &str = "app:theme";

/// 跟随模式的轮询间隔。
const POLL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemePayload {
    /// 解析后的实际主题，只会是 "light" 或 "dark"
    pub theme: String,
}

/// `$DSH_HOME/settings.yaml`
fn settings_path() -> Option<PathBuf> {
    Some(crate::bootstrap::dsh::home()?.join("settings.yaml"))
}

/// 取 `ui-theme.preference` 的原始值。
///
/// 手写解析而不是引 YAML 库：只取一个固定位置的标量
fn preference(text: &str) -> Option<String> {
    let mut in_block = false;

    for line in text.lines() {
        // 顶层键（无缩进）决定进入或离开 ui-theme 段
        if !line.starts_with([' ', '\t']) {
            in_block = line.trim_end() == "ui-theme:";
            continue;
        }
        if !in_block {
            continue;
        }
        if let Some(v) = line.trim().strip_prefix("preference:") {
            return Some(v.trim().trim_matches(['"', '\'']).to_string());
        }
    }
    None
}

/// 系统配色。与 DSH 网页里 `prefers-color-scheme` 读的是同一个系统设置
fn system_theme(app: &AppHandle) -> String {
    app.get_webview_window(crate::windows::MAIN)
        .and_then(|w| w.theme().ok())
        .map(|t| match t {
            tauri::Theme::Light => "light",
            _ => "dark",
        })
        .unwrap_or("dark")
        .to_string()
}

/// 当前应该用的主题。
///
/// 外壳自己的模式（settings.json 的 themeMode）优先：显式亮/暗直接定案；
/// "follow"（默认）才去读 DSH 的偏好。
pub fn current(app: &AppHandle) -> String {
    match crate::settings::theme_mode(app).as_str() {
        "light" => "light".into(),
        "dark" => "dark".into(),
        _ => follow(app),
    }
}

/// 跟随 DSH：读它的 settings.yaml，读不到按系统走。
fn follow(app: &AppHandle) -> String {
    let pref = settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| preference(&t))
        .unwrap_or_else(|| "system".into());

    match pref.as_str() {
        "light" => "light".into(),
        "dark" => "dark".into(),
        // 未知值（上游允许注册第三方主题 id）按系统解析 —— 不猜它是亮是暗
        _ => system_theme(app),
    }
}

pub async fn push_to_dsh(app: &AppHandle, theme: &str) -> bool {
    let Some(base) = app.state::<crate::runtime::DshState>().url() else {
        dlog!("[theme] dsh 服务未就绪，主题只切外壳");
        return false;
    };

    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .build()
    else {
        return false;
    };

    // rpcId 服务端只回显校验，不约束格式；时间戳足够唯一
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let body = serde_json::json!({
        "type": "client-request",
        "rpcId": format!("desktop-theme-{nanos}"),
        "method": "settings.update",
        "payload": { "ns": "ui-theme", "patch": { "preference": theme } }
    });

    let url = format!("{}/api/settings.update", base.trim_end_matches('/'));
    match client.post(&url).json(&body).send().await {
        Ok(resp) if resp.status().is_success() => {
            // 失败时必须把 DSH 自己的错误码带出来。
            let text = resp.text().await.unwrap_or_default();
            let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
                dlog!("[theme] 写入返回的不是 JSON：{}", head(&text));
                return false;
            };

            if json
                .pointer("/result/ok")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                dlog!("[theme] 已把 {theme} 写入 DSH 的外观设置");
                return true;
            }

            let code = json
                .pointer("/result/error/code")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("(无错误码)");
            let msg = json
                .pointer("/result/error/message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            dlog!("[theme] DSH 拒绝写入：{code} {msg}");
            false
        }
        Ok(resp) => {
            dlog!("[theme] 主题写入返回 HTTP {}", resp.status());
            false
        }
        Err(e) => {
            dlog!("[theme] 主题写入失败：{e}");
            false
        }
    }
}

/// 截断到前 200 字符，够看出是 HTML 还是别的什么，又不会把日志刷爆
fn head(text: &str) -> String {
    text.chars().take(200).collect()
}

/// 后台盯着主题变化，变了就广播给所有窗口。
pub fn spawn_watcher(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut last = current(&app);
        // DSH 自己那一侧的解析结果，与上面的「最终生效值」分开记
        let mut last_dsh = follow(&app);

        loop {
            tokio::time::sleep(POLL).await;

            // 每拍只读一次 DSH 的文件：治愈判断与最终取值共用这份快照，
            // 既省一半 IO，也不会出现「判断用的一份、广播用的另一份」的错位
            let dsh = follow(&app);

            // DSH 侧变了 = 用户在 DSH 里切了主题，它重新掌权：
            // 退出外壳独立模式回到跟随。
            if dsh != last_dsh {
                last_dsh.clone_from(&dsh);
                if crate::settings::theme_mode(&app) != "follow" {
                    dlog!("[theme] DSH 侧主题已变化，退出外壳独立模式，恢复跟随");
                    crate::settings::set_theme_mode(&app, "follow");
                }
            }

            // 治愈可能刚把模式改回 follow，所以模式要在治愈之后读
            let now = match crate::settings::theme_mode(&app).as_str() {
                "light" => "light".to_string(),
                "dark" => "dark".to_string(),
                _ => dsh,
            };
            if now != last {
                dlog!("[theme] 主题切到 {now}");
                last.clone_from(&now);
                let _ = app.emit(EVENT_THEME, ThemePayload { theme: now });
            }
        }
    });
}

// #[cfg(test)]
// mod tests {
//     use super::preference;

//     /// 真机 `~/.dsh/settings.yaml` 的片段：ui-theme 前后都有别的顶层段
//     const REAL: &str = "ui-onboarding:\n  welcomeNoticeVersion: 2026-08-13.1\nui-theme:\n  preference: light\npet:\n  visible: true\n";

//     #[test]
//     fn reads_preference_between_other_blocks() {
//         assert_eq!(preference(REAL).as_deref(), Some("light"));
//     }

//     #[test]
//     fn absent_block_is_none() {
//         assert!(preference("pet:\n  visible: true\n").is_none());
//     }

//     /// 别的段里同名的键不能串味
//     #[test]
//     fn ignores_preference_in_other_blocks() {
//         let text = "locale:\n  preference: zh\npet:\n  visible: true\n";
//         assert!(preference(text).is_none());
//     }

//     #[test]
//     fn strips_quotes() {
//         assert_eq!(
//             preference("ui-theme:\n  preference: \"dark\"\n").as_deref(),
//             Some("dark")
//         );
//     }
// }
