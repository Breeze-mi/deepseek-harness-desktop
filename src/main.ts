import { createApp } from "vue";
import { createPinia } from "pinia";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { error as logError } from "@tauri-apps/plugin-log";

import App from "./App.vue";
import router from "./router";
import { api, events } from "@/api";
import "@/assets/styles/main.css";

// 前端未捕获的错误也进运行日志（%APPDATA%\...\app.log）。

const label = getCurrentWindow().label;
window.addEventListener("error", (e) => {
	void logError(`[web:${label}] ${e.message} @ ${e.filename}:${e.lineno}`);
});
window.addEventListener("unhandledrejection", (e) => {
	void logError(`[web:${label}] unhandled rejection: ${String(e.reason)}`);
});

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

/**
 * 外壳配色跟随 DSH。真值在 Rust 侧解析（读 DSH 自己的 settings.yaml），
 * 这里只负责挂到根元素上，样式表按 `[data-theme="light"]` 覆盖暗色默认值。
 */
function applyTheme(theme: string) {
	document.documentElement.dataset.theme = theme;
}
void events.onTheme((p) => applyTheme(p.theme));

// 首帧之前把主题定下来，否则亮色下会先闪一下深色底。
Promise.allSettled([
	router.replace(initialPath(label)),
	api.getTheme().then(applyTheme),
]).finally(() => {
	app.mount("#app");

	// 主窗口在 tauri.conf.json 里是 visible:false —— WebView2 的初始底色是白的，
	// 窗口先亮必闪一屏白。等 Vue 挂载、首帧真正画出来（双 rAF）之后再显示。
	// 关闭确认框绝不能在这里 show：它是预创建常驻的，只在用户点关闭时出现。
	const win = getCurrentWindow();
	if (win.label !== "close-confirm") {
		requestAnimationFrame(() => {
			requestAnimationFrame(() => void win.show());
		});
	}
});
