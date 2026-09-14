//! 平台（Steam / Epic）游戏清单扫描
//!
//! 数据来源全部是**本机既有文件**，不联网、不登录、不调私有接口：
//! - **Epic**：`%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests\*.item`（JSON）
//! - **Steam**：注册表 `HKCU\Software\Valve\Steam\SteamPath` → `steamapps\libraryfolders.vdf`
//!   → 各库 `steamapps\appmanifest_<appid>.acf`（Valve KeyValues 文本）
//!
//! 关键设计（2026-09-13 拍板）：
//! - 本模块**只负责"发现游戏 + 给出定位信息"**，不碰数据库、不碰追踪器；
//! - **时长追踪一律以 `install_path` 目录前缀为基准**，不依赖 exe 文件名——
//!   Steam 的 acf 根本不记录 exe 名，Epic 的 `LaunchExecutable` 还可能指向启动器壳
//!   （实测 `Launcher.exe` / `PlayRDR2.exe`），靠 exe 名匹配必然漏追。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// 平台游戏候选（扫描结果，尚未入库）
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PlatformGame {
    /// 平台标识："steam" | "epic"
    pub platform: String,
    /// 平台侧标识：Steam=appid（如 "550"）；Epic=`CatalogNamespace:CatalogItemId:AppName`
    pub platform_id: String,
    /// 展示名
    pub name: String,
    /// 安装目录（**追踪与截图识别的匹配基准**）
    pub install_path: String,
    /// 主程序完整路径。Epic 来自 `InstallLocation\LaunchExecutable`；Steam **恒为 None**
    pub exe_path: Option<String>,
    /// 主程序文件名。Steam **恒为 None**
    pub exe_name: Option<String>,
    /// 占用字节数（Steam=`SizeOnDisk`；Epic=`InstallSize`）
    pub size_bytes: Option<u64>,
    /// 版本标识（Steam=`buildid`；Epic=`AppVersionString`），替代读 exe 资源的方式
    pub version: Option<String>,
    /// 是否已在库中（按安装路径判定；由命令层填充，扫描器本身恒为 false）
    #[serde(default)]
    pub already_added: bool,
    /// 库中是否已有同名条目（安装路径不同；由命令层填充，仅作提示、不阻止导入）
    #[serde(default)]
    pub same_name_in_library: bool,
}

/// 支持的平台常量
pub const PLATFORM_STEAM: &str = "steam";
pub const PLATFORM_EPIC: &str = "epic";

/// 构造反向启动 URI（交由 Windows shell 转给对应客户端）
///
/// - Steam：`steam://rungameid/{appid}`
/// - Epic：`com.epicgames.launcher://apps/{ns}%3A{item}%3A{app}?action=launch&silent=true`
///   （三段 ID 用 URL 编码的冒号 `%3A` 连接，格式由 Epic 官方协议激活文档给出）
pub fn launch_uri(platform: &str, platform_id: &str) -> Result<String> {
    match platform {
        PLATFORM_STEAM => {
            if platform_id.trim().is_empty() {
                anyhow::bail!("Steam appid 为空");
            }
            Ok(format!("steam://rungameid/{}", platform_id.trim()))
        }
        PLATFORM_EPIC => {
            let parts: Vec<&str> = platform_id.split(':').collect();
            if parts.len() != 3 || parts.iter().any(|p| p.trim().is_empty()) {
                anyhow::bail!("Epic 三段 ID 格式非法: {}", platform_id);
            }
            Ok(format!(
                "com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true",
                parts[0].trim(),
                parts[1].trim(),
                parts[2].trim()
            ))
        }
        other => anyhow::bail!("不支持反向启动的平台: {}", other),
    }
}

// ==================== Epic ====================

/// Epic 清单目录：`%ProgramData%\Epic\EpicGamesLauncher\Data\Manifests`
fn epic_manifests_dir() -> Result<PathBuf> {
    let pd = std::env::var("ProgramData")
        .or_else(|_| std::env::var("ALLUSERSPROFILE"))
        .context("无法获取 ProgramData 目录")?;
    Ok(PathBuf::from(pd)
        .join("Epic")
        .join("EpicGamesLauncher")
        .join("Data")
        .join("Manifests"))
}

