use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::Path;
use crate::models::*;

/// SQLite 数据库管理
pub struct Database {
    conn: Connection,
}

impl Database {
    /// 创建新的数据库连接
    pub fn new(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
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

            -- 插入默认设置
            INSERT OR IGNORE INTO settings (key, value) VALUES ('theme', 'dark');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('language', 'zh-CN');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('auto_scan_on_start', 'true');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('scan_depth', '3');
            INSERT OR IGNORE INTO settings (key, value) VALUES ('steamgriddb_api_key', '');
        ")?;
        Ok(())
    }

    // ==================== 辅助函数 ====================

    /// 从数据库行构建 Game 对象
    fn row_to_game(row: &rusqlite::Row) -> rusqlite::Result<Game> {
        let genres_str: String = row.get(11)?;
        let genres: Vec<String> = serde_json::from_str(&genres_str).unwrap_or_default();

        Ok(Game {
            id: row.get(0)?,
            name: row.get(1)?,
            install_path: row.get(2)?,
            exe_path: row.get(3)?,
            exe_name: row.get(4)?,
            cover_local: row.get(5)?,
            cover_url: row.get(6)?,
            description: row.get(7)?,
            developer: row.get(8)?,
            publisher: row.get(9)?,
            release_date: row.get(10)?,
            genres,
            play_time_seconds: row.get::<_, i64>(12).unwrap_or(0).max(0) as u64,
            last_played: row.get(13)?,
            play_count: row.get::<_, i64>(14).unwrap_or(0).max(0) as u32,
            is_favorite: row.get::<_, i64>(15).unwrap_or(0) != 0,
            added_at: row.get(16)?,
            updated_at: row.get(17)?,
        })
    }

    const GAME_COLUMNS: &'static str = "
        id, name, install_path, exe_path, exe_name,
        cover_local, cover_url, description, developer, publisher, release_date,
        genres, play_time_seconds, last_played, play_count,
        is_favorite, added_at, updated_at
    ";

    // ==================== 游戏 CRUD ====================

    /// 插入或更新游戏
    pub fn upsert_game(&self, game: &Game) -> Result<()> {
        self.conn.execute(
            "INSERT INTO games (
                id, name, install_path, exe_path, exe_name,
                cover_local, cover_url, description, developer, publisher, release_date,
                genres, play_time_seconds, last_played, play_count,
                is_favorite, added_at, updated_at
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
            )
            ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                install_path = excluded.install_path,
                exe_path = excluded.exe_path,
                exe_name = excluded.exe_name,
                cover_local = COALESCE(excluded.cover_local, games.cover_local),
                cover_url = COALESCE(excluded.cover_url, games.cover_url),
                description = COALESCE(excluded.description, games.description),
                developer = COALESCE(excluded.developer, games.developer),
                publisher = COALESCE(excluded.publisher, games.publisher),
                release_date = COALESCE(excluded.release_date, games.release_date),
                genres = excluded.genres,
                updated_at = excluded.updated_at
            ",
            params![
                game.id,
                game.name,
                game.install_path,
                game.exe_path,
                game.exe_name,
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
                game.added_at,
                game.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 更新游戏封面 URL
    pub fn update_game_cover_url(&self, game_id: &str, cover_url: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET cover_url = ?1, updated_at = datetime('now') WHERE id = ?2",
            params![cover_url, game_id],
        )?;
        Ok(())
    }

    /// 获取所有游戏
    pub fn get_games(&self, filter: &GameFilter) -> Result<Vec<Game>> {
        let mut sql = String::from(
            "SELECT id, name, install_path, exe_path, exe_name,
                    cover_local, cover_url, description, developer, publisher, release_date,
                    genres, play_time_seconds, last_played, play_count,
                    is_favorite, added_at, updated_at
             FROM games WHERE 1=1"
        );

        let mut bind_values: Vec<String> = Vec::new();

        if let Some(ref search) = filter.search {
            sql.push_str(&format!(" AND name LIKE ?{}", bind_values.len() + 1));
            bind_values.push(format!("%{}%", search));
        }
        if filter.favorites_only {
            sql.push_str(" AND is_favorite = 1");
        }

        // 排序
        let sort_column = match filter.sort_by.as_str() {
            "name" => "name",
            "last_played" => "last_played",
            "play_time" => "play_time_seconds",
            "added_at" => "added_at",
            _ => "last_played",
        };
        let sort_order = if filter.sort_order == "asc" { "ASC" } else { "DESC" };
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

    /// 更新游戏信息
    pub fn update_game(&self, game: &Game) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET
                name = ?1, install_path = ?2, exe_path = ?3, exe_name = ?4,
                cover_local = ?5, cover_url = ?6, description = ?7,
                developer = ?8, publisher = ?9, release_date = ?10,
                genres = ?11, is_favorite = ?12, updated_at = ?13
             WHERE id = ?14",
            params![
                game.name,
                game.install_path,
                game.exe_path,
                game.exe_name,
                game.cover_local,
                game.cover_url,
                game.description,
                game.developer,
                game.publisher,
                game.release_date,
                serde_json::to_string(&game.genres)?,
                game.is_favorite as i64,
                chrono::Utc::now().to_rfc3339(),
                game.id,
            ],
        )?;
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

    /// 记录游戏会话
    pub fn add_play_session(&self, game_id: &str, start_time: &str, duration_seconds: u64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO play_sessions (game_id, start_time, end_time, duration_seconds) VALUES (?1, ?2, ?3, ?4)",
            params![game_id, start_time, chrono::Utc::now().to_rfc3339(), duration_seconds as i64],
        )?;

        // 更新游戏总时长和启动次数
        self.conn.execute(
            "UPDATE games SET play_time_seconds = play_time_seconds + ?1, play_count = play_count + 1, last_played = ?2, updated_at = ?2 WHERE id = ?3",
            params![duration_seconds as i64, chrono::Utc::now().to_rfc3339(), game_id],
        )?;

        Ok(())
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

    /// 获取每日游玩统计
    pub fn get_daily_stats(&self, days: u32) -> Result<Vec<DailyStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT DATE(start_time) as date, SUM(duration_seconds) as total, COUNT(*) as sessions
             FROM play_sessions
             WHERE start_time >= DATE('now', '-' || ?1 || ' days')
             GROUP BY DATE(start_time)
             ORDER BY date DESC"
        )?;

        let stats = stmt.query_map(params![days], |row| {
            Ok(DailyStats {
                date: row.get(0)?,
                total_seconds: row.get::<_, i64>(1).unwrap_or(0).max(0) as u64,
                sessions_count: row.get::<_, i64>(2).unwrap_or(0).max(0) as u32,
            })
        })?.collect::<Result<Vec<_>, _>>()?;

        Ok(stats)
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
}
