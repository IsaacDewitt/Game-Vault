export interface ChangelogEntry {
  version: string;
  date: string;
  changes: string[];
}

export const changelog: ChangelogEntry[] = [
  {
    version: "0.4.1",
    date: "2026-06-21",
    changes: [
      "提取颜色函数和数据库辅助函数，消除重复代码",
      "CoverFetcher 和 Settings 加载改为 Result 错误传播，消除 expect/unwrap panic",
      "ZIP 存档导入增加路径穿越防护（绝对路径检查、canonicalize 校验）",
      "修复 HLTB 值为 0 时误判为无数据的 bug",
      "封面组件新增渲染失败检测，损坏的 base64 数据自动回退到占位符",
      "App.vue 改用 provide/inject 替代 window 全局属性",
      "StatsView watch 优化为 shallow，避免深层遍历封面缓存",
      "SettingsView API Key 输入改为密码类型，新增定时器清理",
      "games store 增量更新 exe_version，避免竞态覆盖",
      "formatDate 增加无效日期和未来时间的健壮性处理",
    ],
  },
  {
    version: "0.4.0",
    date: "2026-06-21",
    changes: [
      "新增 SteamGridDB 封面选择器，支持从多张可选封面中挑选",
      "新增批量刷新游戏信息功能",
      "添加自定义标题栏与关于对话框",
      "游戏详情页显示最近三次游玩记录",
    ],
  },
  {
    version: "0.3.0",
    date: "2026-06-21",
    changes: [
      "添加游戏版本号读取、存档路径管理和存档备份导出/导入功能",
      "游戏详情页支持点击重新选择可执行文件路径并自动刷新版本号",
      "移除 LLM 提供商选择，统一按协议区分认证",
      "新增 OpenAI web_search 工具调用支持",
      "修复手动更换封面后图片不显示的问题",
      "修复多个 bug 并统一提取魔法值为常量",
    ],
  },
  {
    version: "0.2.0",
    date: "2026-06-19",
    changes: [
      "添加数据导入/恢复功能",
      "添加游戏筛选分类与最近游玩",
      "添加游戏会话历史与 HLTB 时长显示",
      "添加游戏类型统计（饼图和条形图）",
      "支持系统托盘最小化",
      "支持主题与强调色自定义",
      "关闭窗口时弹出自定义确认对话框",
      "精简主页筛选选项，只保留全部、收藏、已通关",
    ],
  },
  {
    version: "0.1.0",
    date: "2026-06-19",
    changes: [
      "项目初始版本，支持本地游戏库管理",
      "支持通过 SteamGridDB 自动获取游戏封面",
      "支持 LLM 智能获取游戏信息",
      "添加游戏卡片右键菜单（刷新信息、删除封面）",
      "添加主页空白区域自定义右键菜单",
      "优化游戏详情页封面布局",
      "添加游戏状态管理与统计功能",
      "游戏统计页面响应式布局优化",
      "时长排行条形图用游戏图标和封面主题色替换文字名称",
    ],
  },
];