/// 扫描全部 Epic 已安装应用（仅在 Epic 未安装时返回空列表，不算错误）
pub fn scan_epic() -> Result<Vec<PlatformGame>> {
    let dir = epic_manifests_dir()?;
    if !dir.is_dir() {
        tracing::info!("Epic 清单目录不存在，跳过: {}", dir.display());
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    let entries = std::fs::read_dir(&dir)
        .with_context(|| format!("读取 Epic 清单目录失败: {}", dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        // 只认 *.item；Pending/ 子目录等一律跳过
        if path.extension().and_then(|s| s.to_str()) != Some("item") {
            continue;
        }
        match parse_epic_item(&path) {
            Ok(Some(g)) => out.push(g),
            Ok(None) => {}
            Err(e) => tracing::warn!("解析 Epic 清单失败 {}: {}", path.display(), e),
        }
    }

    sort_by_name(&mut out);
    tracing::info!("Epic 扫描完成：{} 个可导入条目", out.len());
    Ok(out)
}

/// 解析单个 `.item` 清单
fn parse_epic_item(path: &Path) -> Result<Option<PlatformGame>> {
    let text = std::fs::read_to_string(path)?;
    let v: serde_json::Value = serde_json::from_str(&text)?;

    // 过滤：非应用条目（DLC / 引擎 / 素材）与未完成安装（正在下载/更新）
    if v.get("bIsApplication").and_then(|x| x.as_bool()) == Some(false) {
        return Ok(None);
    }
    if v.get("bIsIncompleteInstall").and_then(|x| x.as_bool()) == Some(true) {
        return Ok(None);
    }

    let s = |k: &str| -> String {
        v.get(k)
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string()
    };

    let name = s("DisplayName");
    let install = s("InstallLocation");
    let launch_exe = s("LaunchExecutable");
    let ns = s("CatalogNamespace");
    let item = s("CatalogItemId");
    let app = s("AppName");

    if name.is_empty() || install.is_empty() {
        return Ok(None);
    }
    // 三段 ID 是构造启动 URI 的必需项，缺失则本条无法反向启动
    if ns.is_empty() || item.is_empty() || app.is_empty() {
        tracing::warn!("Epic 条目缺少三段 ID，无法反向启动，跳过: {}", name);
        return Ok(None);
    }

    // exe 仅作展示与"手动改路径"的初值；追踪/截图一律走 install_path 前缀匹配，
    // 因此这里即使是启动器壳（Launcher.exe 之类）也无妨，照实记录即可。
    let (exe_path, exe_name) = if launch_exe.is_empty() {
        (None, None)
    } else {
        let full = Path::new(&install).join(launch_exe.replace('/', "\\"));
        let fname = Path::new(&launch_exe)
            .file_name()
            .map(|f| f.to_string_lossy().to_string());
        (Some(full.to_string_lossy().to_string()), fname)
    };

    Ok(Some(PlatformGame {
        platform: PLATFORM_EPIC.to_string(),
        platform_id: format!("{}:{}:{}", ns, item, app),
        name,
        install_path: install,
        exe_path,
        exe_name,
        size_bytes: v.get("InstallSize").and_then(|x| x.as_u64()),
        version: v
            .get("AppVersionString")
            .and_then(|x| x.as_str())
            .map(|x| x.to_string())
            .filter(|x| !x.is_empty()),
        already_added: false,
        same_name_in_library: false,
    }))
}

// ==================== Steam ====================

/// 从注册表读取 Steam 安装根目录
///
/// 刻意用 `reg query` 子进程而非 windows crate 直连注册表 API：本函数仅在用户
/// 主动扫描时调用一次，几十毫秒成本无感，换来零 API 绑定风险（windows-rs 的
/// 注册表签名琐碎且编译期难以验证，出错即整包编译失败）。
fn steam_root_from_registry() -> Option<PathBuf> {
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim();
        let rest = match line.strip_prefix("SteamPath") {
            Some(r) => r.trim_start(),
            None => continue,
        };
        let val = rest.strip_prefix("REG_SZ").unwrap_or(rest).trim();
        if !val.is_empty() {
            // 注册表里存的是正斜杠形式（c:/program files (x86)/steam），统一回反斜杠
            return Some(PathBuf::from(val.replace('/', "\\")));
        }
    }
    None
}

/// 从 `libraryfolders.vdf` 提取额外库路径
///
/// 只做"提取所有 `"path"` 值"这一件事，不实现完整 KeyValues 解析器——
/// 该文件其余内容（contentid / label / apps 明细）对本功能无用，
/// appid 清单直接靠扫各库的 `appmanifest_*.acf` 获得，更不容易受格式变动影响。
fn steam_library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let vdf = steam_root.join("steamapps").join("libraryfolders.vdf");
    let text = match std::fs::read_to_string(&vdf) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with("\"path\"") {
            continue;
        }
        // 形如：  "path"		"D:\\SteamLibrary"
        let parts: Vec<&str> = line.split('"').collect();
        if let Some(raw) = parts.get(3) {
            // vdf 中反斜杠是转义的（\\），还原成单反斜杠
            let p = raw.replace("\\\\", "\\");
            if !p.is_empty() {
                out.push(PathBuf::from(p));
            }
        }
    }
    out
}

