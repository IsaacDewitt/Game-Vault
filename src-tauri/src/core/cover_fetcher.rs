use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use reqwest::Client;
use crate::models::*;
use crate::models::CoverOption;
use crate::utils::constants::*;
use crate::utils::net::{budget_timeout, fetch_bytes_with_retry};

/// 封面图获取器（异步版本）
pub struct CoverFetcher {
    cache_dir: PathBuf,
    steamgriddb_api_key: String,
    /// 复用 HTTP 客户端，避免每次请求都创建新的连接池（用于 SteamGridDB API 调用）
    client: Client,
    /// 图片下载专用客户端：重定向策略比 API 客户端更严（见 `new` 内注释）
    download_client: Client,
}

/// 为 SteamGridDB 挑选检索词。
///
/// SteamGridDB 只收录**官方英文名**：中文名（「全面战争：三国」）必然 0 命中
/// （2026-10-03 实测）。故名字含非 ASCII 字符时优先用 `name_en`；`name_en` 为空
/// 则退回原名 —— 这正是「没启用 LLM 也能照常搜图」的兜底（搜得到与否取决于
/// 原名本身，不会因为缺英文名而更差）。纯 ASCII 名直接用 `name`（`name_en`
/// 通常与它相同，用哪个都一样，选 `name` 少一次比较）。
fn pick_search_term<'a>(name: &'a str, name_en: Option<&'a str>) -> &'a str {
    if name.is_ascii() {
        return name;
    }
    match name_en {
        Some(en) if !en.trim().is_empty() => en,
        _ => name,
    }
}

/// SteamGridDB 图源域名白名单（子域一并放行）
///
/// `steamstatic.com` 是 Valve 官方图床（走 Akamai 边缘）。把它纳入白名单是
/// 2026-10-18 那轮改动的一部分：Steam 游戏的封面优先从官方图床按 appid 直取，
/// 彻底绕开 SteamGridDB 所在的 Cloudflare 链路 —— 实测官方图床 5/5 连通、
/// 单张 0.8 秒，而 Cloudflare 那条路有约 20% 的 TLS 握手卡死。
/// 这里的 URL 全部由 `steam_library_cover_url` 用**纯数字 appid** 拼出，
/// 不接受外部输入，故放行无 SSRF 风险。
const TRUSTED_COVER_HOSTS: &[&str] = &["steamgriddb.com", "steamstatic.com"];

/// Steam 官方图床主机（Akamai 边缘，2026-10-18 实测 5/5 连通、TLS 握手 ~450ms）
const STEAM_CDN_HOST: &str = "cdn.cloudflare.steamstatic.com";

/// URL 是否落在受信任的图源域名下（含子域，大小写不敏感）
fn is_trusted_cover_host(url: &reqwest::Url) -> bool {
    url.host_str()
        .map(|h| h.to_ascii_lowercase())
        .map(|h| {
            TRUSTED_COVER_HOSTS
                .iter()
                .any(|t| h == *t || h.ends_with(&format!(".{t}")))
        })
        .unwrap_or(false)
}

/// 由 Steam appid 拼出官方竖版库封面地址（600×900，比例与网格卡一致）。
///
/// 只接受**纯数字** appid：这个值会拼进 URL 路径，放行任何非数字字符
/// 都等于把路径控制权交给数据源。超过 12 位也拒绝（Steam appid 是 u32）。
fn steam_library_cover_url(appid: &str) -> Option<String> {
    let appid = appid.trim();
    if appid.is_empty() || appid.len() > 12 || !appid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "https://{STEAM_CDN_HOST}/steam/apps/{appid}/library_600x900.jpg"
    ))
}

/// 该游戏能否走 Steam 官方图床
///
/// 两条来源：
/// - Steam 平台条目：直接用 `platform_id`（就是 appid）；
/// - 本地/repack 条目：从安装目录的 appid 线索文件反查（见 `detect_local_steam_appid`）。
///
/// 后者是 2026-10-18 加的：老爷库里 32 个本地条目有 12 个带 `steam_emu.ini` /
/// `steam_appid.txt`，值都是真 appid（对马岛 2215430、战神诸神黄昏 2322010…）。
/// 认出来就能走 Akamai 官方图床，一次 SGDB 都不用打。
pub(crate) fn steam_cover_url_for(game: &Game) -> Option<String> {
    if game.platform.eq_ignore_ascii_case("steam") {
        return steam_library_cover_url(game.platform_id.as_deref()?);
    }
    let install_path = game.install_path.as_deref()?;
    steam_library_cover_url(&detect_local_steam_appid(install_path)?)
}

/// 可能藏着 appid 的文件名（按可信度排序，先找到先用）
const APPID_HINT_FILES: &[&str] = &[
    "steam_appid.txt",
    // Goldberg 新版布局把配置收进 steam_settings/ 子目录：
    // 老爷的《使命召唤：现代战争》appid=2000950 就只在这里，根目录没有
    "steam_settings/steam_appid.txt",
    "steam_emu.ini",
    "codex.ini",
    "flt.ini",
    "coldclientloader.ini",
];

/// Spacewar：各类模拟器模板的默认占位 appid。认它会给出**错误**封面
/// （满库游戏都变成同一张测试图），比认不出来更糟。
const PLACEHOLDER_APPID: u32 = 480;

/// 从安装目录反查 Steam appid（本地/repack 条目专用）
fn detect_local_steam_appid(install_path: &str) -> Option<String> {
    let dir = Path::new(install_path);
    if !dir.is_dir() {
        return None;
    }
    for name in APPID_HINT_FILES {
        let path = dir.join(name);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        // 只读小文件：这类 ini 都是几百字节，遇到异常大文件直接跳过
        if !meta.is_file() || meta.len() > 64 * 1024 {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);

        let found = if name.to_ascii_lowercase().ends_with(".txt") {
            // 这类 .txt 的唯一用途就是放一个裸 appid
            parse_appid(text.lines().next().unwrap_or(""))
        } else {
            // ini 形式：找 `AppId = <数字>`（大小写不敏感）
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                if key.trim().eq_ignore_ascii_case("appid") {
                    parse_appid(value.trim())
                } else {
                    None
                }
            })
        };

        if found.is_some() {
            return found;
        }
    }
    None
}

