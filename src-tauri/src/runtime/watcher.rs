//! 任务活动监视：轮询 dsh 的宠物状态接口，任务跑完时提醒用户。
//!
//! **为什么能直接从 Rust 拿到状态**：`@linxin666/dsh-pet` 在 dsh 服务端注册了
//! 同源 JSON 路由 `GET /api/pet/state`，返回当前会话的活动相位。
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

use crate::{tray, windows};

/// 轮询间隔。
///
/// 从 2 秒收到 1 秒：`done` 相位可能只持续很短一段（短回复尤其明显），
/// 采样周期比它长就会整个跳过，表现为「有时候不提醒」。
/// 目标是本机回环上的一个 JSON 接口，1 秒一次的开销可以忽略。
///
/// 注意这**只是缩小了漏采窗口，没有消除它** —— 真正的解法是找一个
/// 不依赖采样到瞬时状态的信号（比如单调递增的回合计数），
/// 但那需要先摸清 `/api/pet/state` 的完整响应结构。
const POLL: Duration = Duration::from_secs(1);

/// 连续多少次**连不上**才放弃。
///
/// 之前设成 3 太脆：dsh 偶尔一次超时就能把监视器永久干掉。
/// 按 1 秒间隔算，30 次约 30 秒，足以扛过一次卡顿；真正该退出的场景是
/// dsh 被换了端口（重启服务时会起新的监视器），那时连不上是必然的，退出正确。
const MAX_FAILURES: u8 = 30;

/// Windows toast 的提示音。名字必须能被 `Sound::from_str` 认下 ——
/// notify-rust 对解析失败是 `.ok()` 静默吞掉，写错了就是没声音，不会报错。
const SOUND: &str = "Default";

/// 一次轮询的结果。
///
/// **「拿不到相位」和「连不上服务」必须分开。** 早先把两者都当失败计数，
/// 于是宠物在某些状态下响应里没有 `phase` 字段时，监视器会在几秒内
/// 耗尽失败次数并永久退出 —— 表现就是「只有第一次任务有提醒」。
enum Poll {
    Phase {
        phase: String,
        /// `affinity.turns`。**目前只用来打日志，不参与判定。**
        ///
        /// 实测响应里有这个单调递增的回合计数，它理论上能根治「done 相位
        /// 太短、被采样跳过」的漏报 —— 计数是累积量，错过一拍也能从
        /// 「数字变了」推断出中间完成过。
        ///
        /// 但我只见过一个采样点，无法确认它是「每完成一轮智能体任务 +1」
        /// 还是也统计摸头投喂之类的互动。拿一个样本就改判据，等于重复
        /// 之前猜相位词表的错误 —— 先记录，攒够数据再决定。
        turns: Option<u64>,
    },
    /// 服务正常，但这次响应里没有相位。不算故障，跳过即可。
    NoPhase,
    Unreachable,
}

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

