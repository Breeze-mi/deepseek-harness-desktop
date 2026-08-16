//! dsh 与界面插件的版本检测。
//!
//! **已装版本一律读 `package.json`，不起子进程。** `dsh --version` 要冷启动一次
//! Node（0.5-1s），而且输出格式没有稳定契约；`package.json` 里的 `version`
//! 是 npm 自己写的，读文件就够了。
//!
//! 两者的语义不同，不要混：
//! - **dsh** 跟随上游最新版。它是全局安装的，和用户命令行里的 dsh 是同一份，
//! 不该把它硬编码在某个版本上。

use std::path::Path;
use std::time::Duration;

use serde::Serialize;

use super::mirror::NPM_REGISTRIES;
use super::{dsh, plugins};

/// registry 查询超时。这是设置页上的一次点击，用户在等，不能吊太久。
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionStatus {
    /// 展示用名称
    pub name: String,
    pub installed: Option<String>,
    /// dsh 取 registry 最新版，插件取我们硬编码的版本
    pub target: Option<String>,
    /// registry 上的最新版。插件这一行它与 target 不同 ——
    /// 我们硬编码的版本可能落后于上游，前端要靠它给出「升级到 X」的入口。
    pub latest: Option<String>,
    /// latest 是否严格新于 installed。
    ///
    /// **这个判断必须在 Rust 侧做。** 放到前端就得用 JS 比字符串，
    /// 而 `"0.1.2" > "0.1.12"` 在 JS 里是 true —— 正是本文件的
    /// `compares_numerically_not_lexically` 测试在防的那个错。
    pub latest_is_newer: bool,
    /// installed 与 target 不一致，且 target 更新
    pub upgradable: bool,
    /// 补充说明。查不到、被钉住、版本号解析不了都靠它讲清楚。
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeReport {
    pub dsh: VersionStatus,
    pub bundle: VersionStatus,
}

/// 从 package.json 读 version 字段
fn version_at(pkg_dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(pkg_dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(json.get("version")?.as_str()?.to_string())
}

/// 已装的 dsh 版本。
///
/// entry 是 `<root>/@deepseek-ai/dsh/lib/bin.js`，往上两级就是包目录。
pub fn installed_dsh(entry: &Path) -> Option<String> {
    version_at(entry.parent()?.parent()?)
}

/// 已装的插件聚合包版本
pub fn installed_bundle() -> Option<String> {
    plugins::installed_version()
}

/// 问 registry 要某个包的最新版本号。镜像优先，全失败返回 None。
async fn latest_version(client: &reqwest::Client, package: &str) -> Option<String> {
    for registry in NPM_REGISTRIES {
        // scoped 包用不编码的斜杠即可，两个 registry 都支持 /{pkg}/latest
        let url = format!("{}/{}/latest", registry.base.trim_end_matches('/'), package);

        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(v) = json.get("version").and_then(serde_json::Value::as_str) {
                        return Some(v.to_string());
                    }
                }
                dlog!("[upgrade] {} 返回的 JSON 里没有 version", registry.name);
            }
            Ok(resp) => dlog!("[upgrade] {} 返回 {}", registry.name, resp.status()),
            Err(e) => dlog!("[upgrade] {} 查询 {package} 失败：{e}", registry.name),
        }
    }
    None
}

/// 宽松解析版本号。取第一个能被 semver 认下的空白分隔片段 ——
/// 命令输出里混着别的字样时也不至于整条失效。
fn parse(raw: &str) -> Option<semver::Version> {
    raw.split_whitespace()
        .find_map(|tok| semver::Version::parse(tok.trim_start_matches('v')).ok())
}

/// `target` 是否严格新于 `installed`。任一侧解析不出来就判为不可升级 ——
/// 宁可漏报，也不要拿一个解析失败的字符串去怂恿用户重装。
///
/// `plugins::is_installed` 也用它判断插件是否过期，改这里要连带想清楚那边。
pub(crate) fn is_newer(installed: Option<&str>, target: Option<&str>) -> bool {
    match (installed.and_then(parse), target.and_then(parse)) {
        (Some(cur), Some(next)) => next > cur,
        _ => false,
    }
}

