//! HTTP 重试与超时工具。
//!
//! ## 为什么需要重试（2026-10-18 实测定性）
//!
//! 本机到 SteamGridDB 所在的 Cloudflare 边缘，**TLS 握手约有 20% 的概率直接卡死**：
//! TCP 三次握手正常（~250ms），ClientHello 发出后对端毫无响应，只能等到超时。
//! 成功时握手耗时在 0.4~12 秒之间剧烈抖动，图床一次成功下载甚至要 41 秒。
//! 同一个 URL 紧接着再发一次，往往就通——所以对这种故障模式，
//! **重试不是权宜之计，而是正确解**（实测 6 次里第 6 次 200）。
//!
//! ## 重试与退避的取舍
//!
//! 卡死发生在**建连阶段**，旧连接已经废了，立刻新建一条才是对的，故退避只留
//! 400ms 起步的极短间隔（纯粹为了避开同一瞬间的网络抖动窗口）。
//! 而 429（限流）重试只会更糟，故在 `send_with_retry` 里**刻意不重试 429**，
//! 交给调用方如实报给用户。
//!
//! ## 为什么还要识别系统代理
//!
//! reqwest **只读 `HTTP(S)_PROXY` 环境变量，不读 Windows 的 WinINET 设置**。
//! 也就是说老爷把 Clash 的系统代理打开，本应用照样在直连 —— 那条被干扰的
//! TLS 链路一步都没绕开。这里补上「跟随系统代理」：开着就借道，关着就直连，
//! 并在借用前先探一下端口，避免 Clash 崩了却把应用一起拖死。

use anyhow::Result;
use reqwest::RequestBuilder;
use std::net::ToSocketAddrs;
use std::time::Duration;

use crate::utils::constants::{COVER_RETRY_ATTEMPTS, COVER_RETRY_BACKOFF_MS};

/// 传输层错误才值得重试：超时 / 建连失败 / 请求根本没发出去。
/// 反序列化之类的错误重试也没用。
pub fn is_retryable_error(e: &reqwest::Error) -> bool {
    e.is_timeout() || e.is_connect() || e.is_request()
}

/// 重试间隔。
///
/// 握手卡死的重试理论上可以立刻发，但留一点间隔能避开"同一瞬间的网络抖动窗口"；
/// 400ms 足够短，用户感知不到。
fn backoff(attempt: u32) -> Duration {
    let factor = 1u64 << (attempt.saturating_sub(1).min(3));
    Duration::from_millis(COVER_RETRY_BACKOFF_MS * factor)
}

/// 本次请求可用的超时：取「原超时上限」与「距预算截止还剩多少」的较小值。
///
/// 这是"点了刷新不会一直转圈"的实现要点：把预算余量直接压进每一次请求的
/// timeout 里，超出的部分由 reqwest 自己掐断，而不是发完再检查。
pub fn budget_timeout(deadline: std::time::Instant, cap: Duration) -> Duration {
    let remain = deadline.saturating_duration_since(std::time::Instant::now());
    remain.min(cap).max(Duration::from_secs(1))
}