/// 在 JSON 里递归找第一个整数型 `turns` 字段。
///
/// 实测它嵌在 `affinity` 里，但和 `phase` 一样不押注层级 —— 按字段名找。
fn find_turns(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(n) = map.get("turns").and_then(serde_json::Value::as_u64) {
                return Some(n);
            }
            map.values().find_map(find_turns)
        }
        serde_json::Value::Array(items) => items.iter().find_map(find_turns),
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
        let mut last_turns: Option<u64> = None;
        // 已经为哪个 turns 值提醒过。防止两个完成信号在同一拍重复触发。
        let mut notified_turns: Option<u64> = None;
        let mut failures: u8 = 0;

        loop {
            tokio::time::sleep(POLL).await;

            let (phase, turns) = match fetch(&client, &endpoint).await {
                Poll::Phase { phase, turns } => {
                    failures = 0;
                    (phase, turns)
                }
                // 服务活着，只是这一拍没有相位。保持 last_phase 不动 ——
                // 清掉的话下一次真正的 done 会因为「前一个相位未知」而漏报。
                Poll::NoPhase => {
                    failures = 0;
                    continue;
                }
                Poll::Unreachable => {
                    failures += 1;
                    if failures >= MAX_FAILURES {
                        eprintln!(
                            "[watcher] {endpoint} 连续 {MAX_FAILURES} 次连不上，停止监视。\
                             正常情况：dsh 已重启（新监视器已接管）或插件未安装。"
                        );
                        return;
                    }
                    continue;
                }
            };

            let prev_phase = last_phase.replace(phase.clone());
            let prev_turns = std::mem::replace(&mut last_turns, turns);

            // 日志只在相位变化时打，否则每一拍刷一行会把终端淹掉
            if prev_phase.as_deref() != Some(phase.as_str()) {
                eprintln!(
                    "[watcher] 相位 {} -> {phase}（turns={}）",
                    prev_phase.as_deref().unwrap_or("(初次)"),
                    turns.map_or_else(|| "?".to_string(), |n| n.to_string())
                );
            }

            // 完成信号有两个来源，任一命中即算完成：
            //
            // 1. **相位切进 done** —— 直观，但依赖恰好采到 done 那一拍。
            //    done 持续时间短于轮询间隔时会整个漏掉（短回复尤其容易）。
            //
            // 2. **turns 增加** —— 累积量，漏采多少拍都能补回来。
            //    实测一整轮里 turns 保持 34 不变（经过 idle/waiting/thinking/
            //    tool/waiting/thinking/review 七次切换），恰好在 done 那一刻变 35。
            //
            // 第 2 条是根治性的，第 1 条留着兜住「响应里没有 turns 字段」的情况。
            let phase_done = phase == "done"
                && prev_phase.is_some()
                && prev_phase.as_deref() != Some("done");

            let turns_bumped = matches!((turns, prev_turns), (Some(now), Some(before)) if now > before);

            if !(phase_done || turns_bumped) {
                continue;
            }

            // 两个信号通常在**同一拍**同时命中（相位变 done 的那次采样，
            // turns 也刚好变了），不去重就会一次完成响两声。
            // 按 turns 值去重；没有 turns 时靠第 1 条本身是边沿触发，不会重复。
            if turns.is_some() && turns == notified_turns {
                continue;
            }
            notified_turns = turns;

            if !enabled.get() {
                eprintln!("[watcher] 任务完成，但通知已被关闭");
                continue;
            }
            // 这两条日志用来区分漏报的成因：走到这里说明**判定为已完成**，
            // 那么没提醒就只可能是认为用户在看。
            if user_is_watching(&app) {
                eprintln!("[watcher] 任务完成，主窗口在前台，不打扰");
                continue;
            }

            eprintln!(
                "[watcher] 任务完成，发出提醒（触发源：{}）",
                if turns_bumped { "turns" } else { "相位" }
            );
            notify(&app);
            windows::flash_main(&app);
            tray::start_blink(&app);
        }
    });
}

async fn fetch(client: &reqwest::Client, endpoint: &str) -> Poll {
    let Ok(resp) = client.get(endpoint).send().await else {
        return Poll::Unreachable;
    };
    if !resp.status().is_success() {
        return Poll::Unreachable;
    }
    let Ok(json) = resp.json::<serde_json::Value>().await else {
        return Poll::Unreachable;
    };
    match find_phase(&json) {
        Some(phase) => Poll::Phase {
            phase,
            turns: find_turns(&json),
        },
        None => Poll::NoPhase,
    }
}

/// 用户此刻是不是真的在看主窗口。
///
/// **不能只看 `is_focused()`。** 最小化的窗口用户绝对看不到，但焦点状态
/// 是否可靠地变成 false，取决于 Tauri 查的是原生窗口还是 webview、
/// 以及最小化那一瞬的时序 —— 这一点我没有验证过，不该拿它当唯一判据。
///
/// 三个条件都满足才算「在看」：可见、没最小化、有焦点。宁可多提醒一次，
/// 也不要在用户切走之后静悄悄地什么都不做。
fn user_is_watching(app: &AppHandle) -> bool {
    let Some(w) = app.get_webview_window(windows::MAIN) else {
        return false;
    };
    w.is_visible().unwrap_or(false)
        && !w.is_minimized().unwrap_or(false)
        && w.is_focused().unwrap_or(false)
}

/// 发通知。
///
/// **返回的 Ok 不代表通知真的弹出来了。** 插件内部是
/// `tauri::async_runtime::spawn(async move { let _ = notification.show(); })`
/// （见 tauri-plugin-notification-2.3.3/src/desktop.rs:216），
/// 真正的投递错误被 `let _ =` 吞掉了，我们这里只能拿到构造阶段的错误。
/// 所以「没弹通知」这类问题不要指望日志，得去看 Windows 的通知设置。
fn notify(app: &AppHandle) {
    let result = app
        .notification()
        .builder()
        .title("DeepSeek Harness")
        .body("任务已完成")
        .sound(SOUND)
        .show();

    if let Err(e) = result {
        eprintln!("[watcher] 构造通知失败：{e}");
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
