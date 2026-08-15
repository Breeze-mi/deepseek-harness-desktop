//! 任务活动监视：轮询 dsh 的宠物状态接口，任务跑完时发系统通知。
//!
//! **为什么能直接从 Rust 拿到状态**：`@linxin666/dsh-pet` 在 dsh 服务端注册了
//! 同源 JSON 路由 `GET /api/pet/state`，返回当前会话的活动相位
//! （`idle / waiting / thinking / tool / done`）。
//!
//! `/api` 有一道 browser-trust 围栏，但它校验的是 `Host` 头（防 DNS rebinding），
//! 上游源码注释原话：「Non-browser and remote clients pass the same fence via
//! loopback」。我们请求 `127.0.0.1` 属于回环，直接放行 ——
//! 所以不用注入页面、也不用 `--trusted-host`。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::windows;

/// 轮询间隔。宠物动画本身是秒级的，2 秒足够捕捉相位切换，
/// 又不至于给本地服务添太多无谓请求。
const POLL: Duration = Duration::from_secs(2);

/// 连续失败多少次就放弃。插件没装或接口改名时不该无限刷日志。
const MAX_FAILURES: u8 = 3;

/// 会被认为「正在干活」的相位。只有从这些相位切到 done 才算一次任务完成 ——
/// 否则应用刚启动读到 done 也会误报一条通知。
const ACTIVE_PHASES: &[&str] = &["thinking", "tool", "waiting"];

/// 通知开关，供设置页实时切换（不用重启监视器）
#[derive(Clone, Default)]
pub struct NotifyEnabled(Arc<AtomicBool>);

impl NotifyEnabled {
    pub fn get(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set(&self, value: bool) {
        self.0.store(value, Ordering::Relaxed);
    }
}

/// 在 JSON 里递归找第一个字符串型 `phase` 字段。
///
/// 上游没有对响应外层结构做稳定承诺（可能是裸对象、也可能包一层 `ok`/`state`），
/// 与其押注某个形状，不如按字段名找 —— 外层怎么变都不影响。
fn find_phase(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(p)) = map.get("phase") {
                return Some(p.clone());
            }
            map.values().find_map(find_phase)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_phase),
        _ => None,
    }
}

/// 启动后台监视。base_url 形如 `http://127.0.0.1:12345`。
pub fn spawn(app: AppHandle, base_url: String, enabled: NotifyEnabled) {
    tauri::async_runtime::spawn(async move {
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
        else {
            return;
        };

        let endpoint = format!("{}/api/pet/state", base_url.trim_end_matches('/'));
        let mut last_phase: Option<String> = None;
        let mut failures: u8 = 0;

        loop {
            tokio::time::sleep(POLL).await;

            let phase = match fetch_phase(&client, &endpoint).await {
                Some(p) => {
                    failures = 0;
                    p
                }
                None => {
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        eprintln!(
                            "[watcher] {endpoint} 连续 {MAX_FAILURES} 次不可用，停止任务通知。\
                             通常是 dsh-pet 插件未安装。"
                        );
                        return;
                    }
                    continue;
                }
            };

            let previous = last_phase.replace(phase.clone());

            // 只在「从干活切到完成」的那一刻通知，且用户不在看的时候才打扰
            let finished = phase == "done"
                && previous
                    .as_deref()
                    .is_some_and(|p| ACTIVE_PHASES.contains(&p));

            if finished && enabled.get() && !main_window_focused(&app) {
                notify(&app);
                // 通知可能被专注助手吞掉、或在未安装时署名错误，
                // 任务栏闪烁是那种情况下唯一还在的信号
                windows::flash_main(&app);
            }
        }
    });
}

async fn fetch_phase(client: &reqwest::Client, endpoint: &str) -> Option<String> {
    let resp = client.get(endpoint).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    find_phase(&resp.json::<serde_json::Value>().await.ok()?)
}

/// 用户正盯着窗口时不该弹通知 —— 他已经看见了
fn main_window_focused(app: &AppHandle) -> bool {
    app.get_webview_window(windows::MAIN)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

fn notify(app: &AppHandle) {
    let result = app
        .notification()
        .builder()
        .title("DeepSeek Harness")
        .body("任务已完成")
        .show();

    if let Err(e) = result {
        eprintln!("[watcher] 发送通知失败：{e}");
    }
}

#[cfg(test)]
mod tests {
    use super::find_phase;
    use serde_json::json;

    #[test]
    fn finds_phase_at_top_level() {
        assert_eq!(
            find_phase(&json!({ "phase": "thinking", "line": "tool: grep" })).as_deref(),
            Some("thinking")
        );
    }

    #[test]
    fn finds_phase_when_wrapped() {
        // 外层多包一层也能找到 —— 上游没承诺过响应结构
        assert_eq!(
            find_phase(&json!({ "ok": true, "state": { "phase": "done" } })).as_deref(),
            Some("done")
        );
    }

    #[test]
    fn ignores_non_string_phase() {
        assert!(find_phase(&json!({ "phase": 3 })).is_none());
    }

    #[test]
    fn returns_none_without_phase() {
        assert!(find_phase(&json!({ "affinity": 25, "name": "小鲸" })).is_none());
    }
}
