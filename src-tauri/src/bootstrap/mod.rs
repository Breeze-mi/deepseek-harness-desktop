//! 引导编排：从「什么都没有」到「dsh web 就绪、插件装好」。
//!
//! 每个阶段都会向前端推进度事件。引导失败时用户面对的是一个空窗口，
//! 事件流是他唯一能看到的东西，所以宁可多报也不要静默。

pub mod download;
pub mod dsh;
pub mod mirror;
pub mod node;
pub mod plugins;
pub mod proc;
pub mod upgrade;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::error::{BootstrapError, ErrorPayload, Result};
use crate::runtime::{health, process, watcher, DshState};
use crate::settings;

pub const EVENT_PROGRESS: &str = "bootstrap:progress";
pub const EVENT_READY: &str = "bootstrap:ready";
pub const EVENT_FAILED: &str = "bootstrap:failed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    CheckingNode,
    DownloadingNode,
    CheckingDsh,
    InstallingDsh,
    InitProfile,
    InstallingPlugins,
    VerifyingPlugins,
    StartingDsh,
    WaitingReady,
}

impl Stage {
    pub const TOTAL: u8 = 9;

    pub fn label(self) -> &'static str {
        match self {
            Self::CheckingNode => "检查 Node.js 环境",
            Self::DownloadingNode => "下载 Node.js 运行时",
            Self::CheckingDsh => "检查 dsh 版本",
            Self::InstallingDsh => "安装 DeepSeek Harness",
            Self::InitProfile => "初始化配置",
            Self::InstallingPlugins => "安装界面插件",
            Self::VerifyingPlugins => "校验插件挂载",
            Self::StartingDsh => "启动 dsh 服务",
            Self::WaitingReady => "等待服务就绪",
        }
    }

    pub fn index(self) -> u8 {
        match self {
            Self::CheckingNode => 1,
            Self::DownloadingNode => 2,
            Self::CheckingDsh => 3,
            Self::InstallingDsh => 4,
            Self::InitProfile => 5,
            Self::InstallingPlugins => 6,
            Self::VerifyingPlugins => 7,
            Self::StartingDsh => 8,
            Self::WaitingReady => 9,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub stage: Stage,
    pub label: String,
    pub detail: Option<String>,
    /// 仅下载类阶段有值，0.0 ~ 1.0
    pub fraction: Option<f64>,
    pub index: u8,
    pub total: u8,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyPayload {
    pub url: String,
}

/// 进度上报器。所有阶段共用，保证事件格式一致。
#[derive(Clone)]
pub struct Reporter {
    app: AppHandle,
}

impl Reporter {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }

    fn emit(&self, p: Progress) {
        // 上报失败不应影响引导本身继续跑
        let _ = self.app.emit(EVENT_PROGRESS, p);
    }

    pub fn stage(&self, stage: Stage) {
        self.emit(Progress {
            stage,
            label: stage.label().to_string(),
            detail: None,
            fraction: None,
            index: stage.index(),
            total: Stage::TOTAL,
        });
    }

    pub fn detail(&self, stage: Stage, detail: impl Into<String>) {
        self.emit(Progress {
            stage,
            label: stage.label().to_string(),
            detail: Some(detail.into()),
            fraction: None,
            index: stage.index(),
            total: Stage::TOTAL,
        });
    }

    /// 下载进度。total 未知时（服务端没给 Content-Length）只报已下载量。
    pub fn download(&self, stage: Stage, done: u64, total: Option<u64>) {
        let (detail, fraction) = match total {
            Some(t) if t > 0 => (
                format!("{:.1} / {:.1} MB", mb(done), mb(t)),
                Some(done as f64 / t as f64),
            ),
            _ => (format!("{:.1} MB", mb(done)), None),
        };

        self.emit(Progress {
            stage,
            label: stage.label().to_string(),
            detail: Some(detail),
            fraction,
            index: stage.index(),
            total: Stage::TOTAL,
        });
    }

    pub fn ready(&self, url: impl Into<String>) {
        let _ = self.app.emit(EVENT_READY, ReadyPayload { url: url.into() });
    }

    pub fn fail(&self, err: &BootstrapError) {
        eprintln!("[bootstrap] 失败: {err}");
        let _ = self.app.emit(EVENT_FAILED, ErrorPayload::from(err));
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0
}

/// 引导主流程。失败时把错误交给 Reporter 推给前端，由用户决定是否重试。
pub async fn run(app: AppHandle) {
    let reporter = Reporter::new(app.clone());

    match pipeline(&app, &reporter).await {
        Ok(url) => reporter.ready(url),
        Err(e) => reporter.fail(&e),
    }
}

async fn pipeline(app: &AppHandle, reporter: &Reporter) -> Result<String> {
    let node = ensure_node(reporter).await?;
    let entry = ensure_dsh(app, &node, reporter).await?;

    // dsh 首次运行会自行创建 ~/.dsh 与 web profile，不需要显式 init
    reporter.stage(Stage::InitProfile);

    // 界面插件（鲸鱼娘 / 任务看板 / 皮肤中心都在这个包里）
    ensure_plugins(&node, &entry, reporter).await?;

    // ---- 启动 dsh ----
    reporter.stage(Stage::StartingDsh);

    // **必须放进阻塞线程池。** `wait_for_url` 内部是 `recv_timeout`，
    // 标准库的同步阻塞调用 —— 直接在 async 里跑会把一个 tokio worker
    // 占死最多 60 秒，期间连进度事件都推不出去。
    let node_for_spawn = node.clone();
    let entry_for_spawn = entry.clone();
    let (handle, url) = tokio::task::spawn_blocking(
        move || -> Result<(process::DshHandle, String)> {
            let (handle, rx) = process::spawn(&node_for_spawn, &entry_for_spawn)?;
            let url = process::wait_for_url(&rx, 60)?;
            Ok((handle, url))
        },
    )
    .await
    .map_err(|e| BootstrapError::Other(format!("启动任务异常退出：{e}")))??;

    // 句柄交给 managed state 保管。若只放局部变量，函数返回时会被 drop，
    // Drop 实现会把刚起来的 dsh 直接杀掉。
    app.state::<DshState>().set(handle, url.clone());
    // 端口是内部细节，对用户零信息量 —— 只进日志（process.rs 已打 [dsh] 前缀那行）
    reporter.detail(Stage::StartingDsh, "服务已启动");

    // ---- 等真正能服务 ----
    reporter.stage(Stage::WaitingReady);
    health::wait_ready(&url, 60).await?;
    reporter.detail(Stage::WaitingReady, "即将进入…");

    // 任务完成通知：轮询 dsh-pet 的状态接口。插件没装时会自行退出，不影响主链路。
    watcher::spawn(
        app.clone(),
        url.clone(),
        app.state::<watcher::NotifyEnabled>().inner().clone(),
    );

    Ok(url)
}

/// 三级降级：系统 Node → 已装的便携版 → 下载便携版
async fn ensure_node(reporter: &Reporter) -> Result<node::NodeInfo> {
    reporter.stage(Stage::CheckingNode);

    if let Some(info) = node::detect_system_node() {
        reporter.detail(
            Stage::CheckingNode,
            format!("使用系统 Node.js {}", info.version),
        );
        return Ok(info);
    }

    if let Some(info) = download::installed_portable_node() {
        reporter.detail(
            Stage::CheckingNode,
            format!("使用便携版 Node.js {}", info.version),
        );
        return Ok(info);
    }

    // 明确区分「没装」与「装了但版本不对」—— 用户的后续动作完全不同
    let why = match node::system_node_version() {
        Some(v) => format!("系统 Node.js {v} 不满足要求，将下载便携版"),
        None => "未检测到 Node.js，将下载便携版".to_string(),
    };
    reporter.detail(Stage::CheckingNode, why);

    reporter.stage(Stage::DownloadingNode);
    let info = download::install_portable_node(reporter).await?;
    reporter.detail(
        Stage::DownloadingNode,
        format!("便携版 Node.js {} 就绪", info.version),
    );
    Ok(info)
}

/// 安装界面插件并自检。
///
/// 装不上的表现是「DSH 能用但侧边栏空空、鲸鱼娘不出现」，用户只会以为软件坏了，
/// 所以自检失败必须明确报错，不能静默放过。
async fn ensure_plugins(
    node: &node::NodeInfo,
    entry: &std::path::Path,
    reporter: &Reporter,
) -> Result<()> {
    reporter.stage(Stage::InstallingPlugins);

    let node = node.clone();
    let entry = entry.to_path_buf();
    let rep = reporter.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        if plugins::is_installed() {
            rep.detail(Stage::InstallingPlugins, "界面插件已就绪");
        } else {
            // 首次安装要拉几十个子包，可能几分钟
            plugins::install(&node, &entry, &rep)?;
        }

        rep.stage(Stage::VerifyingPlugins);
        plugins::verify(&node, &entry)?;
        rep.detail(Stage::VerifyingPlugins, "插件校验通过");
        Ok(())
    })
    .await
    .map_err(|e| BootstrapError::Other(format!("插件任务异常退出：{e}")))?
}

