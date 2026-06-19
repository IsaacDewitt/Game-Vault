# 🎮 Game Vault

轻量级 Windows 本地游戏管理器，帮助你管理游戏库、追踪游玩时长、查看统计数据。

基于 Rust + Tauri 2.0 + Vue 3 构建。

## ✨ 功能特性

### 游戏库管理
- 📂 手动添加本地游戏（支持任意 exe 文件）
- 🚀 一键启动游戏
- ✏️ 重命名游戏
- 🗑️ 删除游戏
- ❤️ 收藏功能，快速访问常玩游戏

### 游戏状态追踪
- 支持四种状态标记：
  - 🟣 未游玩 (Unplayed)
  - 🔵 游玩中 (Playing)
  - 🟢 已通关 (Completed)
  - ⚫ 已弃坑 (Abandoned)
- ⏱️ 自动检测游戏进程，记录游玩时长
- 支持多个游戏同时追踪

### 封面与信息获取
- 🖼️ 自动从 SteamGridDB 获取游戏封面
- 🤖 集成 LLM（支持小米 MiMo、DeepSeek）自动获取游戏信息
- 自动纠正游戏名称，获取准确的游戏元数据

### 数据统计分析
- 📊 游戏概览：总数、总时长、本月/今日时长
- 🥧 游戏状态分布饼图
- 🌹 游戏类型分布环形图
- 🏆 时长排行榜（Top 10，带封面和主色调提取）
- 📈 每日游玩时长趋势折线图
- 🔥 GitHub 风格年度热力图
- ⏰ 游玩时段分布热力图（24h × 7天）

### 数据管理
- 💾 导出游戏数据为 JSON 备份文件

## 🛠️ 技术栈

| 层级 | 技术 |
|------|------|
| 前端框架 | Vue 3 + TypeScript |
| UI 组件库 | Naive UI |
| 图表库 | ECharts + vue-echarts |
| 状态管理 | Pinia |
| 桌面框架 | Tauri 2.0 |
| 后端语言 | Rust |
| 数据库 | SQLite (rusqlite) |
| 构建工具 | Vite |
| 打包格式 | NSIS 安装包 |

## 📦 环境要求

- [Node.js](https://nodejs.org/) >= 18
- [Rust](https://www.rust-lang.org/tools/install) >= 1.70
- [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (Windows)
- WebView2 (Windows 10/11 自带)

## 🚀 快速开始

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
npm run tauri build
```

构建产物位于 `src-tauri/target/release/bundle/` 目录。

## ⚙️ 配置说明

### SteamGridDB API Key

用于自动获取游戏封面图，可在 [SteamGridDB](https://www.steamgriddb.com/) 免费申请 API Key。

在应用设置页面填入 API Key 即可使用封面自动获取功能。

### LLM 配置

用于智能获取游戏信息（描述、开发商、发行商等），支持以下提供商：

| 提供商 | Base URL | 模型 | 协议格式 |
|--------|----------|------|----------|
| 小米 MiMo | `https://api.xiaomimimo.com/v1` | `mimo-v2.5-pro` | OpenAI 格式 |
| DeepSeek | `https://api.deepseek.com/v1` | `deepseek-chat` | OpenAI / Anthropic 格式 |

在设置页面选择提供商后，系统会自动填充默认的 Base URL 和模型名称。

### 应用数据存储

应用数据存储在 `%APPDATA%/GameVault/` 目录：
- `gamevault.db` - SQLite 数据库
- `covers/` - 封面图缓存

## 📁 项目结构

```
game-vault/
├── src/                        # 前端源码 (Vue 3)
│   ├── components/             # UI 组件
│   │   ├── GameCard.vue        # 游戏卡片组件
│   │   ├── GameDetail.vue      # 游戏详情面板
│   │   └── ContextMenu.vue     # 右键菜单组件
│   ├── views/                  # 页面视图
│   │   ├── HomeView.vue        # 主页（游戏库）
│   │   ├── StatsView.vue       # 统计页面
│   │   └── SettingsView.vue    # 设置页面
│   ├── stores/                 # 状态管理 (Pinia)
│   │   └── games.ts            # 游戏数据状态
│   ├── lib/                    # 工具函数和 API 封装
│   │   └── tauri.ts            # Tauri API 调用封装
│   └── App.vue                 # 根组件
│
├── src-tauri/                  # 后端源码 (Rust)
│   ├── src/
│   │   ├── commands/           # Tauri 命令（前后端桥接）
│   │   │   ├── games.rs        # 游戏相关命令
│   │   │   ├── stats.rs        # 统计相关命令
│   │   │   └── settings.rs     # 设置相关命令
│   │   ├── core/               # 核心业务逻辑
│   │   │   ├── database.rs     # 数据库操作
│   │   │   ├── launcher.rs     # 游戏启动器
│   │   │   ├── tracker.rs      # 游玩时长追踪
│   │   │   ├── cover_fetcher.rs# 封面图获取
│   │   │   └── llm_fetcher.rs  # LLM 信息获取
│   │   ├── models/             # 数据模型
│   │   │   ├── game.rs         # 游戏模型
│   │   │   ├── play_session.rs # 游玩会话模型
│   │   │   └── settings.rs     # 设置模型
│   │   └── utils/              # 工具模块
│   └── Cargo.toml              # Rust 依赖配置
│
├── package.json                # Node.js 依赖配置
├── vite.config.ts              # Vite 构建配置
└── README.md
```

## 📝 开发命令

```bash
npm run dev          # 启动 Vite 开发服务器（仅前端）
npm run build        # 构建前端
npm run preview      # 预览构建产物
npm run tauri dev    # 启动 Tauri 开发模式（前后端联动）
npm run tauri build  # 构建完整应用安装包
```

## 📄 许可证

MIT License

## 👤 作者

Isaac
