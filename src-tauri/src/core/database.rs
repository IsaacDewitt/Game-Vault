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
        ")?;

        // 迁移：为旧数据库添加 status 字段（必须在索引创建之前）
        self.migrate_add_status_column()?;

        // 迁移：为旧数据库添加 HLTB 字段
        self.migrate_add_hltb_columns()?;

        // 迁移：为旧数据库添加 save_paths 字段
        self.migrate_add_save_paths_column()?;

        // 迁移：为旧数据库添加 exe_version 字段
        self.migrate_add_exe_version_column()?;

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
    fn has_column(&self, table: &str, column: &str) -> Result<bool> {
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
        })
    }

    const GAME_COLUMNS: &'static str = "
        id, name, install_path, exe_path, exe_name, exe_version,
        cover_local, cover_url, description, developer, publisher, release_date,
        genres, play_time_seconds, last_played, play_count,
        is_favorite, status, added_at, updated_at,
        hltb_main_story, hltb_main_extra, hltb_completionist,
        save_paths
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
                save_paths
            ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24
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
                save_paths = excluded.save_paths
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
                save_paths = ?19
             WHERE id = ?20",
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
                game.id,
            ],
        )?;
        Ok(())
    }

    /// 更新游戏状态
    pub fn set_game_status(&self, id: &str, status: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE games SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, chrono::Utc::now().to_rfc3339(), id],
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

    /// 记录游戏会话（使用事务保证原子性）
    pub fn add_play_session(&self, game_id: &str, start_time: &str, duration_seconds: u64) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        let now = chrono::Utc::now().to_rfc3339();

        // 从 start_time + duration_seconds 计算真实的结束时间
        let end_time = chrono::DateTime::parse_from_rfc3339(start_time)
            .ok()
            .map(|start| (start + chrono::Duration::seconds(duration_seconds as i64)).to_rfc3339())
            .unwrap_or_else(|| now.clone());

        tx.execute(
            "INSERT INTO play_sessions (game_id, start_time, end_time, duration_seconds) VALUES (?1, ?2, ?3, ?4)",
            params![game_id, start_time, end_time, duration_seconds as i64],
        )?;

        // 更新游戏总时长和启动次数
        tx.execute(
            "UPDATE games SET play_time_seconds = play_time_seconds + ?1, play_count = play_count + 1, last_played = ?2, updated_at = ?2 WHERE id = ?3",
            params![duration_seconds as i64, now, game_id],
        )?;

        tx.commit()?;
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
        let mut stmt = self.conn.prepare(
            "SELECT DATE(start_time) as date, SUM(duration_seconds) as total
             FROM play_sessions
             WHERE start_time >= DATE('now', '-' || ?1 || ' days')
             GROUP BY DATE(start_time)
             ORDER BY date"
        )?;

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
        let mut stmt = self.conn.prepare(
            "SELECT
                CAST(strftime('%H', start_time) AS INTEGER) as hour,
                CAST(strftime('%w', start_time) AS INTEGER) as weekday,
                SUM(duration_seconds) as total
             FROM play_sessions
             GROUP BY hour, weekday
             ORDER BY weekday, hour"
        )?;

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

    /// 获取游戏状态统计
    pub fn get_status_stats(&self) -> Result<StatusStats> {
        let mut stmt = self.conn.prepare(
            "SELECT status, COUNT(*) FROM games GROUP BY status"
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
}