/// 扫描全部 Steam 已安装游戏
pub fn scan_steam() -> Result<Vec<PlatformGame>> {
    let steam_root = match steam_root_from_registry() {
        Some(p) => p,
        None => {
            tracing::info!("未找到 Steam 安装（注册表无 SteamPath），跳过");
            return Ok(Vec::new());
        }
    };

    // 主库 + libraryfolders.vdf 声明的额外库
    let mut libraries = vec![steam_root.clone()];
    libraries.extend(steam_library_folders(&steam_root));

    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for lib in libraries {
        let apps_dir = lib.join("steamapps");
        let entries = match std::fs::read_dir(&apps_dir) {
            Ok(e) => e,
            Err(_) => continue, // 库盘未挂载/无权限 → 静默跳过
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let fname = match path.file_name().and_then(|s| s.to_str()) {
                Some(s) => s,
                None => continue,
            };
            if !fname.starts_with("appmanifest_") || !fname.ends_with(".acf") {
                continue;
            }
            match parse_steam_acf(&path, &lib) {
                Ok(Some(g)) => {
                    // 同一 appid 可能出现在多个库（迁移残留），只取首个
                    if seen.insert(g.platform_id.clone()) {
                        out.push(g);
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::warn!("解析 Steam acf 失败 {}: {}", path.display(), e),
            }
        }
    }

    sort_by_name(&mut out);
    tracing::info!("Steam 扫描完成：{} 个可导入条目", out.len());
    Ok(out)
}

/// 从 KeyValues 文本中取指定键的字符串值（键必须被引号包裹，避免前缀误匹配）
fn kv_value(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    for line in text.lines() {
        let line = line.trim();
        let rest = match line.strip_prefix(&needle) {
            Some(r) => r.trim_start(),
            None => continue,
        };
        if let Some(v) = rest.strip_prefix('"') {
            if let Some(end) = v.find('"') {
                return Some(v[..end].to_string());
            }
        }
    }
    None
}

/// 解析单个 `appmanifest_<appid>.acf`
fn parse_steam_acf(path: &Path, lib_root: &Path) -> Result<Option<PlatformGame>> {
    let text = std::fs::read_to_string(path)?;

    // acf 里没有 exe 名，只有 installdir —— 这正是"必须靠目录前缀匹配"的根因
    let appid = match kv_value(&text, "appid") {
        Some(a) if !a.trim().is_empty() => a.trim().to_string(),
        _ => return Ok(None),
    };
    let name = kv_value(&text, "name").unwrap_or_else(|| appid.clone());
    let installdir = match kv_value(&text, "installdir") {
        Some(d) if !d.trim().is_empty() => d,
        _ => return Ok(None),
    };

    let install_path = lib_root.join("steamapps").join("common").join(&installdir);
    // 目录不存在（已卸载残留 / 库盘未挂载）→ 跳过，避免入库后既不能启动也不能追踪
    if !install_path.is_dir() {
        tracing::debug!("Steam 游戏目录不存在，跳过: {} ({})", name, install_path.display());
        return Ok(None);
    }

    Ok(Some(PlatformGame {
        platform: PLATFORM_STEAM.to_string(),
        platform_id: appid,
        name,
        install_path: install_path.to_string_lossy().to_string(),
        exe_path: None, // acf 不记录 exe 名
        exe_name: None,
        size_bytes: kv_value(&text, "SizeOnDisk").and_then(|s| s.trim().parse::<u64>().ok()),
        version: kv_value(&text, "buildid").filter(|s| !s.is_empty()),
        already_added: false,
        same_name_in_library: false,
    }))
}

/// 按名称忽略大小写排序（中英混排时保持稳定可预期）
fn sort_by_name(list: &mut [PlatformGame]) {
    list.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.platform_id.cmp(&b.platform_id))
    });
}

