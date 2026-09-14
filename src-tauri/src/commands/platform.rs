//! 平台（Steam / Epic）游戏扫描与批量导入命令
//!
//! 两条命令分工：
//! - `scan_platform_games`：只读扫描本机清单，标出"已在库 / 同名"状态供前端勾选；
//! - `import_platform_games`：把勾选结果批量入库，**复用 `insert_game_with_reclaim`**
//!   以共享"删除后重装可续接历史"的墓碑认领语义。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tauri::State;

use super::games::insert_game_with_reclaim;
use super::lock_or_recover;
use crate::core::platform::{self as plat, PlatformGame};
use crate::core::Database;
use crate::models::*;

/// 批量导入结果
#[derive(Debug, Default, serde::Serialize)]
pub struct ImportSummary {
    /// 成功入库数
    pub imported: u32,
    /// 跳过数（安装路径已在库中）
    pub skipped: u32,
    /// 其中走墓碑认领（复用旧 id、续接历史统计）的数量
    pub reclaimed: u32,
    /// 失败数
    pub failed: u32,
    /// 失败原因（供前端提示）
    pub errors: Vec<String>,
}

/// 安装路径归一化：统一小写、统一分隔符、去尾部反斜杠
///
/// 用于"是否已在库"的精确比对。刻意**不做** `canonicalize`——那会触发磁盘 I/O，
/// 且要求路径真实存在（库盘未挂载时反而比对失败）。
fn normalize_install_path(p: &str) -> String {
    let s = p.trim().replace('/', "\\").to_lowercase();
    // 保留盘根形式（"d:\"），其余一律去掉尾部反斜杠
    if s.len() > 3 {
        s.trim_end_matches('\\').to_string()
    } else {
        s
    }
}

/// 扫描平台已安装游戏
///
/// 返回候选列表并标记：
/// - `already_added`：安装路径已在库中（前端应禁止重复勾选）
/// - `same_name_in_library`：库中有同名条目但路径不同（仅提示，不阻止——可能是两份不同安装）
#[tauri::command]
pub fn scan_platform_games(
    db: State<'_, Arc<Mutex<Database>>>,
    platform_name: String,
) -> Result<Vec<PlatformGame>, String> {
    let mut list = plat::scan(&platform_name).map_err(|e| e.to_string())?;

    // 一次性取库内既有路径/名字快照，随后立即释放 DB 锁（扫描是纯读，不占锁）
    let (existing_paths, existing_names) = {
        let db_guard = lock_or_recover(&db);
        let games = db_guard
            .get_games(&GameFilter::default())
            .map_err(|e| e.to_string())?;
        let mut paths = HashSet::new();
        let mut names = HashSet::new();
        for g in &games {
            if let Some(p) = g.install_path.as_deref() {
                if !p.trim().is_empty() {
                    paths.insert(normalize_install_path(p));
                }
            }
            let n = g.name.trim().to_lowercase();
            if !n.is_empty() {
                names.insert(n);
            }
        }
        (paths, names)
    };

    for item in list.iter_mut() {
        item.already_added = existing_paths.contains(&normalize_install_path(&item.install_path));
        item.same_name_in_library = existing_names.contains(&item.name.trim().to_lowercase());
    }

    tracing::info!("平台扫描 [{}]：{} 个候选", platform_name, list.len());
    Ok(list)
}

/// 批量导入平台游戏
///
/// 去重按安装路径归一化精确比对；同名不同路径**允许**导入（可能是两份不同安装）。
/// 入库统一走 `insert_game_with_reclaim`，与手动添加共享墓碑认领语义。
#[tauri::command]
pub fn import_platform_games(
    db: State<'_, Arc<Mutex<Database>>>,
    items: Vec<PlatformGame>,
) -> Result<ImportSummary, String> {
    let db = lock_or_recover(&db);

    // 库内既有安装路径快照：既防同一批内重复，也防与既有条目撞车
    let mut existing_paths: HashSet<String> = {
        let games = db
            .get_games(&GameFilter::default())
            .map_err(|e| e.to_string())?;
        games
            .iter()
            .filter_map(|g| g.install_path.as_deref())
            .filter(|p| !p.trim().is_empty())
            .map(normalize_install_path)
            .collect()
    };

    let mut summary = ImportSummary::default();

    for item in items {
        // 平台取值防御：只接受 steam / epic
        if item.platform != plat::PLATFORM_STEAM && item.platform != plat::PLATFORM_EPIC {
            summary.failed += 1;
            summary
                .errors
                .push(format!("「{}」平台标识非法: {}", item.name, item.platform));
            continue;
        }

        // 安装路径是时长追踪的唯一锚点（Steam 没有 exe 名可依赖），
        // 缺失则入库即废人——既追踪不了，也没法靠目录识别截图。
        if item.install_path.trim().is_empty() {
            summary.failed += 1;
            summary.errors.push(format!(
                "「{}」缺少安装路径，入库后无法追踪时长，已跳过",
                item.name
            ));
            continue;
        }

        if !existing_paths.insert(normalize_install_path(&item.install_path)) {
            summary.skipped += 1;
            continue;
        }

        let mut game = Game::new(item.name.clone());
        game.platform = item.platform.clone();
        game.platform_id = Some(item.platform_id.clone());
        game.install_path = Some(item.install_path.clone());
        game.exe_path = item.exe_path.clone();
        game.exe_name = item.exe_name.clone();
        // 平台侧版本标识（Steam=buildid / Epic=AppVersionString）：
        // 平台游戏读不了 exe 资源，用它顶上"版本"一栏，免得详情页空着
        game.exe_version = item.version.clone();

        let fresh_id = game.id.clone();
        match insert_game_with_reclaim(&db, game) {
            Ok(saved) => {
                // id 被改写 = 命中了墓碑认领（复用了旧 id、历史统计已续接）
                if saved.id != fresh_id {
                    summary.reclaimed += 1;
                }
                summary.imported += 1;
            }
            Err(e) => {
                summary.failed += 1;
                summary.errors.push(format!("「{}」入库失败: {}", item.name, e));
            }
        }
    }

    tracing::info!(
        "平台导入完成：新增 {}，跳过（已在库）{}，认领历史 {}，失败 {}",
        summary.imported,
        summary.skipped,
        summary.reclaimed,
        summary.failed
    );
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_install_path_unifies_separators_and_case() {
        // 大小写 + 分隔符差异必须归一到同一形式，否则去重形同虚设
        assert_eq!(
            normalize_install_path("D:/SteamLibrary/steamapps/common/Left 4 Dead 2"),
            normalize_install_path("d:\\SteamLibrary\\steamapps\\common\\Left 4 Dead 2\\")
        );
        // 尾部反斜杠被抹平
        assert_eq!(normalize_install_path("E:\\Frostpunk\\"), "e:\\frostpunk");
        // 盘根保持原样（不能把 "d:\" 削成 "d:"）
        assert_eq!(normalize_install_path("D:\\"), "d:\\");
        // 前后空白被忽略
        assert_eq!(normalize_install_path("  H:\\SonicMania  "), "h:\\sonicmania");
    }

    #[test]
    fn normalize_install_path_distinguishes_sibling_dirs() {
        // 前缀相近但不是同一目录，必须区分（避免 "Steam"/"SteamLibrary" 式误判）
        assert_ne!(
            normalize_install_path("D:\\Games\\Steam"),
            normalize_install_path("D:\\Games\\SteamLibrary")
        );
    }
}
