//! 截图管理器：抓帧 → 色调映射 → 命名归档 → 保存。
//!
//! 命名规则（与 NVIDIA App / Steam 保持一致的目录组织）：
//!   `{截图目录}/{进程名}/{进程名}_{日期}_{时间}.png`
//!
//! 当前路线为 WGC（方案 A）：抓 FP16 scRGB → `tonemap_rgba16f_to_srgb`。
//! 注入（方案 C）的取数与格式分派代码存于 injection 分支，恢复注入时从那边合并，
//! master 不保留无调用者的镜像实现。

use crate::core::tonemap::{self, ToneMapPath};

/// 一次截图的最终结果
#[derive(Debug, Clone)]
pub struct ScreenshotResult {
    pub path: String,
    /// 归档文件夹名（题名或会话 exe 名，取决于 `resolve_capture_target` 的判定来源），
    /// 前端据此提示"截图已保存到「X」文件夹"。
    pub process_name: String,
    pub width: u32,
    pub height: u32,
    pub tone_map_path: ToneMapPath,
}

/// 目录 / 文件名分量消毒：把 Windows 非法字符换成**空格**（与用户库里既有的
/// `Call of Duty  Black Ops`、`Assassin's Creed  Brotherhood` 这类目录命名一致），再去首尾空白。
///
/// 题名（如 `Mafia: The Old Country`）也要走这里——既可作目录名，也保证不会因 `:` `/` 越出截图根目录。
/// 非法字符换成空格后可能出现连续空格，这是有意保留的（跟随既有目录的习惯，便于肉眼对应）。
pub fn sanitize_component(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|c| match c {
            '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => ' ',
            _ => c,
        })
        .collect();
    cleaned.trim().to_string()
}

/// 从进程名中提取文件 stem（去掉 .exe 扩展名），用于目录与文件命名
pub fn process_stem(exe_name: &str) -> String {
    let name = exe_name.trim();
    let lower = name.to_lowercase();
    let stem = if lower.ends_with(".exe") {
        &name[..name.len() - 4]
    } else {
        name
    };
    sanitize_component(stem)
}

/// 计算某个游戏（进程）的截图目录：`{screenshot_dir}/{process_stem}`
pub fn screenshot_dir_for_process(screenshot_dir: &str, exe_name: &str) -> std::path::PathBuf {
    let expanded = crate::utils::path::expand_env_vars(screenshot_dir);
    std::path::PathBuf::from(expanded).join(process_stem(exe_name))
}

// ==================== 截图落盘目标解析（2026-09-14，改前必读） ====================

/// 落盘目录的命名来源（只用于日志与排查）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureDirSource {
    /// 手账里手动指定的截图目录
    Journal,
    /// 截图根目录下已存在的同名目录
    ExistingDir,
    /// 按游戏标题新建
    GameTitle,
    /// 按会话 exe 名兜底
    SessionExe,
}

impl CaptureDirSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Journal => "手账指定",
            Self::ExistingDir => "已有同名目录",
            Self::GameTitle => "游戏标题新建",
            Self::SessionExe => "会话 exe 名兜底",
        }
    }
}

/// 一次截图的落盘目标：目录 + 文件名前缀 + 命名来源
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTarget {
    pub dir: std::path::PathBuf,
    pub stem: String,
    pub source: CaptureDirSource,
}