/// 定位 dsh，没有就装。
///
/// 缓存命中时跳过 `npm root -g`（npm 冷启动 1-2s）与 `dsh --version`（0.5-1s）。
/// 这两步是每次启动最大的固定开销，而它们的结果几乎从不变化。
///
/// npm 安装是阻塞的且可能跑 1-3 分钟，必须扔到阻塞线程池 ——
/// 直接在 async 上下文里跑会占死一个 tokio worker，进度事件都推不出去。
async fn ensure_dsh(
    app: &AppHandle,
    node: &node::NodeInfo,
    reporter: &Reporter,
) -> Result<std::path::PathBuf> {
    reporter.stage(Stage::CheckingDsh);

    // 快路径：缓存里的路径仍然存在就直接用
    if let Some(entry) = settings::dsh_entry(app) {
        return Ok(entry);
    }

    let node = node.clone();
    let rep = reporter.clone();

    let entry = tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf> {
        match dsh::entry_point()? {
            Some(entry) => {
                let label = match dsh::installed_version(&node, &entry) {
                    Some(v) => format!("已安装 dsh {v}"),
                    None => "已安装 dsh".to_string(),
                };
                rep.detail(Stage::CheckingDsh, label);
                Ok(entry)
            }
            None => {
                rep.stage(Stage::InstallingDsh);
                dsh::install(&rep)
            }
        }
    })
    .await
    .map_err(|e| BootstrapError::Other(format!("安装任务异常退出：{e}")))??;

    settings::set_dsh_entry(app, &entry);
    Ok(entry)
}
