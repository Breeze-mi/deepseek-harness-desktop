import { defineStore } from "pinia";
import { ref } from "vue";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api, events } from "@/api";
import type { ErrorPayload, Progress } from "@/types";

export const useBootstrapStore = defineStore("bootstrap", () => {
  const progress = ref<Progress | null>(null);
  const error = ref<ErrorPayload | null>(null);
  const readyUrl = ref<string | null>(null);
  const running = ref(false);

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
    ]);
  }

  async function start() {
    if (running.value) return;
    await subscribe();

    error.value = null;
    progress.value = null;
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

  function dispose() {
    unlisteners.forEach((fn) => fn());
    unlisteners = [];
  }

  return { progress, error, readyUrl, running, start, dispose };
});
