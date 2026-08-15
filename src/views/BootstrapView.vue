<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { api } from "@/api";
import { useBootstrapStore } from "@/stores/bootstrap";

const store = useBootstrapStore();

/** 已用时（秒）。长时间停在同一阶段时，这是用户判断「是不是卡死了」的唯一线索 */
const elapsed = ref(0);
let ticker: number | undefined;

/**
 * 已完成的事实，形如「已检测到 Node.js v24.19.0」。
 * 只留最近几条 —— 完整流水属于日志，不该堆在启动屏上。
 */
const facts = ref<string[]>([]);
const MAX_FACTS = 3;

const barPercent = computed(() => {
  const p = store.progress;
  if (!p) return 4;
  // 下载类阶段有真实百分比，其余按阶段序号粗略推进
  const within = p.fraction ?? 0.5;
  return Math.max(4, ((p.index - 1 + within) / p.total) * 100);
});

const title = computed(() => {
  if (store.error) {
    return store.error.severity === "pending" ? "尚未完成" : "启动失败";
  }
  return store.progress ? `${store.progress.label}…` : "正在启动…";
});

// detail 里带的是「做成了什么」，攒起来当作已完成事实
watch(
  () => store.progress?.detail,
  (detail) => {
    if (!detail) return;
    if (facts.value.at(-1) === detail) return;
    facts.value = [...facts.value, detail].slice(-MAX_FACTS);
  },
);

function restart() {
  facts.value = [];
  elapsed.value = 0;
  store.start();
}

onMounted(() => {
  ticker = window.setInterval(() => (elapsed.value += 1), 1000);
  store.start();
});

onUnmounted(() => {
  window.clearInterval(ticker);
  store.dispose();
});

// 就绪后把主窗口整体导航到 DSH Web UI。
// 从这一刻起本 Vue 应用就不在主窗口里了 —— 设置等自有 UI 走独立窗口承载。
watch(
  () => store.readyUrl,
  (url) => {
    if (url) window.location.replace(url);
  },
);
</script>

<template>
  <div class="wrap">
    <div class="ring" :class="{ stopped: !!store.error }" />

    <div class="body">
      <div class="title" :class="{ bad: store.error?.severity === 'error' }">
        {{ title }}
      </div>

      <ul v-if="facts.length" class="facts">
        <li v-for="f in facts" :key="f">
          <span class="tick">✓</span>{{ f }}
        </li>
      </ul>

      <template v-if="store.error">
        <div class="msg">{{ store.error.message }}</div>
        <div v-if="store.error.hint" class="hint">{{ store.error.hint }}</div>
        <div class="actions">
          <button v-if="store.error.retryable" class="primary" @click="restart">
            重试
          </button>
          <button @click="api.openSettings()">设置</button>
        </div>
      </template>

      <template v-else>
        <div class="bar">
          <div class="bar-fill" :style="{ width: `${barPercent}%` }" />
        </div>
        <div class="elapsed">已用时 {{ elapsed }} 秒</div>
      </template>
    </div>
  </div>
</template>

<style scoped>
.wrap {
  height: 100%;
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 42px;
  padding: 32px 48px;
}

.ring {
  width: 104px;
  height: 104px;
  flex-shrink: 0;
  border: 3px solid var(--surface-2);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 1.1s cubic-bezier(0.5, 0.1, 0.4, 0.9) infinite;
}

.ring.stopped {
  animation: none;
  border-top-color: var(--border);
}

@keyframes spin {
  to {
    transform: rotate(360deg);
  }
}

.body {
  width: 100%;
  max-width: 420px;
}

.title {
  font-size: 17px;
  font-weight: 600;
  margin-bottom: 10px;
}

.title.bad {
  color: var(--danger);
}

.facts {
  list-style: none;
  margin: 0 0 16px;
  padding: 0;
  display: grid;
  gap: 5px;
  font-size: 13px;
  color: var(--ok);
}

.facts li {
  display: flex;
  align-items: center;
  gap: 7px;
}

.tick {
  flex-shrink: 0;
}

.bar {
  height: 5px;
  background: var(--surface-2);
  border-radius: 3px;
  overflow: hidden;
}

.bar-fill {
  height: 100%;
  background: var(--accent);
  border-radius: 3px;
  transition: width 0.35s ease;
}

.elapsed {
  margin-top: 8px;
  text-align: right;
  font-size: 12px;
  color: var(--text-dim);
}

.msg {
  white-space: pre-wrap;
  line-height: 1.6;
  user-select: text;
  font-size: 13px;
}

.hint {
  margin-top: 10px;
  padding: 10px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text-dim);
  line-height: 1.6;
  font-size: 12px;
}

.actions {
  display: flex;
  gap: 8px;
  margin-top: 14px;
}
</style>
