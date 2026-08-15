//! 便携版 Node.js 的获取：版本发现 → 下载 → 校验 → 解压。
//!
//! 三个镜像源依次尝试，任一成功即止。**下载完必须校验 SHA256**，
//! 而且**校验和要从官方源单独取**：如果哈希和 zip 来自同一个镜像，
//! 校验就只能挡住传输损坏 —— 镜像运营方可以同时给出改过的包和匹配的哈希。
//! 分开取之后，一个被攻陷的镜像才无法悄悄换掉用户机器上要执行的二进制。

use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::mirror::{NODE_MIRRORS, NODE_OFFICIAL};
use super::node::{is_supported, NodeInfo};
use super::{Reporter, Stage};
use crate::error::{BootstrapError, Result};

/// 进度事件的最小间隔（字节）。每个 chunk 都推会把前端淹了。
const PROGRESS_STEP: u64 = 512 * 1024;

/// nodejs.org / 镜像的 index.json 条目。
/// `lts` 字段是 `false`（非 LTS）或版本代号字符串（如 "Iron"），
/// 所以用 Value 接再判断类型，不能直接当 bool。
#[derive(Deserialize)]
struct Release {
    version: String,
    #[serde(default)]
    lts: serde_json::Value,
    #[serde(default)]
    files: Vec<String>,
}

impl Release {
    fn is_lts(&self) -> bool {
        self.lts.is_string()
    }

    fn has_win_x64_zip(&self) -> bool {
        self.files.iter().any(|f| f == "win-x64-zip")
    }

    fn semver(&self) -> Option<Version> {
        Version::parse(self.version.trim_start_matches('v')).ok()
    }
}

/// 便携 Node 的安装根目录：`%APPDATA%\deepseek-harness-desktop\runtime\node`
pub fn runtime_node_dir() -> Result<PathBuf> {
    let base = dirs::data_dir()
        .ok_or_else(|| BootstrapError::Other("无法定位系统数据目录".into()))?;
    Ok(base.join("deepseek-harness-desktop").join("runtime").join("node"))
}

/// 找已经装好的便携 Node（解压出来是 `node-vX.Y.Z-win-x64/node.exe`）
pub fn installed_portable_node() -> Option<NodeInfo> {
    let root = runtime_node_dir().ok()?;
    let entries = std::fs::read_dir(&root).ok()?;

    for entry in entries.flatten() {
        let exe = entry.path().join("node.exe");
        if !exe.is_file() {
            continue;
        }

        // 目录名形如 node-v22.19.0-win-x64，从中取版本号；
        // 不去执行它来问版本 —— 启动进程比读目录名慢得多，而且这一步在启动关键路径上
        //
        // 解析失败必须 `continue` 而不是 `?`：`?` 会从**整个函数**返回 None，
        // 于是一个名字不规范的残留目录（比如中断留下的 .part）就能让扫描提前中止，
        // 哪怕后面还躺着一份完好的便携 Node 也找不到。
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(ver) = name
            .strip_prefix("node-v")
            .and_then(|s| s.split('-').next())
            .and_then(|s| Version::parse(s).ok())
        else {
            continue;
        };

        if is_supported(&ver) {
            return Some(NodeInfo {
                path: exe,
                version: ver.to_string(),
                portable: true,
            });
        }
    }

    None
}

/// 下载并安装便携 Node。三个源依次尝试，全失败才报错。
pub async fn install_portable_node(reporter: &Reporter) -> Result<NodeInfo> {
    let client = reqwest::Client::builder()
        .user_agent("deepseek-harness-desktop")
        .build()
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    let mut last_err = String::new();

    for mirror in NODE_MIRRORS {
        reporter.detail(Stage::DownloadingNode, format!("尝试 {}", mirror.name));

        match try_mirror(&client, mirror.base, reporter).await {
            Ok(info) => return Ok(info),
            Err(e) => {
                // 单个源失败是预期内的，记下来继续换下一个
                eprintln!("[node] 源 {} 失败: {e}", mirror.name);
                last_err = format!("{}：{e}", mirror.name);
            }
        }
    }

    Err(BootstrapError::Download(format!(
        "全部 {} 个下载源均失败。最后一个错误 —— {last_err}",
        NODE_MIRRORS.len()
    )))
}

async fn try_mirror(
    client: &reqwest::Client,
    base: &str,
    reporter: &Reporter,
) -> Result<NodeInfo> {
    let version = pick_version(client, base).await?;
    let dir_name = format!("node-v{version}-win-x64");
    let file_name = format!("{dir_name}.zip");

    reporter.detail(
        Stage::DownloadingNode,
        format!("准备下载 Node.js v{version}"),
    );

    // 先取校验和清单，拿不到就别下了 —— 下完无法校验等于白下
    let expected = fetch_expected_sha(client, base, &version, &file_name).await?;

    let root = runtime_node_dir()?;
    std::fs::create_dir_all(&root)?;
    let tmp = root.join(format!("{file_name}.part"));

    let actual = download_to(client, &format!("{base}/v{version}/{file_name}"), &tmp, reporter)
        .await
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;

    if actual != expected {
        let _ = std::fs::remove_file(&tmp);
        return Err(BootstrapError::ChecksumMismatch { expected, actual });
    }

    reporter.detail(Stage::DownloadingNode, "校验通过，正在解压");
    extract_zip(&tmp, &root).await?;
    let _ = std::fs::remove_file(&tmp);

    let exe = root.join(&dir_name).join("node.exe");
    if !exe.is_file() {
        return Err(BootstrapError::Extract(format!(
            "解压完成但未找到 {}",
            exe.display()
        )));
    }

    Ok(NodeInfo {
        path: exe,
        version,
        portable: true,
    })
}

