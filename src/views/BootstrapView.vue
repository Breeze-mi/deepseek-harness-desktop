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

/** 已就绪，但有东西没装上 —— 停在这一屏等用户确认，不自动跳走 */
const heldByWarning = computed(() => !!store.warning && !!store.readyUrl);

const title = computed(() => {
  if (store.error) {
    return store.error.severity === "pending" ? "尚未完成" : "启动失败";
  }
  if (heldByWarning.value) return "已就绪，但有一项没装上";
  return store.progress ? `${store.progress.label}…` : "正在启动…";
});

// detail 里带的是「做成了什么」，攒起来当作已完成事实。
// 瞬态进度（下载字节数、安装计数）除外 —— 那些一秒好几条，
// 进了列表就会把真正的里程碑挤掉（真机截图里出现过两条 30/31、31/31）。
watch(
  () => store.progress,
  (p) => {
    const detail = p?.detail;
    if (!detail || p?.transient) return;
    if (facts.value.at(-1) === detail) return;
    facts.value = [...facts.value, detail].slice(-MAX_FACTS);
  },
);

/** 瞬态进度的展示位：进度条下方单独一行，被下一条覆盖，不留痕 */
const activity = computed(() => {
  const p = store.progress;
  return p?.transient && p.detail ? p.detail : "";
});

/** 没有真实比例时进度条打脉冲，让「在跑」和「卡死」看得出区别 */
const indeterminate = computed(() => store.progress?.fraction == null);

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
  
});

function enter() {
  if (store.readyUrl) store.entered = true;
}

// 就绪后交给外壳显示 DSH 界面。

watch(
  [() => store.readyUrl, () => store.warning],
  ([url, warning]) => {
    if (url && !warning) enter();
  },
);
</script>

<template>
  <div class="wrap">
    <div class="ring" :class="{ stopped: !!store.error || heldByWarning }" />

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
          <button @click="api.openLog()">打开日志</button>
          <button @click="api.openSettings()">设置</button>
        </div>
      </template>

      <template v-else-if="heldByWarning">
        <div class="msg warn">{{ store.warning }}</div>
        <div class="actions">
          <button class="primary" @click="enter">继续使用</button>
          <button @click="restart">重试安装</button>
          <button @click="api.openLog()">打开日志</button>
        </div>
      </template>

      <template v-else>
        <div class="bar">
          <div
            class="bar-fill"
            :class="{ pulse: indeterminate }"
            :style="{ width: `${barPercent}%` }"
          />
        </div>
        <div class="meta">
          <span class="activity">{{ activity }}</span>
          <span>已用时 {{ elapsed }} 秒</span>
        </div>
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

/* 没有真实比例时打脉冲：静止的满色条和卡死没法区分 */
.bar-fill.pulse {
  animation: pulse 1.6s ease-in-out infinite;
}

@keyframes pulse {
  50% {
    opacity: 0.55;
  }
}

.meta {
  margin-top: 8px;
  display: flex;
  justify-content: space-between;
  gap: 12px;
  font-size: 12px;
  color: var(--text-dim);
  /* 固定高度：activity 在空与非空之间切换时布局不能跳 */
  min-height: 18px;
}

.activity {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.msg {
  white-space: pre-wrap;
  line-height: 1.6;
  user-select: text;
  font-size: 13px;
}

/* 警告不是崩溃：给一条边而不是整段红字，别吓人 */
.msg.warn {
  padding: 10px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-left: 3px solid var(--warn, var(--accent));
  border-radius: 6px;
  /* 警告可能携带 pnpm 的整段输出，滚动收纳，别把按钮挤出窗口 */
  max-height: 180px;
  overflow-y: auto;
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
