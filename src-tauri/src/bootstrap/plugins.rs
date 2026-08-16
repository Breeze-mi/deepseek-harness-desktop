//! dsh-web-ui 插件全家桶：安装、pnpm-workspace 配置、自检。
//!
//! 鲸鱼娘、任务看板、皮肤中心都来自聚合包。装不上的表现是「界面能用但什么都没有」，
//! 所以装完要自检；但自检**只对聚合包整体缺失硬失败**，其余一律降级为警告 ——
//! 误判的代价（应用打不开）远大于漏判（少个挂件）。
//!
//! 这条链路的三个坑全部真机踩过、有官方 README 背书，本文件的结构因它们而来：
//!
//! 1. **布局**：pnpm isolated 布局把传递依赖收进 `.pnpm/`，而 dsh 按包名从
//!    profile 根解析 cordis 补丁 —— 必须 `nodeLinker: hoisted`，
//!    顶层见不到就等于没装好，`.pnpm/` 里找到了也不算。
//! 2. **构建白名单时机**：`allowBuilds` 必须在 pnpm **第一次跑之前**就位；
//!    事后补写会被「Already up to date」跳过，重试必须连 node_modules 一起清。
//! 3. **挂载凭据**：`dsh.profile.bundles` 由 dsh 在 pnpm 成功后才写 ——
//!    包在磁盘上 ≠ 插件已挂载，两处都要查，否则坏状态永远不自愈。
//!
//! `pnpm-workspace.yaml` 的处理原则：**上游优先、增量补丁、快照兜底**
//! （`ensure_profile_scaffold` → `ensure_allow_builds` → `SCAFFOLD`），绝不整体重写。

use std::fs;
use std::path::{Path, PathBuf};

use super::mirror::NPM_REGISTRIES;
use super::node::NodeInfo;
use super::proc::{run_checked, run_streaming};
use super::{Reporter, Stage};
use crate::error::{BootstrapError, Result};

/// 聚合包与硬编码的安装版本。上游两天发过 12 个版本、且有过坏版本先例，
/// 用 latest 是在赌 registry。硬编码的是「已实测可用」的版本，不是上限 ——
/// 用户可从设置页显式升级，`is_installed` 不会把更新的降回来。
/// 往上提版本前必须先真机实测一次。
pub const BUNDLE: &str = "@linxin666/dsh-web-ui-all";
pub const BUNDLE_VERSION: &str = "0.1.15";

const PROFILE: &str = "web";

/// 构建脚本裁决表（pnpm 11 `allowBuilds` 语义：true = 放行执行，
/// false = **明确拒绝、静默跳过** —— 不写才会炸 ERR_PNPM_IGNORED_BUILDS）。
/// 必须在 pnpm 第一次跑之前写好（坑 2）。
///
/// - cloudflared / ssh2：postinstall 是功能必需（下载隧道二进制、装配 crypto），放行。
/// - cpu-features：ssh2 的**可选**加速件。绝大多数用户机器没有 C++ 编译器，
///   放行的结果是 node-gyp 白试 5 秒、再往日志甩一整页像崩溃的堆栈。
///   `false` 的语义已对 pnpm 11.0 发布说明、维护者答复与用户实测三方核实。
const ALLOW_BUILDS: &[(&str, bool)] = &[
    ("cloudflared", true),
    ("cpu-features", false),
    ("ssh2", true),
];

/// 自检要确认这几个子包真的落地了。dsh-pet 单列是因为它就是鲸鱼娘，
/// 而且官方明确警告过某些版本会缺运行时文件。
const REQUIRED_PACKAGES: &[&str] = &[
    "@linxin666/dsh-pet",
    "@linxin666/dsh-client-ui-task-board",
    "@linxin666/dsh-skins",
];

pub fn profile_dir() -> Result<PathBuf> {
    let home =
        dirs::home_dir().ok_or_else(|| BootstrapError::Other("无法定位用户主目录".into()))?;
    Ok(home.join(".dsh").join("profiles").join(PROFILE))
}