/// 发送请求并**连同响应体一起**读完，建连 / 响应头 / 读体任一环节失败都重试。
///
/// 为什么必须把读体也包进来（2026-10-18 老爷实测）：日志里连续出现
/// `error decoding response body`，那是 reqwest 在**读体阶段**连接被掐断时报的错，
/// 此时 `send()` 早就返回 Ok 了 —— 只重试 `send()` 对这类失败毫无帮助。
/// 实测 grids 请求正是在拿到响应头 21 秒后断在读体上。
///
/// `deadline` 是整次抓取的截止时刻：过线立刻放弃，不再发起新尝试。
///
/// - 传输层错误 / 读体失败 → 重试
/// - 5xx → 重试
/// - **429 / 4xx → 不重试**，原样返回状态码让调用方如实报告（重试只会加深限流）
///
/// `make` 每次调用都必须构造一个**全新的** `RequestBuilder`：`RequestBuilder`
/// 不实现 `Clone`，而且重试必须换一条新连接。
pub async fn fetch_bytes_with_retry<F>(
    label: &str,
    deadline: std::time::Instant,
    mut make: F,
) -> Result<(reqwest::StatusCode, Vec<u8>)>
where
    F: FnMut() -> RequestBuilder,
{
    let mut last: Option<anyhow::Error> = None;

    for attempt in 1..=COVER_RETRY_ATTEMPTS {
        if std::time::Instant::now() >= deadline {
            return Err(last.unwrap_or_else(|| anyhow::anyhow!("{label}: 抓取超出时间预算")));
        }

        match make().send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_server_error() && attempt < COVER_RETRY_ATTEMPTS {
                    tracing::warn!(
                        "{label}: 服务端 HTTP {status}（第 {attempt}/{COVER_RETRY_ATTEMPTS} 次），重试"
                    );
                    last = Some(anyhow::anyhow!("服务端返回 HTTP {status}"));
                    continue;
                }

                match resp.bytes().await {
                    Ok(body) => return Ok((status, body.to_vec())),
                    Err(e) => {
                        tracing::warn!(
                            "{label}: 读取响应体失败（第 {attempt}/{COVER_RETRY_ATTEMPTS} 次）: {e}"
                        );
                        last = Some(anyhow::anyhow!("读取响应体失败: {e}"));
                        if attempt < COVER_RETRY_ATTEMPTS {
                            sleep(backoff(attempt)).await;
                            continue;
                        }
                        return Err(last.unwrap_or_else(|| anyhow::anyhow!("{label}: 读体失败")));
                    }
                }
            }
            Err(e) => {
                let retryable = is_retryable_error(&e);
                if retryable && attempt < COVER_RETRY_ATTEMPTS {
                    tracing::warn!(
                        "{label}: {e}（第 {attempt}/{COVER_RETRY_ATTEMPTS} 次），重试"
                    );
                    let wait = backoff(attempt);
                    last = Some(e.into());
                    sleep(wait).await;
                    continue;
                }
                return Err(e.into());
            }
        }
    }

    Err(last.unwrap_or_else(|| anyhow::anyhow!("{label}: 重试次数耗尽")))
}

/// 轻量异步睡眠。
///
/// 项目未直接依赖 tokio（它只是 tauri 的传递依赖），为几毫秒的退避
/// 去新增一个直接依赖不划算。这里用 `tauri::async_runtime::spawn_blocking`
/// 把阻塞睡眠丢到线程池，不占用 async 工作线程。
pub async fn sleep_ms(ms: u64) {
    if ms == 0 {
        return;
    }
    let _ = tauri::async_runtime::spawn_blocking(move || {
        std::thread::sleep(Duration::from_millis(ms))
    })
    .await;
}

async fn sleep(dur: Duration) {
    sleep_ms(dur.as_millis() as u64).await;
}

// ==================== 系统代理（Windows WinINET） ====================

/// 读取系统代理设置，返回可直接交给 `reqwest::Proxy::all` 的地址。
///
/// 三条判定，缺一不可：
/// 1. `ProxyEnable` 必须是 1（Clash 关掉系统代理时这里是 0 —— 此时绝不代理）；
/// 2. `ProxyServer` 能解析出主机端口；
/// 3. 该端口**当前真能连上**（300ms 超时）。Clash 崩了而注册表残留时，
///    直连虽然慢，总好过让应用彻底断网。
pub fn system_proxy_url() -> Option<String> {
    if read_reg_value("ProxyEnable")?.trim() != "0x1" {
        return None;
    }
    let url = parse_proxy_server(&read_reg_value("ProxyServer")?)?;
    if !proxy_reachable(&url) {
        tracing::warn!("系统代理 {url} 端口不通，本次走直连");
        return None;
    }
    Some(url)
}

/// 解析 `ProxyServer` 的两种写法：
/// - `127.0.0.1:7890`（Clash 默认）
/// - `http=127.0.0.1:7891;https=127.0.0.1:7890`（分协议）
///
/// 分协议时优先 https —— 本应用抓的都是 https 资源。
fn parse_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let mut https: Option<String> = None;
    let mut http: Option<String> = None;
    let mut keyed = false;

    for part in raw.split(';') {
        let Some((scheme, addr)) = part.split_once('=') else {
            continue;
        };
        let addr = addr.trim();
        if addr.is_empty() {
            continue;
        }
        keyed = true;
        match scheme.trim().to_ascii_lowercase().as_str() {
            // 同名键出现多次时取第一个（system 代理很少这么写，纯防御）
            "https" => {
                https.get_or_insert_with(|| addr.to_string());
            }
            "http" => {
                http.get_or_insert_with(|| addr.to_string());
            }
            _ => {}
        }
    }

    let addr = match (https, http) {
        (Some(a), _) => a,
        (None, Some(a)) => a,
        // 只有 http=/https= 之外的形式（比如 socks=）时，不猜
        (None, None) if keyed => return None,
        (None, None) => raw.to_string(),
    };

    if addr.contains("://") {
        Some(addr)
    } else {
        Some(format!("http://{addr}"))
    }
}

