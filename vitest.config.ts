import { defineConfig } from "vitest/config";

/**
 * 前端单测配置。
 *
 * 刻意**不**复用 `vite.config.ts`：那个配置带着 `@vitejs/plugin-vue` 与 Tauri 的
 * dev server 参数（固定端口 1420、忽略 src-tauri），对纯逻辑测试全是负担。
 *
 * 当前覆盖面：`src/lib/*.ts` 里的纯函数——筛选排序口径（gameFilter）、
 * 时长格式化（format）、成就分组/完成度/量纲文案（achievements）、
 * 评价极简 Markdown 解析（richText）、启动三态 reducer（launchState）、
 * 手账失败草稿簿（reviewDrafts）。
 * 这几处最容易在重构中静默改变行为，且都能在不碰 DOM / Tauri IPC 的前提下测干净。
 * 改上述任一文件前先补/改对应用例；新纯逻辑也请放进 `src/lib/` 以便纳入覆盖。
 */
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
