use anyhow::Result;
use std::path::{Path, PathBuf};
use reqwest::blocking::Client;
use crate::models::*;

/// 封面图获取器
pub struct CoverFetcher {
    cache_dir: PathBuf,
    steamgriddb_api_key: String,
    /// 复用 HTTP 客户端，避免每次请求都创建新的连接池
    client: Client,
}

impl CoverFetcher {
    pub fn new(cache_dir: PathBuf, steamgriddb_api_key: String) -> Self {
        // 确保缓存目录存在
        std::fs::create_dir_all(&cache_dir).ok();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("无法创建 HTTP 客户端");

        Self {
            cache_dir,
            steamgriddb_api_key,
            client,
        }
    }

    /// 获取游戏封面
    pub fn fetch_cover(&self, game: &Game) -> Result<Option<String>> {
        // 1. 检查缓存（需要验证文件有效性）
        let cache_path = self.get_cache_path(&game.id);
        if cache_path.exists() {
            // 检查文件大小，如果小于 100 字节，认为是无效的缓存文件
            if let Ok(metadata) = std::fs::metadata(&cache_path) {
                if metadata.len() >= 100 {
                    return Ok(Some(cache_path.to_string_lossy().to_string()));
                }
                // 文件太小，可能是损坏的，删除它继续获取
                tracing::warn!("缓存文件太小({} bytes)，删除重新获取: {}", metadata.len(), game.name);
                let _ = std::fs::remove_file(&cache_path);
            }
        }

        // 2. 尝试从本地游戏目录获取
        if let Some(ref install_path) = game.install_path {
            if let Some(local_cover) = self.find_local_cover(install_path) {
                // 复制到缓存
                std::fs::copy(&local_cover, &cache_path)?;
                return Ok(Some(cache_path.to_string_lossy().to_string()));
            }
        }

        // 3. 尝试 SteamGridDB（先用游戏名搜索，再用文件夹名搜索）
        if !self.steamgriddb_api_key.is_empty() {
            // 用游戏名搜索
            match self.search_steamgriddb(&game.name) {
                Ok(Some(cover_url)) => {
                    if self.download_image(&cover_url, &cache_path).is_ok() {
                        return Ok(Some(cache_path.to_string_lossy().to_string()));
                    }
                }
                Err(e) => return Err(e), // API Key 错误等，直接上抛
                _ => {}
            }

            // 用文件夹名作为备用搜索词
            if let Some(ref install_path) = game.install_path {
                let folder_name = std::path::Path::new(install_path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string());

                if let Some(ref folder) = folder_name {
                    if *folder != game.name {
                        tracing::info!("尝试用文件夹名搜索封面: {}", folder);
                        match self.search_steamgriddb(folder) {
                            Ok(Some(cover_url)) => {
                                if self.download_image(&cover_url, &cache_path).is_ok() {
                                    return Ok(Some(cache_path.to_string_lossy().to_string()));
                                }
                            }
                            Err(e) => return Err(e),
                            _ => {}
                        }
                    }
                }
            }
        }

        // 4. 返回 None（使用默认封面）
        Ok(None)
    }

    /// 获取缓存路径
    fn get_cache_path(&self, game_id: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.jpg", game_id))
    }

    /// 从本地游戏目录查找封面
    fn find_local_cover(&self, install_path: &str) -> Option<PathBuf> {
        let path = Path::new(install_path);
        if !path.exists() {
            return None;
        }

        let cover_names = [
            "cover.jpg", "cover.png", "folder.jpg", "folder.png",
            "poster.jpg", "poster.png", "thumbnail.jpg", "thumbnail.png",
            "header.jpg", "header.png", "banner.jpg", "banner.png",
            "logo.png", "logo.jpg",
        ];

        for name in &cover_names {
            let cover_path = path.join(name);
            if cover_path.exists() {
                return Some(cover_path);
            }
        }

        // 查找任何 jpg/png 文件
        for entry in std::fs::read_dir(path).ok()? {
            let entry = entry.ok()?;
            let file_path = entry.path();
            if file_path.extension().map_or(false, |e| {
                e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("png")
            }) {
                // 检查文件名是否包含关键词
                let file_name = file_path.file_stem().unwrap().to_string_lossy().to_lowercase();
                if file_name.contains("cover") || file_name.contains("header") ||
                   file_name.contains("poster") || file_name.contains("banner") {
                    return Some(file_path);
                }
            }
        }

        None
    }

