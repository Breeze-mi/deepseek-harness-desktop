//! 子命令执行的统一入口。
//!
//! 引导过程要跑几十次 node / npm / dsh。在 Windows 上从 GUI 进程直接
//! spawn 控制台程序会闪出黑框，必须统一加 CREATE_NO_WINDOW，
//! 否则用户会看到一连串窗口闪烁。

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use crate::error::{BootstrapError, Result};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 本次选中的 Node 所在目录（`node.exe` 的父目录）。
static NODE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// 记录选中的 Node 目录，之后所有子进程的 PATH 都会带上它。

///把这两个目录前置到子进程的 PATH 里。
///
/// 只有便携版才需要登记 —— 系统 Node 按定义就在 PATH 上。
pub fn set_node_dir(dir: PathBuf) {
    // **系统 Node 的 `NodeInfo.path` 是裸的 `"node"`**（靠 PATH 解析），
    // 而 `PathBuf::from("node").parent()` 返回的是 `Some("")` 而不是 None。
    // 空条目在 Windows 的 PATH 里等价于「当前工作目录」—— 既不正确，
    // 还是个 exe 劫持的口子。守卫放在这里，调用方就不可能踩到。
    if dir.as_os_str().is_empty() {
        return;
    }
    let _ = NODE_DIR.set(dir);
}

/// 本次会话是否选定了便携版 Node（引导里 `set_node_dir` 过才有值）。
///
/// 给 `runtime::restart` 用：登记过便携版之后，`node --version` 这类裸探测
/// 会被我们注入的 PATH 命中**便携版自己**。重启时据此直接沿用便携版，不再探测。
pub fn node_dir() -> Option<PathBuf> {
    NODE_DIR.get().cloned()
}

/// 构造给子进程用的 PATH：`<node 目录>;<npm 全局 prefix>;<原 PATH>`。
///
/// 前置而不是追加：用户机器上可能有别的、版本不合格的 node/npm，
/// 我们选中的那个必须优先。
fn child_path() -> Option<std::ffi::OsString> {
    let mut parts: Vec<PathBuf> = Vec::new();

    // 便携版才有；系统 Node 时这里是空的，靠原 PATH 就能找到
    if let Some(dir) = NODE_DIR.get() {
        parts.push(dir.clone());
    }
    // npm 在 Windows 上的默认全局 prefix（官方文档：Windows 上是
    // `%AppData%\npm`，且全局包直接放在 prefix 下，没有 bin 子目录）。
    // dsh 内部 `spawnSync("pnpm")` 就是靠这个目录找到 pnpm 的。
    // 用户改过 prefix 的话，那个目录基本必然已经在他自己的 PATH 里。
    if let Some(appdata) = dirs::data_dir() {
        parts.push(appdata.join("npm"));
    }

    if parts.is_empty() {
        return None;
    }

    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut joined = std::env::join_paths(parts).ok()?;
    if !original.is_empty() {
        joined.push(";");
        joined.push(&original);
    }
    Some(joined)
}

/// 构造一个不弹控制台窗口的 Command
pub fn command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null());

    if let Some(path) = child_path() {
        cmd.env("PATH", path);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        // 钉住 ComSpec。**dsh 内部 `spawnSync("pnpm", …, { shell: true })`
        // 用的就是这个变量**，Node 还会固定附上 cmd.exe 专用的 `/d /s /c`。
        // 受管控的机器上 ComSpec 可能被改成别的解释器，那几个参数就会落到
        // 一个不认识它们的壳上，失败信息还会被 dsh 吞成一句「pnpm failed」。
        // 顺带也避开 PowerShell 那条路 —— npm 装出来的 `pnpm.ps1` 会受
        // 执行策略管辖，而同目录的 `pnpm.cmd` 不会。
        if let Some(root) = std::env::var_os("SystemRoot") {
            let shell = PathBuf::from(root).join("System32").join("cmd.exe");
            if shell.is_file() {
                cmd.env("ComSpec", shell);
            }
        }

        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    cmd
}

/// 跑一条命令并要求它成功退出，否则带上 stderr 报错。
///
/// stderr 原样带出而不是吞掉 —— npm / dsh 的失败原因几乎全在 stderr 里，
/// 吞了之后排查就只剩「命令失败」四个字。
pub fn run_checked<S: AsRef<OsStr>>(program: S, args: &[&str]) -> Result<Output> {
    run_checked_env(program, args, &[])
}

