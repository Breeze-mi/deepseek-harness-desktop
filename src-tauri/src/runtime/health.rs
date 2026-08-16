//! 就绪探测：dsh 打印出监听地址不等于能服务了。

use std::time::Duration;

use crate::error::{BootstrapError, Result};

const POLL_INTERVAL_MS: u64 = 250;

/// 轮询直到 HTTP 可用。
/// dsh 打印 URL 与真正开始接受请求之间有窗口期，直接导航过去会白屏，
/// 所以必须轮询确认。任何 HTTP 响应都算就绪 —— 401/403 说明服务起来了，
/// 只是有鉴权围栏，不该当成失败。
pub async fn wait_ready(url: &str, timeout_secs: u64) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| BootstrapError::Other(e.to_string()))?;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs);
    let mut last: Option<String> = None;

    while tokio::time::Instant::now() < deadline {
        match client.get(url).send().await {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e.to_string()),
        }
        tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
    }

    dlog!("[health] 就绪探测超时，最后一次错误: {last:?}");
    Err(BootstrapError::ReadyTimeout { secs: timeout_secs })
}
