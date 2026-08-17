import { defineStore } from "pinia";
import { ref } from "vue";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api, events } from "@/api";
import type { ErrorPayload, Progress } from "@/types";

export const useBootstrapStore = defineStore("bootstrap", () => {
	const progress = ref<Progress | null>(null);
	const error = ref<ErrorPayload | null>(null);
	/**
	 * 非致命问题（目前只有「界面插件没装上」）。
	 */
	const warning = ref<string | null>(null);
	const readyUrl = ref<string | null>(null);
	const running = ref(false);
	/**
	 * 是否已经进入 DSH 界面。
	 */
	const entered = ref(false);

	let unlisteners: UnlistenFn[] = [];

	/** 订阅必须在调用 startBootstrap 之前完成，否则会漏掉最开始的几个事件 */
	async function subscribe() {
		if (unlisteners.length) return;

		unlisteners = await Promise.all([
			events.onProgress((p) => {
				progress.value = p;
				error.value = null;
			}),
			events.onReady((p) => {
				running.value = false;
				readyUrl.value = p.url;
			}),
			events.onFailed((e) => {
				running.value = false;
				error.value = e;
			}),
			events.onWarning((w) => {
				warning.value = w.message;
			}),
		]);
	}

	async function start() {
		if (running.value) return;
		await subscribe();

		error.value = null;
		warning.value = null;
		progress.value = null;
		// 必须一起清掉：留着的话，「重试」清空 warning 的瞬间跳转条件
		// （有 url 且无 warning）就成立了，会直接跳进 DSH，重试根本跑不起来。
		readyUrl.value = null;
		entered.value = false;
		running.value = true;

		try {
			await api.startBootstrap();
		} catch (e) {
			running.value = false;
			error.value = {
				message: String(e),
				hint: null,
				retryable: true,
				severity: "error",
			};
		}
	}

	return {
		progress,
		error,
		warning,
		readyUrl,
		running,
		entered,
		start,
	};
});