/// 已装的聚合包版本。没装返回 None。
pub fn installed_version() -> Option<String> {
    let modules = profile_dir().ok()?.join("node_modules");
    let dir = resolve_package(&modules, BUNDLE)?;
    let text = fs::read_to_string(dir.join("package.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(json.get("version")?.as_str()?.to_string())
}

/// 插件是否已装好、不需要再动它。
///
/// 三个条件缺一不可：运行时文件在顶层（坑 1）、真的挂载了（坑 3）、
/// 版本不老于硬编码值 —— 少了版本比较，`BUNDLE_VERSION` 上调后老用户
/// 会因为文件都在而永远停在旧版。装得更新则保持原样，不强行降级。
pub fn is_installed() -> bool {
    let Ok(dir) = profile_dir() else { return false };
    let modules = dir.join("node_modules");

    if !REQUIRED_PACKAGES
        .iter()
        .all(|pkg| package_is_usable(&modules, pkg))
    {
        return false;
    }

    if !is_mounted() {
        return false;
    }

    !super::upgrade::is_newer(installed_version().as_deref(), Some(BUNDLE_VERSION))
}

/// 聚合包是否真的挂到了 profile 上（坑 3）。
///
/// `dependencies` 是 pnpm 写的，`dsh.profile.bundles` 才是 dsh 写的挂载凭据。
/// 真机出现过 pnpm 全装好、dsh 因退出码 1 没写 bundles 的状态 ——
/// 只看 node_modules 会把它误判成已装好，永远走不到修复分支。
fn is_mounted() -> bool {
    let Ok(dir) = profile_dir() else { return false };
    let Ok(text) = fs::read_to_string(dir.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };

    json.pointer("/dsh/profile/bundles")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(BUNDLE)))
}

/// 顶层路径 `node_modules/@scope/name`。
fn top_level(modules: &Path, pkg: &str) -> PathBuf {
    pkg.split('/')
        .fold(modules.to_path_buf(), |acc, seg| acc.join(seg))
}

/// 包只存在于 pnpm 的 isolated 仓库里时，返回它的真实位置。
///
/// 目录名是把 `/` 换成 `+` 再接 `@版本`，版本不预设、按前缀扫。
fn in_pnpm_store(modules: &Path, pkg: &str) -> Option<PathBuf> {
    let prefix = format!("{}@", pkg.replace('/', "+"));
    for entry in fs::read_dir(modules.join(".pnpm")).ok()?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with(&prefix) {
            continue;
        }
        let nested = top_level(&entry.path().join("node_modules"), pkg);
        if nested.join("package.json").is_file() {
            return Some(nested);
        }
    }
    None
}

/// 定位一个包，顶层找不到就去 `.pnpm/` 里找。
///
/// **只用于读版本号这类信息性用途**，不能拿来判断插件可不可用 —— 见
/// `package_is_usable` 上的说明。
fn resolve_package(modules: &Path, pkg: &str) -> Option<PathBuf> {
    let direct = top_level(modules, pkg);
    if direct.join("package.json").is_file() {
        return Some(direct);
    }
    in_pnpm_store(modules, pkg)
}

