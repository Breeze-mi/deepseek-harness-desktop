//! dsh 子进程的生命周期管理。
//!
//! 两个必须做对的点：
//!
//! 1. **Job Object**。Tauri 的 shell 插件只在正常退出时 kill 子进程。主进程一旦
//!    崩溃或被任务管理器强杀，`dsh web` 的 node 进程就变成孤儿常驻后台，占着端口，
//!    用户下次启动撞冲突还找不到元凶。Job Object 让内核保证父进程消失时子进程一起死。
//!
//! 2. **必须持续排空 stdout/stderr**。管道缓冲区填满后子进程会阻塞在写日志上，
//!    表现为「跑着跑着卡死」，极难排查。所以后台线程一直读到 EOF，不能读到 URL 就撒手。

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use crate::bootstrap::node::NodeInfo;
use crate::bootstrap::proc::command;
use crate::error::{BootstrapError, Result};

/// dsh 启动后打印形如 `dsh web: http://127.0.0.1:3080`
fn parse_url(line: &str) -> Option<String> {
    let idx = line.find("http://")?;
    let rest = &line[idx..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ')']);
    url.contains("127.0.0.1").then(|| url.to_string())
}

pub struct DshHandle {
    child: Child,
    #[cfg(windows)]
    _job: Option<win_job::Job>,
}

impl DshHandle {
    /// 正常退出时的优雅关闭。Job Object 兜异常情况，这里兜正常路径 ——
    /// 让 dsh 有机会把状态落盘，而不是被硬杀。
    pub fn shutdown(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for DshHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// 启动 `dsh --profile web --host 127.0.0.1 --port 0`。
///
/// `--port 0` 是 dsh 官方支持的「让 OS 挑空闲端口」，所以我们不用自己
/// bind-then-release 去抢端口 —— 那种做法在释放到 dsh 真正监听之间存在竞态。
/// 真实端口从 dsh 的 stdout 里读回来。
pub fn spawn(node: &NodeInfo, entry: &Path) -> Result<(DshHandle, Receiver<String>)> {
    let mut child = command(&node.path)
        .arg(entry)
        .args(["--profile", "web", "--host", "127.0.0.1", "--port", "0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BootstrapError::Other(format!("启动 dsh 失败：{e}")))?;

    #[cfg(windows)]
    let job = win_job::Job::new().inspect(|j| {
        if !j.assign(child.id()) {
            // 绑定失败不阻断启动，但要让日志留痕：这台机器上主进程被强杀会留孤儿
            eprintln!("[dsh] 警告：子进程未能绑定到 Job Object，异常退出时可能残留");
        }
    });

    let (tx, rx) = channel::<String>();

    // stdout：找 URL + 持续排空
    if let Some(out) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut sent = false;
            for line in BufReader::new(out).lines().map_while(std::result::Result::ok) {
                eprintln!("[dsh] {line}");
                if !sent {
                    if let Some(url) = parse_url(&line) {
                        sent = tx.send(url).is_ok();
                    }
                }
            }
        });
    }

    // stderr：只排空 + 转日志。不排空会把子进程卡死。
    if let Some(err) = child.stderr.take() {
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(std::result::Result::ok) {
                eprintln!("[dsh:err] {line}");
            }
        });
    }

    Ok((
        DshHandle {
            child,
            #[cfg(windows)]
            _job: job,
        },
        rx,
    ))
}

/// 等 dsh 把监听地址打印出来
pub fn wait_for_url(rx: &Receiver<String>, timeout_secs: u64) -> Result<String> {
    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(url) => Ok(url),
        Err(RecvTimeoutError::Timeout) => Err(BootstrapError::ReadyTimeout { secs: timeout_secs }),
        Err(RecvTimeoutError::Disconnected) => Err(BootstrapError::Other(
            "dsh 进程在打印监听地址前就退出了，请在设置里查看日志".into(),
        )),
    }
}

#[cfg(windows)]
mod win_job {
    use std::ffi::c_void;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject,
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    pub struct Job(HANDLE);

    // HANDLE 内部是 `*mut c_void`，因此默认不是 Send，会一路传染到 DshState
    // 让 Tauri 的 .manage() 拒收。
    //
    // 安全性：Win32 内核句柄是**进程级**资源，不绑定创建它的线程 ——
    // 在任意线程上使用或 CloseHandle 都是良定义的。Job 本身也没有内部可变状态。
    unsafe impl Send for Job {}

    impl Job {
        pub fn new() -> Option<Self> {
            unsafe {
                let handle = CreateJobObjectW(None, PCWSTR::null()).ok()?;

                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                // 这一位是全部意义所在：Job 句柄关闭（进程无论怎么死都会关）时，
                // 内核终止 Job 内所有进程。
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;

                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const c_void,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
                .ok()?;

                Some(Job(handle))
            }
        }

        pub fn assign(&self, pid: u32) -> bool {
            unsafe {
                let Ok(proc) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)
                else {
                    return false;
                };
                let ok = AssignProcessToJobObject(self.0, proc).is_ok();
                let _ = CloseHandle(proc);
                ok
            }
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_url;

    #[test]
    fn parses_dsh_startup_line() {
        assert_eq!(
            parse_url("dsh web: http://127.0.0.1:3080").as_deref(),
            Some("http://127.0.0.1:3080")
        );
        assert_eq!(
            parse_url("listening on http://127.0.0.1:51234 now").as_deref(),
            Some("http://127.0.0.1:51234")
        );
    }

    #[test]
    fn ignores_non_loopback_and_noise() {
        assert!(parse_url("see http://example.com/docs").is_none());
        assert!(parse_url("starting up...").is_none());
    }

    #[test]
    fn strips_trailing_punctuation() {
        assert_eq!(
            parse_url("ready at http://127.0.0.1:3080.").as_deref(),
            Some("http://127.0.0.1:3080")
        );
    }
}
