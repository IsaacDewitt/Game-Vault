# 🎮 Game Vault

轻量级 Windows 本地游戏管理器 — 管理游戏库、接入 Steam / Epic、自动追踪游玩时长、游戏内截图（支持 HDR）、可视化统计数据。

基于 **Tauri 2.0** + **Rust** + **Vue 3** 构建，本机运行，数据完全离线。

<p align="center">
  <img src="https://img.shields.io/badge/version-0.8.1-6366f1" alt="version">
  <img src="https://img.shields.io/badge/platform-Windows%2010%2B-blue" alt="platform">
  <img src="https://img.shields.io/badge/license-MIT-green" alt="license">
</p>

## ✨ 功能特性

### 🎮 游戏库管理

| 功能 | 说明 |
|------|------|
| 手动添加 | 选择本地 exe 文件，一键添加到游戏库 |
| 一键启动 | 点击卡片上的启动按钮或右键菜单启动游戏 |
| 状态标记 | 未游玩 / 游玩过 / 已通关 三种状态，收藏为独立维度 |
| 智能筛选 | 按状态、类型筛选；支持名称搜索；多种排序方式 |
| 重命名 / 删除 | 右键菜单操作；删除只移除库内条目，游玩历史、手账、成就、封面全部保留 |
| 游戏详情 | 侧边抽屉展示完整信息：封面、版本、时长、HLTB、存档路径 |
| 版本号读取 | 自动从 exe 文件读取产品版本号，文件元数据缓存避免重复读取 |
| 存档检查 | 一键检查所有游戏存档路径是否存在，游戏卡片显示状态图标（蓝色=存在，灰色=不存在） |
| 重装认领 | 卸载重装后自动认领旧条目（墓碑机制），历史数据无缝衔接 |

### 🕹️ 平台接入（Steam / Epic）

| 功能 | 说明 |
|------|------|
| 库扫描 | 自动发现本机 Steam 库区与 Epic 安装目录，一键扫描待导入游戏 |
| 智能过滤 | 自动排除 Steamworks 运行库、Proton 等 Valve 工具条目与未完全安装条目 |
| 一键启动 | 通过平台协议拉起游戏，无需手动定位 exe |
| 待命转正 | 客户端拉起的游戏在待命窗口内现身即自动转正计费；超时弹窗说明原因，不计入时长 |
| 实时反馈 | 游戏进程现身、会话转正后，卡片立即显示「运行中」 |
| 截图分流 | 平台游戏与手动添加的游戏统一纳入游戏内截图体系 |

### 📸 游戏内截图（支持 HDR）

| 功能 | 说明 |
|------|------|
| 全局热键 | 游戏内一键截图（默认 F9，可自定义），全局热键 + 游戏内键盘钩子双通道 |
| HDR 捕获 | 基于 Windows Graphics Capture 的 scRGB 捕获，HDR 游戏截图不失真 |
| 色调映射 | 内置 HDR → SDR tonemap（肩部压缩 + 亮度守恒），SDR 白电平自动查询系统值，多屏拓扑按目标屏取值 |
| 目录整理 | 截图按游戏分目录落盘；自动匹配既有目录（含变音符号折叠、标点/空格/大小写抹平），写入与查看永远同一套解析 |
| 手动指定 | 手账里可为游戏指定截图文件夹，最权威、删游戏/改名都不受影响 |

### 🤖 智能信息获取

| 功能 | 说明 |
|------|------|
| 封面自动获取 | 通过 SteamGridDB API 自动匹配游戏封面 |
| 封面选择器 | 从多张候选封面中手动挑选，支持原图/缩略图切换 |
| 手动换封面 | 本地图片替换封面 |
| 中英文双搜 | 手账支持英文题名字段，中文题名也能拉到正确封面 |
| LLM 信息获取 | 调用大模型自动补全游戏名称、简介、开发商、发行商、类型、存档路径等 |
| 批量刷新 | 一键为所有缺封面的游戏获取封面，为缺信息的游戏补全元数据 |
| HLTB 时长 | 展示 HowLongToBeat 主线 / 主线+支线 / 完美通关时长参考 |
| 协议兼容 | LLM 支持 OpenAI 和 Anthropic 两种协议格式，兼容主流 API 提供商 |
| 封面索引 | 封面按内容哈希存储、引用计数管理，多游戏共用封面不重复占空间 |

### 📒 游戏手账

| 功能 | 说明 |
|------|------|
| 独立手账库 | 与游戏库平行的存档簿：题名、评分（满分自定）、评语、状态、封面、类型等 |
| 手账截图库 | 每款游戏独立截图目录，自持解析（游戏删除后依旧可用） |
| 灵活补充 | 手动补录未入库的老游戏、已卸载的游戏，历史不受游戏库变动影响 |

### ⏱️ 游玩时长追踪

- 自动检测游戏进程启动 / 退出，精确记录游玩时长
- 支持多个游戏同时运行，独立追踪
- 进程名 + 安装目录前缀双重匹配，兼容 Windows NT 设备路径格式
- 碎片会话自动合并 / 过滤，时长不流失也不虚报
- 实时显示当前正在运行的游戏（卡片高亮 + 启动按钮旋转）
- 双账本设计：预聚合表（日 / 时段粒度）永久保留，**删除游戏不缩水总时长**

