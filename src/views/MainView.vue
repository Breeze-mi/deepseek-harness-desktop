<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import type { UnlistenFn } from "@tauri-apps/api/event";

import { api, events } from "@/api";
import TitleBar from "@/components/TitleBar.vue";
import { useBootstrapStore } from "@/stores/bootstrap";
import type { ThemeMode } from "@/types";
import BootstrapView from "./BootstrapView.vue";

/**
 * 应用外壳：标题栏 + 内容区（引导页或装着 DSH 页面的 iframe）。
 */
const store = useBootstrapStore();

const url = computed(() => store.readyUrl ?? "");
const shell = computed(() => store.entered && !!store.readyUrl);

// ---- 顶栏提示条 ----

const note = ref("");
let noteTimer: ReturnType<typeof setTimeout> | undefined;

function flash(msg: string) {
  note.value = msg;
  clearTimeout(noteTimer);
  noteTimer = setTimeout(() => (note.value = ""), 6000);
}

// ---- 主题按钮 ----

const themeMode = ref<ThemeMode>("follow");
/** 当前实际生效的主题，驱动按钮图标（太阳/月亮） */
const effectiveTheme = ref<"light" | "dark">("dark");
let unlistenTheme: UnlistenFn | undefined;

const themeTitle = computed(() =>
  themeMode.value === "follow"
    ? "切换明暗主题（与 DSH 双向同步）"
    : "外壳独立主题（未能同步到 DSH）：点击切换，右键恢复跟随",
);

/**
 * 点击：切到当前主题的反面。选择会写进 DSH 的外观设置（它是唯一真值，
 * 页面实时翻转、下次启动也生效），外壳这边由命令先行广播、
 */
async function toggleTheme() {
  const next = effectiveTheme.value === "light" ? "dark" : "light";
  effectiveTheme.value = next;
  themeMode.value = await api.setThemeMode(next);
  if (themeMode.value !== "follow") {
    flash("未能写入 DSH 的外观设置（原因见日志），暂时只切换了外壳主题；重启 dsh 服务多半能恢复");
  }
}

/** 右键：恢复跟随。正常路径模式本就停在跟随，这是降级态的手动出口。 */
async function followTheme() {
  themeMode.value = await api.setThemeMode("follow");
}

// ---- dsh 服务重启 ----

const restarting = ref(false);

/**
 * 重启期间先把 iframe 摘掉：旧端口已经死了，留着只会让用户看到
 * WebView2 的「无法访问此页面」。新地址由 ready 事件送回 store。
 */
async function restart() {
  if (restarting.value) return;
  restarting.value = true;
  note.value = "";

  try {
    await api.restartDsh();
  } catch (e) {
    flash(String(e));
  } finally {
    restarting.value = false;
  }
}

// ---- 服务状态灯----
const ready = ref(false);
let readyTicker: number | undefined;

const svcTitle = computed(() => {
  if (restarting.value) return "正在重启 dsh 服务…";
  return ready.value
    ? "dsh 服务运行中"
    : "dsh 服务异常，可点左上角「重启服务」";
});

function pollReady() {
  api
    .serviceReady()
    .then((v) => (ready.value = v))
    .catch(() => (ready.value = false));
}

watch(
  () => store.readyUrl,
  (url) => {
    if (url) ready.value = true;
  },
);

// ---- 生命周期 ----

onMounted(async () => {
  themeMode.value = await api.getThemeMode();
  // main.ts 在挂载前已把主题写上根元素，这里读回来当图标初值
  effectiveTheme.value =
    document.documentElement.dataset.theme === "light" ? "light" : "dark";
  unlistenTheme = await events.onTheme((p) => (effectiveTheme.value = p.theme));

  pollReady();
  readyTicker = window.setInterval(pollReady, 3000);
});

onUnmounted(() => {
  unlistenTheme?.();
  window.clearInterval(readyTicker);
});
</script>

<template>
  <div class="frame">
    <TitleBar>
      <template #left>
        <button
          v-if="shell"
          class="tb"
          :disabled="restarting"
          @click="restart"
        >
          {{ restarting ? "重启中…" : "重启服务" }}
        </button>
        <button class="tb" @click="api.openLog()">日志</button>
        <button class="tb" @click="api.openSettings()">设置</button>
        <button
          class="tb icon"
          :title="themeTitle"
          @click="toggleTheme"
          @contextmenu.prevent="followTheme"
        >
          <!-- 亮色：太阳 -->
          <svg
            v-if="effectiveTheme === 'light'"
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linecap="round"
          >
            <circle cx="12" cy="12" r="4" />
            <path
              d="M12 2v3M12 19v3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M2 12h3M19 12h3M4.9 19.1L7 17M17 7l2.1-2.1"
            />
          </svg>
          <!-- 暗色：月亮 -->
          <svg
            v-else
            width="15"
            height="15"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="2"
            stroke-linejoin="round"
          >
            <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
          </svg>
        </button>
        <span v-if="note" class="note">{{ note }}</span>
      </template>

      <template #right>
        <span class="brand" data-tauri-drag-region :title="svcTitle">
          DeepSeek Harness
        </span>
        <span
          v-if="shell"
          class="dot"
          :class="{ busy: restarting, off: !ready && !restarting }"
          :title="svcTitle"
        />
      </template>
    </TitleBar>

    <main class="content">
      <template v-if="shell">
        <div v-if="restarting" class="pending">正在重启 dsh 服务…</div>
        <iframe
          v-else
          class="stage"
          :src="url"
          allow="clipboard-read; clipboard-write; fullscreen"
        />
      </template>
      <BootstrapView v-else />
    </main>
  </div>
</template>

<style scoped>
.frame {
  height: 100%;
  display: flex;
  flex-direction: column;
}

.brand {
  font-size: 14px;
  font-weight: 600;
  color: var(--text);
  letter-spacing: 0.3px;
}

.dot {
  width: 7px;
  height: 7px;
  border-radius: 50%;
  background: var(--ok);
  flex: 0 0 auto;
}

.dot.busy {
  background: var(--warn);
  animation: pulse 1.2s ease-in-out infinite;
}

.dot.off {
  background: var(--danger);
}

@keyframes pulse {
  50% {
    opacity: 0.25;
  }
}

.note {
  font-size: 12px;
  color: var(--warn);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  max-width: 420px;
}

.content {
  flex: 1 1 auto;
  min-height: 0;
  display: flex;
  flex-direction: column;
}

.content > * {
  flex: 1 1 auto;
  min-height: 0;
}

.stage {
  width: 100%;
  border: 0;
  display: block;
  background: var(--bg);
}

.pending {
  display: flex;
  align-items: center;
  justify-content: center;
  color: var(--text-dim);
  font-size: 13px;
}
</style>
