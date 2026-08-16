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

/// 「已经自动重启过一次」的标记，随 `AppHandle::restart` 传给下一代进程。
///
/// dsh 偶发起不来时我们会整个重启应用再试（见 `pipeline`）。用环境变量而不是
/// 文件做标记：重启出来的子进程天然继承环境，而用户手动重开应用时环境是
/// 干净的 —— 于是「每次人工启动最多自动重启一次」，构造上不可能循环。
const RELAUNCH_ENV: &str = "DSH_DESKTOP_AUTO_RELAUNCHED";

use crate::error::{BootstrapError, ErrorPayload, Result};
use crate::runtime::{health, process, watcher, DshState};
use crate::settings;

pub const EVENT_PROGRESS: &str = "bootstrap:progress";
pub const EVENT_READY: &str = "bootstrap:ready";
pub const EVENT_FAILED: &str = "bootstrap:failed";
/// 非致命问题：引导会继续走完，但用户应该知道少了什么。
pub const EVENT_WARNING: &str = "bootstrap:warning";

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
    /// 仅下载/安装类阶段有值，0.0 ~ 1.0
    pub fraction: Option<f64>,
    pub index: u8,
    pub total: u8,
    /// 瞬态进度（下载字节数、安装计数）：界面上只刷新「当前活动」一行，
    /// 不进「已完成」列表 —— 否则几十条进度刷屏会把真正的里程碑挤掉。
    pub transient: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadyPayload {
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WarningPayload {
    pub message: String,
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
            transient: false,
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
            transient: false,
        });
    }

    /// 瞬态活动：马上会被下一条覆盖的那种进度（安装计数、校验中）。
    /// 带 fraction 时进度条会真的动起来，而不是停在阶段起点干等。
    pub fn activity(&self, stage: Stage, detail: impl Into<String>, fraction: Option<f64>) {
        self.emit(Progress {
            stage,
            label: stage.label().to_string(),
            detail: Some(detail.into()),
            fraction,
            index: stage.index(),
            total: Stage::TOTAL,
            transient: true,
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

        // 字节数是典型的瞬态信息，一秒好几条，进不得「已完成」列表
        self.emit(Progress {
            stage,
            label: stage.label().to_string(),
            detail: Some(detail),
            fraction,
            index: stage.index(),
            total: Stage::TOTAL,
            transient: true,
        });
    }

    pub fn ready(&self, url: impl Into<String>) {
        let _ = self.app.emit(EVENT_READY, ReadyPayload { url: url.into() });
    }

    /// 非致命问题。引导继续走完，但要留下用户看得见的痕迹 ——
    /// 悄悄跳过的后果是「软件看起来好了，但功能莫名其妙少一块」，
    /// 那种故障用户根本不会联想到安装环节。
    pub fn warn(&self, message: impl Into<String>) {
        let message = message.into();
        dlog!("[bootstrap] 警告：{message}");
        let _ = self.app.emit(EVENT_WARNING, WarningPayload { message });
    }

    pub fn fail(&self, err: &BootstrapError) {
        dlog!("[bootstrap] 失败: {err}");
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

    // 选定 Node 之后立刻登记它的目录：从这里开始，我们启动的每个子进程
    // 都会带上「这个 Node 的目录 + npm 全局 prefix」的 PATH。
    // 没有这一步，无 Node 的机器上后面的 npm 与 pnpm 全都找不到。
    if let Some(dir) = node.path.parent() {
        proc::set_node_dir(dir.to_path_buf());
    }

    let entry = ensure_dsh(app, reporter).await?;

    // dsh 首次运行会自行创建 ~/.dsh 与 web profile，不需要显式 init
    reporter.stage(Stage::InitProfile);

    // 界面插件（鲸鱼娘 / 任务看板 / 皮肤中心都在这个包里）。
    //
    // **装不上不阻断启动。** DSH 本体不依赖它们 —— 两个参考实现
    // （Buktal/deepseek-desktop、steven-kid/deepseek-harness-desktop）
    // 压根不装插件，照样是能用的桌面版。装失败顶多少了鲸鱼娘和任务看板；
    // 而把整条启动流程拦下，等于把「本来能用的 DSH」也一起弄没了。
    // 两边代价差得太远，这里必须放行。
    if let Err(e) = ensure_plugins(&node, &entry, reporter).await {
        reporter.warn(format!(
            "界面插件未安装成功（{e}）。DSH 可正常使用，但鲸鱼娘、任务看板等界面功能不会出现，可稍后在设置页重装。"
        ));
    }

    // ---- 启动 dsh ----
    reporter.stage(Stage::StartingDsh);
    // 预期管理：首启/刚更新后的 boot 可能被杀毒扫描新文件拖到一两分钟，
    // 不说明的话用户只能对着计时器怀疑卡死。瞬态提示，不进「已完成」列表。
    reporter.activity(
        Stage::StartingDsh,
        "正在启动服务；首次启动可能需要一两分钟",
        None,
    );

    // **必须放进阻塞线程池。** `wait_for_url` 内部是 `recv_timeout`，
    // 标准库的同步阻塞调用 —— 直接在 async 里跑会把一个 tokio worker
    // 占死最多两分钟，期间连进度事件都推不出去。
    let node_for_spawn = node.clone();
    let entry_for_spawn = entry.clone();
    let rep_for_spawn = reporter.clone();
    // 120 秒而不是 60。真机复现：首次安装完成后的第一次 boot，dsh 曾整整
    // 60 秒零输出（刚写入的两千多个文件正被杀毒软件实时扫描，Node 加载
    // 被拖爆），到点即被误杀
    let started = tokio::task::spawn_blocking(move || {
        process::spawn_and_wait(&node_for_spawn, &entry_for_spawn, 120, |n, total| {
            rep_for_spawn.activity(
                Stage::StartingDsh,
                format!("dsh 启动未成功，自动重试（{n}/{total}）…"),
                None,
            );
        })
    })
    .await
    .map_err(|e| BootstrapError::Other(format!("启动任务异常退出：{e}")))?;

    // dsh 上游存在偶发启动竞态：同一份安装，这次 boot 加载不了它自己的
    // `@deepseek-ai/*` 组件（ERR_MODULE_NOT_FOUND 一崩一大片），重开又全好。
    // 真机多轮复现：与是否刚安装过**无关**，纯启动也会撞上，包始终在盘上。
    //
    // 进程内已经连掷了三把（见 spawn_and_wait）；仍失败就把整个应用重启一次
    // 再掷 —— 真机上「彻底退出重进」是命中率最高的动作。环境变量做一次性
    // 标记：重启出来的进程带着它，不会再次自动重启，构造上不可能循环。
    let (handle, url) = match started {
        Ok(pair) => pair,
        Err(BootstrapError::DshExitedEarly) if std::env::var_os(RELAUNCH_ENV).is_none() => {
            dlog!("[bootstrap] dsh 多次提前退出，自动重启应用再试一轮");
            reporter.detail(Stage::StartingDsh, "dsh 启动异常，正在重启应用重试…");
            std::env::set_var(RELAUNCH_ENV, "1");
            app.restart();
        }
        Err(e) => return Err(e),
    };

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
/// 装不上的表现是「DSH 能用但侧边栏空空、鲸鱼娘不出现」，用户只会以为软件坏了。
/// 自检结果分级：聚合包整体缺失才算硬失败，其余降级为警告继续放行
/// （分级理由见 plugins.rs 模块头）。
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
        // 早先的版本会在 dsh 之前抢写 pnpm-workspace.yaml，害 pnpm 用错
        // node_modules 布局。中招的机器上那份文件还在，先删掉并强制重装一次。
        let repaired = plugins::repair_stale_workspace();

        if !repaired && plugins::is_installed() {
            rep.detail(Stage::InstallingPlugins, "界面插件已就绪");
        } else {
            // dsh 装插件时在内部调 pnpm。干净机器上没有它，
            // 失败信息只会是 dsh 吐出的一个退出码 1 加一行
            // 「'pnpm' 不是内部或外部命令」，很难定位到根因，
            // 所以在这里先补齐，而不是等它炸。
            if !dsh::pnpm_available() {
                rep.detail(Stage::InstallingPlugins, "正在安装 pnpm");
                dsh::install_pnpm()?;
            }

            // 首次安装要拉几十个子包，可能几分钟
            plugins::install(&node, &entry, &rep)?;
        }

        rep.stage(Stage::VerifyingPlugins);
        // 自检只有「聚合包整个没装上」才返回 Err。子包对不上返回警告，照常放行。
        match plugins::verify(&node, &entry)? {
            Some(warning) => rep.warn(warning),
            None => rep.detail(Stage::VerifyingPlugins, "插件校验通过"),
        }
        Ok(())
    })
    .await
    .map_err(|e| BootstrapError::Other(format!("插件任务异常退出：{e}")))?
}

/// 定位 dsh，没有就装。
///
/// 缓存命中时跳过 `npm root -g`（npm 冷启动 1-2s）—— 这是每次启动最大的
/// 固定开销，而结果几乎从不变化。已装版本一律读 package.json，**从不起
/// 子进程问 `--version`**：那要冷启动一次 Node，杀毒扫描时能拖到好几秒，
/// 而这一步正好在首启的关键路径上。
///
/// npm 安装是阻塞的且可能跑 1-3 分钟，必须扔到阻塞线程池 ——
/// 直接在 async 上下文里跑会占死一个 tokio worker，进度事件都推不出去。
async fn ensure_dsh(app: &AppHandle, reporter: &Reporter) -> Result<std::path::PathBuf> {
    reporter.stage(Stage::CheckingDsh);

    // 快路径：缓存里的路径仍然存在就直接用
    if let Some(entry) = settings::dsh_entry(app) {
        return Ok(entry);
    }

    let rep = reporter.clone();

    let entry = tokio::task::spawn_blocking(move || -> Result<std::path::PathBuf> {
        match dsh::entry_point()? {
            Some(entry) => {
                let label = match upgrade::installed_dsh(&entry) {
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