/// 检测两个包的版本状态。任何一侧查询失败都不影响另一侧。
pub async fn check(entry: Option<&Path>) -> UpgradeReport {
    let client = reqwest::Client::builder().timeout(TIMEOUT).build().ok();

    let dsh_cur = entry.and_then(installed_dsh);
    let bundle_cur = installed_bundle();

    // 两个查询互不依赖，并发发出去 —— 串行最坏要等 2×8s，
    // 而这是设置页上用户正在干等的一次点击
    let (latest_dsh, latest_bundle) = match &client {
        Some(c) => tokio::join!(
            latest_version(c, dsh::PACKAGE),
            latest_version(c, plugins::BUNDLE)
        ),
        None => (None, None),
    };

    let dsh_upgradable = is_newer(dsh_cur.as_deref(), latest_dsh.as_deref());

    let dsh_note = match (&dsh_cur, &latest_dsh) {
        (None, _) => Some("未检测到已安装的 dsh".to_string()),
        (_, None) => Some("无法连接 npm registry，稍后再试".to_string()),
        _ if dsh_upgradable => None,
        _ => Some("已是最新".to_string()),
    };

    // 插件的目标版本是我们硬编码的那个，不是 registry 最新版
    let pinned = plugins::BUNDLE_VERSION;
    let bundle_upgradable = is_newer(bundle_cur.as_deref(), Some(pinned));

    let bundle_note = match &bundle_cur {
        None => Some("未检测到已安装的界面插件".to_string()),
        Some(cur) if cur == pinned => match &latest_bundle {
            // 上游更新了但我们钉在旧版：讲清楚是有意为之，别让用户以为坏了
            Some(up) if is_newer(Some(pinned), Some(up)) => {
                Some(format!("上游已发布 {up}，本应用固定使用 {pinned}"))
            }
            _ => Some("已是本应用固定的版本".to_string()),
        },
        Some(cur) if bundle_upgradable => Some(format!("当前 {cur}，本应用需要 {pinned}")),
        // 装的比硬编码的还新（多半是用户从命令行装的），不动它
        Some(cur) => Some(format!("当前 {cur}，新于本应用固定的 {pinned}，保持不变")),
    };

    UpgradeReport {
        dsh: VersionStatus {
            name: dsh::PACKAGE.to_string(),
            installed: dsh_cur.clone(),
            target: latest_dsh.clone(),
            latest_is_newer: is_newer(dsh_cur.as_deref(), latest_dsh.as_deref()),
            latest: latest_dsh,
            upgradable: dsh_upgradable,
            note: dsh_note,
        },
        bundle: VersionStatus {
            name: plugins::BUNDLE.to_string(),
            installed: bundle_cur.clone(),
            target: Some(pinned.to_string()),
            latest_is_newer: is_newer(bundle_cur.as_deref(), latest_bundle.as_deref()),
            latest: latest_bundle,
            upgradable: bundle_upgradable,
            note: bundle_note,
        },
    }
}

// #[cfg(test)]
// mod tests {
//     use super::{is_newer, parse};

//     #[test]
//     fn parses_bare_and_prefixed() {
//         assert_eq!(parse("1.2.3").unwrap().to_string(), "1.2.3");
//         assert_eq!(parse("v1.2.3").unwrap().to_string(), "1.2.3");
//     }

//     /// `dsh --version` 之类的输出可能带前缀字样
//     #[test]
//     fn picks_version_out_of_noisy_output() {
//         assert_eq!(parse("dsh 0.4.1").unwrap().to_string(), "0.4.1");
//     }

//     #[test]
//     fn rejects_garbage() {
//         assert!(parse("unknown").is_none());
//         assert!(parse("").is_none());
//     }

//     #[test]
//     fn compares_versions() {
//         assert!(is_newer(Some("1.0.0"), Some("1.0.1")));
//         assert!(!is_newer(Some("1.0.1"), Some("1.0.0")));
//         assert!(!is_newer(Some("1.0.0"), Some("1.0.0")));
//     }

//     /// 预发布版要按 semver 规则比，不能按字符串
//     #[test]
//     fn handles_prerelease() {
//         assert!(is_newer(Some("1.0.0-rc.5"), Some("1.0.0-rc.9")));
//         assert!(is_newer(Some("1.0.0-rc.9"), Some("1.0.0")));
//         assert!(!is_newer(Some("1.0.0"), Some("1.0.0-rc.9")));
//     }

//     /// 解析不出来时必须判为不可升级，避免误导用户去重装
//     #[test]
//     fn unparseable_never_upgradable() {
//         assert!(!is_newer(None, Some("1.0.0")));
//         assert!(!is_newer(Some("1.0.0"), None));
//         assert!(!is_newer(Some("garbage"), Some("1.0.0")));
//     }

//     /// 0.1.2 > 0.1.12 是字符串比较的经典错法，这里必须按数字比
//     #[test]
//     fn compares_numerically_not_lexically() {
//         assert!(is_newer(Some("0.1.2"), Some("0.1.12")));
//         assert!(!is_newer(Some("0.1.12"), Some("0.1.2")));
//     }
// }
