use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use reqwest::Client;
use crate::models::*;
use crate::models::CoverOption;
use crate::utils::constants::*;

/// 封面图获取器（异步版本）
pub struct CoverFetcher {
    cache_dir: PathBuf,
    steamgriddb_api_key: String,
    /// 复用 HTTP 客户端，避免每次请求都创建新的连接池
    client: Client,
}

impl CoverFetcher {
    pub fn new(cache_dir: PathBuf, steamgriddb_api_key: String) -> Result<Self> {
        // 确保缓存目录存在
        std::fs::create_dir_all(&cache_dir).ok();

        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS))
            .build()
            .context("无法创建 HTTP 客户端")?;

        Ok(Self {
            cache_dir,
            steamgriddb_api_key,
            client,
        })
    }

    /// 获取游戏封面（异步）
    pub async fn fetch_cover(&self, game: &Game) -> Result<Option<String>> {
        // 1. 检查缓存（需要验证文件有效性，兼容 jpg/png/webp 扩展名）
        if let Some(cache_path) = self.find_cached_cover(&game.id) {
            return Ok(Some(cache_path.to_string_lossy().to_string()));
        }

        // 2. 尝试从本地游戏目录获取
        if let Some(ref install_path) = game.install_path {
            if let Some(local_cover) = self.find_local_cover(install_path) {
                // 复制到缓存（按源文件扩展名决定目标扩展名，避免 MIME 误判）
                let dest = self.get_cache_path(&game.id);
                let src_ext = local_cover.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e.to_lowercase());
                let final_dest = match src_ext.as_deref() {
                    Some("png") => dest.with_extension("png"),
                    Some("webp") => dest.with_extension("webp"),
                    _ => dest,
                };
                std::fs::copy(&local_cover, &final_dest)?;
                return Ok(Some(final_dest.to_string_lossy().to_string()));
            }
        }

        // 3. 尝试 SteamGridDB（先用游戏名搜索，再用文件夹名搜索）
        if !self.steamgriddb_api_key.is_empty() {
            let cache_path = self.get_cache_path(&game.id);
            // 用游戏名搜索
            match self.search_steamgriddb(&game.name).await {
                Ok(Some(cover_url)) => {
                    if let Ok(actual_path) = self.download_image(&cover_url, &cache_path).await {
                        return Ok(Some(actual_path.to_string_lossy().to_string()));
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
                        match self.search_steamgriddb(folder).await {
                            Ok(Some(cover_url)) => {
                                if let Ok(actual_path) = self.download_image(&cover_url, &cache_path).await {
                                    return Ok(Some(actual_path.to_string_lossy().to_string()));
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

    /// 查找已缓存的封面（兼容 jpg/png/webp 扩展名），文件有效则返回路径
    /// 历史缓存可能存为任意扩展名（早期固定 .jpg），此处统一遍历
    fn find_cached_cover(&self, game_id: &str) -> Option<PathBuf> {
        for ext in ["jpg", "png", "webp"] {
            let p = self.cache_dir.join(format!("{}.{}", game_id, ext));
            if p.exists() {
                if let Ok(metadata) = std::fs::metadata(&p) {
                    if metadata.len() >= COVER_MIN_FILE_SIZE {
                        return Some(p);
                    }
                    // 文件太小，可能是损坏的，删除它继续获取
                    tracing::warn!("缓存文件太小({} bytes)，删除重新获取: {}", metadata.len(), game_id);
                    let _ = std::fs::remove_file(&p);
                }
            }
        }
        None
    }

    /// 获取缓存路径（默认 .jpg；实际保存时按图片内容决定扩展名）
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

    /// 搜索 SteamGridDB（异步）
    async fn search_steamgriddb(&self, game_name: &str) -> Result<Option<String>> {
        // 搜索游戏（URL 编码游戏名，支持中文等非 ASCII 字符）
        let encoded_name = urlencoding::encode(game_name);
        let search_url = format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            encoded_name
        );

        let response = self.client
            .get(&search_url)
            .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            tracing::warn!("SteamGridDB 搜索失败: {} (状态码: {})", game_name, status);
            // 401/403 表示 API Key 无效，返回明确错误
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!("SteamGridDB API Key 无效，请在设置中检查");
            }
            // 429 表示限流（免费层约 1 req/s），提示用户稍后重试而非误报"未找到"
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                anyhow::bail!("SteamGridDB 请求过于频繁（429），请稍后重试");
            }
            return Ok(None);
        }

        let data: serde_json::Value = response.json().await?;
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
                    .send()
                    .await?;

                if grids_response.status().is_success() {
                    let grids_data: serde_json::Value = grids_response.json().await?;
                    if let Some(grids) = grids_data["data"].as_array() {
                        if let Some(first_grid) = grids.first() {
                            // 优先使用原图 url，回退到缩略图 thumb
                            let url = first_grid["url"].as_str().filter(|s| !s.is_empty())
                                .or_else(|| first_grid["thumb"].as_str().filter(|s| !s.is_empty()));
                            if let Some(url) = url {
                                return Ok(Some(url.to_string()));
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

    /// 获取游戏的所有可选封面（异步）
    /// 先用游戏名搜索，如果没有结果再用文件夹名搜索
    pub async fn fetch_cover_options(&self, game_name: &str, install_path: Option<&str>) -> Result<Vec<CoverOption>> {
        // 用游戏名搜索
        let mut grids = self.search_grids_for_game(game_name).await?;

        // 如果没结果，用文件夹名搜索
        if grids.is_empty() {
            if let Some(path) = install_path {
                let folder_name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string());
                if let Some(ref folder) = folder_name {
                    if *folder != game_name {
                        tracing::info!("尝试用文件夹名搜索封面选项: {}", folder);
                        grids = self.search_grids_for_game(folder).await?;
                    }
                }
            }
        }

        Ok(grids)
    }

    /// 搜索 SteamGridDB 获取游戏的所有 grids（内部方法）
    async fn search_grids_for_game(&self, game_name: &str) -> Result<Vec<CoverOption>> {
        let encoded_name = urlencoding::encode(game_name);
        let search_url = format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            encoded_name
        );

        let response = self.client
            .get(&search_url)
            .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!("SteamGridDB API Key 无效，请在设置中检查");
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                anyhow::bail!("SteamGridDB 请求过于频繁（429），请稍后重试");
            }
            return Ok(Vec::new());
        }

        let data: serde_json::Value = response.json().await?;
        let games = match data["data"].as_array() {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let first_game = match games.first() {
            Some(g) => g,
            None => return Ok(Vec::new()),
        };

        let game_id = first_game["id"].as_i64().unwrap_or(0);
        let grids_url = format!(
            "https://www.steamgriddb.com/api/v2/grids/game/{}",
            game_id
        );

        let grids_response = self.client
            .get(&grids_url)
            .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
            .send()
            .await?;

        if !grids_response.status().is_success() {
            return Ok(Vec::new());
        }

        let grids_data: serde_json::Value = grids_response.json().await?;
        let grids = match grids_data["data"].as_array() {
            Some(arr) => arr,
            None => return Ok(Vec::new()),
        };

        let options: Vec<CoverOption> = grids.iter().filter_map(|grid| {
            // 优先用缩略图展示，无缩略图时用原图作为回退
            let thumb_url = grid["thumb"].as_str().filter(|s| !s.is_empty())
                .or_else(|| grid["url"].as_str().filter(|s| !s.is_empty()))
                .unwrap_or("");
            let url = grid["url"].as_str().unwrap_or("");
            if url.is_empty() {
                return None;
            }
            Some(CoverOption {
                thumb_url: thumb_url.to_string(),
                url: url.to_string(),
                width: grid["width"].as_u64().unwrap_or(0) as u32,
                height: grid["height"].as_u64().unwrap_or(0) as u32,
                style: grid["style"].as_str().unwrap_or("unknown").to_string(),
            })
        }).collect();

        Ok(options)
    }

    /// 从 URL 下载图片到指定路径（供外部调用）
    /// 返回实际写入的文件路径（按图片内容决定扩展名）
    pub async fn download_from_url(&self, url: &str, save_path: &Path) -> Result<PathBuf> {
        self.download_image(url, save_path).await
    }

    /// 根据文件头魔数检测图片格式，返回扩展名（jpg/png/webp）
    /// 非图片内容返回 None，用于拒绝将 HTML 错误页等误存为封面
    fn detect_image_ext(bytes: &[u8]) -> Option<&'static str> {
        if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
            Some("jpg")
        } else if bytes.len() >= 8
            && bytes[0] == 0x89 && bytes[1] == b'P' && bytes[2] == b'N' && bytes[3] == b'G'
            && bytes[4] == 0x0D && bytes[5] == 0x0A && bytes[6] == 0x1A && bytes[7] == 0x0A
        {
            Some("png")
        } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
            Some("webp")
        } else {
            None
        }
    }

    /// 下载图片（异步）
    /// 校验内容魔数并按实际格式保存（避免 .jpg 扩展名存 PNG 导致 MIME 误判），
    /// 返回实际写入的文件路径
    async fn download_image(&self, url: &str, save_path: &Path) -> Result<PathBuf> {
        let response = self.client.get(url).send().await?;

        if !response.status().is_success() {
            anyhow::bail!("下载失败: HTTP {}", response.status());
        }

        let bytes = response.bytes().await?;

        // 检查下载的内容是否有效（至少 100 字节）
        if (bytes.len() as u64) < COVER_MIN_FILE_SIZE {
            anyhow::bail!("下载失败: 响应内容太小({} bytes)", bytes.len());
        }

        // 魔数校验：必须是 JPEG/PNG/WebP 之一，拒绝 HTML 错误页等非图片内容
        let ext = Self::detect_image_ext(&bytes)
            .ok_or_else(|| anyhow::anyhow!("下载内容不是有效的图片格式 (JPEG/PNG/WebP)"))?;

        // 若请求路径的扩展名与实际格式不符，改用正确扩展名
        let actual_path = if save_path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case(ext))
            == Some(true)
        {
            save_path.to_path_buf()
        } else {
            save_path.with_extension(ext)
        };

        // 先写入临时文件，成功后再重命名，避免留下损坏的文件
        let temp_path = actual_path.with_extension("tmp");
        std::fs::write(&temp_path, &bytes)?;

        // 验证临时文件大小
        let metadata = std::fs::metadata(&temp_path)?;
        if metadata.len() < COVER_MIN_FILE_SIZE {
            let _ = std::fs::remove_file(&temp_path);
            anyhow::bail!("下载失败: 写入后文件太小({} bytes)", metadata.len());
        }

        // 重命名为正式文件（Windows 上 rename 不覆盖已有文件，先删除旧文件）
        let _ = std::fs::remove_file(&actual_path);
        std::fs::rename(&temp_path, &actual_path)?;

        Ok(actual_path)
    }
}