    /// 搜索 SteamGridDB
    fn search_steamgriddb(&self, game_name: &str) -> Result<Option<String>> {
        // 搜索游戏（URL 编码游戏名，支持中文等非 ASCII 字符）
        let encoded_name = urlencoding::encode(game_name);
        let search_url = format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            encoded_name
        );

        let response = self.client
            .get(&search_url)
            .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
            .send()?;

        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!("SteamGridDB 搜索失败: {} (状态码: {})", game_name, status);
            // 401/403 表示 API Key 无效，返回明确错误
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!("SteamGridDB API Key 无效，请在设置中检查");
            }
            return Ok(None);
        }

        let data: serde_json::Value = response.json()?;
        let games = data["data"].as_array();

        if let Some(games) = games {
            if let Some(first_game) = games.first() {
                tracing::info!("SteamGridDB 搜索到游戏: {} (ID: {})", game_name, first_game["id"]);
                let game_id = first_game["id"].as_i64().unwrap_or(0);

                // 获取封面
                let grids_url = format!(
                    "https://www.steamgriddb.com/api/v2/grids/game/{}",
                    game_id
                );

                let grids_response = self.client
                    .get(&grids_url)
                    .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
                    .send()?;

                if grids_response.status().is_success() {
                    let grids_data: serde_json::Value = grids_response.json()?;
                    if let Some(grids) = grids_data["data"].as_array() {
                        if let Some(first_grid) = grids.first() {
                            let thumb_url = first_grid["thumb"].as_str().unwrap_or_default();
                            if !thumb_url.is_empty() {
                                return Ok(Some(thumb_url.to_string()));
                            }
                        }
                    }
                }
            } else {
                tracing::warn!("SteamGridDB 未搜索到游戏: {}", game_name);
            }
        } else {
            tracing::warn!("SteamGridDB 响应格式异常: {}", game_name);
        }

        Ok(None)
    }

    /// 下载图片
    fn download_image(&self, url: &str, save_path: &Path) -> Result<()> {
        let response = self.client.get(url).send()?;

        if !response.status().is_success() {
            anyhow::bail!("下载失败: HTTP {}", response.status());
        }

        let bytes = response.bytes()?;

        // 检查下载的内容是否有效（至少 100 字节）
        if bytes.len() < 100 {
            anyhow::bail!("下载失败: 响应内容太小({} bytes)", bytes.len());
        }

        // 先写入临时文件，成功后再重命名，避免留下损坏的文件
        let temp_path = save_path.with_extension("jpg.tmp");
        std::fs::write(&temp_path, &bytes)?;

        // 验证临时文件大小
        let metadata = std::fs::metadata(&temp_path)?;
        if metadata.len() < 100 {
            let _ = std::fs::remove_file(&temp_path);
            anyhow::bail!("下载失败: 写入后文件太小({} bytes)", metadata.len());
        }

        // 重命名为正式文件
        std::fs::rename(&temp_path, save_path)?;

        Ok(())
    }

    /// 批量获取封面
    pub fn fetch_covers_batch(&self, games: &[Game]) -> Vec<(String, Option<String>)> {
        let mut results = Vec::new();

        for game in games {
            match self.fetch_cover(game) {
                Ok(cover_path) => results.push((game.id.clone(), cover_path)),
                Err(e) => {
                    tracing::warn!("获取封面失败 {}: {}", game.name, e);
                    results.push((game.id.clone(), None));
                }
            }
        }

        results
    }
}
