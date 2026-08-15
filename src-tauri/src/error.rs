use serde::Serialize;

/// 引导过程的统一错误。
///
/// 每个变体都要能翻译成「用户看得懂 + 知道下一步干什么」的文案 ——
/// 引导失败时用户面对的是一个白屏应用，错误信息是他唯一的线索。
#[derive(Debug, thiserror::Error)]
pub enum BootstrapError {
    #[error("未找到可用的 Node.js")]
    NodeMissing,

    #[error("Node.js 版本不满足要求：当前 {found}，需要 ^22.19.0 || >=24.0.0")]
    NodeVersionUnsupported { found: String },

    #[error("下载 Node.js 失败：{0}")]
    Download(String),

    #[error("文件校验失败：期望 {expected}，实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("解压失败：{0}")]
    Extract(String),

    #[error("命令执行失败：{cmd}（退出码 {code}）\n{stderr}")]
    Command {
        cmd: String,
        code: i32,
        stderr: String,
    },

    #[error("等待 dsh 就绪超时（{secs}s）")]
    ReadyTimeout { secs: u64 },

    #[error("插件自检未通过：{0}")]
    PluginVerify(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    /// 阶段尚未实现。这不是故障，前端要按「进行中/待开发」渲染，
    /// 不能长得像崩溃 —— 否则每次分阶段交付都在吓用户。
    #[error("{0}")]
    NotImplemented(String),

    #[error("{0}")]
    Other(String),
}

impl BootstrapError {
    /// 给用户的可操作建议。没有有效建议时返回 None，不要写「请重试」这类废话。
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::NodeMissing | Self::NodeVersionUnsupported { .. } => Some(
                "程序可以下载一份便携版 Node.js 放在应用目录下，不会影响你系统里已有的版本。",
            ),
            Self::Download(_) => Some(
                "检查网络连接。程序会依次尝试阿里云、清华与官方源，若三个都失败通常是本机网络或代理问题。",
            ),
            Self::ChecksumMismatch { .. } => Some(
                "下载的文件已损坏或被篡改，出于安全考虑已中止安装。请重试；反复出现请换一个网络环境。",
            ),
            Self::ReadyTimeout { .. } => {
                Some("dsh 启动超时。可在设置里查看日志，或尝试重启 dsh 服务。")
            }
            Self::PluginVerify(_) => {
                Some("插件未正确挂载，鲸鱼娘等界面功能不会出现。点重试通常可以解决。")
            }
            _ => None,
        }
    }

    /// 该错误是否值得让用户点「重试」。
    /// 版本不合格重试多少次都是同一个结果，不给重试按钮。
    pub fn retryable(&self) -> bool {
        !matches!(
            self,
            Self::NodeVersionUnsupported { .. } | Self::NotImplemented(_)
        )
    }

    /// 前端据此决定用告警样式还是中性样式
    pub fn severity(&self) -> &'static str {
        match self {
            Self::NotImplemented(_) => "pending",
            _ => "error",
        }
    }
}

/// 跨 IPC 边界传给前端的错误形态
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPayload {
    pub message: String,
    pub hint: Option<String>,
    pub retryable: bool,
    /// "error" | "pending"
    pub severity: String,
}

impl From<&BootstrapError> for ErrorPayload {
    fn from(e: &BootstrapError) -> Self {
        Self {
            message: e.to_string(),
            hint: e.hint().map(str::to_string),
            retryable: e.retryable(),
            severity: e.severity().to_string(),
        }
    }
}

pub type Result<T> = std::result::Result<T, BootstrapError>;
