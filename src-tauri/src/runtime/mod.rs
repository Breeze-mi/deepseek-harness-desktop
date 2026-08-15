pub mod health;
pub mod process;
pub mod watcher;

use std::sync::Mutex;

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