/// 包是否真的能被 dsh 加载。
///
/// **只认 `node_modules/` 顶层**（坑 1）—— `.pnpm/` 里找到也判不通过，
/// 那是假通过：我们报正常，用户那边鲸鱼娘照样不出现。判据取自包的实际声明：
/// `cordis.patch.yml`（由 `dsh.bundle.patch` 声明）必须在；声明了 `main`
/// 就必须真的存在（官方警告过缺运行时文件的坏版本）；纯聚合包没有 main，不强求。
fn package_is_usable(modules: &Path, pkg: &str) -> bool {
    let dir = top_level(modules, pkg);

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

/// 清掉旧版本应用留下的残缺 profile（只有 allowBuilds、缺 packages/nodeLinker
/// 的那种 stub），返回是否真的动了手。判据取「dsh 一定写、stub 一定没写」的
/// 两个键，**两个都缺才动手** —— 宁可漏删不误删。
/// 连 node_modules 一起删是钝器但构造上正确：yaml 改了，pnpm 未必重排已有的树。
pub fn repair_stale_workspace() -> bool {
    let Ok(dir) = profile_dir() else {
        return false;
    };
    let path = dir.join("pnpm-workspace.yaml");
    let Ok(text) = fs::read_to_string(&path) else {
        return false;
    };

    let has_key = |k: &str| text.lines().any(|l| l.trim_start().starts_with(k));
    if has_key("packages:") || has_key("nodeLinker:") {
        return false;
    }

    dlog!(
        "[plugins] 发现残缺的 pnpm-workspace.yaml（缺 nodeLinker: hoisted），清理 profile 后重装"
    );
    if fs::remove_file(&path).is_err() {
        return false;
    }
    // 删不动也继续：配置已经没了，dsh 会重新生成。最坏情况是布局仍旧不对，
    // 那时自检会明确告诉用户手动删 profile。
    if let Err(e) = fs::remove_dir_all(dir.join("node_modules")) {
        dlog!("[plugins] node_modules 清理失败（继续重装）：{e}");
    }
    true
}

/// 硬编码快照：dsh 0.1.0-rc.6 scaffold 的逐字抄本，只含**上游自己的**键。
/// 我们需要的键全走增量补丁 —— 快照越小，上游改格式时伪造错的越少。
/// 这是三级顺序的最后兜底，见 `ensure_profile_scaffold`。
const SCAFFOLD: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\n";

/// 三级顺序的第一级：跑一条只读命令触发 **dsh 自己**初始化 profile
/// （真机日志：`dsh: initialized profile web at …`）—— 它建的文件永远是
/// 它当前版本想要的样子。失败无所谓：`plugin add` 那步反正也会初始化。
fn ensure_profile_scaffold(node: &NodeInfo, entry: &Path) {
    let Ok(dir) = profile_dir() else { return };
    if dir.join("pnpm-workspace.yaml").is_file() {
        return;
    }

    dlog!("[plugins] 先让 dsh 自行初始化 profile（避免动用硬编码快照）");
    let _ = run_checked(
        &node.path,
        &[
            &entry.to_string_lossy(),
            "--profile",
            PROFILE,
            "--dump-config",
        ],
    );
}

/// 三级顺序的第二级：幂等补上我们必需的两段配置 ——
/// `allowBuilds`（坑 2）与 `@linxin666/*` 的 release-age 排除
/// （官方 README：新版发布后约 10 天内 pnpm 11 可能静默装回旧版并写坏配置）。
/// 对 dsh 生成的文件与硬编码快照一视同仁，存量安装下次也会被补齐。
/// 绝不整体重写：文件归 dsh 管，YAML 解析重排会弄砸别人的配置。
pub fn ensure_allow_builds() -> Result<()> {
    let dir = profile_dir()?;
    let path = dir.join("pnpm-workspace.yaml");

    let original = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            // dsh 初始化没产出文件才落到快照；出处留痕，上游变了以此为排查起点
            dlog!("[plugins] dsh 未生成 pnpm-workspace.yaml，使用硬编码快照（抄自 dsh 0.1.0-rc.6 实际输出）");
            fs::create_dir_all(&dir)?;
            SCAFFOLD.to_string()
        }
    };
    let patched = patch_release_age(&patch_allow_builds(&original));

    if patched != original {
        fs::write(&path, patched)?;
    }
    Ok(())
}

/// 纯函数，方便测。返回补齐后的完整文本。
fn patch_allow_builds(src: &str) -> String {
    let missing: Vec<(&str, bool)> = ALLOW_BUILDS
        .iter()
        .copied()
        .filter(|(pkg, _)| !has_allow_entry(src, pkg))
        .collect();

    if missing.is_empty() {
        return src.to_string();
    }

    let added = missing
        .iter()
        .map(|(pkg, allow)| format!("  {pkg}: {allow}\n"))
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
/// **只认键的存在、不校对值**：存量安装里已写的裁决（含老版本写下的
/// `cpu-features: true`）一律不动 —— 改别人已生效的配置比留噪声更危险。
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

/// `minimumReleaseAgeExclude` 块里是否已有整 scope 的通配条目。
/// 只看紧随其后的缩进块 —— dsh/pnpm 写的逐包条目（`- '@linxin666/dsh-pet@0.1.15'`）
/// 不算数，那些挡不住「下一个还没被记录的新版本」。
fn has_release_age_wildcard(src: &str) -> bool {
    let mut in_block = false;
    for line in src.lines() {
        if line.trim_start().starts_with("minimumReleaseAgeExclude:") {
            in_block = true;
            continue;
        }
        if in_block {
            if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
                return false;
            }
            let item = line
                .trim()
                .trim_start_matches('-')
                .trim()
                .trim_matches('\'')
                .trim_matches('"');
            if item == "@linxin666/*" {
                return true;
            }
        }
    }
    false
}

