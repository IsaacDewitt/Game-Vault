import { createApp } from "vue";
import { createPinia } from "pinia";
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