/// 解析截图落盘目录与文件名前缀（纯逻辑，便于单测）。
///
/// # 为什么不能简单按 exe 名建目录（用户库实测，2026-09-14）
///
/// `H:\GameCapture` 是多年积累的库，188 个目录绝大多数按**游戏标题**命名
/// （`God of War` 569 张、`Alan Wake 2` 453 张……），而按 exe 名会建出 `GoW`（0 张）这类
/// 第二摊目录：图截进去了，详情页与手账（都按老目录统计）却看不到，用户以为没截上。
///
/// # 优先级
///
/// 1. `journal_dir`：手账里手动指定的截图目录（单层目录名）——用户自己整理过的，最权威，
///    也覆盖 `SILENT HILL 2` 这种与标题对不上的特殊命名；
/// 2. 根目录下与任一候选名（英文名 → 中文名）**对得上**的已存在目录：复用其原始名字，
///    既不新建也不改名（保留用户目录里的大小写与空格）；
/// 3. 都没有：按第一个可用候选名（游戏标题）新建；标题全空时退回 `fallback`（会话 exe 名）。
///
/// 比较统一走 `match_key`（只留字母数字小写、精确相等），与手账回填同口径，
/// 因此 `Assassin's Creed: Brotherhood` 与 `Assassin's Creed  Brotherhood`（双空格）能对上，
/// 而 `II` / `III` 不会互相误配。
pub fn resolve_capture_target(
    root: &std::path::Path,
    candidates: &[String],
    journal_dir: Option<&str>,
    fallback_exe: &str,
) -> CaptureTarget {
    // 1) 手账手动指定（单层目录名，挡路径穿越）
    if let Some(dir) = journal_dir.and_then(sanitize_dir_name) {
        return CaptureTarget {
            stem: dir.clone(),
            dir: root.join(dir),
            source: CaptureDirSource::Journal,
        };
    }

    // 2) 复用根目录下已存在的同名目录
    let existing = list_subdir_names(root);
    for candidate in candidates {
        let key = match_key(candidate);
        if key.is_empty() {
            continue;
        }
        if let Some(hit) = existing.iter().find(|dir| match_key(dir) == key) {
            return CaptureTarget {
                dir: root.join(hit),
                stem: hit.clone(),
                source: CaptureDirSource::ExistingDir,
            };
        }
    }

    // 3) 新建：游戏标题优先，空则退回会话 exe 名
    let from_title = candidates
        .iter()
        .map(|c| sanitize_component(c))
        .find(|c| !c.is_empty() && c != "." && c != "..");
    match from_title {
        Some(name) => CaptureTarget {
            dir: root.join(&name),
            stem: name,
            source: CaptureDirSource::GameTitle,
        },
        None => {
            let name = process_stem(fallback_exe);
            CaptureTarget {
                dir: root.join(&name),
                stem: name,
                source: CaptureDirSource::SessionExe,
            }
        }
    }
}

/// 列出根目录下的子目录名（不递归；根目录不存在则空）
fn list_subdir_names(root: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect()
}

// ==================== 截图目录枚举 / 匹配（手账自持截图目录用，2026-09-12） ====================

/// 认定为截图的扩展名（与前端「N 张」口径一致）
const IMAGE_EXTS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// 统计目录下的截图张数（非递归；目录不存在返回 0）
pub fn count_images(dir: &std::path::Path) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let path = e.path();
            if !path.is_file() {
                return false;
            }
            matches!(
                path.extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase())
                    .as_deref()
                    .map(|ext| IMAGE_EXTS.contains(&ext)),
                Some(true)
            )
        })
        .count()
}

/// 列出截图根目录下**含图片文件**的子目录：(目录名, 图片张数)，按目录名排序。
///
/// 手账回填与「指定截图目录」选择器共用：只暴露有图的目录，才不会把 `GoW` 这类
/// 空目录（游戏库 exe 名与实际截图目录名不一致时产生）推给用户。
pub fn list_image_subdirs(root: &std::path::Path) -> Vec<(String, usize)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<(String, usize)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let count = count_images(&e.path());
            if count > 0 {
                Some((name, count))
            } else {
                None
            }
        })
        .collect();
    out.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
    out
}

/// 名称 → 匹配键：仅保留字母数字并转小写。
///
/// 用于「手账名 vs 截图目录名」的对齐（`Assassin's Creed II` → `assassinscreedii`，
/// 与目录 `Assassin's Creed  Brotherhood` 这类双空格、全角符号差异一并抹平）。
/// 只做精确相等判定，不做包含匹配——避免 II / III 这类前缀互相误配。
pub fn match_key(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// 校验并规范化用户指定的截图子目录名；非法（路径穿越 / 绝对路径 / 空）返回 None。
///
/// 只接受**单层目录名**：不含路径分隔符、不是 `.`/`..`、不带盘符或前导斜杠。
pub fn sanitize_dir_name(name: &str) -> Option<String> {
    let trimmed = name.trim().trim_end_matches(['\\', '/']).trim();
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return None;
    }
    if trimmed.contains(['\\', '/', ':']) {
        return None;
    }
    Some(trimmed.to_string())
}

