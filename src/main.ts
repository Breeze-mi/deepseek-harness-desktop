import { createApp } from "vue";
import { createPinia } from "pinia";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { error as logError } from "@tauri-apps/plugin-log";

import App from "./App.vue";
import router from "./router";
import "@/assets/styles/main.css";

// 前端未捕获的错误也进运行日志（%APPDATA%\...\app.log）。
// 打包后 webview 的 console 没人看得见 —— 引导页/设置页若崩了，
// 以前只会无声白屏，日志里查无此事。
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

router.replace(initialPath(getCurrentWindow().label)).finally(() => {
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
