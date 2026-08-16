//! dsh 的定位、安装与版本管理。
//!
//! 直接用 node 执行它的 JS 入口：
//! Windows 的 CreateProcess 起不了 `.cmd`（得套 cmd /C），而且 shim 里写死的

use std::path::PathBuf;

use super::mirror::NPM_REGISTRIES;
use super::proc::{command, run_checked};
use super::{Reporter, Stage};
use crate::error::{BootstrapError, Result};

pub const PACKAGE: &str = "@deepseek-ai/dsh";

/// npm 自身也是 `.cmd`，只能通过 cmd /C 调用。
/// `command()` 已带 CREATE_NO_WINDOW，不会闪黑框。
///
/// 走 `run_checked` 而不是自己拼 Command：它会把「执行了什么」与失败时stdout+stderr 合并的完整输出落进日志。
fn npm(args: &[&str]) -> Result<String> {
    let line = format!("npm {}", args.join(" "));
    let out = run_checked("cmd", &["/C", &line])?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 全局 node_modules 根目录，形如 `C:\Users\x\AppData\Roaming\npm\node_modules`
fn npm_root_global() -> Result<PathBuf> {
    let raw = npm(&["root", "-g"])?;
    let path = PathBuf::from(raw.trim());
    if path.is_dir() {
        Ok(path)
    } else {
        Err(BootstrapError::Other(format!(
            "npm root -g 返回了不存在的路径：{}",
            path.display()
        )))
    }
}

/// dsh 装插件时会调用 `pnpm`，而它**不在我们的安装范围内** ——
/// 干净机器上没有 pnpm，`dsh plugin add` 就会失败在
/// 「'pnpm' 不是内部或外部命令」上，而且错误发生在 dsh 内部，
/// 我们只能看到一个退出码 1。
pub fn pnpm_available() -> bool {
    command("cmd")
        .args(["/C", "pnpm --version"])
        .output()
        .is_ok_and(|out| out.status.success())
}

/// 全局安装 pnpm。镜像源优先，失败回落官方。
pub fn install_pnpm() -> Result<()> {
    let mut last: Option<BootstrapError> = None;

    for registry in NPM_REGISTRIES {
        dlog!("[pnpm] 通过 {} 安装", registry.name);
        match npm(&[
            "install",
            "-g",
            "pnpm",
            "--registry",
            registry.base,
            "--no-fund",
            "--no-audit",
        ]) {
            Ok(_) => return Ok(()),
            Err(e) => {
                dlog!("[pnpm] registry {} 安装失败: {e}", registry.name);
                last = Some(e);
            }
        }
    }

    Err(last.unwrap_or_else(|| BootstrapError::Other("没有可用的 npm registry".into())))
}

/// dsh 的 JS 入口。未安装时返回 None。
pub fn entry_point() -> Result<Option<PathBuf>> {
    let root = npm_root_global()?;
    let entry = root
        .join("@deepseek-ai")
        .join("dsh")
        .join("lib")
        .join("bin.js");
    Ok(entry.is_file().then_some(entry))
}

/// 全局安装 dsh。镜像源优先，失败回落官方。
pub fn install(reporter: &Reporter) -> Result<PathBuf> {
    install_with(|msg| reporter.detail(Stage::InstallingDsh, msg))
}

/// 升级到 registry 最新版。
///
/// 与首次安装是同一条 npm 命令、同一套镜像降级 —— `npm i -g` 对已装的包
/// 本来就是升级语义。差别只在这里没有引导页可以上报进度。
pub fn upgrade() -> Result<PathBuf> {
    install_with(|msg| dlog!("[dsh] {msg}"))
}

fn install_with(mut on_step: impl FnMut(String)) -> Result<PathBuf> {
    let mut last: Option<BootstrapError> = None;

    for registry in NPM_REGISTRIES {
        on_step(format!("通过 {} 安装（首次约需 1-3 分钟）", registry.name));

        // 只在本次命令上临时指定 registry，不动用户的全局 npm 配置
        let result = npm(&[
            "install",
            "-g",
            PACKAGE,
            "--registry",
            registry.base,
            "--no-fund",
            "--no-audit",
        ]);

        match result {
            Ok(_) => {
                return entry_point()?.ok_or_else(|| {
                    BootstrapError::Other("npm 报告安装成功，但找不到 dsh 入口文件".into())
                })
            }
            Err(e) => {
                dlog!("[dsh] registry {} 安装失败: {e}", registry.name);
                last = Some(e);
            }
        }
    }

    Err(last.unwrap_or_else(|| BootstrapError::Other("没有可用的 npm registry".into())))
}
