//! 运行日志的项目侧配置。
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

/// 相对启动的秒数。格式化在 lib.rs 的 format 闭包里做，
/// 控制台与文件因此拿到同一行字。
pub fn rel_secs() -> f32 {
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

/// 日志文件路径。
pub fn path() -> Option<PathBuf> {
    Some(
        dirs::data_dir()?
            .join("deepseek-harness-desktop")
            .join("app.log"),
    )
}

/// 启动时调用一次：登记时间起点 + 轮转上一轮日志。

pub fn init() {
    let _ = START.get_or_init(Instant::now);

    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::rename(&p, p.with_extension("log.old"));
}

/// 兼容层
/// log target 统一记 "app"，与依赖库的日志在文件里可区分。
#[macro_export]
macro_rules! dlog {
    ($($arg:tt)*) => {
        ::log::info!(target: "app", $($arg)*)
    };
}