/// 校验并规范化一个 appid：纯数字、非 0、非占位值
fn parse_appid(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 10 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u32 = raw.parse().ok()?;
    if n == 0 || n == PLACEHOLDER_APPID {
        return None;
    }
    Some(n.to_string())
}

/// 从 Steam 商店搜索（storesearch）响应里挑一个 appid。
///
/// 响应里除真正的游戏（`type == "app"`）外还会混进 bundle / sub 之类的包体，
/// 拿它们的 id 拼图床只会 404，故先按 type 过滤。
///
/// 选谁：优先「忽略大小写与首尾空格后完全相等」的名字（"Hollow Knight" 不该
/// 被 "Hollow Knight: Silksong" 顶掉），否则取第一个 app ——
/// 与 SteamGridDB 那条链路「取相关性最高的一条」口径一致。
fn pick_appid_from_store_search(data: &serde_json::Value, term: &str) -> Option<String> {
    let items = data["items"].as_array()?;
    let wanted = term.trim().to_lowercase();
    let mut first: Option<String> = None;

    for app in items.iter().filter(|it| it["type"].as_str() == Some("app")) {
        let Some(id) = app["id"].as_u64() else {
            continue;
        };
        let id = id.to_string();
        if let Some(name) = app["name"].as_str() {
            if name.trim().to_lowercase() == wanted {
                return Some(id);
            }
        }
        if first.is_none() {
            first = Some(id);
        }
    }
    first
}

/// 抓取失败的成因分类。
///
/// 批量抓封面时靠它决定「跳过这一款继续」还是「整批收工」——旧实现只有「成功 / 失败」
/// 两种结局，一个 429 或一次写盘失败就会把剩下的几十款全部放弃，还统一报「网络异常」，
/// 把用户引向错误的方向（2026-10-28 审查指出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverFailureKind {
    /// 网络/接口整体不可用（建连全断、读体全断）：后面的游戏大概率同样失败
    Transport,
    /// 限流 429：歇一会儿换下一款就好，不该整批中止
    RateLimited,
    /// API Key 无效 / 401 / 403：所有游戏都会失败，立刻收工
    Unauthorized,
    /// 其它业务性失败（响应解析、落盘等）：只影响这一条
    Other,
}

impl CoverFailureKind {
    // 【2026-11-08】删掉旧的 `aborts_batch()`：整批中止与否现在由
    // commands/games.rs 按 `failure.kind` 分支决定（Unauthorized/Transport
    // 才 break，RateLimited 退避续跑），这里留一个没人调的判据只会让
    // 两处口径各自漂移。

    fn priority(self) -> u8 {
        match self {
            Self::Unauthorized => 0,
            Self::Transport => 1,
            Self::RateLimited => 2,
            Self::Other => 3,
        }
    }
}

/// `fetch_cover` 的失败体：既带成因分类，也带给人看的说明。
#[derive(Debug)]
pub struct CoverFailure {
    pub kind: CoverFailureKind,
    pub message: String,
}

impl std::fmt::Display for CoverFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// 一次 SteamGridDB 检索的结果。
///
/// 刻意不是 `Option`：旧实现把「网络失败」「限流」「没收录」全部揉成一个
/// `Ok(None)`，上层只能统一报「游戏名可能不在 SteamGridDB 中」——
/// 把网络故障说成"库里没有"，是排查时最大的误导（2026-10-18 老爷报
/// 「拉图片老有问题」时，日志里只留下这一句，真正的原因一个字节都没留下）。
enum SgdbSearch {
    /// 命中：按优先级排好的候选图片 URL（最多 `COVER_DOWNLOAD_CANDIDATES` 个）
    Hit(Vec<String>),
    /// 确定未收录（搜索或 grids 返回空数组）
    NotFound,
    /// 失败。`fatal` 表示换检索词也没用；`kind` 供批量抓取决定是否整批收工
    Failed {
        reason: String,
        fatal: bool,
        kind: CoverFailureKind,
    },
}

/// 传输层失败的标记包装。
///
/// `download_image` 靠它区分「值得重试」（建连失败、读体断流）与
/// 「重试也没用」（404、非图片、超限、写盘失败）。用标记类型而非错误字符串
/// 判断，是因为文案随时会改，而类型不会说谎。
#[derive(Debug)]
struct TransportFailure(anyhow::Error);

impl std::fmt::Display for TransportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TransportFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// 为目标文件生成一个**唯一命名**的同目录临时路径。
///
/// 放在同目录（而非系统临时目录）是必须的：随后的 `rename` 要落在同一卷上，
/// 跨卷 rename 会直接失败。名字里带进程内唯一随机串，是为了让并发写同一目标的
/// 两次下载各有各的落地文件，不会互相覆盖或误删。
pub(crate) fn unique_temp_path(actual_path: &Path) -> PathBuf {
    let stem = actual_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "cover".to_string());
    let unique = uuid::Uuid::new_v4().simple().to_string();
    actual_path.with_file_name(format!(".{stem}.{unique}.tmp"))
}

