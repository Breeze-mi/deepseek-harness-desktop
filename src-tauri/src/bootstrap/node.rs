//! Node.js 探测与版本闸门。

use std::path::PathBuf;

use semver::{Version, VersionReq};
use serde::Serialize;

use super::proc::run_stdout;
use crate::error::{BootstrapError, Result};

/// dsh 及其插件声明的 engines：`^22.19.0 || >=24.0.0`
///
/// 实测 `@linxin666/dsh-pet@0.1.12` 的 package.json 就是这个约束。
/// **注意 Node 23.x 不满足** —— `^22.19.0` 的上界是 `<23.0.0`。
/// 图省事写成 `>= 22.19` 会放行 23.x，故障表现是「装完插件宠物不出现」，
/// 从现象根本查不到根因。
const REQ_22: &str = "^22.19.0";
const REQ_24_PLUS: &str = ">=24.0.0";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    /// node 可执行文件路径；系统 Node 时为 "node"
    pub path: PathBuf,
    pub version: String,
    /// 是应用自带的便携版还是用户系统里的
    pub portable: bool,
}

/// 版本是否满足 `^22.19.0 || >=24.0.0`
///
/// semver crate 的 VersionReq 不支持 `||`（逗号是 AND），所以拆成两个
/// 约束手动取或，而不是自己写字符串比较。
pub fn is_supported(v: &Version) -> bool {
    let r22 = VersionReq::parse(REQ_22).expect("硬编码的版本约束必须可解析");
    let r24 = VersionReq::parse(REQ_24_PLUS).expect("硬编码的版本约束必须可解析");
    r22.matches(v) || r24.matches(v)
}

/// 解析 `node --version` 的输出，形如 `v22.19.0`
pub fn parse_version(raw: &str) -> Result<Version> {
    let cleaned = raw.trim().trim_start_matches('v');
    Version::parse(cleaned)
        .map_err(|_| BootstrapError::Other(format!("无法解析 Node 版本号：{raw}")))
}

/// 探测系统 PATH 里的 Node。找不到或版本不合格都返回 None，
/// 由调用方决定是回落便携版还是报错。
pub fn detect_system_node() -> Option<NodeInfo> {
    let raw = run_stdout("node", &["--version"]).ok()?;
    let version = parse_version(&raw).ok()?;

    if !is_supported(&version) {
        return None;
    }

    Some(NodeInfo {
        path: PathBuf::from("node"),
        version: version.to_string(),
        portable: false,
    })
}

/// 系统 Node 存在但版本不合格时，把版本号带出来用于提示
pub fn system_node_version() -> Option<String> {
    let raw = run_stdout("node", &["--version"]).ok()?;
    parse_version(&raw).ok().map(|v| v.to_string())
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     fn v(s: &str) -> Version {
//         Version::parse(s).unwrap()
//     }

//     #[test]
//     fn accepts_22_19_and_above_within_22() {
//         assert!(is_supported(&v("22.19.0")));
//         assert!(is_supported(&v("22.19.5")));
//         assert!(is_supported(&v("22.20.0")));
//         assert!(is_supported(&v("22.99.99")));
//     }

//     #[test]
//     fn rejects_below_22_19() {
//         assert!(!is_supported(&v("22.18.0")));
//         assert!(!is_supported(&v("22.0.0")));
//         assert!(!is_supported(&v("20.11.0")));
//         assert!(!is_supported(&v("18.20.0")));
//     }

//     /// 这条是整个闸门的关键：23.x 必须被拒绝。
//     /// 写成 `>= 22.19` 的实现会在这里挂掉。
//     #[test]
//     fn rejects_node_23_entirely() {
//         assert!(!is_supported(&v("23.0.0")));
//         assert!(!is_supported(&v("23.5.0")));
//         assert!(!is_supported(&v("23.99.99")));
//     }

//     #[test]
//     fn accepts_24_and_above() {
//         assert!(is_supported(&v("24.0.0")));
//         assert!(is_supported(&v("24.18.0")));
//         assert!(is_supported(&v("26.7.0")));
//     }

//     #[test]
//     fn parses_version_output() {
//         assert_eq!(parse_version("v22.19.0").unwrap(), v("22.19.0"));
//         assert_eq!(parse_version("  v24.1.2\n").unwrap(), v("24.1.2"));
//         assert_eq!(parse_version("22.19.0").unwrap(), v("22.19.0"));
//         assert!(parse_version("not a version").is_err());
//     }
// }
