import {
	createRouter,
	createWebHashHistory,
	type RouteRecordRaw,
} from "vue-router";

/**
 * 三个窗口共用同一份前端资源，靠 window label 决定初始路由。
 * 桌面端必须用 hash 路由 —— 打包后走 file:// 协议，history 模式会 404。
 */
export const routes: RouteRecordRaw[] = [
	{
		path: "/",
		name: "main",
		component: () => import("@/views/MainView.vue"),
	},
	{
		path: "/settings",
		name: "settings",
		component: () => import("@/views/SettingsView.vue"),
	},
	{
		path: "/close-confirm",
		name: "close-confirm",
		component: () => import("@/views/CloseConfirmView.vue"),
	},
];

export default createRouter({
	history: createWebHashHistory(),
	routes,
});
