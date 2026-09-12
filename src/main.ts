import { createApp } from "vue";
import { createPinia } from "pinia";
import { invoke } from "@tauri-apps/api/core";
import App from "./App.vue";

const app = createApp(App);
const pinia = createPinia();

// 全局错误处理：防止组件渲染异常导致白屏
app.config.errorHandler = (err, instance, info) => {
  console.error("[Vue Error]", err);
  console.error("Component:", instance);
  console.error("Info:", info);
};

app.use(pinia);
app.mount("#app");

// 向主进程报到：主进程的「黑屏看门狗」据此确认 WebView 已正常工作。
// 若 WebView 创建失败（如 WebView2 0x8007139F），本脚本根本不会执行，
// 主进程收不到报到即判定黑屏并自动重启一次。
invoke("frontend_ready").catch((e) => {
  console.error("[Bootstrap] 前端报到失败:", e);
});