/// 同 `run_checked`，另外给这一次调用注入环境变量。
///
/// 存在的理由是 **dsh 装插件时会自己 spawn pnpm，而我们插不进 `--registry`**。
/// 命令行参数是 dsh 拼的，我们改不了；但环境变量会被子进程继承，
/// npm 系工具都认 `npm_config_*` 这套前缀，于是镜像加速能穿透一层 spawn 传下去。
///
/// 只作用于这一条命令，不动用户的 npm 配置 —— 和别处 `--registry` 是同一个原则。
pub fn run_checked_env<S: AsRef<OsStr>>(
    program: S,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<Output> {
    let display = format!("{} {}", program.as_ref().to_string_lossy(), args.join(" "));

    let mut cmd = command(&program);
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }

    dlog!("[proc] 执行：{display}");

    let output = cmd.output().map_err(|e| {
        // 区分「命令不存在」与「执行失败」，前者的处理方式完全不同
        if e.kind() == std::io::ErrorKind::NotFound {
            BootstrapError::Other(format!(
                "找不到可执行文件：{}",
                program.as_ref().to_string_lossy()
            ))
        } else {
            BootstrapError::Io(e)
        }
    })?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let detail = failure_detail(&output);
        // 完整原文进日志。传给前端的那份会被弹窗截断
        dlog!("[proc] 失败（退出码 {code}）：{display}\n{detail}");

        return Err(BootstrapError::Command {
            cmd: display,
            code,
            stderr: detail,
        });
    }

    Ok(output)
}

/// 从失败的输出里拼出「够用来定位问题」的那段文字。
///
/// **stdout 必须一起带上。** npm 系工具（尤其是 pnpm）的报错大量走 stdout，
/// 只看 stderr 常常只剩一句「命令失败」。dsh 内部 spawn pnpm 时用的是
/// `stdio: "inherit"`，pnpm 的输出会并进 dsh 的管道 —— 丢掉 stdout
/// 就等于把真正的失败原因扔了，只留一个退出码 1。
///
/// 只留末尾：pnpm 的结论在最后，前面几百行进度条对排查没有价值，
/// 全带上还会把错误弹窗撑爆。
fn failure_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    let mut detail = stderr.trim().to_string();
    let out = stdout.trim();
    if !out.is_empty() {
        if !detail.is_empty() {
            detail.push('\n');
        }
        detail.push_str(out);
    }
    tail(&detail, 2000)
}

/// 按**字符**截尾，不按字节 —— 中文输出上按字节切会切出半个字。
fn tail(s: &str, max: usize) -> String {
    let count = s.chars().count();
    if count <= max {
        return s.to_string();
    }
    s.chars().skip(count - max).collect()
}

/// 跑命令并返回 trim 过的 stdout
pub fn run_stdout<S: AsRef<OsStr>>(program: S, args: &[&str]) -> Result<String> {
    let out = run_checked(program, args)?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 边跑边把输出逐行交出来。
///
/// 装插件要拉几百个包、跑好几分钟，而 `output()` 是**一次性阻塞收集** ——
/// 期间界面上只有一句静止的「正在下载界面插件」，用户没法区分
/// 「在跑」和「卡死」，只会觉得特别慢。pnpm 其实一直在打进度，
/// 以前被我们整段吞掉了，直到结束才看得到。
///
/// **两路管道都必须持续排空。** 只读 stdout 的话，stderr 的管道缓冲区填满后
/// 子进程会阻塞在写日志上，表现为「跑着跑着卡死」，极难排查。
pub fn run_streaming<S: AsRef<OsStr>>(
    program: S,
    args: &[&str],
    envs: &[(&str, &str)],
    on_line: &mut dyn FnMut(&str),
) -> Result<()> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc::channel;

    let display = format!("{} {}", program.as_ref().to_string_lossy(), args.join(" "));

    let mut cmd = command(&program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }

    dlog!("[proc] 执行（流式）：{display}");
    let mut child = cmd.spawn().map_err(BootstrapError::Io)?;

    let (tx, rx) = channel::<String>();
    let mut readers = Vec::new();

    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(out)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let tx = tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(err)
                .lines()
                .map_while(std::result::Result::ok)
            {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    // **必须丢掉本地这一份发送端**，否则读取线程结束后 rx 仍以为还有人会发，
    // 下面的 for 循环永远不会结束。
    drop(tx);

    // 只留末尾若干行给错误信息
    let mut tail: Vec<String> = Vec::new();
    for line in rx {
        dlog!("[out] {line}");
        on_line(&line);
        tail.push(line);
        if tail.len() > 80 {
            tail.remove(0);
        }
    }
    for r in readers {
        let _ = r.join();
    }

    let status = child.wait().map_err(BootstrapError::Io)?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        dlog!("[proc] 失败（退出码 {code}）：{display}");
        return Err(BootstrapError::Command {
            cmd: display,
            code,
            stderr: tail.join("\n"),
        });
    }

    Ok(())
}
