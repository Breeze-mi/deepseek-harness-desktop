<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";

/**
 * 下拉选择。
 */
const props = defineProps<{
  modelValue: string;
  options: readonly { value: string; label: string }[];
  label?: string;
}>();

const emit = defineEmits<{ "update:modelValue": [string] }>();

const open = ref(false);
const root = ref<HTMLElement>();
const active = ref(0);

const current = computed(() =>
  props.options.find((o) => o.value === props.modelValue),
);

function onOutside(e: PointerEvent) {
  if (!root.value?.contains(e.target as Node)) close();
}

function show() {
  const i = props.options.findIndex((o) => o.value === props.modelValue);
  active.value = i < 0 ? 0 : i;
  open.value = true;
  // 捕获阶段监听：点到别的控件上也要先关掉自己
  document.addEventListener("pointerdown", onOutside, true);
}

function close() {
  open.value = false;
  document.removeEventListener("pointerdown", onOutside, true);
}

function pick(value: string) {
  emit("update:modelValue", value);
  close();
}

function onKey(e: KeyboardEvent) {
  const n = props.options.length;
  // 空列表时任何键都无事可做
  if (n === 0) return;

  if (!open.value) {
    if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
      e.preventDefault();
      show();
    }
    return;
  }

  switch (e.key) {
    case "Escape":
      e.preventDefault();
      close();
      break;
    case "ArrowDown":
      e.preventDefault();
      active.value = (active.value + 1) % n;
      break;
    case "ArrowUp":
      e.preventDefault();
      active.value = (active.value - 1 + n) % n;
      break;
    case "Enter":
    case " ":
      e.preventDefault();
      pick(props.options[active.value].value);
      break;
  }
}

onBeforeUnmount(() => document.removeEventListener("pointerdown", onOutside, true));
</script>

<template>
  <div ref="root" class="sel" @keydown="onKey">
    <button
      type="button"
      class="trigger"
      :class="{ open }"
      :aria-label="props.label"
      :aria-expanded="open"
      aria-haspopup="listbox"
      @click="open ? close() : show()"
    >
      <span class="text">{{ current?.label ?? "" }}</span>
      <svg class="chev" width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
        <path
          d="M1.5 3.5L5 7l3.5-3.5"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
          stroke-linejoin="round"
        />
      </svg>
    </button>

    <ul v-if="open" class="menu" role="listbox">
      <li
        v-for="(o, i) in props.options"
        :key="o.value"
        role="option"
        class="opt"
        :class="{ active: i === active, on: o.value === props.modelValue }"
        :aria-selected="o.value === props.modelValue"
        @mouseenter="active = i"
        @click="pick(o.value)"
      >
        <svg class="tick" width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <path
            d="M2 6.4L4.6 9 10 3.4"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
          />
        </svg>
        {{ o.label }}
      </li>
    </ul>
  </div>
</template>

<style scoped>
.sel {
  position: relative;
  flex: 0 0 auto;
}

.trigger {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  /* 固定宽度：选不同项时按钮不该忽宽忽窄 */
  min-width: 156px;
  padding: 6px 10px;
  font-size: 13px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--surface-2);
  color: var(--text);
}

.trigger:hover,
.trigger.open {
  background: var(--hover);
  border-color: var(--hover-border);
}

.trigger:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

.chev {
  color: var(--text-dim);
  flex: 0 0 auto;
  transition: transform 0.15s;
}

.trigger.open .chev {
  transform: rotate(180deg);
}

.menu {
  position: absolute;
  top: calc(100% + 4px);
  right: 0;
  z-index: 20;
  min-width: 100%;
  margin: 0;
  padding: 4px;
  list-style: none;
  border-radius: 8px;
  border: 1px solid var(--border);
  background: var(--surface-2);
  /* 浮层要浮起来：暗色下靠阴影，亮色下阴影更明显 */
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
  animation: pop 0.12s ease-out;
}

@keyframes pop {
  from {
    opacity: 0;
    transform: translateY(-4px);
  }
}

.opt {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  border-radius: 5px;
  font-size: 13px;
  white-space: nowrap;
  cursor: pointer;
}

/* 高亮跟着键盘/鼠标走，选中项只用对勾表示 —— 两者分开才不会互相冒充 */
.opt.active {
  background: var(--hover);
}

.tick {
  flex: 0 0 auto;
  color: var(--accent);
  visibility: hidden;
}

.opt.on .tick {
  visibility: visible;
}
</style>
