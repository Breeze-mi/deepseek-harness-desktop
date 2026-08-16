//! 便携版 Node.js 的获取：版本发现 → 下载 → 校验 → 解压。
//!
//! 三个镜像源依次尝试，任一成功即止。**下载完必须校验 SHA256**，
//! 而且**校验和要从官方源单独取**：如果哈希和 zip 来自同一个镜像，
//! 校验就只能挡住传输损坏 —— 镜像运营方可以同时给出改过的包和匹配的哈希。
//! 分开取之后，一个被攻陷的镜像才无法悄悄换掉用户机器上要执行的二进制。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::mirror::{Mirror, NODE_MIRRORS, NODE_OFFICIAL};
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
    let base =
        dirs::data_dir().ok_or_else(|| BootstrapError::Other("无法定位系统数据目录".into()))?;
    Ok(base
        .join("deepseek-harness-desktop")
        .join("runtime")
        .join("node"))
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

/// 测一个源的实际吞吐：拉 index.json 的前 64KB，计完成耗时。
///
/// 只测延迟（HEAD）不够 —— 国内到各镜像的延迟都低，差的是**带宽**；
/// 64KB 够把「被限速的源」和「健康的源」拉开量级，又不至于浪费流量。
/// 探测失败（超时 / 4xx / 空响应）返回 None，排序时垫底但仍保留兜底资格。
async fn probe_speed(client: &reqwest::Client, m: &'static Mirror) -> Option<std::time::Duration> {
    let start = tokio::time::Instant::now();
    let resp = client
        .get(format!("{}/index.json", m.base))
        .header(reqwest::header::RANGE, "bytes=0-65535")
        .timeout(std::time::Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }

    let mut stream = resp.bytes_stream();
    let mut got: usize = 0;
    while let Some(chunk) = stream.next().await {
        got += chunk.ok()?.len();
        if got >= 64 * 1024 {
            break;
        }
    }
    (got > 0).then(|| start.elapsed())
}

/// 按实测速度给镜像排序（chsrc 同款思路）。
///
/// 写死的顺序只能表达「预期谁快」，表达不了当下 —— 用户实测过的教训：
/// 同一份镜像清单里可以同时存在 6 MB/s 的源和两个 404 的源。
/// 并发探测总共只花最慢一路的时间（上限 4 秒），而这条路径本身
/// 意味着接下来要下 30MB，这几秒买一个数量级的下载速度差，稳赚。
///
/// 全部探测失败（比如离线）时排序退化为清单原序 —— 行为等同于今天，不会更糟。
async fn rank_mirrors(client: &reqwest::Client, reporter: &Reporter) -> Vec<&'static Mirror> {
    reporter.activity(Stage::DownloadingNode, "正在测速下载源…", None);

    let probes = NODE_MIRRORS.iter().map(|m| async move {
        let elapsed = probe_speed(client, m).await;
        match &elapsed {
            Some(d) => dlog!(
                "[node] 测速 {}：64KB 用时 {:.0}ms",
                m.name,
                d.as_secs_f64() * 1000.0
            ),
            None => dlog!("[node] 测速 {}：不可用", m.name),
        }
        (m, elapsed)
    });

    let mut results: Vec<_> = futures_util::future::join_all(probes).await;
    // 稳定排序：探测失败的源之间保持清单原序，兜底顺序不被打乱
    results.sort_by_key(|(_, t)| t.unwrap_or(std::time::Duration::MAX));

    if let Some((best, Some(_))) = results.first() {
        reporter.detail(Stage::DownloadingNode, format!("最快源：{}", best.name));
    }
    results.into_iter().map(|(m, _)| m).collect()
}

/// 下载并安装便携 Node。先测速排序，再依次尝试，全失败才报错。
pub async fn install_portable_node(reporter: &Reporter) -> Result<NodeInfo> {
    let client = reqwest::Client::builder()
        .user_agent("deepseek-harness-desktop")
        // 只设「建连」与「两次读之间」的超时，**不设整单超时**：慢而在动的
        // 下载是合法的，整单超时会把它误杀 —— 和 dsh 首次 boot 被 60 秒
        // 超时误杀是同一类错。反过来，一个超时都不设的话，被黑洞的镜像
        // （连上了但不回数据）会让下载永远挂着，镜像降级链路根本轮不到
        // 下一个源 —— 恰好废掉「hosts 黑洞验证降级」想验证的那条路。
        .connect_timeout(std::time::Duration::from_secs(10))
        .read_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    let mut last_err = String::new();

    for mirror in rank_mirrors(&client, reporter).await {
        reporter.detail(Stage::DownloadingNode, format!("尝试 {}", mirror.name));

        match try_mirror(&client, mirror.base, reporter).await {
            Ok(info) => return Ok(info),
            Err(e) => {
                // 单个源失败是预期内的，记下来继续换下一个
                dlog!("[node] 源 {} 失败: {e}", mirror.name);
                last_err = format!("{}：{e}", mirror.name);
            }
        }
    }

    Err(BootstrapError::Download(format!(
        "全部 {} 个下载源均失败。最后一个错误 —— {last_err}",
        NODE_MIRRORS.len()
    )))
}