/// 幂等地补上 `@linxin666/*` 的 release-age 排除。结构与 `patch_allow_builds`
/// 同款：有键就往块里插一行，没键就追加一段，已有通配就原样返回。
fn patch_release_age(src: &str) -> String {
    if has_release_age_wildcard(src) {
        return src.to_string();
    }

    let added = "  - '@linxin666/*'\n";

    match src
        .lines()
        .position(|l| l.trim_start().starts_with("minimumReleaseAgeExclude:"))
    {
        Some(idx) => {
            let mut out = String::new();
            for (i, line) in src.lines().enumerate() {
                out.push_str(line);
                out.push('\n');
                if i == idx {
                    out.push_str(added);
                }
            }
            out
        }
        None => {
            let mut out = src.to_string();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("minimumReleaseAgeExclude:\n");
            out.push_str(added);
            out
        }
    }
}

/// 安装硬编码的版本。引导流程用。
pub fn install(node: &NodeInfo, entry: &Path, reporter: &Reporter) -> Result<()> {
    install_version(node, entry, BUNDLE_VERSION, Some(reporter))
}

/// 安装指定版本。
///
/// `BUNDLE_VERSION` 是**新装时的已知可用版本**，不是天花板 —— 上游发新版后
/// 用户可以从设置页显式升上去。硬编码只是为了让全新安装落在一个验证过的版本上，
/// 如果没有这条解绑路径，硬编码就从保护变成了长期滞后。
pub fn install_version(
    node: &NodeInfo,
    entry: &Path,
    version: &str,
    reporter: Option<&Reporter>,
) -> Result<()> {
    // 顺序有讲究：先给 dsh 机会自己建 profile，再打我们的增量补丁
    ensure_profile_scaffold(node, entry);
    ensure_allow_builds()?;

    let result = match run_install(node, entry, version, reporter) {
        Ok(()) => Ok(()),
        Err(e) if is_ignored_builds(&e) => {
            dlog!("[plugins] 撞到 ERR_PNPM_IGNORED_BUILDS，补写 allowBuilds 并清空 node_modules 后重试");
            if let Some(r) = reporter {
                r.detail(Stage::InstallingPlugins, "补齐构建白名单后重试");
            }
            ensure_allow_builds()?;

            // 坑 2：只补配置会被 pnpm「Already up to date」跳过（真机日志证实），
            // 必须连 node_modules 一起清。代价小：包都在 pnpm 内容仓库里，
            // 重装是 reused 不是重新下载。
            if let Ok(dir) = profile_dir() {
                if let Err(e) = fs::remove_dir_all(dir.join("node_modules")) {
                    dlog!("[plugins] node_modules 清理失败（仍继续重试）：{e}");
                }
            }

            run_install(node, entry, version, reporter)
        }
        Err(e) => Err(e),
    };

    result.map_err(explain)
}

/// 把已知的、用户自己就能解决的失败，翻译成「照做就能修好」的说明。
///
/// 这类错误的共同点是：真正的原因在环境里，不在我们的代码里，而中间隔着
/// dsh 和 pnpm 两层转述，传到用户眼前时已经只剩一个退出码。
/// 不在这里翻译一次，用户就只能对着「退出码 1」干瞪眼。
fn explain(e: BootstrapError) -> BootstrapError {
    if is_execution_policy_block(&e) {
        return BootstrapError::Other(format!(
            "PowerShell 执行策略禁止运行 pnpm 的脚本，界面插件因此装不上。\n\
             在 PowerShell 里执行下面这条命令后点重试（只影响当前用户，不需要管理员权限）：\n\
             \n    Set-ExecutionPolicy -Scope CurrentUser -ExecutionPolicy RemoteSigned\n\n\
             原始错误：\n{e}"
        ));
    }
    e
}

