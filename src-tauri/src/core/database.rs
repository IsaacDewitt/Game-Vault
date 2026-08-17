use anyhow::Result;
use chrono::Datelike;
use rusqlite::{Connection, params};
use std::path::Path;
use crate::models::*;

/// 将时区偏移（秒）格式化为 SQLite 的 strftime 修饰符
/// 支持非整小时时区（如 UTC+5:30 → "+5.5 hours"，UTC+5:45 → "+5.75 hours"）
fn format_offset_for_sqlite(offset_secs: i32) -> String {
    let offset_minutes = offset_secs / 60;
    // 符号由正负决定，后续用绝对值计算，避免 format 中出现双负号
    let sign = if offset_minutes >= 0 { "+" } else { "-" };
    let abs_minutes = offset_minutes.abs();
    let hours = abs_minutes as f64 / 60.0;
    // 避免浮点精度问题：如果能整除就显示整数
    if abs_minutes % 60 == 0 {
        format!("{}{} hours", sign, abs_minutes / 60)
    } else {
        format!("{}{} hours", sign, hours)
    }
}

/// SQLite 数据库管理
pub struct Database {
    conn: Connection,
}

impl Database {
    /// 创建新的数据库连接
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        // 启用外键约束，确保 ON DELETE CASCADE 等规则生效
        conn.execute("PRAGMA foreign_keys = ON", [])?;
        let db = Self { conn };
        db.init_tables()?;
        Ok(db)
    }

    /// 初始化数据库表
    fn init_tables(&self) -> Result<()> {
        self.conn.execute_batch("
            CREATE TABLE IF NOT EXISTS games (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                install_path TEXT,
                exe_path TEXT,
                exe_name TEXT,
                cover_local TEXT,
                cover_url TEXT,
                description TEXT,
                developer TEXT,
                publisher TEXT,
                release_date TEXT,
                genres TEXT DEFAULT '[]',
                play_time_seconds INTEGER DEFAULT 0,
                last_played TEXT,
                play_count INTEGER DEFAULT 0,
                is_favorite INTEGER DEFAULT 0,
                status TEXT DEFAULT 'unplayed',
                added_at TEXT NOT NULL,
                updated_at TEXT
            );

            CREATE TABLE IF NOT EXISTS play_sessions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                game_id TEXT NOT NULL,
                start_time TEXT NOT NULL,
                end_time TEXT,
                duration_seconds INTEGER NOT NULL,
                FOREIGN KEY (game_id) REFERENCES games(id) ON DELETE CASCADE
            );

            CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS achievement_unlocks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                achievement_id TEXT NOT NULL,
                game_id TEXT NOT NULL DEFAULT '',
                unlocked_at TEXT NOT NULL,
                UNIQUE(achievement_id, game_id)
            );

            -- 插入默认设置
            INSERT OR IGNORE INTO settings (key, value) VALUES ('theme', 'dark');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('language', 'zh-CN');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('auto_scan_on_start', 'true');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('scan_depth', '3');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('steamgriddb_api_key', '');

            -- 索引：提升查询性能
            CREATE INDEX IF NOT EXISTS idx_games_name ON games(name);
            CREATE INDEX IF NOT EXISTS idx_play_sessions_game_id ON play_sessions(game_id);
            CREATE INDEX IF NOT EXISTS idx_play_sessions_start_time ON play_sessions(start_time);
            CREATE INDEX IF NOT EXISTS idx_achievement_unlocks_game ON achievement_unlocks(game_id);
        ")?;

        // 迁移：为旧数据库添加 status 字段（必须在索引创建之前）
        self.migrate_add_status_column()?;

        // 迁移：为旧数据库添加 HLTB 字段
        self.migrate_add_hltb_columns()?;

        // 迁移：为旧数据库添加 save_paths 字段
        self.migrate_add_save_paths_column()?;

        // 迁移：为旧数据库添加 exe_version 字段
        self.migrate_add_exe_version_column()?;

        // 迁移：为旧数据库添加 exe 文件元数据缓存字段
        self.migrate_add_exe_metadata_columns()?;

        // 迁移：为旧数据库添加 abandoned_at 字段（弃坑重玩成就判定用）
        self.migrate_add_abandoned_at_column()?;

        // 创建 status 索引（在列存在之后）
        self.conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_games_status ON games(status);"
        )?;

        Ok(())
    }

    /// 迁移：添加 status 字段到旧数据库
    fn migrate_add_status_column(&self) -> Result<()> {
        if !self.has_column("games", "status")? {
            tracing::info!("status 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN status TEXT DEFAULT 'unplayed'"
            , [])?;
            self.conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_games_status ON games(status)"
            , [])?;
            tracing::info!("已添加 status 字段到 games 表");
        }

        Ok(())
    }

    // ==================== 辅助函数 ====================

    /// 检查表中是否存在指定列
    /// 注意：PRAGMA 不支持参数化查询，table/column 参数必须为内部硬编码值，不可来自用户输入
    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
        debug_assert!(table.chars().all(|c| c.is_alphanumeric() || c == '_'),
            "table name must be alphanumeric: {}", table);
        debug_assert!(column.chars().all(|c| c.is_alphanumeric() || c == '_'),
            "column name must be alphanumeric: {}", column);
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let columns = stmt.query_map([], |row| {
            Ok(row.get::<_, String>(1)?)
        })?;

        for col in columns {
            if let Ok(name) = col {
                if name == column {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// 从数据库行构建 Game 对象
    fn row_to_game(row: &rusqlite::Row) -> rusqlite::Result<Game> {
        // 列顺序必须与 GAME_COLUMNS 完全一致：
        // 0:id 1:name 2:install_path 3:exe_path 4:exe_name 5:exe_version
        // 6:cover_local 7:cover_url 8:description 9:developer 10:publisher
        // 11:release_date 12:genres 13:play_time_seconds 14:last_played
        // 15:play_count 16:is_favorite 17:status 18:added_at 19:updated_at
        // 20:hltb_main_story 21:hltb_main_extra 22:hltb_completionist 23:save_paths
        // 24:exe_modified_at 25:exe_file_size
        let genres_str: String = row.get(12)?;
        let genres: Vec<String> = serde_json::from_str(&genres_str).unwrap_or_default();

        Ok(Game {
            id: row.get(0)?,
            name: row.get(1)?,
            install_path: row.get(2)?,
            exe_path: row.get(3)?,
            exe_name: row.get(4)?,
            exe_version: row.get(5)?,
            cover_local: row.get(6)?,
            cover_url: row.get(7)?,
            description: row.get(8)?,
            developer: row.get(9)?,
            publisher: row.get(10)?,
            release_date: row.get(11)?,
            genres,
            play_time_seconds: row.get::<_, i64>(13).unwrap_or(0).max(0) as u64,
            last_played: row.get(14)?,
            play_count: row.get::<_, i64>(15).unwrap_or(0).max(0) as u32,
            is_favorite: row.get::<_, i64>(16).unwrap_or(0) != 0,
            status: row.get(17).unwrap_or_else(|_| "unplayed".to_string()),
            added_at: row.get(18)?,
            updated_at: row.get(19)?,
            hltb_main_story: row.get::<_, Option<i64>>(20)?.map(|v| v.max(0) as u32),
            hltb_main_extra: row.get::<_, Option<i64>>(21)?.map(|v| v.max(0) as u32),
            hltb_completionist: row.get::<_, Option<i64>>(22)?.map(|v| v.max(0) as u32),
            save_paths: {
                let paths_str: Option<String> = row.get(23)?;
                paths_str
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_default()
            },
            exe_modified_at: row.get(24)?,
            exe_file_size: row.get(25)?,
        })
    }

    const GAME_COLUMNS: &'static str = "
        id, name, install_path, exe_path, exe_name, exe_version,
        cover_local, cover_url, description, developer, publisher, release_date,
        genres, play_time_seconds, last_played, play_count,
        is_favorite, status, added_at, updated_at,
        hltb_main_story, hltb_main_extra, hltb_completionist,
        save_paths, exe_modified_at, exe_file_size
    ";

    // ==================== 游戏 CRUD ====================

    /// 插入或更新游戏
    pub fn upsert_game(&self, game: &Game) -> Result<()> {
        self.conn.execute(
            "INSERT INTO games (
                id, name, install_path, exe_path, exe_name, exe_version,
                cover_local, cover_url, description, developer, publisher, release_date,
                genres, play_time_seconds, last_played, play_count,
                is_favorite, status, added_at, updated_at,
                hltb_main_story, hltb_main_extra, hltb_completionist,
                save_paths, exe_modified_at, exe_file_size
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26
            )
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                install_path = excluded.install_path,
                exe_path = excluded.exe_path,
                exe_name = excluded.exe_name,
                exe_version = excluded.exe_version,
                cover_local = COALESCE(excluded.cover_local, games.cover_local),
                cover_url = COALESCE(excluded.cover_url, games.cover_url),
                description = COALESCE(excluded.description, games.description),
                developer = COALESCE(excluded.developer, games.developer),
                publisher = COALESCE(excluded.publisher, games.publisher),
                release_date = COALESCE(excluded.release_date, games.release_date),
                genres = excluded.genres,
                play_time_seconds = games.play_time_seconds,
                last_played = games.last_played,
                play_count = games.play_count,
                status = excluded.status,
                updated_at = excluded.updated_at,
                hltb_main_story = COALESCE(excluded.hltb_main_story, games.hltb_main_story),
                hltb_main_extra = COALESCE(excluded.hltb_main_extra, games.hltb_main_extra),
                hltb_completionist = COALESCE(excluded.hltb_completionist, games.hltb_completionist),
                save_paths = excluded.save_paths,
                exe_modified_at = excluded.exe_modified_at,
                exe_file_size = excluded.exe_file_size
            ",
            params![
                game.id,
                game.name,
                game.install_path,
                game.exe_path,
                game.exe_name,
                game.exe_version,
                game.cover_local,
                game.cover_url,
                game.description,
                game.developer,
                game.publisher,
                game.release_date,
                serde_json::to_string(&game.genres)?,
                game.play_time_seconds as i64,
                game.last_played,
                game.play_count as i64,
                game.is_favorite as i64,
                game.status,
                game.added_at,
                game.updated_at,
                game.hltb_main_story.map(|v| v as i64),
                game.hltb_main_extra.map(|v| v as i64),
                game.hltb_completionist.map(|v| v as i64),
                serde_json::to_string(&game.save_paths)?,
                game.exe_modified_at,
                game.exe_file_size,
            ],
        )?;
        Ok(())
    }

    /// 更新游戏封面 URL
    pub fn update_game_cover_url(&self, game_id: &str, cover_url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET cover_url = ?1, updated_at = ?2 WHERE id = ?3",
            params![cover_url, chrono::Utc::now().to_rfc3339(), game_id],
        )?;
        Ok(())
    }

    /// 获取所有游戏
    pub fn get_games(&self, filter: &GameFilter) -> Result<Vec<Game>> {
        let mut sql = format!("SELECT {} FROM games WHERE 1=1", Self::GAME_COLUMNS);

        let mut bind_values: Vec<String> = Vec::new();

        if let Some(ref search) = filter.search {
            sql.push_str(&format!(" AND name LIKE ?{}", bind_values.len() + 1));
            bind_values.push(format!("%{}%", search));
        }
        if filter.favorites_only {
            sql.push_str(" AND is_favorite = 1");
        }
        if let Some(ref status) = filter.status {
            if !status.is_empty() {
                sql.push_str(&format!(" AND status = ?{}", bind_values.len() + 1));
                bind_values.push(status.clone());
            }
        }
        if let Some(ref genre) = filter.genre {
            if !genre.is_empty() {
                sql.push_str(&format!(" AND genres LIKE ?{}", bind_values.len() + 1));
                bind_values.push(format!("%{}%", genre));
            }
        }

        // 排序（白名单校验防止注入）
        let sort_column = match filter.sort_by.as_str() {
            "name" => "name",
            "last_played" => "last_played",
            "play_time" => "play_time_seconds",
            "added_at" => "added_at",
            _ => "last_played",
        };
        let sort_order = match filter.sort_order.as_str() {
            "asc" => "ASC",
            _ => "DESC",
        };
        sql.push_str(&format!(" ORDER BY {} {}", sort_column, sort_order));

        let mut stmt = self.conn.prepare(&sql)?;

        let games = if bind_values.is_empty() {
            stmt.query_map([], Self::row_to_game)?
                .collect::<Result<Vec<_>, _>>()?
        } else {
            let params: Vec<&dyn rusqlite::types::ToSql> = bind_values
                .iter()
                .map(|v| v as &dyn rusqlite::types::ToSql)
                .collect();
            stmt.query_map(params.as_slice(), Self::row_to_game)?
                .collect::<Result<Vec<_>, _>>()?
        };

        Ok(games)
    }

    /// 根据 ID 获取游戏
    pub fn get_game_by_id(&self, id: &str) -> Result<Option<Game>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM games WHERE id = ?1",
            Self::GAME_COLUMNS
        ))?;

        let mut games = stmt.query_map(params![id], Self::row_to_game)?;

        match games.next() {
            Some(Ok(game)) => Ok(Some(game)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 根据 exe_path 查找游戏（用于去重）
    pub fn find_game_by_exe_path(&self, exe_path: &str) -> Result<Option<Game>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {} FROM games WHERE exe_path = ?1",
            Self::GAME_COLUMNS
        ))?;

        let mut games = stmt.query_map(params![exe_path], Self::row_to_game)?;

        match games.next() {
            Some(Ok(game)) => Ok(Some(game)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 删除游戏
    pub fn delete_game(&self, id: &str) -> Result<()> {
        self.conn.execute("DELETE FROM games WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// 更新游戏封面（本地文件路径）
    pub fn update_game_cover(&self, id: &str, cover_local: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET cover_local = ?1, updated_at = ?2 WHERE id = ?3",
            params![cover_local, chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// 清除游戏封面（将 cover_url 和 cover_local 设置为 NULL）
    pub fn remove_game_cover(&self, id: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET cover_url = NULL, cover_local = NULL, updated_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(())
    }

    /// 更新游戏信息
    pub fn update_game(&self, game: &Game) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET
                name = ?1, install_path = ?2, exe_path = ?3, exe_name = ?4,
                exe_version = ?5,
                cover_local = ?6, cover_url = ?7, description = ?8,
                developer = ?9, publisher = ?10, release_date = ?11,
                genres = ?12, is_favorite = ?13, status = ?14, updated_at = ?15,
                hltb_main_story = ?16, hltb_main_extra = ?17, hltb_completionist = ?18,
                save_paths = ?19, exe_modified_at = ?20, exe_file_size = ?21
             WHERE id = ?22",
            params![
                game.name,
                game.install_path,
                game.exe_path,
                game.exe_name,
                game.exe_version,
                game.cover_local,
                game.cover_url,
                game.description,
                game.developer,
                game.publisher,
                game.release_date,
                serde_json::to_string(&game.genres)?,
                game.is_favorite as i64,
                game.status,
                chrono::Utc::now().to_rfc3339(),
                game.hltb_main_story.map(|v| v as i64),
                game.hltb_main_extra.map(|v| v as i64),
                game.hltb_completionist.map(|v| v as i64),
                serde_json::to_string(&game.save_paths)?,
                game.exe_modified_at,
                game.exe_file_size,
                game.id,
            ],
        )?;
        Ok(())
    }

    /// 更新游戏状态
    /// 当状态变为 abandoned 时记录 abandoned_at（最近一次弃坑时间），供「弃坑后重玩」成就判定
    pub fn set_game_status(&self, id: &str, status: &str) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        if status == "abandoned" {
            self.conn.execute(
                "UPDATE games SET status = ?1, abandoned_at = ?2, updated_at = ?2 WHERE id = ?3",
                params![status, now, id],
            )?;
        } else {
            self.conn.execute(
                "UPDATE games SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status, now, id],
            )?;
        }
        Ok(())
    }

    /// 切换收藏状态
    pub fn toggle_favorite(&self, id: &str) -> Result<bool> {
        let current: i64 = self.conn.query_row(
            "SELECT is_favorite FROM games WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        let new_value = if current == 0 { 1 } else { 0 };
        self.conn.execute(
            "UPDATE games SET is_favorite = ?1, updated_at = ?2 WHERE id = ?3",
            params![new_value, chrono::Utc::now().to_rfc3339(), id],
        )?;
        Ok(new_value != 0)
    }

    // ==================== 游戏会话 ====================

    /// 记录游戏会话（使用事务保证原子性）
    pub fn add_play_session(&self, game_id: &str, start_time: &str, duration_seconds: u64) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let end_time = chrono::DateTime::parse_from_rfc3339(start_time)
            .ok()
            .map(|start| (start + chrono::Duration::seconds(duration_seconds as i64)).to_rfc3339())
            .unwrap_or_else(|| now.clone());

        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO play_sessions (game_id, start_time, end_time, duration_seconds) VALUES (?1, ?2, ?3, ?4)",
            params![game_id, start_time, end_time, duration_seconds as i64],
        )?;
        tx.execute(
            "UPDATE games SET play_time_seconds = play_time_seconds + ?1, play_count = play_count + 1, last_played = ?2, updated_at = ?2 WHERE id = ?3",
            params![duration_seconds as i64, now, game_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 批量记录游戏会话（单个事务内完成，多个游戏同时退出时减少事务开销）
    pub fn add_play_sessions_batch(
        &self,
        sessions: &[(String, String, u64)], // (game_id, start_time, duration_seconds)
    ) -> Result<usize> {
        if sessions.is_empty() {
            return Ok(0);
        }
        let now = chrono::Utc::now().to_rfc3339();
        let tx = self.conn.unchecked_transaction()?;
        let mut saved = 0usize;
        for (game_id, start_time, duration_seconds) in sessions {
            let end_time = chrono::DateTime::parse_from_rfc3339(start_time)
                .ok()
                .map(|start| (start + chrono::Duration::seconds(*duration_seconds as i64)).to_rfc3339())
                .unwrap_or_else(|| now.clone());
            tx.execute(
                "INSERT INTO play_sessions (game_id, start_time, end_time, duration_seconds) VALUES (?1, ?2, ?3, ?4)",
                params![game_id, start_time, end_time, *duration_seconds as i64],
            )?;
            tx.execute(
                "UPDATE games SET play_time_seconds = play_time_seconds + ?1, play_count = play_count + 1, last_played = ?2, updated_at = ?2 WHERE id = ?3",
                params![*duration_seconds as i64, now, game_id],
            )?;
            saved += 1;
        }
        tx.commit()?;
        Ok(saved)
    }

    /// 获取游戏时长排行榜
    pub fn get_play_stats(&self, limit: u32) -> Result<Vec<GamePlayStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, play_time_seconds, play_count, last_played
             FROM games WHERE play_time_seconds > 0
             ORDER BY play_time_seconds DESC LIMIT ?1"
        )?;

        let stats = stmt.query_map(params![limit], |row| {
            Ok(GamePlayStats {
                game_id: row.get(0)?,
                game_name: row.get(1)?,
                total_seconds: row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
                play_count: row.get::<_, i64>(3).unwrap_or(0).max(0) as u32,
                last_played: row.get(4)?,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(stats)
    }

    /// 获取每日游玩统计（补零：无游玩记录的日期也返回 total_seconds=0，保证折线图连续）
    pub fn get_daily_stats(&self, days: u32) -> Result<Vec<DailyStats>> {
        // 获取本地时区偏移（秒），支持非整小时时区（如 UTC+5:30）
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);

        let sql = format!(
            "SELECT DATE(start_time, '{}') as date, SUM(duration_seconds) as total, COUNT(*) as sessions
             FROM play_sessions
             WHERE start_time >= DATE('now', '{}', '-' || ?1 || ' days')
             GROUP BY DATE(start_time, '{}')
             ORDER BY date DESC",
            offset_str, offset_str, offset_str
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut stats = stmt.query_map(params![days], |row| {
            Ok(DailyStats {
                date: row.get(0)?,
                total_seconds: row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
                sessions_count: row.get::<_, i64>(2).unwrap_or(0).max(0) as u32,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        // 补零：生成完整日期序列（本地时区的今天往前 days 天）
        let today_local = chrono::Local::now().date_naive();
        let start_date = today_local - chrono::Days::new(days.saturating_sub(1).max(1) as u64);
        let mut by_date: std::collections::HashMap<String, DailyStats> = stats
            .drain(..)
            .map(|s| (s.date.clone(), s))
            .collect();

        let mut full: Vec<DailyStats> = Vec::with_capacity(days as usize);
        let mut cursor = start_date;
        while cursor <= today_local {
            let key = cursor.format("%Y-%m-%d").to_string();
            let entry = by_date.remove(&key).unwrap_or(DailyStats {
                date: key.clone(),
                total_seconds: 0,
                sessions_count: 0,
            });
            full.push(entry);
            cursor = cursor.checked_add_days(chrono::Days::new(1)).unwrap_or(cursor);
        }
        full.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(full)
    }

    /// 获取本月游玩时长（秒），直接按本地时区的年月聚合
    pub fn get_monthly_play_time(&self) -> Result<u64> {
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);
        let month_prefix = chrono::Local::now().format("%Y-%m").to_string();

        let sql = format!(
            "SELECT COALESCE(SUM(duration_seconds),0) FROM play_sessions \
             WHERE strftime('%Y-%m', start_time, '{}') = ?1",
            offset_str
        );
        let total: i64 = self.conn.query_row(&sql, params![month_prefix], |r| r.get(0))?;
        Ok(total.max(0) as u64)
    }

    // ==================== 设置 ====================

    /// 获取设置值
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self.conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| {
            Ok(row.get::<_, String>(0)?)
        })?;

        match rows.next() {
            Some(Ok(value)) => Ok(Some(value)),
            Some(Err(e)) => Err(e.into()),
            None => Ok(None),
        }
    }

    /// 设置值
    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    /// 获取游戏总数
    pub fn get_game_count(&self) -> Result<u32> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM games",
            [],
            |row| row.get(0),
        )?;
        Ok(count as u32)
    }

    /// 获取总游玩时长
    pub fn get_total_play_time(&self) -> Result<u64> {
        let total: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(play_time_seconds), 0) FROM games",
            [],
            |row| row.get(0),
        )?;
        Ok(total as u64)
    }

    /// 获取游戏类型统计
    pub fn get_genre_stats(&self) -> Result<Vec<GenreStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT genres, play_time_seconds FROM games"
        )?;

        let mut genre_map: std::collections::HashMap<String, (u64, u32)> = std::collections::HashMap::new();

        let rows = stmt.query_map([], |row| {
            let genres_str: String = row.get(0)?;
            let play_time: i64 = row.get(1)?;
            Ok((genres_str, play_time.max(0) as u64))
        })?;

        for row in rows {
            let (genres_str, play_time) = row?;
            let genres: Vec<String> = serde_json::from_str(&genres_str).unwrap_or_default();
            for genre in genres {
                let entry = genre_map.entry(genre).or_insert((0, 0));
                entry.0 += play_time;
                entry.1 += 1;
            }
        }

        let mut stats: Vec<GenreStats> = genre_map
            .into_iter()
            .map(|(genre, (total_seconds, game_count))| GenreStats {
                genre,
                total_seconds,
                game_count,
            })
            .collect();

        stats.sort_by(|a, b| b.total_seconds.cmp(&a.total_seconds));
        Ok(stats)
    }

    /// 获取热力图数据（按日期聚合游玩时长）
    pub fn get_heatmap_stats(&self, days: u32) -> Result<Vec<HeatmapDay>> {
        // 获取本地时区偏移（秒），支持非整小时时区
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);

        let sql = format!(
            "SELECT DATE(start_time, '{}') as date, SUM(duration_seconds) as total
             FROM play_sessions
             WHERE start_time >= DATE('now', '{}', '-' || ?1 || ' days')
             GROUP BY DATE(start_time, '{}')
             ORDER BY date",
            offset_str, offset_str, offset_str
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let stats = stmt.query_map(params![days], |row| {
            Ok(HeatmapDay {
                date: row.get(0)?,
                total_seconds: row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(stats)
    }

    /// 获取游玩时段分布（24小时 x 7天）
    pub fn get_hourly_stats(&self) -> Result<Vec<HourlyStats>> {
        // 获取本地时区偏移（秒），支持非整小时时区
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);

        let sql = format!(
            "SELECT
                CAST(strftime('%H', start_time, '{}') AS INTEGER) as hour,
                CAST(strftime('%w', start_time, '{}') AS INTEGER) as weekday,
                SUM(duration_seconds) as total
             FROM play_sessions
             GROUP BY hour, weekday
             ORDER BY weekday, hour",
            offset_str, offset_str
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let stats = stmt.query_map([], |row| {
            let weekday_raw: u32 = row.get(1)?;
            // SQLite strftime('%w'): 0=Sunday, 1=Monday, ..., 6=Saturday
            // 转换为: 1=Monday, ..., 7=Sunday
            let weekday = if weekday_raw == 0 { 7 } else { weekday_raw };

            Ok(HourlyStats {
                hour: row.get(0)?,
                weekday,
                total_seconds: row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(stats)
    }

    /// 获取游戏状态统计（智能推导：有游玩时长但状态仍为 unplayed 的游戏视为 playing）
    pub fn get_status_stats(&self) -> Result<StatusStats> {
        let mut stmt = self.conn.prepare(
            "SELECT
                CASE
                    WHEN play_time_seconds = 0 THEN 'unplayed'
                    WHEN status = 'unplayed' AND play_time_seconds > 0 THEN 'playing'
                    ELSE status
                END as effective_status,
                COUNT(*)
            FROM games GROUP BY effective_status"
        )?;

        let mut stats = StatusStats {
            unplayed: 0,
            playing: 0,
            completed: 0,
            abandoned: 0,
        };

        let rows = stmt.query_map([], |row| {
            let status: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((status, count as u32))
        })?;

        for row in rows {
            let (status, count) = row?;
            match status.as_str() {
                "unplayed" => stats.unplayed = count,
                "playing" => stats.playing = count,
                "completed" => stats.completed = count,
                "abandoned" => stats.abandoned = count,
                _ => {}
            }
        }

        Ok(stats)
    }

    /// 获取所有游戏类型（去重）
    pub fn get_all_genres(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT genres FROM games")?;
        let rows = stmt.query_map([], |row| {
            Ok(row.get::<_, String>(0)?)
        })?;

        let mut genre_set = std::collections::HashSet::new();
        for row in rows {
            let genres_str = row?;
            let genres: Vec<String> = serde_json::from_str(&genres_str).unwrap_or_default();
            for genre in genres {
                if !genre.is_empty() {
                    genre_set.insert(genre);
                }
            }
        }

        let mut genres: Vec<String> = genre_set.into_iter().collect();
        genres.sort();
        Ok(genres)
    }

    /// 获取游玩会话详情（联表查询，含游戏名）
    pub fn get_play_sessions(
        &self,
        game_id: Option<&str>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<PlaySessionDetail>> {
        let mut sql = String::from(
            "SELECT ps.id, ps.game_id, g.name, ps.start_time, ps.end_time, ps.duration_seconds
             FROM play_sessions ps
             JOIN games g ON ps.game_id = g.id
             WHERE 1=1"
        );

        let mut bind_values: Vec<String> = Vec::new();

        if let Some(gid) = game_id {
            if !gid.is_empty() {
                sql.push_str(&format!(" AND ps.game_id = ?{}", bind_values.len() + 1));
                bind_values.push(gid.to_string());
            }
        }

        let limit_val = limit as i64;
        let offset_val = offset as i64;

        sql.push_str(" ORDER BY ps.start_time DESC");
        sql.push_str(&format!(" LIMIT ?{} OFFSET ?{}", bind_values.len() + 1, bind_values.len() + 2));

        let mut stmt = self.conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::types::ToSql> = bind_values
            .iter()
            .map(|v| v as &dyn rusqlite::types::ToSql)
            .collect();
        params.push(&limit_val);
        params.push(&offset_val);

        let sessions = stmt.query_map(params.as_slice(), |row| {
            Ok(PlaySessionDetail {
                id: row.get(0)?,
                game_id: row.get(1)?,
                game_name: row.get(2)?,
                start_time: row.get(3)?,
                end_time: row.get(4)?,
                duration_seconds: row.get::<_, i64>(5).unwrap_or(0).max(0) as u64,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(sessions)
    }

    // ==================== 成就系统 ====================

    /// 尝试解锁成就（INSERT OR IGNORE），返回是否为新解锁
    /// 全局成就 game_id 传空字符串，单游戏成就传游戏 ID
    pub fn try_unlock_achievement(&self, achievement_id: &str, game_id: &str, unlocked_at: &str) -> Result<bool> {
        let changed = self.conn.execute(
            "INSERT OR IGNORE INTO achievement_unlocks (achievement_id, game_id, unlocked_at) VALUES (?1, ?2, ?3)",
            params![achievement_id, game_id, unlocked_at],
        )?;
        Ok(changed > 0)
    }

    /// 获取全部成就解锁记录
    pub fn get_achievement_unlocks(&self) -> Result<Vec<AchievementUnlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT achievement_id, game_id, unlocked_at FROM achievement_unlocks"
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(AchievementUnlock {
                achievement_id: row.get(0)?,
                game_id: row.get(1)?,
                unlocked_at: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// LLM 补全元数据计数（成就 G-13 用）
    pub fn get_llm_filled_count(&self) -> Result<u64> {
        let value = self.get_setting("llm_filled_count")?.unwrap_or_default();
        Ok(value.parse().unwrap_or(0))
    }

    /// LLM 补全计数 +1
    pub fn increment_llm_filled_count(&self) -> Result<()> {
        let count = self.get_llm_filled_count()? + 1;
        self.set_setting("llm_filled_count", &count.to_string())
    }

    /// 计算日期列表中的最长连续天数（日期须为升序去重的 "%Y-%m-%d" 列表）
    fn longest_streak(dates: &[&str]) -> u64 {
        if dates.is_empty() {
            return 0;
        }
        let mut max_streak = 1u64;
        let mut current = 1u64;
        for window in dates.windows(2) {
            let prev = chrono::NaiveDate::parse_from_str(window[0], "%Y-%m-%d").ok();
            let next = chrono::NaiveDate::parse_from_str(window[1], "%Y-%m-%d").ok();
            if let (Some(p), Some(n)) = (prev, next) {
                if n.signed_duration_since(p).num_days() == 1 {
                    current += 1;
                    max_streak = max_streak.max(current);
                } else {
                    current = 1;
                }
            }
        }
        max_streak
    }

    /// 聚合全局成就检测所需的统计
    pub fn get_achievement_global_stats(&self) -> Result<AchievementGlobalStats> {
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);
        let mut s = AchievementGlobalStats::default();

        s.game_count = self.conn.query_row("SELECT COUNT(*) FROM games", [], |r| r.get::<_, i64>(0))? as u64;
        s.total_play_time = self.conn.query_row(
            "SELECT COALESCE(SUM(play_time_seconds),0) FROM games", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.total_sessions = self.conn.query_row("SELECT COUNT(*) FROM play_sessions", [], |r| r.get::<_, i64>(0))? as u64;
        s.completed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE status='completed'", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.favorite_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE is_favorite=1", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.cover_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE cover_local IS NOT NULL OR cover_url IS NOT NULL",
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.save_paths_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE save_paths IS NOT NULL AND save_paths != '' AND save_paths != '[]'",
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.llm_filled_count = self.get_llm_filled_count()?;
        s.max_session_duration = self.conn.query_row(
            "SELECT COALESCE(MAX(duration_seconds),0) FROM play_sessions", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.night_session = self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM play_sessions WHERE CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 0 AND 4 LIMIT 1)", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? != 0;
        s.dawn_session = self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM play_sessions WHERE CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 5 AND 7 LIMIT 1)", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? != 0;
        s.weekend_total = self.conn.query_row(
            &format!("SELECT COALESCE(SUM(duration_seconds),0) FROM play_sessions WHERE CAST(strftime('%w', start_time, '{}') AS INTEGER) IN (0,6)", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.hltb_completed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE hltb_completionist IS NOT NULL AND play_time_seconds >= hltb_completionist * 60",
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        // 弃坑后重玩判定：以 abandoned_at（最近一次弃坑时间）为准，
        // 不能用 updated_at——它会被任何无关操作刷新，且会话落库时与 last_played 同时写入导致恒等
        s.abandoned_replayed = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE status='abandoned' AND last_played IS NOT NULL AND abandoned_at IS NOT NULL AND last_played > abandoned_at",
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.max_distinct_games_per_day = self.conn.query_row(
            &format!("SELECT COALESCE(MAX(cnt),0) FROM (SELECT COUNT(DISTINCT game_id) cnt FROM play_sessions GROUP BY DATE(start_time, '{}'))", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? as u64;

        // ===== 第二批成就新增统计 =====
        s.unplayed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE play_time_seconds = 0", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.played_under_1h_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE play_time_seconds > 0 AND play_time_seconds < 3600", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.over_main_not_completed_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE hltb_main_story IS NOT NULL AND play_time_seconds >= hltb_main_story * 60 AND status != 'completed'", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.total_exe_size_gb = self.conn.query_row(
            "SELECT COALESCE(SUM(exe_file_size),0) FROM games", [], |r| r.get::<_, i64>(0),
        )? as u64 / (1024 * 1024 * 1024);
        s.games_over_100h_count = self.conn.query_row(
            "SELECT COUNT(*) FROM games WHERE play_time_seconds >= 360000", [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.max_day_seconds = self.conn.query_row(
            &format!("SELECT COALESCE(MAX(total),0) FROM (SELECT SUM(duration_seconds) total FROM play_sessions GROUP BY DATE(start_time, '{}'))", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.max_weekend_total = self.conn.query_row(
            &format!("SELECT COALESCE(MAX(total),0) FROM (SELECT SUM(duration_seconds) total FROM play_sessions WHERE CAST(strftime('%w', start_time, '{}') AS INTEGER) IN (0,6) GROUP BY DATE(start_time, '{}'))", offset_str, offset_str),
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.max_distinct_games_per_week = self.conn.query_row(
            &format!("SELECT COALESCE(MAX(cnt),0) FROM (SELECT COUNT(DISTINCT game_id) cnt FROM play_sessions GROUP BY strftime('%Y-%W', start_time, '{}'))", offset_str),
            [], |r| r.get::<_, i64>(0),
        )? as u64;
        s.day_night_same_day = self.conn.query_row(
            &format!(
                "SELECT EXISTS(SELECT 1 FROM (\
                 SELECT DATE(start_time, '{}') d, \
                 MAX(CASE WHEN CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 0 AND 4 THEN 1 ELSE 0 END) night, \
                 MAX(CASE WHEN CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 9 AND 17 THEN 1 ELSE 0 END) day \
                 FROM play_sessions GROUP BY d) WHERE night = 1 AND day = 1)",
                offset_str, offset_str, offset_str
            ),
            [], |r| r.get::<_, i64>(0),
        )? != 0;
        s.has_full_week = self.conn.query_row(
            &format!("SELECT EXISTS(SELECT 1 FROM (SELECT strftime('%Y-%W', start_time, '{}') wk, COUNT(DISTINCT DATE(start_time, '{}')) dcnt FROM play_sessions GROUP BY wk) WHERE dcnt >= 7)", offset_str, offset_str),
            [], |r| r.get::<_, i64>(0),
        )? != 0;

        // 老游戏计数（release_date 取年份，≤ 当前年份 - 20）
        {
            let current_year = chrono::Utc::now().year();
            let mut old_count = 0u64;
            let mut stmt = self.conn.prepare("SELECT release_date FROM games WHERE release_date IS NOT NULL")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                if let Ok(rd) = row {
                    if let Some(y) = rd.split('-').next().and_then(|s| s.trim().parse::<i32>().ok()) {
                        if y > 0 && y <= current_year - 20 {
                            old_count += 1;
                        }
                    }
                }
            }
            s.old_game_count = old_count;
        }

        // 去重游戏类型数
        {
            let mut genre_set = std::collections::HashSet::new();
            let mut stmt = self.conn.prepare("SELECT genres FROM games")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                if let Ok(g) = row {
                    let genres: Vec<String> = serde_json::from_str(&g).unwrap_or_default();
                    for genre in genres {
                        if !genre.is_empty() {
                            genre_set.insert(genre);
                        }
                    }
                }
            }
            s.distinct_genre_count = genre_set.len() as u64;
        }

        // 同一开发商最多游戏数
        {
            let mut dev_map: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
            let mut stmt = self.conn.prepare("SELECT developer FROM games WHERE developer IS NOT NULL AND developer != ''")?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                if let Ok(d) = row {
                    *dev_map.entry(d).or_insert(0) += 1;
                }
            }
            s.max_dev_count = dev_map.values().copied().max().unwrap_or(0);
        }

        // 入库超 1 年才首次启动的游戏数（首玩日期 - 入库日期 ≥ 365 天）
        {
            let mut late = 0u64;
            let mut stmt = self.conn.prepare(
                "SELECT g.added_at, MIN(ps.start_time) FROM games g \
                 JOIN play_sessions ps ON ps.game_id = g.id WHERE g.play_count > 0 GROUP BY g.id",
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            for row in rows {
                if let Ok((added, first)) = row {
                    if let (Ok(a), Ok(f)) = (
                        chrono::DateTime::parse_from_rfc3339(&added),
                        chrono::DateTime::parse_from_rfc3339(&first),
                    ) {
                        if (f - a).num_days() >= 365 {
                            late += 1;
                        }
                    }
                }
            }
            s.late_bloomer_count = late;
        }

        // 连续凌晨（0–5 点）游玩天数
        {
            let mut night_dates: Vec<String> = Vec::new();
            let mut stmt = self.conn.prepare(
                &format!("SELECT DISTINCT DATE(start_time, '{}') FROM play_sessions WHERE CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 0 AND 4", offset_str, offset_str),
            )?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                if let Ok(d) = row {
                    night_dates.push(d);
                }
            }
            night_dates.sort();
            let refs: Vec<&str> = night_dates.iter().map(|s| s.as_str()).collect();
            s.night_streak = Self::longest_streak(&refs);
        }

        // 某游戏间隔 ≥ 180 天后重玩
        {
            let mut long_gap = false;
            let mut stmt = self.conn.prepare(
                &format!("SELECT game_id, DATE(start_time, '{}') d FROM play_sessions GROUP BY game_id, d", offset_str),
            )?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
            let mut per_game: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
            for row in rows {
                if let Ok((gid, d)) = row {
                    per_game.entry(gid).or_default().push(d);
                }
            }
            'outer: for (_, mut dates) in per_game {
                dates.sort();
                for w in dates.windows(2) {
                    if let (Ok(a), Ok(b)) = (
                        chrono::NaiveDate::parse_from_str(&w[0], "%Y-%m-%d"),
                        chrono::NaiveDate::parse_from_str(&w[1], "%Y-%m-%d"),
                    ) {
                        if (b - a).num_days() >= 180 {
                            long_gap = true;
                            break 'outer;
                        }
                    }
                }
            }
            s.long_gap_replay = long_gap;
        }

        // 库龄（首款入库）与距最后一次游玩的天数
        {
            let now = chrono::Utc::now();
            let first_add: Option<String> = self.conn.query_row(
                "SELECT MIN(added_at) FROM games", [], |r| r.get(0),
            )?;
            s.days_since_first_add = first_add
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok())
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_days().max(0) as u64)
                .unwrap_or(0);
            let last_play: Option<String> = self.conn.query_row(
                "SELECT MAX(start_time) FROM play_sessions", [], |r| r.get(0),
            )?;
            s.days_since_last_play = last_play
                .and_then(|v| chrono::DateTime::parse_from_rfc3339(&v).ok())
                .map(|t| (now - t.with_timezone(&chrono::Utc)).num_days().max(0) as u64)
                .unwrap_or(0);
        }

        // 全局最长连续游玩天数（所有游戏日期的并集）
        let mut dates: Vec<String> = Vec::new();
        let mut stmt = self.conn.prepare(
            &format!("SELECT DISTINCT DATE(start_time, '{}') FROM play_sessions", offset_str),
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            if let Ok(d) = row {
                dates.push(d);
            }
        }
        dates.sort();
        let refs: Vec<&str> = dates.iter().map(|s| s.as_str()).collect();
        s.longest_streak = Self::longest_streak(&refs);

        Ok(s)
    }

    /// 聚合每个游戏的成就检测统计
    pub fn get_per_game_achievement_stats(&self) -> Result<Vec<PerGameStats>> {
        let local_offset = chrono::Local::now().offset().local_minus_utc();
        let offset_str = format_offset_for_sqlite(local_offset);

        // 1. 游戏基本信息
        let mut stats_map: std::collections::HashMap<String, PerGameStats> = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT id, name, status, play_time_seconds, play_count, hltb_main_story, hltb_completionist, abandoned_at, last_played FROM games",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3).unwrap_or(0).max(0) as u64,
                    row.get::<_, i64>(4).unwrap_or(0).max(0) as u64,
                    row.get::<_, Option<i64>>(5)?.map(|v| v.max(0) as u64),
                    row.get::<_, Option<i64>>(6)?.map(|v| v.max(0) as u64),
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })?;
            for row in rows {
                let (id, name, status, play_time, play_count, hltb_main, hltb_comp, abandoned_at, last_played) = row?;
                // RFC3339 同格式字符串可直接比较（弃坑后是否有新游玩）
                // 以 abandoned_at（最近一次弃坑时间）为准；不能用 updated_at（会被无关操作刷新）
                let replayed = status == "abandoned"
                    && last_played.is_some()
                    && abandoned_at.is_some()
                    && last_played.as_deref() > abandoned_at.as_deref();
                stats_map.insert(id.clone(), PerGameStats {
                    game_id: id,
                    game_name: name,
                    status,
                    play_time_seconds: play_time,
                    play_count,
                    hltb_main_story: hltb_main,
                    hltb_completionist: hltb_comp,
                    replayed_after_abandon: replayed,
                    ..Default::default()
                });
            }
        }

        // 2. 会话聚合：启动次数、单次最长
        {
            let mut stmt = self.conn.prepare(
                "SELECT game_id, COUNT(*), COALESCE(MAX(duration_seconds),0) FROM play_sessions GROUP BY game_id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
                    row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
                ))
            })?;
            for row in rows {
                let (gid, count, max_dur) = row?;
                if let Some(stats) = stats_map.get_mut(&gid) {
                    stats.sessions_count = count;
                    stats.max_session_duration = max_dur;
                }
            }
        }

        // 3. 每日聚合：不同游玩日期、单日最长、最长连续
        {
            let sql = format!(
                "SELECT game_id, DATE(start_time, '{}') as day, SUM(duration_seconds) as total \
                 FROM play_sessions GROUP BY game_id, day",
                offset_str
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2).unwrap_or(0).max(0) as u64,
                ))
            })?;
            let mut day_map: std::collections::HashMap<String, Vec<(String, u64)>> = std::collections::HashMap::new();
            for row in rows {
                let (gid, day, total) = row?;
                day_map.entry(gid).or_default().push((day, total));
            }
            for (gid, mut days) in day_map {
                if let Some(stats) = stats_map.get_mut(&gid) {
                    days.sort();
                    stats.distinct_days = days.len() as u64;
                    stats.max_day_seconds = days.iter().map(|(_, t)| *t).max().unwrap_or(0);
                    let refs: Vec<&str> = days.iter().map(|(d, _)| d.as_str()).collect();
                    stats.longest_streak = Self::longest_streak(&refs);
                }
            }
        }

        // 4. 凌晨会话（0:00-4:59）游戏集合
        {
            let sql = format!(
                "SELECT DISTINCT game_id FROM play_sessions WHERE CAST(strftime('%H', start_time, '{}') AS INTEGER) BETWEEN 0 AND 4",
                offset_str
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
            for row in rows {
                if let Ok(gid) = row {
                    if let Some(stats) = stats_map.get_mut(&gid) {
                        stats.night_session = true;
                    }
                }
            }
        }

        let mut result: Vec<PerGameStats> = stats_map.into_values().collect();
        result.sort_by(|a, b| a.game_id.cmp(&b.game_id));
        Ok(result)
    }

    /// 迁移：添加 HLTB 字段到旧数据库
    fn migrate_add_hltb_columns(&self) -> Result<()> {
        let columns_to_add = [
            ("hltb_main_story", "INTEGER"),
            ("hltb_main_extra", "INTEGER"),
            ("hltb_completionist", "INTEGER"),
        ];

        for (col_name, col_type) in &columns_to_add {
            if !self.has_column("games", col_name)? {
                tracing::info!("{} 字段不存在，正在添加...", col_name);
                self.conn.execute(
                    &format!("ALTER TABLE games ADD COLUMN {} {}", col_name, col_type),
                    [],
                )?;
                tracing::info!("已添加 {} 字段到 games 表", col_name);
            }
        }

        Ok(())
    }

    /// 迁移：添加 save_paths 字段到旧数据库
    fn migrate_add_save_paths_column(&self) -> Result<()> {
        if !self.has_column("games", "save_paths")? {
            tracing::info!("save_paths 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN save_paths TEXT DEFAULT '[]'",
                [],
            )?;
            tracing::info!("已添加 save_paths 字段到 games 表");
        }

        Ok(())
    }

    /// 迁移：添加 exe_version 字段到旧数据库
    fn migrate_add_exe_version_column(&self) -> Result<()> {
        if !self.has_column("games", "exe_version")? {
            tracing::info!("exe_version 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN exe_version TEXT",
                [],
            )?;
            tracing::info!("已添加 exe_version 字段到 games 表");
        }

        Ok(())
    }

    /// 迁移：添加 abandoned_at 字段到旧数据库
    /// 记录最近一次被标记为「弃坑」的时间，用于判断弃坑后是否重新游玩
    fn migrate_add_abandoned_at_column(&self) -> Result<()> {
        if !self.has_column("games", "abandoned_at")? {
            tracing::info!("abandoned_at 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN abandoned_at TEXT",
                [],
            )?;
            tracing::info!("已添加 abandoned_at 字段到 games 表");
        }
        Ok(())
    }

    /// 迁移：添加 exe 文件元数据缓存字段到旧数据库
    fn migrate_add_exe_metadata_columns(&self) -> Result<()> {
        if !self.has_column("games", "exe_modified_at")? {
            tracing::info!("exe_modified_at 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN exe_modified_at INTEGER",
                [],
            )?;
            tracing::info!("已添加 exe_modified_at 字段到 games 表");
        }

        if !self.has_column("games", "exe_file_size")? {
            tracing::info!("exe_file_size 字段不存在，正在添加...");
            self.conn.execute(
                "ALTER TABLE games ADD COLUMN exe_file_size INTEGER",
                [],
            )?;
            tracing::info!("已添加 exe_file_size 字段到 games 表");
        }

        Ok(())
    }
}