impl CoverFetcher {
    pub fn new(cache_dir: PathBuf, steamgriddb_api_key: String) -> Result<Self> {
        // 确保缓存目录存在
        std::fs::create_dir_all(&cache_dir).ok();

        // 系统代理：reqwest 只认环境变量，不认 WinINET —— Clash 开着系统代理时
        // 本应用却仍在直连那条被干扰的链路（2026-10-18 实测确认）。这里补上。
        // `system_proxy_url` 已确认「开关为 1 且端口真能连上」，否则返回 None。
        let proxy = crate::utils::net::system_proxy_url();
        if let Some(ref p) = proxy {
            tracing::info!("封面抓取走系统代理: {p}");
        }
        let proxy_of = || -> Option<reqwest::Proxy> {
            let p = proxy.as_ref()?;
            match reqwest::Proxy::all(p) {
                Ok(px) => Some(px),
                Err(e) => {
                    tracing::warn!("系统代理地址不可用（{p}）: {e}，改用直连");
                    None
                }
            }
        };

        let mut api_builder = Client::builder()
            // 三个超时各管一段，缺一不可（2026-10-18）：
            //   connect_timeout 管 TCP+TLS 握手卡死 —— 快速失败才能尽快重试；
            //   read_timeout    管"两次收数据之间"的最长停顿 —— 慢但不断的响应不被误杀；
            //   timeout         管整次请求的兜底上限。
            .connect_timeout(std::time::Duration::from_secs(COVER_CONNECT_TIMEOUT_SECS))
            .read_timeout(std::time::Duration::from_secs(COVER_READ_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS));

        // 下载客户端单独一份。图源 URL 有两个来源：SteamGridDB 的**第三方响应**，
        // 以及用户在设置页**手动粘贴**的地址 —— 都不是本应用能控制的内容。
        // reqwest 默认最多跟随 10 跳且允许跨主机，图源一旦用 302 把请求引向
        // 内网/本机端点，就是一次 SSRF。这里收紧到 3 跳（正常图床 0~1 跳），
        // 并配合 `download_image` 的 scheme 校验、域名白名单与体积上限三层防线。
        // API 客户端保持默认策略：SteamGridDB 的 API 端点可能合法依赖重定向。
        let mut dl_builder = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(COVER_CONNECT_TIMEOUT_SECS))
            .read_timeout(std::time::Duration::from_secs(COVER_READ_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(COVER_DOWNLOAD_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(3));

        if let Some(px) = proxy_of() {
            api_builder = api_builder.proxy(px.clone());
            dl_builder = dl_builder.proxy(px);
        }

        let client = api_builder.build().context("无法创建 HTTP 客户端")?;
        let download_client = dl_builder
            .build()
            .context("无法创建图片下载 HTTP 客户端")?;

        Ok(Self {
            cache_dir,
            steamgriddb_api_key,
            client,
            download_client,
        })
    }

    /// 获取游戏封面（异步）
    pub async fn fetch_cover(
        &self,
        game: &Game,
    ) -> std::result::Result<Option<String>, CoverFailure> {
        // 整次抓取的硬预算：所有请求的 timeout 都会被压到「剩余预算」以内，
        // 保证不会出现"点了刷新一直转圈"（2026-10-18 实测 3.5 分钟那次）。
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(COVER_TOTAL_BUDGET_SECS);

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
                std::fs::copy(&local_cover, &final_dest).map_err(|e| CoverFailure {
                    kind: CoverFailureKind::Other,
                    message: format!("复制本地封面失败: {e}"),
                })?;
                return Ok(Some(final_dest.to_string_lossy().to_string()));
            }
        }

        // 本轮遇到过的失败（按成因分类）。收尾时靠它区分「真没收录」与
        // 「网挂了 / 限流 / 找到了但下不下来」—— 这三类旧实现全被谎报成「未收录」。
        let mut failures: Vec<(CoverFailureKind, String)> = Vec::new();

        // 3. Steam 官方图床（按 appid 精确直取，无需 API Key）
        //
        //    这是 2026-10-18 "治本"改动的核心：本机到 SteamGridDB 所在的
        //    Cloudflare 边缘有约 20% 的 TLS 握手卡死，而 Valve 官方图床走
        //    Akamai（实测 15 个 Steam 条目命中 14 个、每张 600~800ms）。
        //    能绕开那条烂路就绕开 —— 顺带还消灭了"autocomplete 取第一条搜错游戏"
        //    这个老问题（appid 是精确标识，不会认错）。
        //    注：老爷库里 Steam 条目约 28%（15/53），其余 local/Epic 仍要走 SGDB，
        //    故这条是分流，不是万能药。
        if let Some(url) = steam_cover_url_for(game) {
            let cache_path = self.get_cache_path(&game.id);
            match self.download_image(&url, &cache_path, true, deadline).await {
                Ok(actual_path) => {
                    tracing::info!("使用 Steam 官方封面: {}", game.name);
                    return Ok(Some(actual_path.to_string_lossy().to_string()));
                }
                // 没这张图（老游戏/DLC/工具类常见 404）或网络失败：都记下来再走 SGDB。
                // 以前这里只打日志，于是「没配 Key + Steam 图床又抓不到」时报不出真实原因。
                Err(e) => {
                    let kind = failure_kind_of(&e);
                    tracing::warn!("Steam 官方封面不可用（{url}）: {e}，回退 SteamGridDB");
                    failures.push((kind, format!("Steam 官方封面不可用: {e}")));
                }
            }
        }

        // 4. SteamGridDB：英文名 → 游戏名 → 文件夹名，先命中先返回
        //
        //    没配 Key 不代表没法抓图：Steam 官方图床是免钥匙的，走到这里说明它也没成。
        //    此时只把网络/鉴权类失败如实上抛，其余按「没有这张图」处理。
        if self.steamgriddb_api_key.is_empty() {
            return match worst_failure(&failures) {
                Some(kind) if kind != CoverFailureKind::Other => Err(CoverFailure {
                    kind,
                    message: join_failures(&failures),
                }),
                _ => Ok(None),
            };
        }

