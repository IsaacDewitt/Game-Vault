# Game Vault - 游戏管理器

轻量级 Windows 本地游戏管理器，基于 Rust + Tauri 2.0 + Vue 3 构建。

## 功能

- 📂 手动添加本地游戏（支持任意 exe 文件）
- 🚀 一键启动游戏
- ⏱️ 自动记录游戏时长（进程监控）
- 📊 多维度统计图表（时长排行、每日趋势）
- 🖼️ 自动获取游戏封面图（SteamGridDB）
- 🤖 LLM 获取游戏信息（描述、开发商、发行商等）
- ❤️ 收藏管理

## 技术栈

- **后端**: Rust + Tauri 2.0
- **前端**: Vue 3 + TypeScript + Naive UI + ECharts
- **数据库**: SQLite
- **打包**: NSIS 安装包

## 开发环境搭建

### 前置要求

1. **Node.js** (>= 18)
2. **Rust** (>= 1.70)
3. **Visual Studio Build Tools** (Windows)
4. **WebView2** (Windows 10/11 自带)

### 安装依赖

```bash
npm install
```

### 开发模式

```bash
npm run tauri dev
```

### 构建 Windows EXE

```bash
npm run tauri build
```

构建产物位于 `src-tauri/target/release/bundle/` 目录。

## 项目结构

```
game-vault/
├── src/                    # 前端 (Vue 3)
│   ├── components/         # UI 组件
│   ├── views/              # 页面
│   ├── stores/             # 状态管理 (Pinia)
│   └── lib/                # 工具函数和 API 封装
│
├── src-tauri/              # 后端 (Rust)
│   ├── src/
│   │   ├── commands/       # Tauri 命令（前后端桥接）
│   │   ├── core/           # 核心业务逻辑
│   │   ├── models/         # 数据模型
│   │   └── utils/          # 工具模块
│   └── Cargo.toml
│
└── package.json
```

## 配置

应用数据存储在 `%APPDATA%/GameVault/` 目录：
- `gamevault.db` - SQLite 数据库
- `covers/` - 封面图缓存

## 许可证

MIT
