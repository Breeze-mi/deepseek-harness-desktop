<script setup lang="ts">
/**
 * 滑动开关。
 */
const props = defineProps<{
  modelValue: boolean;
  label?: string;
}>();

const emit = defineEmits<{ "update:modelValue": [boolean] }>();
</script>

<template>
  <button
    type="button"
    role="switch"
    class="sw"
    :class="{ on: props.modelValue }"
    :aria-checked="props.modelValue"
    :aria-label="props.label"
    @click="emit('update:modelValue', !props.modelValue)"
  >
    <span class="knob" />
  </button>
</template>

<style scoped>
.sw {
  width: 40px;
  height: 22px;
  padding: 0;
  border-radius: 999px;
  border: 1px solid var(--text-dim);
  background: transparent;
  cursor: pointer;
  flex: 0 0 auto;
  transition: background 0.18s, border-color 0.18s;
}

.sw:hover {
  background: var(--surface-2);
  border-color: var(--text);
}

.sw.on,
.sw.on:hover {
  background: var(--accent);
  border-color: var(--accent);
}

.knob {
  display: block;
  width: 12px;
  height: 12px;
  margin-left: 4px;
  border-radius: 50%;
  background: var(--text-dim);
  transition: transform 0.18s, background 0.18s, width 0.18s, height 0.18s;
}

.sw:hover .knob {
  width: 14px;
  height: 14px;
  margin-left: 3px;
  background: var(--text);
}

.sw.on .knob,
.sw.on:hover .knob {
  background: #fff;
  transform: translateX(18px);
}

.sw:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}
</style>