/// 按平台扫描（命令层入口）
pub fn scan(platform: &str) -> Result<Vec<PlatformGame>> {
    match platform {
        PLATFORM_STEAM => scan_steam(),
        PLATFORM_EPIC => scan_epic(),
        other => anyhow::bail!("未知平台: {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_uri_formats() {
        // Steam：简单前缀
        assert_eq!(
            launch_uri("steam", "550").unwrap(),
            "steam://rungameid/550"
        );

        // Epic：三段 ID 必须以 %3A 连接，且带 action/silent
        let pid = "f2bfff793b224f6190a394f461c9a4b8:a1402f3af7e948ed9ad1cf783e493ba3:BatfishS2";
        assert_eq!(
            launch_uri("epic", pid).unwrap(),
            "com.epicgames.launcher://apps/f2bfff793b224f6190a394f461c9a4b8%3Aa1402f3af7e948ed9ad1cf783e493ba3%3ABatfishS2?action=launch&silent=true"
        );

        // 非法输入必须报错而非拼出坏 URI
        assert!(launch_uri("epic", "only-one-part").is_err());
        assert!(launch_uri("epic", "a::b").is_err());
        assert!(launch_uri("steam", "  ").is_err());
        assert!(launch_uri("gog", "123").is_err());
    }

    #[test]
    fn kv_value_parses_acf_lines() {
        let text = "\"AppState\"\n{\n\t\"appid\"\t\t\"550\"\n\t\"name\"\t\t\"Left 4 Dead 2\"\n\t\"installdir\"\t\t\"Left 4 Dead 2\"\n\t\"buildid\"\t\t\"23990068\"\n}\n";
        assert_eq!(kv_value(text, "appid").as_deref(), Some("550"));
        assert_eq!(kv_value(text, "name").as_deref(), Some("Left 4 Dead 2"));
        assert_eq!(kv_value(text, "installdir").as_deref(), Some("Left 4 Dead 2"));
        assert_eq!(kv_value(text, "buildid").as_deref(), Some("23990068"));
        // 键不存在 → None（不得前缀误匹配 "app" 命中 "appid"）
        assert!(kv_value(text, "app").is_none());
        assert!(kv_value(text, "AppState").is_none());
    }

    #[test]
    fn vdf_path_extraction_unescapes_backslashes() {
        // 模拟 libraryfolders.vdf：反斜杠双重转义
        let vdf = "\"libraryfolders\"\n{\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n\t\"2\"\n\t{\n\t\t\"path\"\t\t\"E:\\\\SteamLibrary\"\n\t}\n}\n";
        let tmp = std::env::temp_dir().join(format!("gv_vdf_test_{}", uuid::Uuid::new_v4()));
        let _ = std::fs::create_dir_all(tmp.join("steamapps"));
        std::fs::write(tmp.join("steamapps").join("libraryfolders.vdf"), vdf).unwrap();

        let libs = steam_library_folders(&tmp);
        assert_eq!(libs.len(), 2, "应提取出 2 个额外库: {:?}", libs);
        assert_eq!(libs[0].to_string_lossy(), "D:\\SteamLibrary");
        assert_eq!(libs[1].to_string_lossy(), "E:\\SteamLibrary");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
