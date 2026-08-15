pub mod health;
pub mod process;
pub mod watcher;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager};

use process::DshHandle;

/// dsh 运行时状态，由 Tauri 的 managed state 保管。
///
/// 句柄放这里而不是局部变量，是为了让它活到进程退出 —— 一旦被 drop，
/// Drop 实现会 kill 子进程，dsh 就没了。
/// URL 一并存着：重启 dsh、状态展示都要用，P4 的诊断页也会读它。
#[derive(Default)]
pub struct DshState {
    handle: Mutex<Option<DshHandle>>,
    url: Mutex<Option<String>>,
}

impl DshState {
    pub fn set(&self, handle: DshHandle, url: String) {
        if let Ok(mut slot) = self.handle.lock() {
            // 旧句柄在这里被 drop，顺带把上一个 dsh 关掉，避免重启时留两份
            *slot = Some(handle);
        }
        if let Ok(mut slot) = self.url.lock() {
            *slot = Some(url);
        }
    }

    /// 服务地址。未就绪时为 None。
    pub fn url(&self) -> Option<String> {
        self.url.lock().ok()?.clone()
    }

    pub fn shutdown(&self) {
        if let Ok(mut slot) = self.handle.lock() {
            if let Some(mut h) = slot.take() {
                h.shutdown();
            }
        }
        if let Ok(mut slot) = self.url.lock() {
            *slot = None;
        }
    }
}

/// 是否正在重启。
///
/// 托盘菜单点两下、或「升级 dsh」与「重启服务」撞在一起时，两个 restart 并发跑
/// 会互相踩：都 shutdown、都 spawn，`DshState::set` 又会把先起来的那个 drop 掉
/// （连带杀掉它的进程），最后留下两个监视器和一段莫名其妙的日志。
/// 用一个标志挡住，第二次直接告诉用户在忙。
static RESTARTING: AtomicBool = AtomicBool::new(false);

/// 重启 dsh 服务，但不重启整个应用。托盘菜单的「重启 dsh 服务」走这里。
///
/// **不能靠给前端 emit 事件来做。** 主窗口在引导完成后已经导航到 DSH 的网页，
/// 我们自己的 Vue 应用连同它的事件监听器都不在主窗口里了 —— 事件发出去没人接。
/// 这类活只能全程留在 Rust 侧。
///
/// 顺序是刻意的：先把 Node 与入口找齐再停旧服务。反过来的话，一旦探测失败，
/// 用户就只剩一个被杀掉的 dsh 和一个白屏窗口。
pub async fn restart(app: AppHandle) -> std::result::Result<String, String> {
    use crate::bootstrap::{download, node};

    if RESTARTING.swap(true, Ordering::SeqCst) {
        return Err("dsh 正在重启中，请稍候".into());
    }
    // 提前拿好，保证后面每条 return 路径都会解锁
    let _guard = RestartGuard;

    let entry = crate::settings::dsh_entry(&app)
        .ok_or_else(|| "找不到 dsh 安装位置，请重启应用重新引导".to_string())?;

    let node = node::detect_system_node()
        .or_else(download::installed_portable_node)
        .ok_or_else(|| "找不到可用的 Node.js 运行时".to_string())?;

    app.state::<DshState>().shutdown();

    // **spawn 和 wait_for_url 必须一起放进阻塞线程池。**
    // wait_for_url 内部是 `recv_timeout`，标准库的同步阻塞调用；
    // 直接在 async 里 await 不到它，只会把一个 tokio worker 占死最多 60 秒。
    let (handle, url) = tokio::task::spawn_blocking(
        move || -> crate::error::Result<(DshHandle, String)> {
            let (handle, rx) = process::spawn(&node, &entry)?;
            let url = process::wait_for_url(&rx, 60)?;
            Ok((handle, url))
        },
    )
    .await
    .map_err(|e| format!("重启任务异常退出：{e}"))?
    .map_err(|e| e.to_string())?;

    health::wait_ready(&url, 60).await.map_err(|e| e.to_string())?;

    app.state::<DshState>().set(handle, url.clone());

    // 端口是 OS 重新分配的，一定和之前不同，所以必须把主窗口导过去
    if let Some(w) = app.get_webview_window(crate::windows::MAIN) {
        match tauri::Url::parse(&url) {
            Ok(parsed) => {
                let _ = w.navigate(parsed);
            }
            // 这里不 return Err：服务其实已经起来了，报错会让设置页显示
            // 「服务已停止，请重启应用」，与事实相反。只记日志。
            Err(e) => eprintln!("[runtime] dsh 返回的地址无法解析（{url}）：{e}"),
        }
    }

    // 旧的监视器还指着已经死掉的端口，它会连续连不上若干次后自行退出，不用管
    watcher::spawn(
        app.clone(),
        url.clone(),
        app.state::<watcher::NotifyEnabled>().inner().clone(),
    );

    Ok(url)
}

/// 保证 `RESTARTING` 在任何一条 return 路径（包括 `?` 提前返回）上都会解锁。
struct RestartGuard;

impl Drop for RestartGuard {
    fn drop(&mut self) {
        RESTARTING.store(false, Ordering::SeqCst);
    }
}