/// 跑一次 `dsh plugin add`，registry 走镜像降级。
///
/// 如果哪天 pnpm 不认这个变量，效果等同于今天（走用户默认源），不会更糟。
fn run_install(
    node: &NodeInfo,
    entry: &Path,
    version: &str,
    reporter: Option<&Reporter>,
) -> Result<()> {
    let spec = format!("{BUNDLE}@{version}");
    let entry = entry.to_string_lossy();
    let args: [&str; 6] = [&entry, "plugin", "--profile", PROFILE, "add", &spec];

    let mut last: Option<BootstrapError> = None;

    for registry in NPM_REGISTRIES {
        // 包名版本号是内部细节，用户只需要知道在装什么、大概要多久
        dlog!("[plugins] 安装 {spec}（registry: {}）", registry.name);
        if let Some(r) = reporter {
            r.detail(
                Stage::InstallingPlugins,
                "正在下载界面插件（首次需要几分钟）",
            );
        }

        // 别叫 last —— 会遮蔽外层记录安装错误的同名变量
        let mut last_activity = String::new();
        let result = run_streaming(
            &node.path,
            &args,
            &[("npm_config_registry", registry.base)],
            &mut |line| {
                let Some((text, fraction)) = install_progress(line) else {
                    return;
                };
                // pnpm 会连续打出计数相同的进度行，原样转发只是白刷事件
                if text == last_activity {
                    return;
                }
                last_activity = text.clone();
                if let Some(r) = reporter {
                    r.activity(Stage::InstallingPlugins, text, fraction);
                }
            },
        );

        match result {
            Ok(()) => return Ok(()),
            // 构建白名单被拦不是网络问题，换源解决不了。
            // 直接交回上层去补配置重试，别在这儿白跑第二个源。
            Err(e) if is_ignored_builds(&e) => return Err(e),
            Err(e) => {
                dlog!("[plugins] registry {} 安装失败: {e}", registry.name);
                last = Some(e);
            }
        }
    }

    Err(last.unwrap_or_else(|| BootstrapError::Other("没有可用的 npm registry".into())))
}

/// 把 pnpm 的一行输出翻译成「当前活动」+ 完成比例。返回 None 表示这行没信息量。
///
/// 首次装插件要几分钟，原先界面上只有一句静止的「正在下载界面插件」，
/// 用户无法区分「在跑」和「卡死」—— 而 pnpm 一直在打进度，只是被我们吞了。
/// 有了比例，这个最长的阶段里进度条能真的动起来。
fn install_progress(line: &str) -> Option<(String, Option<f64>)> {
    let line = line.trim();

    // 这一步在真机上花了 7 秒且毫无动静，不说明用户会以为卡住了
    if line.contains("Verifying lockfile") {
        return Some(("正在校验依赖来源…".into(), None));
    }

    // `Progress: resolved 31, reused 30, downloaded 0, added 16`
    let rest = line.strip_prefix("Progress:")?;
    let field = |name: &str| -> Option<u32> {
        rest.split(',')
            .find_map(|part| part.trim().strip_prefix(name)?.trim().parse().ok())
    };

    let total = field("resolved")?;
    let done = field("added").unwrap_or(0);
    let fraction = (total > 0).then(|| f64::from(done) / f64::from(total));
    Some((format!("正在安装界面插件… {done}/{total}"), fraction))
}

/// pnpm 拒绝执行依赖的构建脚本时的特征错误
fn is_ignored_builds(err: &BootstrapError) -> bool {
    matches!(err, BootstrapError::Command { stderr, .. } if stderr.contains("ERR_PNPM_IGNORED_BUILDS"))
}

/// PowerShell 执行策略挡住了 pnpm 的 `.ps1` 垫片。
///
/// 受管控的 Windows（域内机器、企业镜像）默认策略常是 `Restricted`，
/// npm 装出来的 `pnpm.ps1` 直接跑不了。这个报错原文写得很清楚，
/// 但它会被 dsh 吞成一句「pnpm failed」再传给我们 —— 用户看到的只剩退出码 1，
/// 根本不可能联想到执行策略。所以必须在这里认出来，把解法直接写给他。
fn is_execution_policy_block(err: &BootstrapError) -> bool {
    matches!(err, BootstrapError::Command { stderr, .. }
        if stderr.contains("PSSecurityException")
            || stderr.contains("UnauthorizedAccess")
            || stderr.contains("禁止运行脚本")
            || stderr.contains("running scripts is disabled"))
}