async fn try_mirror(client: &reqwest::Client, base: &str, reporter: &Reporter) -> Result<NodeInfo> {
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

    // 失败时**保留** .part —— 这是断点续传的本钱，下次启动接着下。
    // 不用担心坏文件反复坑人：校验不过的在下面的分支里删，
    // 续传范围无效的在 download_to 里删。
    let actual = download_to(
        client,
        &format!("{base}/v{version}/{file_name}"),
        &tmp,
        reporter,
    )
    .await?;

    if actual != expected {
        let _ = std::fs::remove_file(&tmp);
        return Err(BootstrapError::ChecksumMismatch { expected, actual });
    }

    reporter.detail(Stage::DownloadingNode, "校验通过，正在解压");

    // **解压必须原子化**。直接往最终位置解压的话，
    // 中途被强杀会留下一棵**带 node.exe 的半截树** —— 下次启动
    // `installed_portable_node` 一看 node.exe 在就选中它，npm 却已经残缺，
    // 后面每一步都在用一个坏运行时。先解到 staging，成功后一次 rename 落位。
    let staging = root.join(format!("{dir_name}.extracting"));
    let _ = std::fs::remove_dir_all(&staging);
    extract_zip(&tmp, &staging).await?;

    // 官方 zip 内层固定是 node-vX.Y.Z-win-x64/ 目录
    let inner = staging.join(&dir_name);
    if !inner.join("node.exe").is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(BootstrapError::Extract(format!(
            "压缩包结构异常：{} 下没有 node.exe",
            inner.display()
        )));
    }

    let final_dir = root.join(&dir_name);
    // 清掉旧的半截目录（老版本应用非原子解压可能留下的），再落位
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&inner, &final_dir)?;
    let _ = std::fs::remove_dir_all(&staging);

    // 事成之后小扫除：当前 zip 和所有 .part 一起清。换过版本后残留的
    // 旧 .part 永远不会再被续上，纯占磁盘（一个就是 30MB）。
    let _ = std::fs::remove_file(&tmp);
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".part") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let exe = final_dir.join("node.exe");

    Ok(NodeInfo {
        path: exe,
        version,
        portable: true,
    })
}

/// 硬编码下载版本（形如 `"24.19.0"`，不带 v）。

/// const HARDCODED_NODE_VERSION: Option<&str> = Some("22.19.0");
const HARDCODED_NODE_VERSION: Option<&str> = None;
/// 选版本：有硬编码就用硬编码的（顺便省一次 index.json 请求），
/// 否则从 index.json 挑最新合格 LTS，没有合适的 LTS 才退到最新合格版。
async fn pick_version(client: &reqwest::Client, base: &str) -> Result<String> {
    if let Some(hardcoded) = HARDCODED_NODE_VERSION {
        let v = Version::parse(hardcoded).map_err(|_| {
            BootstrapError::Other(format!("硬编码的 Node 版本号无法解析：{hardcoded}"))
        })?;
        if !is_supported(&v) {
            return Err(BootstrapError::Other(format!(
                "硬编码的 Node {hardcoded} 不满足 ^22.19.0 || >=24.0.0，装上插件也会全挂"
            )));
        }
        return Ok(hardcoded.to_string());
    }

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
    let usable =
        |r: &&Release| r.has_win_x64_zip() && r.semver().map(|v| is_supported(&v)).unwrap_or(false);

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
            Err(e) => dlog!(
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

/// 边下边算哈希，省掉一次完整读盘。
///
/// **支持断点续传**：dest 已有内容时带 Range 头接着下。真机反馈过
/// 「下到一半退出，重开后进度全丢」—— 30MB 的包在慢网络下这很伤。
///
/// 跨镜像续传也是安全的：三个源分发的是同一份官方产物，而且下载完
/// 一律对着官方 SHA256 校验 —— 就算续错了，也会在校验处被拦下删掉重来，
/// 不会有一个坏文件被解压执行。
async fn download_to(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    reporter: &Reporter,
) -> Result<String> {
    // 把已有的部分先喂进哈希 —— 校验和必须覆盖整个文件，不能只算新下的这段
    let mut hasher = Sha256::new();
    let mut done: u64 = 0;

    if let Ok(mut f) = std::fs::File::open(dest) {
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            done += n as u64;
        }
    }

    let mut req = client.get(url);
    if done > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={done}-"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    // 416 = 手里的 .part 比服务端文件还长，多半是上次换过版本留下的坏账：
    // 丢弃重来。递归深度固定是 1 —— 这次文件已删，不会再进这个分支。
    if resp.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        dlog!("[node] 续传范围无效，丢弃旧进度重新下载");
        let _ = std::fs::remove_file(dest);
        return Box::pin(download_to(client, url, dest, reporter)).await;
    }

    let resp = resp
        .error_for_status()
        .map_err(|e| BootstrapError::Download(e.to_string()))?;

    let resumed = done > 0 && resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let mut file = if resumed {
        reporter.detail(
            Stage::DownloadingNode,
            format!("断点续传，已有 {:.1} MB", done as f64 / 1024.0 / 1024.0),
        );
        std::fs::OpenOptions::new().append(true).open(dest)?
    } else {
        // 没有旧进度，或服务端不认 Range（返回 200 全量）：从头写。
        // 哈希也必须重开 —— 上面喂进去的旧内容这下作废了。
        hasher = Sha256::new();
        done = 0;
        std::fs::File::create(dest)?
    };

    // 206 的 content_length 是**剩余**字节数，总量要把已有的加回来
    let total = resp.content_length().map(|len| len + done);
    let mut stream = resp.bytes_stream();

    let mut next_report: u64 = done;

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
        let mut archive =
            zip::ZipArchive::new(file).map_err(|e| BootstrapError::Extract(e.to_string()))?;
        archive
            .extract(&dest)
            .map_err(|e| BootstrapError::Extract(e.to_string()))?;
        Ok(())
    })
    .await
    .map_err(|e| BootstrapError::Extract(format!("解压任务异常退出：{e}")))?
}
