# 🎮 Game Vault

轻量级 Windows 本地游戏管理器 — 管理游戏库、接入 Steam / Epic、自动追踪游玩时长、游戏内截图（支持 HDR）、游戏手账与成就系统、可视化统计数据。

基于 **Tauri 2.0** + **Rust** + **Vue 3** 构建，单文件便携运行，数据完全留在本机。

<p align="center">
  <img src="https://img.shields.io/badge/version-0.8.9-6366f1" alt="version">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2B-blue" alt="platform">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="license">
</p>

## ✨ 功能特性

### 🎮 游戏库管理

| 功能 | 说明 |
|------|------|
| 手动添加 | 选择本地 exe 文件一键入库；添加过程不联网，不自动拉封面、也不改写你填的名字 |
| 一键启动 | 点击卡片上的启动按钮或右键菜单启动游戏，支持填启动参数 |
| 状态标记 | 未游玩 / 游玩过 / 已通关 三种状态，收藏为独立维度 |
| 工具栏 | 搜索框 + 排序组合（字段与升降序）+ 漏斗式筛选弹层，筛选不再占横排空间 |
| 排序口径 | 默认「最近互动」：玩过的按最近游玩时间、没玩过的按入库时间——刚添加的游戏排在第一位 |
| 常驻动作区 | 侧边栏左下角固定「添加游戏」与「刷新」，侧栏折叠时收成图标键 |
| 重命名 / 删除 | 右键菜单操作；删除只移除库内条目，游玩历史、手账、成就、封面全部保留 |
| 游戏详情 | 侧边抽屉展示完整信息：封面、版本、时长、HLTB、存档路径、启动参数 |
| 版本号读取 | 自动从 exe 文件读取产品版本号，文件元数据缓存避免重复读取 |
| 存档检查 | 一键检查所有游戏存档路径是否存在，游戏卡片显示状态图标（蓝色=存在，灰色=不存在） |
| 重装认领 | 卸载重装后自动认领旧条目（墓碑机制）；改名后重新导入同样能接回历史 |

### 🕹️ 平台接入（Steam / Epic）

| 功能 | 说明 |
|------|------|
| 库扫描 | 自动发现本机 Steam 库区与 Epic 安装目录，一键扫描待导入游戏（约 0.03 秒） |
| 智能过滤 | 自动排除 Steamworks 运行库、Proton 等 Valve 工具条目与未完全安装条目 |
| 一键启动 | 通过平台协议拉起游戏，无需手动定位 exe |
| 启动三态 | 「启动中 / 客户端更新中 / 游玩中」分开展示：等启动、下更新、排队登录的时间不计入游玩时长 |
| 待命转正 | 客户端拉起的游戏在待命窗口内现身即自动转正计费；超时弹窗说明原因，不计入时长 |
| 停止计时 | 详情页「停止游戏」只结束本应用的计时，不关闭游戏进程 |
| 实时反馈 | 游戏进程现身、会话转正后，卡片立即显示「运行中」 |
| 截图分流 | 平台游戏与手动添加的游戏统一纳入游戏内截图体系 |

### 📸 游戏内截图（支持 HDR）

| 功能 | 说明 |
|------|------|
| 全局热键 | 游戏内一键截图（默认 **F12**，可自定义），全局热键 + 游戏内键盘钩子双通道，按下即响（约 10 毫秒） |
| 手柄截图键 | 另可设手柄键触发，支持组合键（如 LB+A）；只在本库游戏运行期间读取手柄 |
| HDR 捕获 | 基于 Windows Graphics Capture 的 scRGB 捕获，HDR 游戏截图不失真 |
| 色调映射 | 内置 HDR → SDR tonemap（扩展范围走 Khronos PBR Neutral 压缩支，保黑位、单调压向白点），SDR 白电平自动查询系统值，多屏拓扑按目标屏取值 |
| 目录整理 | 截图按游戏分目录落盘；自动匹配既有目录（含变音符号折叠、标点/空格/大小写抹平），写入与查看永远同一套解析 |
| 手动指定 | 手账里可为游戏指定截图文件夹，最权威、删游戏/改名都不受影响 |