/// 装完自检。返回 `Some(警告)` = 能用但有疑点；只有聚合包整体缺失才 Err ——
/// 误判的代价（应用打不开）远大于漏判（少个挂件）。
/// 判定按可信度排序：文件系统 → `dsh --dump-config`
/// （后者才是真正加载插件的一方，但输出无稳定契约，只用它翻案、不定罪）。
pub fn verify(node: &NodeInfo, entry: &Path) -> Result<Option<String>> {
    let modules = profile_dir()?.join("node_modules");

    // 聚合包都找不到 = 安装是真没发生（而不是摆放方式不同）
    if resolve_package(&modules, BUNDLE).is_none() {
        return Err(BootstrapError::PluginVerify(format!(
            "{BUNDLE} 没有落地，界面插件全部不可用。"
        )));
    }

    // 包在、但没写进 dsh.profile.bundles —— 装了却没挂上。
    if !is_mounted() {
        return Ok(Some(
            "界面插件已下载但没有挂载到 profile（dsh 在写 bundles 前就失败了），\
             所以鲸鱼娘、皮肤中心等不会出现。点「重试安装」可以补上。"
                .into(),
        ));
    }

    let missing: Vec<&str> = REQUIRED_PACKAGES
        .iter()
        .copied()
        .filter(|pkg| !package_is_usable(&modules, pkg))
        .collect();

    if missing.is_empty() {
        return Ok(None);
    }

    // 顶层没有、`.pnpm/` 里却有 —— 这不是没装，是 pnpm 用错了布局。
    // 症状完全不同，给的处置办法也完全不同，必须分开说。
    let stashed: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|pkg| in_pnpm_store(&modules, pkg).is_some())
        .collect();

    if !stashed.is_empty() {
        // 把配置的实际状态打进日志：下次拿到日志就能直接定案，不用再猜
        match fs::read_to_string(profile_dir()?.join("pnpm-workspace.yaml")) {
            Ok(text) => dlog!(
                "[plugins] pnpm-workspace.yaml 现状：nodeLinker={} packages={}",
                text.contains("nodeLinker"),
                text.contains("packages:")
            ),
            Err(e) => dlog!("[plugins] pnpm-workspace.yaml 读不到：{e}"),
        }

        return Ok(Some(format!(
            "插件已下载但摆放位置不对（{}），dsh 加载不到。\
             删除 ~/.dsh/profiles/web 整个目录后重启即可重装。",
            stashed.join("、")
        )));
    }

    // 文件层没对上，再问配置层。dump-config 的输出格式没有稳定契约，
    // 所以只用它"翻案"、不用它定罪 —— 拿不到就当没问过。
    if let Ok(out) = run_checked(
        &node.path,
        &[
            &entry.to_string_lossy(),
            "--profile",
            PROFILE,
            "--dump-config",
        ],
    ) {
        let text = String::from_utf8_lossy(&out.stdout);
        if missing.iter().all(|pkg| text.contains(pkg)) {
            dlog!("[plugins] 子包不在预期路径，但 dump-config 里都在，判定正常");
            return Ok(None);
        }
    }

    dlog!("[plugins] 警告：未确认落地的子包：{}", missing.join("、"));
    Ok(Some(format!(
        "以下界面插件未能确认挂载：{}。若鲸鱼娘等功能没出现，可在设置页重装界面插件。",
        missing.join("、")
    )))
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     /// 测试夹具：真实 pnpm-workspace.yaml 的逐字抄本（dsh 0.1.0-rc.6 + pnpm 11，
//     /// 采样自 Windows；**内容本身平台无关**，同一批键在 Linux CI 有公开实证）。
//     /// 行结束符固定 `\n`：解析端 `str::lines()` 对 `\r\n` 同样兼容。
//     /// 夹具的职责就是「固定的已知输入」——上游格式变了改这里即可，不影响运行时。
//     const REAL: &str = "packages:\n  - .\n\nnodeLinker: hoisted\nautoInstallPeers: false\nallowBuilds:\n  cloudflared: true\n  cpu-features: true\n  ssh2: true\n";

//     /// 同时钉死「存量值不改写」：REAL 里 cpu-features 是老版本写的 true，
//     /// 与新默认（false）不同，补丁也必须原样放过
//     #[test]
//     fn leaves_complete_file_untouched() {
//         assert_eq!(patch_allow_builds(REAL), REAL);
//     }

//     #[test]
//     fn appends_block_when_absent() {
//         let src = "packages:\n  - .\n\nnodeLinker: hoisted\n";
//         let out = patch_allow_builds(src);
//         assert!(out.starts_with(src));
//         assert!(out.contains("allowBuilds:\n"));
//         for (pkg, allow) in ALLOW_BUILDS {
//             assert!(out.contains(&format!("  {pkg}: {allow}")), "缺 {pkg}");
//         }
//     }

//     #[test]
//     fn fills_only_missing_entries() {
//         let src = "allowBuilds:\n  ssh2: true\n";
//         let out = patch_allow_builds(src);
//         assert_eq!(out.matches("ssh2: true").count(), 1, "不该重复添加");
//         assert!(out.contains("cloudflared: true"));
//         assert!(out.contains("cpu-features: false"));
//     }

