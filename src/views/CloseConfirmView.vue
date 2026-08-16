<script setup lang="ts">
import { ref } from "vue";
import { api } from "@/api";
import type { CloseAction } from "@/types";

const remember = ref(false);

async function choose(action: CloseAction) {
  // cancel 不允许被记住，否则用户下次就永远关不掉窗口了
  await api.resolveClose(action, action === "cancel" ? false : remember.value);
  // 这个窗口是隐藏复用的，不会重建重挂载 —— 勾选状态要手动归零，
  // 否则「勾了记住但点了取消」之后，下次弹出来还带着上次的勾
  remember.value = false;
}
</script>

<template>
  <!-- data-tauri-drag-region 只对元素本身生效、不含子元素，
       所以标题和说明文字也要各自标注，按钮和勾选框保持可点 -->
  <div class="box" data-tauri-drag-region>
    <div class="title" data-tauri-drag-region>关闭 DeepSeek Harness</div>
    <div class="desc" data-tauri-drag-region>
      最小化到托盘后，dsh 服务会继续在后台运行。
    </div>

    <label class="remember">
      <input v-model="remember" type="checkbox" />
      记住我的选择，不再询问
    </label>

    <div class="actions">
      <button class="danger" @click="choose('quit')">退出应用</button>
      <button class="primary" @click="choose('tray')">最小化到托盘</button>
      <button @click="choose('cancel')">取消</button>
    </div>
  </div>
</template>

<style scoped>
.box {
  /* 铺满窗口、零外边距、零圆角。 */
  height: 100%;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  justify-content: center;
  gap: 10px;
  padding: 20px 22px;
  background: var(--surface);
  border: 1px solid var(--border);
}

.title {
  font-size: 15px;
  font-weight: 600;
}

.desc {
  color: var(--text-dim);
  font-size: 13px;
  line-height: 1.6;
}

.remember {
  display: flex;
  align-items: center;
  gap: 7px;
  font-size: 12px;
  color: var(--text-dim);
  cursor: pointer;
  margin-top: 2px;
}

.remember input {
  cursor: pointer;
  margin: 0;
}

.actions {
  display: flex;
  gap: 8px;
  margin-top: 6px;
}

.actions button {
  flex: 1;
  padding: 7px 4px;
}
</style>