/// 生成一个不重名的截图文件路径（同一秒多次截图自动加序号）
fn build_unique_path(dir: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let now = chrono::Local::now();
    let base = now.format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut candidate = dir.join(format!("{}_{}.png", stem, base));
    let mut i = 1u32;
    while candidate.exists() {
        candidate = dir.join(format!("{}_{}_{}.png", stem, base, i));
        i += 1;
    }
    candidate
}

/// 通用保存：把已 tonemap 好的 sRGB RGBA8 帧编码为 PNG 并归档。
/// WGC 与注入两条路径共用。目录与前缀由 `resolve_capture_target` 解析后传入。
pub fn save_srgb_png(
    srgb: &[u8],
    width: u32,
    height: u32,
    dir: &std::path::Path,
    stem: &str,
    tone_path: ToneMapPath,
) -> anyhow::Result<ScreenshotResult> {
    std::fs::create_dir_all(dir)
        .map_err(|e| anyhow::anyhow!("创建截图目录失败 {}: {e}", dir.display()))?;
    let path = build_unique_path(dir, stem);

    tonemap::encode_png(srgb, width, height, &path)?;

    tracing::info!("截图已保存: {} (tonemap: {:?})", path.display(), tone_path);
    Ok(ScreenshotResult {
        path: path.to_string_lossy().to_string(),
        process_name: stem.to_string(),
        width,
        height,
        tone_map_path: tone_path,
    })
}

/// 方案 A：从指定 HWND 抓取一帧并保存到指定目录（WGC 路径）。
pub fn capture_and_save(
    hwnd: isize,
    dir: &std::path::Path,
    stem: &str,
) -> anyhow::Result<ScreenshotResult> {
    let frame = crate::core::capture::capture_window_fp16(hwnd)?;

    // 查询窗口所在显示器的真实 SDR 白电平（HDR 滑块 80~480nits，默认 80）。
    // 硬编码 1.0 会在滑块偏离默认时截图偏暗（<80nits）或误判为 HDR 压灰（>80nits）。
    let white_level_scale = crate::core::sdr_white::sdr_white_scale_for_window(hwnd);

    tracing::info!(
        "WGC 抓到帧 {}x{} ({} bytes Rgba16F, sdr_white={:.3})",
        frame.width,
        frame.height,
        frame.rgba16f.len(),
        white_level_scale
    );

    let (srgb, tone_path) = tonemap::tonemap_rgba16f_to_srgb(
        &frame.rgba16f,
        frame.width,
        frame.height,
        white_level_scale,
    );

    save_srgb_png(&srgb, frame.width, frame.height, dir, stem, tone_path)
}

/// 截图反馈音效（内嵌合成 wav，经 winmm PlaySound 播放，零外部文件依赖）。
///
/// 之前用 `MessageBeep` 播系统提示音：Success=Asterisk、Error=Critical Stop、
/// Notice=Exclamation —— 全是 Windows 报错腔，游戏内听感刺耳且三者区分度差。
/// 现改为三段内嵌音效（`include_bytes!` 静态存储，SND_ASYNC 异步播放不阻塞热键线程）：
/// - Success: 相机快门「咔嚓」（两次机械 click，Steam 截图同风格）
/// - Error:   柔和低音「咚」（180Hz 指数衰减，不再用刺耳的 Critical Stop）
/// - Notice:  轻 UI tick（短促高频），提示无目标/前台不匹配
#[derive(Debug, Clone, Copy)]
pub enum FeedbackTone {
    /// 截图成功：快门音
    Success,
    /// 截图失败/出错：低音提示
    Error,
    /// 收到按键但无可截图目标（无活跃游戏 / 前台窗口不匹配）
    Notice,
}

/// SND_MEMORY 播放的内存映像必须是 WAV 文件格式字节流；
/// include_bytes! 得到的 &'static [u8] 常驻只读段，满足 SND_ASYNC 的
/// 「播放期间缓冲必须持续有效」要求，无生命周期问题。
static SOUND_SHUTTER: &[u8] = include_bytes!("../../assets/sounds/shutter.wav");
static SOUND_ERROR: &[u8] = include_bytes!("../../assets/sounds/error.wav");
static SOUND_NOTICE: &[u8] = include_bytes!("../../assets/sounds/notice.wav");