### 🤖 智能信息获取

| 功能 | 说明 |
|------|------|
| 封面三源 | Steam 官方图床优先，SteamGridDB 兜底，也可用手边任意本地图片 |
| 免 Key 取图 | 未配置 SteamGridDB API Key 时，Steam 官方封面照样能取 |
| 封面选择器 | 从多张候选封面中手动挑选，支持原图/缩略图切换 |
| 中英文双搜 | 手账支持英文题名字段，中文题名也能拉到正确封面 |
| LLM 信息获取 | 调用大模型补全游戏名称、简介、开发商、发行商、类型、存档路径等 |
| 手动入口 | 补信息与补封面均按需触发：详情页「获取信息」「更换封面」，侧栏「刷新」可批量 |
| HLTB 时长 | 展示 HowLongToBeat 主线 / 主线+支线 / 完美通关时长参考 |
| 协议兼容 | LLM 支持 OpenAI 和 Anthropic 两种协议格式，兼容主流 API 提供商 |
| 网络健壮 | 抖动自动重试；失败提示区分「确实没这张图」与「网络没连通」 |
| 代理跟随 | 网络请求跟随 Windows 系统代理，本地/内网地址直连 |
| 封面索引 | 封面按内容哈希存储、引用计数管理，多游戏与手账共用封面不重复占空间 |

### 📒 游戏手账

| 功能 | 说明 |
|------|------|
| 独立手账库 | 与游戏库平行的存档簿：题名、评分（满分自定）、评语、封面、类型等（手账只记玩过的游戏，不设状态分类） |
| 评语排版 | 长评默认按正文阅读（空行分段、行首「- 」成列表、星号夹住即加粗），超过 8 行折叠；点「编辑」才回到输入框 |
| 评语检索 | 卡片上显示两行摘要，搜索框可按评语正文查找 |
| 封面三源 | 本地图片 / Steam 官方 / SteamGridDB 任选；添加或刷新不再覆盖手动挑好的那张 |
| 手账截图库 | 每款游戏独立截图目录，自持解析（游戏删除后依旧可用） |
| 草稿保护 | 写库失败时内容留在草稿簿，切换页面后重开仍可恢复重试 |
| 灵活补充 | 手动补录未入库的老游戏、已卸载的游戏，历史不受游戏库变动影响 |

### ⏱️ 游玩时长追踪

- 自动检测游戏进程启动 / 退出，精确记录游玩时长；游戏退出后约 1 秒内完成结算
- 支持多个游戏同时运行，独立追踪
- 进程名 + 安装目录前缀双重匹配，兼容 Windows NT 设备路径格式
- 启动器另起子进程、主进程先退的游戏也能正确收尾，不会关掉了还显示「游玩中」
- 碎片会话自动合并 / 过滤，时长不流失也不虚报
- 实时显示当前正在运行的游戏（卡片高亮 + 启动按钮旋转）
- 双账本设计：预聚合表（日 / 时段粒度）永久保留，**删除游戏不缩水总时长**

### 📊 数据统计

| 图表 | 类型 | 说明 |
|------|------|------|
| 概览统计 | 卡片 | 游戏总数、总时长、本月时长、今日时长 |
| 类型分布 | 环形图 | 游戏类型（RPG、动作、策略等）分布 |
| 时长排行 | 条形图 | 库里玩过的游戏全部入榜，展示封面与封面主色调 |
| 每日趋势 | 折线图 | 一段时间内的游玩时长变化趋势 |
| 年度热力图 | 热力图 | GitHub 贡献图风格，一年中每天的游玩分布 |
| 时段分布 | 热力图 | 24h × 7天，展示一周中每个时段的游玩热度 |

### 🏆 成就系统