//     /// cpu-features 必须是**明确拒绝**（false）而不是不写：pnpm 11 对「不写」
//     /// 炸 ERR_PNPM_IGNORED_BUILDS，对 false 才是静默跳过（官方发布说明 +
//     /// 维护者答复 + 用户实测三方核实过，别改回 true 或删掉）
//     #[test]
//     fn cpu_features_is_denied_not_missing() {
//         let seeded = patch_allow_builds(SCAFFOLD);
//         assert!(seeded.contains("cpu-features: false"));
//         assert!(!seeded.contains("cpu-features: true"));
//     }

//     #[test]
//     fn preserves_unrelated_keys() {
//         let src = "packages:\n  - .\nminimumReleaseAgeExclude:\n  - 'a@1 || 2'\nallowBuilds:\n  ssh2: true\n";
//         let out = patch_allow_builds(src);
//         assert!(out.contains("minimumReleaseAgeExclude:"));
//         assert!(out.contains("- 'a@1 || 2'"));
//     }

//     #[test]
//     fn does_not_match_entries_outside_the_block() {
//         // allowBuilds 段之外出现同名字符串，不能算已配置
//         let src = "someOther:\n  cloudflared: true\nallowBuilds:\n  ssh2: true\n";
//         assert!(!has_allow_entry(src, "cloudflared"));
//         assert!(has_allow_entry(src, "ssh2"));
//     }

//     /// 快照 + 补丁链的产物必须带齐三段，各对应模块头的一个真机事故
//     #[test]
//     fn seeded_config_has_both_critical_keys() {
//         let seeded = patch_release_age(&patch_allow_builds(SCAFFOLD));

//         assert!(seeded.contains("nodeLinker: hoisted"), "丢了 nodeLinker");
//         assert!(seeded.contains("packages:"), "丢了 packages");
//         assert!(
//             seeded.contains("minimumReleaseAgeExclude"),
//             "丢了 release-age 排除（官方 README 点名的静默降级坑）"
//         );
//         for (pkg, allow) in ALLOW_BUILDS {
//             assert!(seeded.contains(&format!("  {pkg}: {allow}")), "缺 {pkg}");
//         }
//     }

//     /// 逐包条目样本取自真实 profile：必须原样保留，通配另起一行且幂等
//     #[test]
//     fn adds_wildcard_into_existing_release_age_block() {
//         let src = "packages:\n  - .\nminimumReleaseAgeExclude:\n  - '@linxin666/dsh-pet@0.1.15'\nallowBuilds:\n  ssh2: true\n";
//         let out = patch_release_age(src);
//         assert!(out.contains("  - '@linxin666/*'"));
//         assert!(out.contains("'@linxin666/dsh-pet@0.1.15'"));
//         assert_eq!(patch_release_age(&out), out, "不该重复添加");
//     }

//     #[test]
//     fn appends_release_age_block_when_absent() {
//         let out = patch_release_age("packages:\n  - .\n");
//         assert!(out.contains("minimumReleaseAgeExclude:\n  - '@linxin666/*'\n"));
//         // 逐包条目不能顶替通配：它挡不住下一个还没被记录的新版本
//         assert!(!has_release_age_wildcard(
//             "minimumReleaseAgeExclude:\n  - '@linxin666/dsh-pet@0.1.15'\n"
//         ));
//     }

//     /// 样本逐字取自真机日志，别改成手编的
//     #[test]
//     fn reads_pnpm_progress_lines() {
//         let (text, fraction) =
//             install_progress("Progress: resolved 31, reused 30, downloaded 0, added 16").unwrap();
//         assert_eq!(text, "正在安装界面插件… 16/31");
//         assert_eq!(fraction, Some(16.0 / 31.0));

//         // added 还没出现时也不能崩，按 0 算
//         let (text, _) = install_progress("Progress: resolved 1, reused 0, downloaded 0").unwrap();
//         assert_eq!(text, "正在安装界面插件… 0/1");

//         assert!(install_progress(
//             "? Verifying lockfile against supply-chain policies (31 entries)..."
//         )
//         .is_some());
//         assert!(install_progress("Already up to date").is_none());
//         assert!(install_progress("Progress: 什么都没有").is_none());
//     }
// }