/// 播放截图反馈音效。
///
/// SND_ASYNC：异步，热键回调线程立即返回；SND_NODEFAULT：声卡设备异常时
/// 静默失败，不回落播放系统默认提示音（避免再次出现"Windows 报错音"）。
/// 连续两次 play_feedback 时后者自动打断前者（PlaySound 语义），符合
/// "Notice(受理) → Error(失败)" 的连播场景。
pub fn play_feedback(tone: FeedbackTone) {
    use windows::core::PCWSTR;
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};

    let data = match tone {
        FeedbackTone::Success => SOUND_SHUTTER,
        FeedbackTone::Error => SOUND_ERROR,
        FeedbackTone::Notice => SOUND_NOTICE,
    };

    unsafe {
        // hmod 在 SND_MEMORY 模式下被忽略，传默认空句柄
        let _ = PlaySoundW(
            PCWSTR(data.as_ptr() as *const u16),
            None,
            SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试临时目录守卫：Drop 时清理
    struct TempDirGuard(std::path::PathBuf);

    impl Drop for TempDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 目录名消毒：只接受单层目录名，挡住路径穿越与绝对路径
    #[test]
    fn sanitize_dir_name_blocks_traversal() {
        assert_eq!(sanitize_dir_name("MafiaTheOldCountry").as_deref(), Some("MafiaTheOldCountry"));
        assert_eq!(sanitize_dir_name("  Alan Wake 2  ").as_deref(), Some("Alan Wake 2"));
        assert_eq!(sanitize_dir_name("dir\\").as_deref(), Some("dir"));

        assert_eq!(sanitize_dir_name(".."), None);
        assert_eq!(sanitize_dir_name("../etc"), None);
        assert_eq!(sanitize_dir_name("a/b"), None);
        assert_eq!(sanitize_dir_name(r"C:\Windows"), None);
        assert_eq!(sanitize_dir_name("/abs"), None);
        assert_eq!(sanitize_dir_name("   "), None);
    }

    /// 匹配键：抹平空格、标点与大小写，但保留字母数字差异（II ≠ III）
    #[test]
    fn match_key_normalizes() {
        assert_eq!(match_key("Assassin's Creed  Brotherhood"), "assassinscreedbrotherhood");
        assert_eq!(match_key("Assassin's Creed: Brotherhood"), "assassinscreedbrotherhood");
        assert_eq!(match_key("SILENT HILL 2"), "silenthill2");
        assert_eq!(match_key("Alan Wake 2"), "alanwake2");
        assert_ne!(match_key("Assassin's Creed II"), match_key("Assassin's Creed III"));
    }

    /// 目录枚举：只收含图片的子目录，忽略空目录与根目录下的散落文件
    #[test]
    fn list_image_subdirs_filters_empty() {
        let root = std::env::temp_dir().join(format!("gv_shot_dirs_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("HasImages")).unwrap();
        std::fs::write(root.join("HasImages").join("a.png"), b"x").unwrap();
        std::fs::write(root.join("HasImages").join("note.txt"), b"x").unwrap();
        std::fs::create_dir_all(root.join("EmptyDir")).unwrap();
        std::fs::write(root.join("loose.png"), b"x").unwrap();
        let _guard = TempDirGuard(root.clone());

        let dirs = list_image_subdirs(&root);
        assert_eq!(dirs.len(), 1);
        assert_eq!(dirs[0].0, "HasImages");
        assert_eq!(dirs[0].1, 1, "txt 不计入张数");

        // 根目录不存在：返回空而非 panic
        assert!(list_image_subdirs(&root.join("nope")).is_empty());
    }

    fn temp_root(tag: &str) -> (std::path::PathBuf, TempDirGuard) {
        let root = std::env::temp_dir().join(format!("gv_target_{tag}_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let guard = TempDirGuard(root.clone());
        (root, guard)
    }

    /// 落盘目标优先级：手账指定 > 已有同名目录 > 标题新建 > 会话 exe 名兜底
    #[test]
    fn capture_target_priority() {
        let (root, _g) = temp_root("prio");
        std::fs::create_dir_all(root.join("God of War")).unwrap();
        std::fs::create_dir_all(root.join("Alan Wake 2")).unwrap();

        // 1) 手账指定最权威：即便根目录下已有"同名目录"也听手账的
        let t = resolve_capture_target(
            &root,
            &["God of War".to_string()],
            Some("GoW"),
            "GoW.exe",
        );
        assert_eq!(t.source, CaptureDirSource::Journal);
        assert_eq!(t.dir, root.join("GoW"));
        assert_eq!(t.stem, "GoW");

        // 2) 复用已存在目录：保留它原始的大小写与空格
        let t = resolve_capture_target(
            &root,
            &["God of War".to_string()],
            None,
            "GoW.exe",
        );
        assert_eq!(t.source, CaptureDirSource::ExistingDir);
        assert_eq!(t.dir, root.join("God of War"));
        assert_eq!(t.stem, "God of War");

        // 3) 标题对不上任何已有目录 → 按标题新建
        let t = resolve_capture_target(
            &root,
            &["Cyberpunk 2077".to_string()],
            None,
            "Cyberpunk2077.exe",
        );
        assert_eq!(t.source, CaptureDirSource::GameTitle);
        assert_eq!(t.dir, root.join("Cyberpunk 2077"));

        // 4) 标题全空 → 退回会话 exe 名
        let t = resolve_capture_target(&root, &["".to_string(), "  ".to_string()], None, "Frostpunk.exe");
        assert_eq!(t.source, CaptureDirSource::SessionExe);
        assert_eq!(t.dir, root.join("Frostpunk"));
    }

    /// 英文名优先于中文名，且两侧都按 match_key 精确比对（标点/空格差异可抹平，II 与 III 不混）
    #[test]
    fn capture_target_matches_existing_dir_by_normalized_name() {
        let (root, _g) = temp_root("match");
        std::fs::create_dir_all(root.join("Death Stranding Director's Cut")).unwrap();
        std::fs::create_dir_all(root.join("Assassin's Creed  Brotherhood")).unwrap();
        std::fs::create_dir_all(root.join("Assassin's Creed III")).unwrap();

        // 英文名全大写也能对上已有目录（Windows 目录名不区分大小写，这里连标点差异一起抹平）
        let t = resolve_capture_target(
            &root,
            &[
                "DEATH STRANDING DIRECTOR'S CUT".to_string(),
                "死亡搁浅：导演剪辑版".to_string(),
            ],
            None,
            "DeathStranding.exe",
        );
        assert_eq!(t.source, CaptureDirSource::ExistingDir);
        assert_eq!(t.dir, root.join("Death Stranding Director's Cut"));

        // 英文名对不上时继续试中文名（这里都没有 → 新建，且冒号被消毒）
        let t = resolve_capture_target(
            &root,
            &[
                "MAFIA: THE OLD COUNTRY".to_string(),
                "四海兄弟：故乡".to_string(),
            ],
            None,
            "MafiaTheOldCountry.exe",
        );
        assert_eq!(t.source, CaptureDirSource::GameTitle);
        assert_eq!(t.dir, root.join("MAFIA  THE OLD COUNTRY"), "非法字符换空格，与既有目录习惯一致");

        // 标点差异抹平（双空格版本可被单冒号版本命中）
        let t = resolve_capture_target(
            &root,
            &["Assassin's Creed: Brotherhood".to_string()],
            None,
            "ACB.exe",
        );
        assert_eq!(t.dir, root.join("Assassin's Creed  Brotherhood"));

        // 数字后缀不得互相误配：II 不该命中 III
        let t = resolve_capture_target(
            &root,
            &["Assassin's Creed II".to_string()],
            None,
            "AC2.exe",
        );
        assert_eq!(t.source, CaptureDirSource::GameTitle);
        assert_eq!(t.dir, root.join("Assassin's Creed II"));
    }

    /// 手账目录值非法（路径穿越 / 绝对路径）时忽略，继续走后续推断
    #[test]
    fn capture_target_ignores_illegal_journal_dir() {
        let (root, _g) = temp_root("evil");
        for evil in [r"..\..\Windows", r"C:\Windows", "a/b", "   "] {
            let t = resolve_capture_target(&root, &["Some Game".to_string()], Some(evil), "sg.exe");
            assert_ne!(
                t.source,
                CaptureDirSource::Journal,
                "非法手账目录 {evil} 不该被采用"
            );
            assert_eq!(t.dir, root.join("Some Game"));
        }
    }
}