| 功能 | 说明 |
|------|------|
| 成就卡片墙 | 每款游戏一张封面卡，卡下直接显示已解锁数量与完成度进度条；点开卡片看全部成就 |
| 筛选排序 | 按「进行中 / 已全成就 / 未解锁」过滤，按完成度、解锁数、名称排序 |
| 全局成就 | 衡量整个游戏库：库规模、总时长、连续游玩、深夜/破晓游玩、收藏、封面等 38 项 |
| 单游戏成就 | 每款游戏独立解锁、独立进度：游玩时长、通关、HLTB 对照等 16 项 |
| 完成度口径 | 只统计当前能达成的成就，未收录通关参考时长的两项不计入分母 |
| 留档可见 | 已删除游戏的成就单独列在页面底部，历史解锁仍可查看 |
| 多级里程碑 | 藏书家、铁杆玩家等成就分多级（最多 4 级），解锁后进度条指向下一级 |
| 四类设计 | 进度 / 收藏 / 趣味 / 挑战，参考 RetroAchievements 设计哲学 |
| 存量结算 | 功能上线时按历史数据回溯解锁，之后增量检测，与 Steam 行为一致 |
| 解锁通知 | 游戏退出、库操作后自动检测，右上角弹出解锁动画 |

### 🎨 个性化

- **深色 / 浅色主题**：一键切换
- **自定义强调色**：8 种预设 + 手动输入，图表、按钮、边框实时跟随
- **自定义标题栏**：双击空白区域最大化 / 还原
- **系统托盘**：关闭窗口时选择最小化到托盘继续追踪
- **开机自启动**：可在设置中开关，Windows 启动时自动运行
- **截图存放**：自定义游戏截图与手柄截图键，默认存 `%USERPROFILE%\Videos`

### 💾 数据管理

| 功能 | 说明 |
|------|------|
| JSON 导出 | 导出全部游戏数据和设置为 JSON 文件 |
| JSON 导入 | 从备份文件恢复数据，自动合并设置 |
| 存档路径 | 为每个游戏配置存档目录，支持批量管理；环境变量展开兼容 `%%VAR%%`、`%VAR%`、裸变量名三种格式 |
| 存档备份 | 将配置的存档路径打包为 ZIP 导出 |
| 存档恢复 | 从 ZIP 备份还原存档文件，含路径穿越安全防护 |

### 🔒 其他

- **单实例窗口**：防止重复启动，再次运行自动聚焦已有窗口
- **关于对话框**：内置完整更新日志，按版本号展示
- **API Key 保护**：密钥输入框为密码类型显示
- **黑屏看门狗**：启动 20 秒内前端未就绪自动记录诊断信息，便于排查 WebView2 环境问题
- **关闭确认**：关闭窗口时弹出自定义确认对话框

## 🛠️ 技术栈

| 层级 | 技术 |
|------|------|
| 桌面框架 | Tauri 2.0 |
| 后端语言 | Rust |
| 前端框架 | Vue 3 + TypeScript |
| UI 组件库 | Naive UI |
| 图表库 | ECharts + vue-echarts |
| 状态管理 | Pinia |
| 数据库 | SQLite (rusqlite) |
| 截图捕获 | Windows.Graphics.Capture (WGC, scRGB) |
| 构建工具 | Vite |
| 测试 | Vitest（前端纯逻辑单测） |

## 🚀 快速开始

### 环境要求

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.70
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (Windows)
- WebView2（Windows 10/11 已预装）

### 安装依赖

```bash
npm install
```

### 开发模式

```bash
npm run tauri dev
```

### 构建发布版

```bash
npm run tauri build -- --no-bundle
```

构建产物为便携版单文件 exe：`src-tauri/target/release/game-vault.exe`，免安装、数据独立于程序目录。

## ⚙️ 配置说明

### SteamGridDB API Key

