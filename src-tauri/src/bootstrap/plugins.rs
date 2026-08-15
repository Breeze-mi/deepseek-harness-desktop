//! dsh-web-ui 插件全家桶的安装与自检。
//!
//! 鲸鱼娘、任务看板、皮肤中心都来自这个包。装不上的表现是「界面能用但什么都没有」，
//! 用户只会以为软件坏了 —— 所以装完必须自检，失败要明确报错。

use std::fs;
use std::path::{Path, PathBuf};

use super::node::NodeInfo;
use super::proc::run_checked;
use super::{Reporter, Stage};
use crate::error::{BootstrapError, Result};

/// 聚合包。**版本号必须钉死**：这个包两天发过 12 个版本，
/// 且官方 README 点名 0.1.1 的 dsh-pet 缺运行时文件。用 latest 是在赌 registry 缓存。
///
/// 钉的是「新装时的已知可用版本」，**不是上限** —— 用户可以从设置页显式升到上游
/// 最新版，`is_installed` 只在「装的比这个旧」时才判定需要重装，不会把人降回来。
///
/// 0.1.15：已在真机验过鲸鱼娘与任务看板均正常，据此从 0.1.12 提上来。
/// 以后往上提之前也要先实测一次，别只看版本号新就换 —— 0.1.1 那次就是教训。
pub const BUNDLE: &str = "@linxin666/dsh-web-ui-all";
pub const BUNDLE_VERSION: &str = "0.1.15";

const PROFILE: &str = "web";

/// 这三个包有原生构建步骤，pnpm 默认拦截，不放行就报 ERR_PNPM_IGNORED_BUILDS。
/// 必须**装之前**写好，而不是等报错再补救。
const ALLOW_BUILDS: &[&str] = &["cloudflared", "cpu-features", "ssh2"];

/// 自检要确认这几个子包真的落地了。dsh-pet 单列是因为它就是鲸鱼娘，
/// 而且官方明确警告过某些版本会缺运行时文件。
const REQUIRED_PACKAGES: &[&str] = &[
    "@linxin666/dsh-pet",
    "@linxin666/dsh-client-ui-task-board",
    "@linxin666/dsh-skins",
];

pub fn profile_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| BootstrapError::Other("无法定位用户主目录".into()))?;
    Ok(home.join(".dsh").join("profiles").join(PROFILE))
}

/// 已装的聚合包版本。没装返回 None。
pub fn installed_version() -> Option<String> {
    let dir = profile_dir().ok()?;
    let pkg = BUNDLE
        .split('/')
        .fold(dir.join("node_modules"), |acc, seg| acc.join(seg));
    let text = fs::read_to_string(pkg.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(json.get("version")?.as_str()?.to_string())
}

/// 插件是否已装好、不需要再动它。
///
/// 除了运行时文件存在性，**还必须比版本**：否则我们把 `BUNDLE_VERSION` 往上调
/// 之后，老用户因为文件都在而永远走不到重装分支，插件会永久停在旧版 ——
/// 这条不加，插件的升级通道就是死的。
///
/// 只在「装的比钉死的旧」时判为需要处理。装的更新（多半是用户自己从命令行装的）
/// 保持原样，不强行降级。
pub fn is_installed() -> bool {
    let Ok(dir) = profile_dir() else { return false };
    let modules = dir.join("node_modules");

    if !REQUIRED_PACKAGES
        .iter()
        .all(|pkg| package_is_usable(&modules, pkg))
    {
        return false;
    }

    !super::upgrade::is_newer(installed_version().as_deref(), Some(BUNDLE_VERSION))
}

/// 判断一个 dsh 插件包是否真的可用。
///
/// 判据来自包的实际契约，不能一刀切地要求 `lib/index.js`：
///
/// - **`cordis.patch.yml` 必须在** —— 这是包被 dsh 识别为插件的凭据，
///   由 `dsh.bundle.patch` 字段声明。聚合包（如 dsh-skins）只有这个，没有 JS 入口。
/// - **声明了 `main` 就必须真的存在** —— 这正是官方 README 警告的
///   「0.1.1 的 dsh-pet 缺 lib 下运行时文件」那种情形：目录在、包也"装了"，
///   但加载不起来，表现为宠物不出现。
fn package_is_usable(modules: &Path, pkg: &str) -> bool {
    let dir = pkg.split('/').fold(modules.to_path_buf(), |acc, seg| acc.join(seg));

    let Ok(text) = fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };

    let patch = json
        .pointer("/dsh/bundle/patch")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("cordis.patch.yml");
    if !dir.join(patch.trim_start_matches("./")).is_file() {
        return false;
    }

    match json.get("main").and_then(serde_json::Value::as_str) {
        Some(main) => dir.join(main).is_file(),
        // 没有 main 的聚合包，有补丁清单就算数
        None => true,
    }
}