/// 代理端口是否真的在监听（300ms 内能建 TCP 即可）
fn proxy_reachable(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    let (Some(host), Some(port)) = (parsed.host_str(), parsed.port_or_known_default()) else {
        return false;
    };
    let Ok(addrs) = (host, port).to_socket_addrs() else {
        return false;
    };
    addrs.into_iter().any(|addr| {
        std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok()
    })
}

/// 读一个 WinINET 注册表值的**最后一段**（REG_SZ 的值 / REG_DWORD 的 `0x1`）。
#[cfg(windows)]
fn read_reg_value(name: &str) -> Option<String> {
    use std::os::windows::process::CommandExt;
    // 2026-10-03 血案：GUI 进程 spawn 控制台程序不带这个标志会闪黑窗 + 卡
    // 约 5 秒（等控制台分配）。reg.exe 是控制台程序，必带。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let out = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            "/v",
            name,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // 该值不存在（例如从没设置过代理）
    }

    let text = String::from_utf8_lossy(&out.stdout);
    // 输出形如：`    ProxyServer    REG_SZ    127.0.0.1:7890`
    let line = text.lines().find(|l| l.trim_start().starts_with(name))?;
    let mut cols = line.split_whitespace();
    cols.next()?; // 值名
    cols.next()?; // 类型 REG_SZ / REG_DWORD
    cols.next().map(|s| s.to_string()) // 值（代理地址不含空格，够用）
}

#[cfg(not(windows))]
fn read_reg_value(_name: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff(1), Duration::from_millis(COVER_RETRY_BACKOFF_MS));
        assert_eq!(backoff(2), Duration::from_millis(COVER_RETRY_BACKOFF_MS * 2));
        // 上限 8 倍，避免极端重试次数把等待拉爆
        assert_eq!(backoff(9), Duration::from_millis(COVER_RETRY_BACKOFF_MS * 8));
    }

    #[test]
    fn proxy_server_plain_hostport() {
        assert_eq!(parse_proxy_server("127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(parse_proxy_server(" 127.0.0.1:7890 ").as_deref(), Some("http://127.0.0.1:7890"));
        // 已经带协议的不要重复拼接
        assert_eq!(parse_proxy_server("http://127.0.0.1:7890").as_deref(), Some("http://127.0.0.1:7890"));
    }

    #[test]
    fn proxy_server_per_scheme_prefers_https() {
        assert_eq!(
            parse_proxy_server("http=127.0.0.1:7891;https=127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        // 只给了 http= 就用它
        assert_eq!(
            parse_proxy_server("http=127.0.0.1:7891").as_deref(),
            Some("http://127.0.0.1:7891")
        );
        // 只给了 socks= 之类：不猜，返回 None
        assert_eq!(parse_proxy_server("socks=127.0.0.1:7890"), None);
    }

    #[test]
    fn proxy_server_rejects_blank() {
        assert_eq!(parse_proxy_server(""), None);
        assert_eq!(parse_proxy_server("   "), None);
    }

    /// 预算裁剪：远未到期时用原上限；快到点时收窄；已过线也给 1 秒（timeout(0) 会报错）
    #[test]
    fn budget_timeout_shrinks_with_remaining_time() {
        let cap = Duration::from_secs(20);

        let far = std::time::Instant::now() + Duration::from_secs(120);
        assert_eq!(budget_timeout(far, cap), cap);

        let near = std::time::Instant::now() + Duration::from_secs(3);
        let t = budget_timeout(near, cap);
        assert!(t <= Duration::from_secs(3), "应收窄到剩余预算以内，实际 {t:?}");
        assert!(t >= Duration::from_secs(1));

        let past = std::time::Instant::now() - Duration::from_secs(5);
        assert_eq!(budget_timeout(past, cap), Duration::from_secs(1));
    }
}
