use serde::{Deserialize, Serialize};

/// 封面主体类型
pub const OWNER_GAME: &str = "game";
pub const OWNER_REVIEW: &str = "review";

/// 封面种类
pub const KIND_MAIN: &str = "main";
pub const KIND_THUMB: &str = "thumb";

/// 封面状态
pub const STATE_ACTIVE: &str = "active";
pub const STATE_ARCHIVED: &str = "archived";

/// covers 索引表的一行：一张图属于谁、文件在哪、内容指纹是什么。
///
/// 设计要点（2026-09-10）：
/// - `rel_path` 相对 covers 目录存，换机/换盘不影响；绝对路径由 `resolve` 拼。
/// - `sha256` 是内容指纹：同一张图被多个主体引用时（手账从游戏导入）**共享同一物理文件**，
///   删除时按指纹计数决定是否真的删文件——根治"删游戏把共享的手账封面一起删掉"。
/// - `state` 区分在库（active）与已移除留档（archived），删条目不再删图。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoverIndexRow {
    /// 'game' | 'review'
    pub owner_kind: String,
    pub owner_id: String,
    /// 'main' | 'thumb'
    pub kind: String,
    /// 相对 covers 目录的路径（正斜杠）
    pub rel_path: String,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    /// 'active' | 'archived'
    pub state: String,
    pub updated_at: String,
}

/// 一次封面入库/查询的结果（绝对路径，前端可直接走 asset 协议）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CoverSet {
    pub main: Option<String>,
    pub thumb: Option<String>,
    /// 主图**最后一次被替换**的时刻（RFC3339）。
    ///
    /// 专供前端拼 asset URL 的破缓存参数：封面文件名恒为 `<owner_id>.<ext>`，
    /// 换封面是**原地覆盖同名文件**，URL 逐字节不变 → WebView2 会直接命中
    /// 图片缓存、继续显示旧图（2026-09-24 老爷实测：卡片换封面后不刷新）。
    ///
    /// 之所以不复用 `games.updated_at` / `reviews.updated_at`：那是**条目**最后
    /// 改动时刻，改评分、涨游玩时长都会刷新它，会让 URL 无谓抖动、白烧一次
    /// 图片解码。这里要的是**这张图**最后变化的时刻，只有封面真的换了才变。
    pub updated_at: Option<String>,
}
