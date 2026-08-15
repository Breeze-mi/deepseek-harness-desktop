import { createApp } from "vue";
import { createPinia } from "pinia";
import { getCurrentWindow } from "@tauri-apps/api/window";

import App from "./App.vue";
import router from "./router";
import "@/assets/styles/main.css";

/**
 * 设置窗口与关闭确认框加载的是同一个 index.html，
 * 用 window label 决定进哪个视图 —— 比在 URL 里拼 hash 稳，
 * 打包后路径变化也不受影响。
 */
function initialPath(label: string): string {
  switch (label) {
    case "settings":
      return "/settings";
    case "close-confirm":
      return "/close-confirm";
    default:
      return "/";
  }
}

const app = createApp(App);
app.use(createPinia());
app.use(router);

router.replace(initialPath(getCurrentWindow().label)).finally(() => {
  app.mount("#app");
});