在 [SteamGridDB](https://www.steamgriddb.com/) 免费注册并获取 API Key，用于获取游戏封面图。在设置页面填入即可使用。

**不填也能用**：Steam 游戏与按 Steam 版安装的游戏直接从 Steam 官方图床取封面，无需任何 Key；SteamGridDB 覆盖的是非 Steam 来源的游戏与备用封面。

### LLM 配置

LLM 用于智能获取游戏元数据（名称纠正、简介、开发商、发行商、类型、存档路径等）。

支持 **OpenAI** 和 **Anthropic** 两种协议格式，兼容以下及更多 API 提供商：

| 提供商 | 协议 | Base URL |
|--------|------|----------|
| 小米 MiMo | OpenAI | `https://api.xiaomimimo.com/v1` |
| DeepSeek | OpenAI / Anthropic | `https://api.deepseek.com/v1` |
| OpenAI | OpenAI | `https://api.openai.com/v1` |
| Anthropic | Anthropic | `https://api.anthropic.com` |
| 其他兼容服务 | 视情况选择 | 自定义 |

默认配置为小米 MiMo。在设置页面填入 API Key 并启用 LLM 即可。

### 数据存储位置

```
%APPDATA%/GameVault/
├── gamevault.db           # SQLite 数据库（WAL 模式，设置与索引都存这里）
├── gamevault.db-wal       # WAL 日志（运行时出现，正常现象）
├── gamevault.db-shm       # 共享内存（同上）
├── covers/                # 封面（按用途分桶）
│   ├── <id>.<ext>         #   主图，游戏与手账共用
│   ├── thumb/             #   缩略图，卡片网格与排行使用
│   └── archive/           #   已移除条目的留档图，删除不删图
└── logs/                  # 运行日志（时间戳为 UTC）
```

游戏截图默认存放在 `%USERPROFILE%\Videos`（与 NVIDIA App 默认一致，可在设置中更改），按游戏分文件夹管理。

## 📁 项目结构

```
game-vault/
├── src/                          # 前端源码 (Vue 3)
│   ├── components/               # UI 组件
│   │   ├── GameCard.vue          # 游戏卡片（封面、状态、右键菜单）
│   │   ├── GameDetail.vue        # 游戏详情抽屉（信息、存档、记录、启动参数）
│   │   ├── AddGameFlow.vue       # 添加游戏流程（三入口共用）
│   │   ├── SidebarActions.vue    # 侧边栏常驻动作区（添加 / 刷新）
│   │   ├── ContextMenu.vue       # 通用右键菜单
│   │   ├── CoverPickerModal.vue  # 封面选择器弹窗
│   │   ├── PlatformImportModal.vue  # 平台批量导入
│   │   ├── GameInfoEditModal.vue # 手动填写游戏信息
│   │   ├── LaunchArgsModal.vue   # 启动参数编辑
│   │   ├── AchievementGameCard.vue    # 成就页封面卡
│   │   ├── GameAchievementsDrawer.vue # 成就详情面板
│   │   ├── AchievementToast.vue  # 解锁通知动画
│   │   └── AboutModal.vue        # 关于对话框（更新日志）
│   ├── views/                    # 页面视图（懒加载）
│   │   ├── HomeView.vue          # 游戏库（工具栏 + 卡片网格）
│   │   ├── StatsView.vue         # 统计页面（图表仪表盘）
│   │   ├── AchievementsView.vue  # 成就页面（卡片墙）
│   │   ├── ReviewsView.vue       # 游戏手账
│   │   └── SettingsView.vue      # 设置页面
│   ├── stores/                   # 状态管理 (Pinia)
│   │   ├── games.ts              # 游戏库 / 启动三态 / 封面映射
│   │   └── reviews.ts            # 手账条目 / 游玩时长映射 / 失败草稿簿
│   ├── lib/                      # 纯逻辑（多数有 vitest 单测）
│   │   ├── tauri.ts              # Tauri 命令封装
│   │   ├── gameFilter.ts         # 筛选 / 排序唯一口径
│   │   ├── achievements.ts       # 成就分组 / 完成度 / 量纲文案
│   │   ├── richText.ts           # 评价极简 Markdown → 结构化块
│   │   ├── launchState.ts        # 启动三态 reducer
│   │   ├── reviewDrafts.ts       # 手账写库失败草稿簿
│   │   ├── addGameFlow.ts        # 添加流程编排（零网络）
│   │   ├── refreshActions.ts     # 刷新信息 / 刷新封面批量入口
│   │   ├── format.ts             # 格式化工具
│   │   ├── color.ts              # 颜色处理
│   │   ├── constants.ts          # 全局常量
│   │   └── useCoverImage.ts      # 封面加载 composable
│   ├── data/
│   │   └── changelog.ts          # 更新日志数据
│   ├── App.vue                   # 根组件（标题栏、主题、导航）
│   └── main.ts                   # 入口
│
├── src-tauri/                    # 后端源码 (Rust)
│   ├── src/
│   │   ├── commands/             # Tauri 命令（前后端桥接）
│   │   │   ├── games.rs          # 游戏增删改查、封面、LLM、存档
│   │   │   ├── reviews.rs        # 手账增删改查、截图库
│   │   │   ├── screenshots.rs    # 截图触发、目录解析、截图库
│   │   │   ├── platform.rs       # Steam / Epic 扫描导入与启动
│   │   │   ├── stats.rs          # 统计查询、概览
│   │   │   ├── achievements.rs   # 成就查询与检测命令
│   │   │   └── settings.rs       # 设置读写
│   │   ├── core/                 # 核心业务逻辑
│   │   │   ├── database.rs       # SQLite 操作（含预聚合账本、迁移）
│   │   │   ├── launcher.rs       # 游戏启动器（含启动参数）
│   │   │   ├── tracker.rs        # 进程监控 + 游玩时长追踪
│   │   │   ├── platform.rs       # 平台库扫描（acf / manifest 解析）
│   │   │   ├── steam_state.rs    # Steam 条目状态判定（更新中 / 未装全）
│   │   │   ├── genres.rs         # 游戏类型规范化表
│   │   │   ├── screenshot.rs     # 截图流程与目录解析
│   │   │   ├── capture.rs        # 画面抓取（WGC scRGB）
│   │   │   ├── tonemap.rs        # HDR → SDR 色调映射
│   │   │   ├── sdr_white.rs      # 系统 SDR 白电平查询
│   │   │   ├── hotkey_hook.rs    # 全局键盘钩子
│   │   │   ├── gamepad.rs        # 手柄轮询（截图键）
│   │   │   ├── cover_fetcher.rs  # 封面获取（Steam 官方 / SteamGridDB）
│   │   │   ├── cover_store.rs    # 封面存储（哈希索引 + 引用计数 + 缩略图）
│   │   │   ├── llm_fetcher.rs    # LLM 游戏信息获取
│   │   │   ├── boot_guard.rs     # 启动看门狗（黑屏自愈）
│   │   │   └── achievements.rs   # 成就引擎（定义 + 检测 + 汇总）
│   │   ├── models/               # 数据模型
│   │   │   ├── game.rs           # 游戏、筛选器
│   │   │   ├── review.rs         # 手账条目
│   │   │   ├── cover.rs          # 封面索引与选项
│   │   │   ├── play_session.rs   # 游玩会话
│   │   │   ├── achievement.rs    # 成就定义与解锁记录
│   │   │   └── settings.rs       # 应用设置
│   │   └── utils/                # 工具模块
│   │       ├── constants.rs      # Rust 端常量
│   │       ├── net.rs            # 网络请求（系统代理 / 超时 / 重试）
│   │       └── path.rs           # 路径工具
│   ├── Cargo.toml
│   └── tauri.conf.json
│
├── package.json
├── vite.config.ts
├── vitest.config.ts
├── index.html
├── LICENSE
└── README.md
```

## 📝 常用命令

```bash
npm run dev                          # 仅启动 Vite 前端开发服务器
npm run build                        # 构建前端
npm run build:check                  # 类型检查 + 构建前端
npm run typecheck                    # TypeScript 类型检查
npm test                             # 运行前端单测（vitest）
npm run test:watch                   # 单测监听模式
npm run tauri dev                    # 启动 Tauri 开发模式（前后端联动）
npm run tauri build -- --no-bundle   # 构建便携版 exe（不生成安装包）
```

## 📄 许可证

[MIT License](LICENSE)

## 👤 作者

Isaac
