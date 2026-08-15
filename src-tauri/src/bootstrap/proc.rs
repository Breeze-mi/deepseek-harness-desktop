//! 子命令执行的统一入口。
//!
//! 引导过程要跑几十次 node / npm / dsh。在 Windows 上从 GUI 进程直接
//! spawn 控制台程序会闪出黑框，必须统一加 CREATE_NO_WINDOW，
//! 否则用户会看到一连串窗口闪烁。

use std::ffi::OsStr;
use std::process::{Command, Output, Stdio};

use crate::error::{BootstrapError, Result};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 构造一个不弹控制台窗口的 Command
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd
}

/// 跑一条命令并要求它成功退出，否则带上 stderr 报错。
///
/// stderr 原样带出而不是吞掉 —— npm / dsh 的失败原因几乎全在 stderr 里，
/// 吞了之后排查就只剩「命令失败」四个字。
pub fn run_checked<S: AsRef<OsStr>>(program: S, args: &[&str]) -> Result<Output> {
    let display = format!(
        "{} {}",
        program.as_ref().to_string_lossy(),
        args.join(" ")
    );

    let output = command(&program).args(args).output().map_err(|e| {
        // 区分「命令不存在」与「执行失败」，前者的处理方式完全不同
        if e.kind() == std::io::ErrorKind::NotFound {
            BootstrapError::Other(format!("找不到可执行文件：{}", program.as_ref().to_string_lossy()))
        } else {
            BootstrapError::Io(e)
        }
    })?;

    if !output.status.success() {
        return Err(BootstrapError::Command {
            cmd: display,
            code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }

    Ok(output)
}

/// 跑命令并返回 trim 过的 stdout
pub fn run_stdout<S: AsRef<OsStr>>(program: S, args: &[&str]) -> Result<String> {
    let out = run_checked(program, args)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
