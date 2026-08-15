//! dsh 的定位、安装与版本管理。
//!
//! **不走 `dsh.cmd` 那个 npm shim**，而是直接用 node 执行它的 JS 入口：
//! Windows 的 CreateProcess 起不了 `.cmd`（得套 cmd /C），而且 shim 里写死的
//! node 路径在我们用便携版 Node 时是错的。直接跑 `lib/bin.js` 把两个问题一起绕开。

use std::path::{Path, PathBuf};

use super::mirror::NPM_REGISTRIES;
use super::node::NodeInfo;
use super::proc::{command, run_stdout};
use super::{Reporter, Stage};
use crate::error::{BootstrapError, Result};

pub const PACKAGE: &str = "@deepseek-ai/dsh";

/// npm 自身也是 `.cmd`，只能通过 cmd /C 调用。
/// `command()` 已带 CREATE_NO_WINDOW，不会闪黑框。
fn npm(args: &[&str]) -> Result<String> {
    let line = format!("npm {}", args.join(" "));
    let out = command("cmd")
        .args(["/C", &line])
        .output()
        .map_err(BootstrapError::Io)?;

    if !out.status.success() {
        return Err(BootstrapError::Command {
            cmd: line,
            code: out.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
        });
    }

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

/// dsh 的 JS 入口。未安装时返回 None。
pub fn entry_point() -> Result<Option<PathBuf>> {
    let root = npm_root_global()?;
    let entry = root.join("@deepseek-ai").join("dsh").join("lib").join("bin.js");
    Ok(entry.is_file().then_some(entry))
}

/// 已安装的 dsh 版本
pub fn installed_version(node: &NodeInfo, entry: &Path) -> Option<String> {
    let out = run_stdout(
        &node.path,
        &[&entry.to_string_lossy(), "--version"],
    )
    .ok()?;
    Some(out.lines().last()?.trim().to_string())
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
    install_with(|msg| eprintln!("[dsh] {msg}"))
}

fn install_with(mut on_step: impl FnMut(String)) -> Result<PathBuf> {
    let mut last: Option<BootstrapError> = None;

    for registry in NPM_REGISTRIES {
        on_step(format!(
            "通过 {} 安装（首次约需 1-3 分钟）",
            registry.name
        ));

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
                eprintln!("[dsh] registry {} 安装失败: {e}", registry.name);
                last = Some(e);
            }
        }
    }

    Err(last.unwrap_or_else(|| BootstrapError::Other("没有可用的 npm registry".into())))
}