/// 往 profile 的 pnpm-workspace.yaml 里补 allowBuilds。
///
/// **这个文件是 dsh 自己管的**（packages / nodeLinker / minimumReleaseAgeExclude
/// 都由它生成），所以只做最小增量修改，绝不整体重写 —— 用 YAML 库解析再序列化会
/// 重排键序、丢注释，等于砸别人的配置。
pub fn ensure_allow_builds() -> Result<()> {
    let dir = profile_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("pnpm-workspace.yaml");

    let original = fs::read_to_string(&path).unwrap_or_default();
    let patched = patch_allow_builds(&original);

    if patched != original {
        fs::write(&path, patched)?;
    }
    Ok(())
}

/// 纯函数，方便测。返回补齐后的完整文本。
fn patch_allow_builds(src: &str) -> String {
    let missing: Vec<&str> = ALLOW_BUILDS
        .iter()
        .copied()
        .filter(|pkg| !has_allow_entry(src, pkg))
        .collect();

    if missing.is_empty() {
        return src.to_string();
    }

    let added = missing
        .iter()
        .map(|pkg| format!("  {pkg}: true\n"))
        .collect::<String>();

    match src.lines().position(|l| l.trim_start() == "allowBuilds:") {
        // 已有 allowBuilds 段：在它下面插入缺的条目
        Some(idx) => {
            let mut out = String::new();
            for (i, line) in src.lines().enumerate() {
                out.push_str(line);
                out.push('\n');
                if i == idx {
                    out.push_str(&added);
                }
            }
            out
        }
        // 没有就追加一段
        None => {
            let mut out = src.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("allowBuilds:\n");
            out.push_str(&added);
            out
        }
    }
}

/// 检测 `allowBuilds:` 段里是否已有该包。只看紧随其后的缩进块，
/// 避免把别处同名字符串误判成已配置。
fn has_allow_entry(src: &str, pkg: &str) -> bool {
    let mut in_block = false;
    for line in src.lines() {
        if line.trim_start() == "allowBuilds:" {
            in_block = true;
            continue;
        }
        if in_block {
            // 顶格的非空行意味着 allowBuilds 段结束
            if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
                return false;
            }
            if let Some((key, _)) = line.trim().split_once(':') {
                if key.trim().trim_matches('\'').trim_matches('"') == pkg {
                    return true;
                }
            }
        }
    }
    false
}

/// 安装钉死的那个版本。引导流程用。
///
/// 装一次可能不够：干净机器上 profile 是 dsh 在安装过程中现场 scaffold 的，
/// 它可能覆盖掉我们预写的 `pnpm-workspace.yaml`，导致 allowBuilds 失效、
/// 照样报 ERR_PNPM_IGNORED_BUILDS。所以撞到这个错就补写配置再试一次。
pub fn install(node: &NodeInfo, entry: &Path, reporter: &Reporter) -> Result<()> {
    install_version(node, entry, BUNDLE_VERSION, Some(reporter))
}

/// 安装指定版本。
///
/// `BUNDLE_VERSION` 是**新装时的已知可用版本**，不是天花板 —— 上游发新版后
/// 用户可以从设置页显式升上去。钉死只是为了让全新安装落在一个验证过的版本上，
/// 如果没有这条解绑路径，钉死就从保护变成了长期滞后。
pub fn install_version(
    node: &NodeInfo,
    entry: &Path,
    version: &str,
    reporter: Option<&Reporter>,
) -> Result<()> {
    ensure_allow_builds()?;

    match run_install(node, entry, version, reporter) {
        Ok(()) => Ok(()),
        Err(e) if is_ignored_builds(&e) => {
            eprintln!("[plugins] 撞到 ERR_PNPM_IGNORED_BUILDS，补写 allowBuilds 后重试");
            if let Some(r) = reporter {
                r.detail(Stage::InstallingPlugins, "补齐构建白名单后重试");
            }
            ensure_allow_builds()?;
            run_install(node, entry, version, reporter)
        }
        Err(e) => Err(e),
    }
}