/// 从 index.json 挑一个满足 engines 的版本。优先 LTS，没有合适的 LTS 才退到最新版。
async fn pick_version(client: &reqwest::Client, base: &str) -> Result<String> {
    let releases: Vec<Release> = client
        .get(format!("{base}/index.json"))
        .send()
        .await
        .map_err(|e| BootstrapError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| BootstrapError::Download(e.to_string()))?
        .json()
        .await
        .map_err(|e| BootstrapError::Download(format!("index.json 解析失败：{e}")))?;

    // index.json 是新版在前，遍历时第一个命中的就是最新的
    let usable = |r: &&Release| {
        r.has_win_x64_zip() && r.semver().map(|v| is_supported(&v)).unwrap_or(false)
    };

    let chosen = releases
        .iter()
        .find(|r| usable(r) && r.is_lts())
        .or_else(|| releases.iter().find(usable));

    match chosen {
        Some(r) => Ok(r.version.trim_start_matches('v').to_string()),
        None => Err(BootstrapError::Download(
            "该源没有满足 ^22.19.0 || >=24.0.0 的 Windows x64 构建".into(),
        )),
    }
}

/// 取校验和。
///
/// **优先向官方源要，而不是向提供 zip 的那个镜像要。**
/// 两者同源的话，校验就只能挡住传输损坏 —— 镜像运营方完全可以同时给出
/// 改过的包和一份匹配的哈希，校验照样通过。分开取才有意义。
///
/// 官方不可达时退回镜像（国内直连 nodejs.org 经常超时，这是常态而非异常），
/// 但要在日志里明说这次降级了：此时的校验只剩「防损坏」，不再「防投毒」。
async fn fetch_expected_sha(
    client: &reqwest::Client,
    mirror_base: &str,
    version: &str,
    file_name: &str,
) -> Result<String> {
    if mirror_base != NODE_OFFICIAL {
        match fetch_sha_from(client, NODE_OFFICIAL, version, file_name).await {
            Ok(sha) => return Ok(sha),
            Err(e) => eprintln!(
                "[node] 官方校验和不可达（{e}），退回镜像自带的哈希 —— \
                 本次只能防传输损坏，不能防镜像投毒"
            ),
        }
    }
    fetch_sha_from(client, mirror_base, version, file_name).await
}

/// SHASUMS256.txt 每行形如 `<64位十六进制>  node-vX-win-x64.zip`
async fn fetch_sha_from(
    client: &reqwest::Client,
    base: &str,
    version: &str,
    file_name: &str,
) -> Result<String> {
    let text = client
        .get(format!("{base}/v{version}/SHASUMS256.txt"))
        .send()
        .await
        .map_err(|e| BootstrapError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| BootstrapError::Download(e.to_string()))?
        .text()
        .await
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    text.lines()
        .find_map(|line| {
            let (sha, name) = line.split_once("  ")?;
            (name.trim() == file_name).then(|| sha.trim().to_lowercase())
        })
        .ok_or_else(|| {
            BootstrapError::Download(format!("SHASUMS256.txt 里没有 {file_name} 的校验和"))
        })
}

/// 边下边算哈希，省掉一次完整读盘
async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    reporter: &Reporter,
) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| BootstrapError::Download(e.to_string()))?
        .error_for_status()
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut file = std::fs::File::create(dest)?;
    let mut hasher = Sha256::new();

    let mut done: u64 = 0;
    let mut next_report: u64 = 0;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| BootstrapError::Download(e.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk)?;

        done += chunk.len() as u64;
        if done >= next_report {
            reporter.download(Stage::DownloadingNode, done, total);
            next_report = done + PROGRESS_STEP;
        }
    }

    file.flush()?;
    reporter.download(Stage::DownloadingNode, done, total);

    Ok(hex::encode(hasher.finalize()))
}

/// zip 解压是纯 CPU + 阻塞 IO，扔到阻塞线程池，别占着 async 执行器
async fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let zip_path = zip_path.to_path_buf();
    let dest = dest.to_path_buf();

    tokio::task::spawn_blocking(move || -> Result<()> {
        let file = std::fs::File::open(&zip_path)?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| BootstrapError::Extract(e.to_string()))?;
        archive
            .extract(&dest)
            .map_err(|e| BootstrapError::Extract(e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|e| BootstrapError::Extract(format!("解压任务异常退出：{e}")))?
}