        let cache_path = self.get_cache_path(&game.id);
        let mut terms: Vec<String> = vec![
            pick_search_term(&game.name, game.name_en.as_deref()).to_string(),
        ];
        if game.name != terms[0] {
            terms.push(game.name.clone());
        }
        // 备用检索词：文件夹名（许多 repack 用官方英文名做目录名，仍值得一试）
        if let Some(folder) = game
            .install_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| n.to_string_lossy().to_string())
        {
            if !terms.contains(&folder) {
                terms.push(folder);
            }
        }

        // 「至少有一次检索给了我明确答复（收录/未收录）」——用来区分
        // 「库里真没有」与「网全挂了」。前者返回 None（走默认封面），
        // 后者返回 Err（让用户知道是网络问题，而不是名字写错了）。
        let mut heard_back = false;
        // 本轮最后一次致命失败（API Key 无效 / 限流 / 网络全断）。
        // 与历史 failures 分开记：已回退成功的图床抖动不该决定整批命运。
        let mut last_fatal_kind: Option<CoverFailureKind> = None;
        // 「检索命中了，但候选图一张都没下来」：这同样算失败，且不能被
        // 后面某个检索词的 NotFound 掩盖成「未收录」。
        let mut hit_download_failed = false;

        for term in &terms {
            match self.search_steamgriddb(term, deadline).await {
                SgdbSearch::Hit(urls) => {
                    heard_back = true;
                    for url in &urls {
                        match self.download_image(url, &cache_path, true, deadline).await {
                            Ok(actual_path) => {
                                return Ok(Some(actual_path.to_string_lossy().to_string()));
                            }
                            Err(e) => {
                                // 旧实现在这里用 `if let Ok` 把错误整个丢掉，
                                // 日志零痕迹 —— 这正是"拉图老有问题"却查不出原因的直接原因。
                                tracing::warn!("封面下载失败（{url}）: {e}");
                                failures.push((failure_kind_of(&e), format!("封面下载失败: {e}")));
                                hit_download_failed = true;
                            }
                        }
                    }
                }
                SgdbSearch::NotFound => {
                    heard_back = true;
                    tracing::info!("SteamGridDB 未收录: {term}");
                }
                SgdbSearch::Failed { reason, fatal, kind } => {
                    tracing::warn!("SteamGridDB 检索失败（{term}）: {reason}");
                    failures.push((kind, reason));
                    if fatal {
                        // API Key 无效 / 限流 / 网络全断：换检索词毫无意义，立刻止损。
                        // 记下这一次（而不是回头翻历史 failures）作为整批的真实成因。
                        last_fatal_kind = Some(kind);
                        break;
                    }
                }
            }
        }

        // 5. 收尾（2026-11-08 修复错误语义）：只看「本轮最后一次致命失败」，
        //    不再对全部历史 failures 取 worst_failure 代表整批。
        //    - 旧版混一次非致命 Other + heard_back=true 时直接 Ok(None)，
        //      把 429 限流谎报成「SteamGridDB 未收录」；
        //    - 旧版把早期一次「已回退成功的 Steam 图床抖动」(Transport)
        //      翻出来压过最后的 429，导致整批中止、剩余几十款全放弃。
        //    只有确实没收到任何明确答复、或候选图下载失败时才回退到历史最严重项。
        let fatal_kind = last_fatal_kind.or_else(|| {
            if !heard_back || hit_download_failed {
                Some(worst_failure(&failures).unwrap_or(CoverFailureKind::Other))
            } else {
                None
            }
        });
        if let Some(kind) = fatal_kind {
            return Err(CoverFailure {
                kind,
                message: join_failures(&failures),
            });
        }
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
            if file_path.extension().is_some_and(|e| {
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
    ///
    /// 返回 `SgdbSearch` 而非 `Option`：把「未收录」「限流」「网络失败」三种结局
    /// 分开，上层才能给用户说实话（见 `SgdbSearch` 的注释）。
    async fn search_steamgriddb(
        &self,
        game_name: &str,
        deadline: std::time::Instant,
    ) -> SgdbSearch {
        // 搜索游戏（URL 编码游戏名，支持中文等非 ASCII 字符）
        let encoded_name = urlencoding::encode(game_name);
        let search_url = format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            encoded_name
        );

        let (status, body) = match fetch_bytes_with_retry("SteamGridDB 搜索", deadline, || {
            self.client
                .get(&search_url)
                .timeout(budget_timeout(
                    deadline,
                    std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS),
                ))
                .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
        })
        .await
        {
            Ok(v) => v,
            Err(e) => return SgdbSearch::Failed {
                reason: format!("搜索请求失败: {e}"),
                fatal: true,
                kind: CoverFailureKind::Transport,
            },
        };

        if !status.is_success() {
            tracing::warn!("SteamGridDB 搜索失败: {} (状态码: {})", game_name, status);
            return match status {
                reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => SgdbSearch::Failed {
                    reason: "SteamGridDB API Key 无效，请在设置中检查".to_string(),
                    fatal: true,
                    kind: CoverFailureKind::Unauthorized,
                },
                // 429 限流（免费层约 1 req/s）：换检索词只会加深限流，直接止损
                reqwest::StatusCode::TOO_MANY_REQUESTS => SgdbSearch::Failed {
                    reason: "SteamGridDB 请求过于频繁（429），请稍后重试".to_string(),
                    fatal: true,
                    kind: CoverFailureKind::RateLimited,
                },
                other => SgdbSearch::Failed {
                    reason: format!("搜索接口返回 HTTP {other}"),
                    fatal: false,
                    kind: CoverFailureKind::Other,
                },
            };
        }

        let data: serde_json::Value = match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => return SgdbSearch::Failed {
                reason: format!("搜索响应解析失败: {e}"),
                fatal: false,
                kind: CoverFailureKind::Other,
            },
        };

        let Some(games) = data["data"].as_array() else {
            tracing::warn!("SteamGridDB 响应格式异常: {}", game_name);
            return SgdbSearch::Failed {
                reason: "搜索响应格式异常".to_string(),
                fatal: false,
                kind: CoverFailureKind::Other,
            };
        };
        let Some(first_game) = games.first() else {
            return SgdbSearch::NotFound;
        };

        tracing::info!("SteamGridDB 搜索到游戏: {} (ID: {})", game_name, first_game["id"]);
        let game_id = first_game["id"].as_i64().unwrap_or(0);
        if game_id <= 0 {
            return SgdbSearch::NotFound;
        }

        let grids_url = format!("https://www.steamgriddb.com/api/v2/grids/game/{game_id}");
        let (g_status, g_body) = match fetch_bytes_with_retry("SteamGridDB grids", deadline, || {
            self.client
                .get(&grids_url)
                .timeout(budget_timeout(
                    deadline,
                    std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS),
                ))
                .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
        })
        .await
        {
            Ok(v) => v,
            Err(e) => return SgdbSearch::Failed {
                reason: format!("封面列表请求失败: {e}"),
                fatal: false,
                kind: CoverFailureKind::Transport,
            },
        };

        // 这里曾经是「静默 Ok(None)」：grids 返回任何非 2xx 都当作"没有封面"，
        // 于是 429/5xx 最终被说成"游戏名可能不在 SteamGridDB 中"。
        if !g_status.is_success() {
            return SgdbSearch::Failed {
                reason: format!("封面列表接口返回 HTTP {g_status}"),
                fatal: g_status == reqwest::StatusCode::TOO_MANY_REQUESTS,
                kind: if g_status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    CoverFailureKind::RateLimited
                } else {
                    CoverFailureKind::Other
                },
            };
        }

        let grids_data: serde_json::Value = match serde_json::from_slice(&g_body) {
            Ok(v) => v,
            Err(e) => return SgdbSearch::Failed {
                reason: format!("封面列表解析失败: {e}"),
                fatal: false,
                kind: CoverFailureKind::Other,
            },
        };
        let Some(grids) = grids_data["data"].as_array() else {
            return SgdbSearch::NotFound;
        };

        // 取前 N 张做候选：旧实现只认 `grids.first()`，第一张下载失败就整体失败。
        // 优先原图 url，回退缩略图 thumb（thumb 更小、在慢网下更容易成功）。
        let mut urls: Vec<String> = Vec::with_capacity(COVER_DOWNLOAD_CANDIDATES);
        for grid in grids {
            for key in ["url", "thumb"] {
                if let Some(u) = grid[key].as_str() {
                    if !u.is_empty() && !urls.iter().any(|existing| existing == u) {
                        urls.push(u.to_string());
                        break;
                    }
                }
            }
            if urls.len() >= COVER_DOWNLOAD_CANDIDATES {
                break;
            }
        }

        if urls.is_empty() {
            SgdbSearch::NotFound
        } else {
            SgdbSearch::Hit(urls)
        }
    }

    /// 获取游戏的所有可选封面（异步）
    /// 检索词顺序：官方英文名（非 ASCII 名时）→ 游戏名 → 文件夹名，先命中先返回
    /// 命中则返回该游戏在 SteamGridDB 上的全部 grids（供「更换封面」弹窗选图）。
    ///
    /// 注意：**这里不掺 Steam 官方封面**。菜单上「Steam 官方」是独立入口
    /// （`set_game_cover_from_steam`），混进来会让用户分不清自己点的是哪一家。
    pub async fn fetch_cover_options(
        &self,
        game_name: &str,
        name_en: Option<&str>,
        install_path: Option<&str>,
    ) -> Result<Vec<CoverOption>> {
        // 弹窗里用户在等，预算比后台抓取更紧
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(COVER_OPTIONS_BUDGET_SECS);

        // 首选：官方英文名 / 游戏名（见 pick_search_term）
        let primary = pick_search_term(game_name, name_en);
        let mut grids = self.search_grids_for_game(primary, deadline).await?;

        // 备用 1：游戏名本身
        if grids.is_empty() && primary != game_name {
            tracing::info!("尝试用游戏名搜索封面选项: {}", game_name);
            grids = self.search_grids_for_game(game_name, deadline).await?;
        }

        // 备用 2：文件夹名
        if grids.is_empty() {
            if let Some(path) = install_path {
                let folder_name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string());
                if let Some(ref folder) = folder_name {
                    if *folder != game_name && folder != primary {
                        tracing::info!("尝试用文件夹名搜索封面选项: {}", folder);
                        grids = self.search_grids_for_game(folder, deadline).await?;
                    }
                }
            }
        }

        Ok(grids)
    }

    /// 搜索 SteamGridDB 获取游戏的所有 grids（内部方法）
    async fn search_grids_for_game(
        &self,
        game_name: &str,
        deadline: std::time::Instant,
    ) -> Result<Vec<CoverOption>> {
        let encoded_name = urlencoding::encode(game_name);
        let search_url = format!(
            "https://www.steamgriddb.com/api/v2/search/autocomplete/{}",
            encoded_name
        );

        let (status, body) = fetch_bytes_with_retry("SteamGridDB 搜索（封面选项）", deadline, || {
            self.client
                .get(&search_url)
                .timeout(budget_timeout(
                    deadline,
                    std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS),
                ))
                .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
        })
        .await?;

        if !status.is_success() {
            if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
                anyhow::bail!("SteamGridDB API Key 无效，请在设置中检查");
            }
            if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                anyhow::bail!("SteamGridDB 请求过于频繁（429），请稍后重试");
            }
            anyhow::bail!("SteamGridDB 搜索接口返回 HTTP {status}");
        }

        let data: serde_json::Value = serde_json::from_slice(&body)?;
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

        let (g_status, g_body) = fetch_bytes_with_retry("SteamGridDB 封面选项列表", deadline, || {
            self.client
                .get(&grids_url)
                .timeout(budget_timeout(
                    deadline,
                    std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS),
                ))
                .header("Authorization", format!("Bearer {}", self.steamgriddb_api_key))
        })
        .await?;

        if !g_status.is_success() {
            // 旧实现静默返回空列表 —— 用户只能看到"这个游戏没有封面"，
            // 而真相可能是限流或服务端 5xx。手动选图是用户主动发起的，
            // 如实报错比给个空列表有用。
            anyhow::bail!("SteamGridDB 封面列表接口返回 HTTP {g_status}");
        }

        let grids_data: serde_json::Value = serde_json::from_slice(&g_body)?;
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

    /// 按名字查 Steam **官方**封面地址（供手账等「无平台信息」条目使用）
    ///
    /// 两步走：Steam 商店搜索接口（storesearch）认出 appid → `steam_library_cover_url`
    /// 拼 Akamai 官方图床。这条链路**不经过 SteamGridDB**，与 `set_game_cover_from_steam`
    /// 最终落到同一个图床，只是 appid 的来源不同。
    ///
    /// storesearch 无需 API Key；2026-10-30 本机实测直连 200、0.97 秒。
    pub async fn steam_cover_url_by_name(&self, name: &str) -> Result<String> {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(COVER_OPTIONS_BUDGET_SECS);
        let url = format!(
            // 【2026-11-08】检索词是官方英文名（见 commands/reviews.rs 的
            // `search_name`），旧版却固定 `l=schinese&cc=CN`：返回名多为中文，
            // 精确同名分支永远不命中，退化成「取相关性第一条」；且 cc=CN 会把
            // 国区未上架的游戏整批过滤掉，明明有官方图床却报「未能确定 AppID」。
            // 改英文语种、去掉地区过滤，让英文 term 与返回名同语种、结果最全。
            "https://store.steampowered.com/api/storesearch/?term={}&l=english",
            urlencoding::encode(name)
        );

        let (status, body) = fetch_bytes_with_retry("Steam 商店搜索", deadline, || {
            self.client.get(&url).timeout(budget_timeout(
                deadline,
                std::time::Duration::from_secs(COVER_FETCH_TIMEOUT_SECS),
            ))
        })
        .await?;

        if !status.is_success() {
            anyhow::bail!("Steam 商店搜索接口返回 HTTP {status}");
        }

        let data: serde_json::Value = serde_json::from_slice(&body)?;
        let appid = pick_appid_from_store_search(&data, name)
            .ok_or_else(|| anyhow::anyhow!("Steam 商店未搜到「{name}」对应的游戏"))?;
        steam_library_cover_url(&appid)
            .ok_or_else(|| anyhow::anyhow!("Steam 返回的 appid 不合法: {appid}"))
    }

    /// 从 URL 下载图片到指定路径（**用户提供的地址**，供外部调用）
    ///
    /// 刻意**不施加域名白名单**：在设置页手动粘贴任意网图当封面是保留的既有功能。
    /// 但仍受 scheme 校验、重定向跳数限制（3 跳）与体积上限（20MB）约束。
    /// 返回实际写入的文件路径（按图片内容决定扩展名）。
    pub async fn download_from_url(&self, url: &str, save_path: &Path) -> Result<PathBuf> {
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(COVER_TOTAL_BUDGET_SECS);
        self.download_image(url, save_path, false, deadline).await
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

    /// 下载图片
    ///
    /// `enforce_trusted_host`：是否要求 URL（含重定向后的最终 URL）落在 SteamGridDB
    /// 域名下。SteamGridDB 返回的图源属**第三方响应**，必须校验；用户在设置页手动
    /// 粘贴的地址不校验（保留「任选网图当封面」的既有自由度）。
    ///
    /// 三道防线：① 仅允许 http(s) 协议；② 体积硬上限，在流式读取中截断
    /// （只预检 `Content-Length` 不够——服务端可以省略或谎报）；③ 可选域名白名单。
    ///
    /// **整段重下**而非续传：断流后字节位置不可靠，且封面只有几百 KB。
    /// 只有传输层失败（建连、读体中段断流）才重试；404 / 非图片 / 超限这类
    /// 业务性拒绝重试无意义，只会白等。
    async fn download_image(
        &self,
        url: &str,
        save_path: &Path,
        enforce_trusted_host: bool,
        deadline: std::time::Instant,
    ) -> Result<PathBuf> {
        let mut last: Option<anyhow::Error> = None;
        for attempt in 1..=COVER_RETRY_ATTEMPTS {
            // 预算耗尽就不再开新一轮下载 —— 这是"不无限转圈"的最后一道闸
            if std::time::Instant::now() >= deadline {
                tracing::warn!("封面下载超出时间预算，放弃: {url}");
                break;
            }
            match self
                .download_image_once(url, save_path, enforce_trusted_host, deadline)
                .await
            {
                Ok(path) => return Ok(path),
                Err(e) => {
                    let transport = e.downcast_ref::<TransportFailure>().is_some();
                    tracing::warn!(
                        "封面下载失败（第 {attempt}/{COVER_RETRY_ATTEMPTS} 次，{url}）: {e}"
                    );
                    let give_up = !transport || attempt == COVER_RETRY_ATTEMPTS;
                    last = Some(e);
                    if give_up {
                        break;
                    }
                    crate::utils::net::sleep_ms(COVER_RETRY_BACKOFF_MS * attempt as u64).await;
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow::anyhow!("封面下载失败: {url}")))
    }

    /// 单次下载尝试（不含重试）
    async fn download_image_once(
        &self,
        url: &str,
        save_path: &Path,
        enforce_trusted_host: bool,
        deadline: std::time::Instant,
    ) -> Result<PathBuf> {
        // ① 协议校验：reqwest 本身只支持 http(s)，显式拒绝是为了让日志给出确切原因
        let parsed =
            reqwest::Url::parse(url).with_context(|| format!("封面 URL 格式不合法: {url}"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            anyhow::bail!("拒绝下载非 http(s) 协议的封面: {}", parsed.scheme());
        }
        if enforce_trusted_host && !is_trusted_cover_host(&parsed) {
            anyhow::bail!("拒绝下载非 SteamGridDB 域名的图源: {url}");
        }

        // 建连与读体的错误一律打上 `TransportFailure` 标记 —— `download_image`
        // 靠它区分「值得重试」和「重试也没用」。
        let mut response = self
            .download_client
            .get(url)
            // 超时压到「剩余预算」以内，绝不拖动整次抓取
            .timeout(budget_timeout(
                deadline,
                std::time::Duration::from_secs(COVER_DOWNLOAD_TIMEOUT_SECS),
            ))
            .send()
            .await
            .map_err(|e| anyhow::Error::new(TransportFailure(e.into())))
            .with_context(|| format!("下载请求失败: {url}"))?;

        if !response.status().is_success() {
            anyhow::bail!("下载失败: HTTP {}", response.status());
        }

        // 重定向后可能落到别的域名——白名单必须对**最终 URL** 再校验一次
        if enforce_trusted_host && !is_trusted_cover_host(response.url()) {
            anyhow::bail!("封面下载被重定向到非信任域名: {}", response.url());
        }

        // ② 体积上限：先按声明值快速拒绝，再在流式读取中硬性截断
        if let Some(len) = response.content_length() {
            if len > COVER_MAX_FILE_SIZE {
                anyhow::bail!(
                    "下载失败: 响应声明大小 {} 字节，超过上限 {} 字节",
                    len,
                    COVER_MAX_FILE_SIZE
                );
            }
        }
        let mut bytes: Vec<u8> = Vec::new();
        loop {
            // 读体中段断流（`error decoding response body` 就是这一层）同样标记为可重试
            let chunk = response
                .chunk()
                .await
                .map_err(|e| anyhow::Error::new(TransportFailure(e.into())))?;
            let Some(chunk) = chunk else { break };
            if bytes.len() as u64 + chunk.len() as u64 > COVER_MAX_FILE_SIZE {
                anyhow::bail!(
                    "下载失败: 响应内容超过上限 {} 字节，已中断",
                    COVER_MAX_FILE_SIZE
                );
            }
            bytes.extend_from_slice(&chunk);
        }

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

        // 先写入**唯一命名**的临时文件，成功后再替换正式文件，避免留下损坏的文件。
        // 名字必须唯一：批量刷新封面与手动选封面可能并发落到同一目标，
        // 旧实现的固定 ".tmp" 名会互相覆盖、甚至被对方的清理误删。
        let temp_path = unique_temp_path(&actual_path);
        if let Err(e) = std::fs::write(&temp_path, &bytes) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(anyhow::anyhow!("下载失败: 写入临时文件失败 ({})", e));
        }

        // 验证临时文件大小
        let metadata = std::fs::metadata(&temp_path)?;
        if metadata.len() < COVER_MIN_FILE_SIZE {
            let _ = std::fs::remove_file(&temp_path);
            anyhow::bail!("下载失败: 写入后文件太小({} bytes)", metadata.len());
        }

        // 直接 rename 替换正式文件。
        // `std::fs::rename` 在 Windows 走 MoveFileEx(MOVEFILE_REPLACE_EXISTING)，
        // **本身就是替换语义**——原实现"先 remove_file 再 rename"不但多余，
        // 还在"删成功、rename 失败"的窗口里让封面凭空消失。
        if let Err(e) = std::fs::rename(&temp_path, &actual_path) {
            let _ = std::fs::remove_file(&temp_path);
            return Err(anyhow::anyhow!(
                "下载失败: 替换封面文件失败 {} ({})",
                actual_path.display(),
                e
            ));
        }

        Ok(actual_path)
    }
}


/// 把内部错误折成批量抓取能看懂的成因分类。
///
/// `download_image` 会给网络类错误打 `TransportFailure` 标记；没有这个标记的
/// （404、非图片、写盘失败）重试与整批中止都救不了，归 Other。
fn failure_kind_of(e: &anyhow::Error) -> CoverFailureKind {
    if e.downcast_ref::<TransportFailure>().is_some() {
        CoverFailureKind::Transport
    } else {
        CoverFailureKind::Other
    }
}

/// 一批失败里最该优先上报的成因（鉴权 > 网络 > 限流 > 其它）。
fn worst_failure(failures: &[(CoverFailureKind, String)]) -> Option<CoverFailureKind> {
    failures.iter().map(|(k, _)| *k).min_by_key(|k| k.priority())
}

/// 拼给人看的失败说明。
fn join_failures(failures: &[(CoverFailureKind, String)]) -> String {
    failures
        .iter()
        .map(|(_, m)| m.as_str())
        .collect::<Vec<_>>()
        .join("；")
}

#[cfg(test)]
mod tests {
    use super::{
        detect_local_steam_appid, is_trusted_cover_host, parse_appid, pick_appid_from_store_search,
        pick_search_term, steam_library_cover_url, CoverFetcher, TransportFailure,
    };

    /// 中文名 + 有官方英文名 → 用英文名搜（SteamGridDB 只收录英文名，
    /// 2026-10-03 实测「全面战争：三国」0 命中、「Total War: Three Kingdoms」命中）
    #[test]
    fn chinese_name_with_name_en_uses_english() {
        assert_eq!(
            pick_search_term("全面战争：三国", Some("Total War: Three Kingdoms")),
            "Total War: Three Kingdoms"
        );
    }

    /// 中文名 + 无英文名 → 退回中文名（不因缺英文名而比旧行为更差）
    #[test]
    fn chinese_name_without_name_en_falls_back_to_name() {
        assert_eq!(pick_search_term("全面战争：三国", None), "全面战争：三国");
        assert_eq!(pick_search_term("艾尔登法环", Some("   ")), "艾尔登法环");
    }

    /// 纯 ASCII 名直接用 name（此时 name_en 通常与它相同，无需比较）
    #[test]
    fn ascii_name_ignores_name_en() {
        assert_eq!(
            pick_search_term("Hollow Knight", Some("Hollow Knight")),
            "Hollow Knight"
        );
    }

    /// 混合 ASCII（含 CJK 标点/假名）按非 ASCII 处理：英文名存在时优先英文名
    #[test]
    fn mixed_script_name_prefers_english() {
        assert_eq!(
            pick_search_term("ペルソナ5", Some("Persona 5")),
            "Persona 5"
        );
        assert_eq!(
            pick_search_term("尼尔：机械纪元", Some("NieR:Automata")),
            "NieR:Automata"
        );
    }

    // ==================== Steam 官方图床 ====================

    /// 正常 appid → 拼出官方竖版封面地址
    #[test]
    fn steam_cover_url_built_from_numeric_appid() {
        assert_eq!(
            steam_library_cover_url("550").as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/550/library_600x900.jpg")
        );
        // 前后空白容忍（平台导入的数据偶尔带空格）
        assert_eq!(
            steam_library_cover_url(" 730 ").as_deref(),
            Some("https://cdn.cloudflare.steamstatic.com/steam/apps/730/library_600x900.jpg")
        );
    }

    /// 商店搜索解析：精确同名的结果优先于排在前面的模糊结果。
    /// 反例：搜 "Hollow Knight" 时 storesearch 会把续作排在前面，直接取第一条会贴错图。
    #[test]
    fn store_search_prefers_exact_name() {
        let data = serde_json::json!({
            "total": 2,
            "items": [
                { "type": "app", "name": "Hollow Knight: Silksong", "id": 1030300 },
                { "type": "app", "name": "Hollow Knight", "id": 367520 }
            ]
        });
        assert_eq!(
            pick_appid_from_store_search(&data, "Hollow Knight").as_deref(),
            Some("367520")
        );
        // 大小写与首尾空格不敏感（前端传来的名字常常带空格）
        assert_eq!(
            pick_appid_from_store_search(&data, "  hollow knight ").as_deref(),
            Some("367520")
        );
    }

    /// 没有精确同名时退化为「第一个 app」；bundle / sub 之类的包体必须跳过
    #[test]
    fn store_search_skips_bundles_and_falls_back_to_first_app() {
        let data = serde_json::json!({
            "items": [
                { "type": "sub", "name": "Half-Life Complete", "id": 90 },
                { "type": "bundle", "name": "Valve Bundle", "id": 100 },
                { "type": "app", "name": "Half-Life 2", "id": 220 }
            ]
        });
        assert_eq!(
            pick_appid_from_store_search(&data, "Half-Life").as_deref(),
            Some("220")
        );
    }

    /// 空结果 / 结构异常 → None（调用方据此报「未搜到」，而不是拼出错误 URL）
    #[test]
    fn store_search_returns_none_when_empty() {
        assert_eq!(pick_appid_from_store_search(&serde_json::json!({ "total": 0, "items": [] }), "x"), None);
        assert_eq!(pick_appid_from_store_search(&serde_json::json!({}), "x"), None);
        // 只有包体没有 app
        assert_eq!(
            pick_appid_from_store_search(&serde_json::json!({ "items": [{ "type": "sub", "id": 1 }] }), "x"),
            None
        );
        // id 不是无符号整数
        assert_eq!(
            pick_appid_from_store_search(&serde_json::json!({ "items": [{ "type": "app", "id": -1 }] }), "x"),
            None
        );
    }

    /// 非数字 / 超长 / 空 → 一律拒绝。
    /// appid 会拼进 URL 路径，放行任意字符等于把路径控制权交给数据源。
    #[test]
    fn steam_cover_url_rejects_non_numeric_appid() {
        assert_eq!(steam_library_cover_url(""), None);
        assert_eq!(steam_library_cover_url("   "), None);
        assert_eq!(steam_library_cover_url("../etc/passwd"), None);
        assert_eq!(steam_library_cover_url("550?x=1"), None);
        assert_eq!(steam_library_cover_url("550/../1"), None);
        assert_eq!(steam_library_cover_url("1234567890123"), None); // 13 位超长
        assert_eq!(steam_library_cover_url("٥٥٠"), None); // 阿拉伯-印度数字不是 ASCII
    }

    /// 白名单必须覆盖 Valve 官方图床，否则 `download_image` 会把自己的 URL 拒了
    #[test]
    fn steam_cdn_is_trusted() {
        let url = reqwest::Url::parse("https://cdn.cloudflare.steamstatic.com/x.jpg").unwrap();
        assert!(is_trusted_cover_host(&url));
        let sgdb = reqwest::Url::parse("https://cdn2.steamgriddb.com/grid/a.png").unwrap();
        assert!(is_trusted_cover_host(&sgdb));
        let evil = reqwest::Url::parse("https://evil.com/steamstatic.com").unwrap();
        assert!(!is_trusted_cover_host(&evil));
    }

    // ==================== 重试判定 ====================

    /// 关键前提：加了 `.with_context()` 之后，`downcast_ref::<TransportFailure>()`
    /// 必须仍然认得出传输失败 —— 否则读体断流永远不进重试分支，
    /// 这个 bug 会以「明明打了重试日志却不重试」的形式再犯一次。
    #[test]
    fn transport_failure_survives_context() {
        // `anyhow::Error::context` 是固有方法，不需要引入 Context trait
        let err = anyhow::Error::new(TransportFailure(anyhow::anyhow!("tls stall")))
            .context("下载请求失败: https://example.com/a.jpg");
        assert!(err.downcast_ref::<TransportFailure>().is_some());
        // 业务性错误（没有标记）不该被误判成可重试
        let rejected = anyhow::anyhow!("下载失败: HTTP 404");
        assert!(rejected.downcast_ref::<TransportFailure>().is_none());
    }

    // ==================== 本地 repack 的 appid 反查 ====================

    #[test]
    fn parse_appid_accepts_only_plausible_ids() {
        assert_eq!(parse_appid("2215430").as_deref(), Some("2215430"));
        assert_eq!(parse_appid("  620\n").as_deref(), Some("620"));
        // 占位值：认了会让满库游戏共用同一张 Spacewar 封面
        assert_eq!(parse_appid("480"), None);
        assert_eq!(parse_appid("0"), None);
        assert_eq!(parse_appid(""), None);
        assert_eq!(parse_appid("abc"), None);
        assert_eq!(parse_appid("-1"), None);
        assert_eq!(parse_appid("12345678901"), None); // 11 位超长
    }

    /// 临时目录里造一个 repack，验证 ini / txt 两条读取路径
    #[test]
    fn detect_appid_from_install_dir() {
        let dir = std::env::temp_dir().join(format!("gv-appid-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();

        // 1) 什么都没有 → None
        assert_eq!(detect_local_steam_appid(&path), None);

        // 2) steam_emu.ini 里写 AppId（Goldberg/RUNE 的常见写法）
        std::fs::write(
            dir.join("steam_emu.ini"),
            "[Settings]\nAppId=2215430\nUserName=test\n",
        )
        .unwrap();
        assert_eq!(detect_local_steam_appid(&path).as_deref(), Some("2215430"));

        // 3) 只有占位值 480 → 不认
        std::fs::write(dir.join("steam_emu.ini"), "[Settings]\nAppId=480\n").unwrap();
        assert_eq!(detect_local_steam_appid(&path), None);

        // 4) steam_appid.txt 优先于 ini
        std::fs::write(dir.join("steam_appid.txt"), "620").unwrap();
        assert_eq!(detect_local_steam_appid(&path).as_deref(), Some("620"));

        // 5) 目录不存在 → None（不 panic）
        assert_eq!(detect_local_steam_appid("Z:\\no\\such\\dir"), None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 联网冒烟测试（**默认忽略**，手动执行）：
    /// `cargo test --ignored steam_cdn_download_smoke`
    ///
    /// 验证的是「白名单 → 下载 → 魔数校验 → 落盘」整条真实链路，
    /// 而不只是 URL 拼接。2026-10-18 排查「拉图老有问题」时用来分诊：
    /// 若这条过了、SGDB 那条不过，就说明病灶在网络路径而非代码。
    #[test]
    #[ignore = "联网冒烟测试，需手动执行"]
    fn steam_cdn_download_smoke() {
        let dir = std::env::temp_dir().join(format!("gv-smoke-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let fetcher = CoverFetcher::new(dir.clone(), String::new()).unwrap();

        let url =
            "https://cdn.cloudflare.steamstatic.com/steam/apps/620/library_600x900.jpg";
        let saved = tauri::async_runtime::block_on(
            fetcher.download_image(
                url,
                &dir.join("smoke.jpg"),
                true,
                std::time::Instant::now() + std::time::Duration::from_secs(60),
            ),
        )
        .expect("Steam 官方图床下载失败");

        let size = std::fs::metadata(&saved).unwrap().len();
        assert!(size > 10_000, "封面体积异常: {size} bytes");
        assert_eq!(
            saved.extension().and_then(|e| e.to_str()),
            Some("jpg"),
            "魔数校验应把扩展名定为 jpg"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