fn run_install(
    node: &NodeInfo,
    entry: &Path,
    version: &str,
    reporter: Option<&Reporter>,
) -> Result<()> {
    let spec = format!("{BUNDLE}@{version}");
    // 包名版本号是内部细节，用户只需要知道在装什么、大概要多久
    eprintln!("[plugins] 安装 {spec}");
    if let Some(r) = reporter {
        r.detail(
            Stage::InstallingPlugins,
            "正在下载界面插件（首次需要几分钟）",
        );
    }

    run_checked(
        &node.path,
        &[
            &entry.to_string_lossy(),
            "plugin",
            "--profile",
            PROFILE,
            "add",
            &spec,
        ],
    )?;

    Ok(())
}

/// pnpm 拒绝执行依赖的构建脚本时的特征错误
fn is_ignored_builds(err: &BootstrapError) -> bool {
    matches!(err, BootstrapError::Command { stderr, .. } if stderr.contains("ERR_PNPM_IGNORED_BUILDS"))
}

/// 装完自检。
///
/// 以**文件系统检查为准**（子包的 lib/index.js 是否真的在），
/// dump-config 只作为补充信号 —— 它的输出格式没有稳定契约，
/// 不该拿它当唯一判据。
pub fn verify(node: &NodeInfo, entry: &Path) -> Result<()> {
    let dir = profile_dir()?;
    let modules = dir.join("node_modules");

    let missing: Vec<&str> = REQUIRED_PACKAGES
        .iter()
        .copied()
        .filter(|pkg| !package_is_usable(&modules, pkg))
        .collect();

    if !missing.is_empty() {
        return Err(BootstrapError::PluginVerify(format!(
            "以下插件未正确落地：{}。鲸鱼娘等界面功能不会出现。",
            missing.join("、")
        )));
    }

    // 补充信号：配置层是否真的挂上了。失败只记日志，不阻断 ——
    // 文件都在的情况下，多半是 dump-config 的输出格式变了，而不是插件坏了。
    match run_checked(
        &node.path,
        &[
            &entry.to_string_lossy(),
            "--profile",
            PROFILE,
            "--dump-config",
        ],
    ) {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            if !text.contains("dsh-web-ui-all") && !text.contains("dsh-pet") {
                eprintln!("[plugins] 警告：dump-config 里没看到插件条目，但运行时文件齐全");
            }
        }
        Err(e) => eprintln!("[plugins] dump-config 自检跳过：{e}"),
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实文件长这样（取自实际 profile），补丁必须原样保留其余内容
    const REAL: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\nallowBuilds:\n  cloudflared: true\n  cpu-features: true\n  ssh2: true\n";

    #[test]
    fn leaves_complete_file_untouched() {
        assert_eq!(patch_allow_builds(REAL), REAL);
    }

    #[test]
    fn appends_block_when_absent() {
        let src = "packages:\n  - .\n\nnodeLinker: hoisted\n";
        let out = patch_allow_builds(src);
        assert!(out.starts_with(src));
        assert!(out.contains("allowBuilds:\n"));
        for pkg in ALLOW_BUILDS {
            assert!(out.contains(&format!("  {pkg}: true")), "缺 {pkg}");
        }
    }

    #[test]
    fn fills_only_missing_entries() {
        let src = "allowBuilds:\n  ssh2: true\n";
        let out = patch_allow_builds(src);
        assert_eq!(out.matches("ssh2: true").count(), 1, "不该重复添加");
        assert!(out.contains("cloudflared: true"));
        assert!(out.contains("cpu-features: true"));
    }

    #[test]
    fn preserves_unrelated_keys() {
        let src = "packages:\n  - .\nminimumReleaseAgeExclude:\n  - 'a@1 || 2'\nallowBuilds:\n  ssh2: true\n";
        let out = patch_allow_builds(src);
        assert!(out.contains("minimumReleaseAgeExclude:"));
        assert!(out.contains("- 'a@1 || 2'"));
    }

    #[test]
    fn does_not_match_entries_outside_the_block() {
        // allowBuilds 段之外出现同名字符串，不能算已配置
        let src = "someOther:\n  cloudflared: true\nallowBuilds:\n  ssh2: true\n";
        assert!(!has_allow_entry(src, "cloudflared"));
        assert!(has_allow_entry(src, "ssh2"));
    }
}
