<script setup lang="ts">
import { onMounted, onUnmounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * 自绘标题栏。所有窗口共用这一份 —— 主窗口、设置窗口，以及将来的 companion 窗口。
 *
 *
 * 已知限制：无边框窗口在 Windows 11 上会失去「悬停最大化按钮弹出贴靠布局」
 * （Snap Layouts）。微软的修法是响应 WM_NCHITTEST 返回 HTMAXBUTTON，只能在
 * Rust 侧做，生态里有 tauri-plugin-snap-layout 之类的现成插件。暂不引入。
 */
const props = withDefaults(
  defineProps<{
    /** full = 最小化/最大化/关闭三键；*/
    controls?: "full" | "close";
    /**
     * 自定义关闭动作。不给就直接关窗口。
     * 设置窗口用它保留自己的兜底逻辑（万一没跑在独立窗口里，退回上一页而不是关掉整个应用）。
     */
    closeAction?: () => void;
  }>(),
  { controls: "full", closeAction: undefined },
);

const win = getCurrentWindow();
const isMax = ref(false);
let unlistenResize: UnlistenFn | undefined;

async function refreshMax() {
  isMax.value = await win.isMaximized().catch(() => false);
}

onMounted(async () => {
  if (props.controls !== "full") return;
  await refreshMax();
  // 拖到屏幕顶边贴靠、双击拖动区之类的最大化不经过我们的按钮，
  // 图标状态只能跟着窗口事件走
  unlistenResize = await win.onResized(() => void refreshMax());
});

onUnmounted(() => unlistenResize?.());

/** 关闭走 close()：会触发 CloseRequested，主窗口那次被 Rust 拦下弹确认框 */
function closeWin() {
  if (props.closeAction) {
    props.closeAction();
    return;
  }
  void win.close();
}
</script>

<template>
  <header class="titlebar" data-tauri-drag-region>
    <slot name="left" />

    <span class="spacer" data-tauri-drag-region />

    <slot name="right" />

    <template v-if="props.controls === 'full'">
      <button class="wc" title="最小化" @click="win.minimize()">
        <svg width="10" height="10" viewBox="0 0 10 10">
          <path d="M0 5h10" stroke="currentColor" stroke-width="1" />
        </svg>
      </button>
      <button
        class="wc"
        :title="isMax ? '还原' : '最大化'"
        @click="win.toggleMaximize()"
      >
        <svg v-if="isMax" width="10" height="10" viewBox="0 0 10 10">
          <path d="M2.5 2.5v-2h7v7h-2" fill="none" stroke="currentColor" />
          <rect x="0.5" y="2.5" width="7" height="7" fill="none" stroke="currentColor" />
        </svg>
        <svg v-else width="10" height="10" viewBox="0 0 10 10">
          <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" />
        </svg>
      </button>
    </template>

    <button class="wc close" title="关闭" @click="closeWin">
      <svg width="10" height="10" viewBox="0 0 10 10">
        <path d="M0 0l10 10M10 0L0 10" stroke="currentColor" stroke-width="1" />
      </svg>
    </button>
  </header>
</template>

<style scoped>
.titlebar {
  display: flex;
  align-items: center;
  gap: 8px;
  height: 38px;
  padding: 0 0 0 12px;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  flex: 0 0 auto;
}

.spacer {
  flex: 1 1 auto;
  align-self: stretch;
}

</style>