### 📊 数据统计

| 图表 | 类型 | 说明 |
|------|------|------|
| 概览统计 | 卡片 | 游戏总数、总时长、本月时长、今日时长 |
| 状态分布 | 饼图 | 未游玩 / 游玩过 / 已通关 占比 |
| 类型分布 | 环形图 | 游戏类型（RPG、动作、策略等）分布 |
| 时长排行 | 条形图 | Top 10 游玩时长，展示封面和封面主色调 |
| 每日趋势 | 折线图 | 一段时间内的游玩时长变化趋势 |
| 年度热力图 | 热力图 | GitHub 贡献图风格，一年中每天的游玩分布 |
| 时段分布 | 热力图 | 24h × 7天，展示一周中每个时段的游玩热度 |

### 🏆 成就系统

| 功能 | 说明 |
|------|------|
| 全局成就 | 衡量整个游戏库：库规模、总时长、连续游玩、深夜/破晓游玩、收藏、封面等 38 项 |
| 单游戏成就 | 每款游戏独立解锁、独立进度：游玩时长、通关、HLTB 对照等 16 项 |
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
- **截图根目录**：自定义游戏截图存放位置（默认独立于游戏库数据目录）

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

在 [SteamGridDB](https://www.steamgriddb.com/) 免费注册并获取 API Key，用于自动获取游戏封面图。在设置页面填入即可使用。

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
├── gamevault.db       # SQLite 数据库（WAL 模式）
├── settings.json      # 设置备份
├── covers/            # 封面图缓存（按内容哈希存储）
└── logs/              # 运行日志（时间戳为 UTC）
```

游戏截图默认存放在独立目录（可在设置中更改），按游戏名分文件夹管理。

## 📁 项目结构

```
game-vault/
├── src/                          # 前端源码 (Vue 3)
│   ├── components/               # UI 组件
│   │   ├── GameCard.vue          # 游戏卡片（封面、状态、右键菜单）
│   │   ├── GameDetail.vue        # 游戏详情抽屉（信息、存档、记录）
│   │   ├── ContextMenu.vue       # 通用右键菜单
│   │   ├── CoverPickerModal.vue  # 封面选择器弹窗
│   │   └── AboutModal.vue        # 关于对话框（更新日志）
│   ├── views/                    # 页面视图
│   │   ├── HomeView.vue          # 主页（游戏库 + 搜索筛选）
│   │   ├── StatsView.vue         # 统计页面（图表仪表盘）
│   │   ├── AchievementsView.vue  # 成就页面（全局 + 单游戏成就）
│   │   └── SettingsView.vue      # 设置页面
│   ├── stores/                   # 状态管理 (Pinia)
│   │   └── games.ts              # 游戏数据 store
│   ├── lib/                      # 工具函数
│   │   ├── tauri.ts              # Tauri 命令封装
│   │   ├── format.ts             # 格式化工具
│   │   ├── color.ts              # 颜色处理
│   │   ├── constants.ts          # 全局常量
│   │   └── useCoverImage.ts      # 封面加载 composable
│   ├── data/
│   │   └── changelog.ts          # 更新日志数据
│   ├── App.vue                   # 根组件（标题栏、主题、路由）
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
│   │   │   ├── database.rs       # SQLite 数据库操作（含预聚合账本）
│   │   │   ├── launcher.rs       # 游戏启动器
│   │   │   ├── tracker.rs        # 进程监控 + 游玩时长追踪
│   │   │   ├── platform.rs       # 平台库扫描（acf / manifest 解析）
│   │   │   ├── screenshot.rs     # 截图捕获（WGC scRGB）
│   │   │   ├── tonemap.rs        # HDR → SDR 色调映射
│   │   │   ├── sdr_white.rs      # 系统 SDR 白电平查询
│   │   │   ├── hotkey_hook.rs    # 全局键盘钩子
│   │   │   ├── cover_fetcher.rs  # SteamGridDB 封面获取
│   │   │   ├── cover_store.rs    # 封面存储（哈希索引 + 引用计数）
│   │   │   ├── llm_fetcher.rs    # LLM 游戏信息获取
│   │   │   └── achievements.rs   # 成就引擎（定义 + 检测 + 汇总）
│   │   ├── models/               # 数据模型
│   │   │   ├── game.rs           # 游戏、封面选项、筛选器
│   │   │   ├── review.rs         # 手账条目
│   │   │   ├── play_session.rs   # 游玩会话
│   │   │   ├── achievement.rs    # 成就定义与解锁记录
│   │   │   └── settings.rs       # 应用设置
│   │   └── utils/                # 工具模块
│   │       ├── constants.rs      # Rust 端常量
│   │       └── path.rs           # 路径工具
│   ├── Cargo.toml
│   └── tauri.conf.json
│
├── package.json
├── vite.config.ts
└── README.md
```

## 📝 常用命令

```bash
npm run dev                          # 仅启动 Vite 前端开发服务器
npm run build                        # 构建前端
npm run typecheck                    # TypeScript 类型检查
npm run tauri dev                    # 启动 Tauri 开发模式（前后端联动）
npm run tauri build -- --no-bundle   # 构建便携版 exe（不生成安装包）
```

## 📄 许可证

[MIT License](LICENSE)

## 👤 作者

Isaac
