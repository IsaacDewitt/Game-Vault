import { defineConfig } from "vitest/config";

/**
 * 前端单测配置。
 *
 * 刻意**不**复用 `vite.config.ts`：那个配置带着 `@vitejs/plugin-vue` 与 Tauri 的
 * dev server 参数（固定端口 1420、忽略 src-tauri），对纯逻辑测试全是负担。
 *
 * 当前覆盖面：`src/lib/*.ts` 里的纯函数——筛选排序口径、时长格式化、
 * 成就完成度算法。这三个是最容易在重构中静默改变行为的地方，且都能在不碰
 * DOM / Tauri IPC 的前提下测干净。
 */
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
