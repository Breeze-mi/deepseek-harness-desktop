//! 下载源清单与降级顺序。
//!
//! 国内直连 nodejs.org / registry.npmjs.org 经常超时甚至不通，
//! 所以镜像放在前面，官方源只作兜底。

pub struct Mirror {
    pub name: &'static str,
    pub base: &'static str,
}

/// Node.js 官方源。
///
/// 单列出来是因为**校验和只认它**：`SHASUMS256.txt` 若和 zip 取自同一个镜像，
/// 校验就只能挡住传输损坏，挡不住镜像本身作恶 —— 它可以同时给出改过的包
/// 和一份匹配的哈希。所以哈希始终优先向官方要，包可以走任意镜像。
pub const NODE_OFFICIAL: &str = "https://nodejs.org/dist";

/// Node.js 二进制。三个源的目录结构完全一致：
/// `{base}/index.json` 与 `{base}/v{VER}/node-v{VER}-win-x64.zip`
///
/// 已实测 npmmirror 的 `index.json` 与官方同构（字段名、lts 语义都一样），
/// 所以版本发现逻辑一套代码通吃，不需要按源分支。
pub const NODE_MIRRORS: &[Mirror] = &[
    Mirror {
        name: "阿里云 npmmirror",
        base: "https://cdn.npmmirror.com/binaries/node",
    },
    Mirror {
        name: "清华 TUNA",
        base: "https://mirrors.tuna.tsinghua.edu.cn/nodejs-release",
    },
    Mirror {
        name: "Node.js 官方",
        base: NODE_OFFICIAL,
    },
];

/// npm registry。安装 dsh 与插件时通过 `--registry` 临时指定，
/// **不写用户的全局 npm 配置** —— 不污染用户环境是底线。
///
/// 已实测 npmmirror 同步了 `@deepseek-ai/dsh` 与 `@linxin666/*`。
pub const NPM_REGISTRIES: &[Mirror] = &[
    Mirror {
        name: "阿里云 npmmirror",
        base: "https://registry.npmmirror.com",
    },
    Mirror {
        name: "npm 官方",
        base: "https://registry.npmjs.org",
    },
];
