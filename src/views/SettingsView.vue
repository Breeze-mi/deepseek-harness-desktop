<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useRouter } from "vue-router";
import { api } from "@/api";
import type { UpgradeReport } from "@/types";

const router = useRouter();
const closeAction = ref<string | null>(null);
const shortcut = ref("");
const savedShortcut = ref("");
const ready = ref(false);
const saving = ref(false);
const notifyOnDone = ref(true);

const report = ref<UpgradeReport | null>(null);
const checking = ref(false);
const upgrading = ref(false);
const confirming = ref(false);
const upgraded = ref("");
const pluginsUpgraded = ref("");
const upgradeError = ref("");
const confirmingPlugins = ref(false);
/** dsh 已被停掉、必须重启应用才能恢复 */
const serviceStopped = ref(false);

const CLOSE_LABEL: Record<string, string> = {
  quit: "直接退出应用",
  tray: "最小化到托盘",
};

const PRESETS = ["Alt+Space", "Ctrl+Shift+Space", "Alt+D", "Ctrl+Alt+D"];

const dirty = computed(() => shortcut.value.trim() !== savedShortcut.value);
const disabled = computed(() => savedShortcut.value === "");

async function load() {
  const [close, sc, isReady, notify] = await Promise.all([
    api.getCloseAction(),
    api.getGlobalShortcut(),
    api.serviceReady(),
    api.getNotifyOnDone(),
  ]);
  closeAction.value = close;
  savedShortcut.value = sc;
  shortcut.value = sc;
  ready.value = isReady;
  notifyOnDone.value = notify;
}

async function toggleNotify() {
  notifyOnDone.value = !notifyOnDone.value;
  await api.setNotifyOnDone(notifyOnDone.value);
}

async function saveShortcut(value?: string) {
  saving.value = true;
  try {
    const next = (value ?? shortcut.value).trim();
    await api.setGlobalShortcut(next);
    savedShortcut.value = next;
    shortcut.value = next;
  } finally {
    saving.value = false;
  }
}

async function resetClose() {
  await api.resetCloseAction();
  closeAction.value = await api.getCloseAction();
}

async function checkUpgrades() {
  checking.value = true;
  upgradeError.value = "";
  try {
    report.value = await api.checkUpgrades();
  } catch (e) {
    upgradeError.value = String(e);
  } finally {
    checking.value = false;
  }
}

/**
 * 两段式确认。升级前会停掉 dsh 子进程 —— 它带原生模块，
 * 运行期间 npm 覆盖不了 —— 所以当前会话必然中断。
 * 升完会自动重起服务并把主窗口导到新端口，不用重启整个应用。
 */
async function upgradeDsh() {
  if (!confirming.value) {
    confirming.value = true;
    return;
  }
  confirming.value = false;
  upgrading.value = true;
  upgradeError.value = "";
  // 服务在这一刻就停了。标记要设在 try 之前 ——
  // 万一升级或重启失败，用户回不去，得靠那个重启按钮兜底。
  serviceStopped.value = true;
  try {
    upgraded.value = await api.upgradeDsh();
    // 走到这里说明服务已经重新起来了
    serviceStopped.value = false;
    pluginsUpgraded.value = "";
    // 刷新版本卡片，否则「升级 dsh」按钮会带着过期状态继续显示
    await checkUpgrades();
  } catch (e) {
    upgradeError.value = String(e);
  } finally {
    upgrading.value = false;
  }
}

/**
 * 正常情况下设置跑在独立窗口里，关掉即可。
 * 但万一是在主窗口里打开的，关窗口会把整个应用关掉，
 * 所以按 window label 区分：不是设置窗口就退回引导页。
 */
async function back() {
  const win = getCurrentWindow();
  if (win.label === "settings") {
    await win.close();
  } else {
    router.replace("/");
  }
}

/**
 * 把界面插件升到上游最新版。
 *
 * 本应用固定的版本只是「新装时的已知可用版本」，不是上限 ——
 * 用户升上去之后引导流程不会再把他降回来。
 */
async function upgradePlugins() {
  const latest = report.value?.bundle.latest;
  if (!latest) return;

  if (!confirmingPlugins.value) {
    confirmingPlugins.value = true;
    return;
  }
  confirmingPlugins.value = false;
  upgrading.value = true;
  upgradeError.value = "";
  serviceStopped.value = true;
  try {
    await api.upgradePlugins(latest);
    serviceStopped.value = false;
    upgraded.value = "";
    pluginsUpgraded.value = latest;
    await checkUpgrades();
  } catch (e) {
    upgradeError.value = String(e);
  } finally {
    upgrading.value = false;
  }
}

onMounted(async () => {
  await load();
  // 托盘的「检查更新」就是直接打开本页，所以挂载时自动查一次，
  // 否则那个菜单项等于只是「打开设置」，名不副实。
  // 不 await：让页面先渲染出来，网络查询在后台跑。
  checkUpgrades();
});
</script>

<template>
  <div class="page">
    <header class="head">
      <h1>设置</h1>
      <div class="head-actions">
        <!-- 日志入口放在最显眼处：装机环境（执行策略、nvm4w、代理、镜像）
             千差万别，出问题时这个文件是唯一靠得住的线索 -->
        <button @click="api.openLog()">打开日志</button>
        <button @click="back">关闭</button>
      </div>
    </header>

    <section class="card">
      <div class="card-title">全局快捷键</div>
      <p class="dim">
        在任何程序里按下都能把主窗口调到前台；已在前台时再按一次收起。
      </p>

      <div class="field">
        <input
          v-model="shortcut"
          class="input"
          placeholder="例如 Alt+Space，留空则关闭"
          spellcheck="false"
          @keyup.enter="saveShortcut()"
        />
        <button class="primary" :disabled="!dirty || saving" @click="saveShortcut()">
          保存
        </button>
      </div>

      <div class="presets">
        <button
          v-for="p in PRESETS"
          :key="p"
          class="chip"
          :class="{ on: savedShortcut === p }"
          @click="saveShortcut(p)"
        >
          {{ p }}
        </button>
        <button class="chip" :class="{ on: disabled }" @click="saveShortcut('')">
          关闭
        </button>
      </div>

      <p class="warn">
        <b>Alt+Space</b> 是 Windows 的系统菜单快捷键，被占用后原功能会失效。
        介意的话换一个组合，或直接关闭。
      </p>

      <div class="row">
        <span class="value">
          {{ disabled ? "已关闭" : `当前：${savedShortcut}` }}
        </span>
        <span class="value">
          dsh 服务{{ ready ? "运行中" : "未就绪" }}
        </span>
      </div>
    </section>

    <section class="card">
      <div class="card-title">任务完成通知</div>
      <p class="dim">
        智能体跑完一轮任务时发一条系统通知，并让任务栏图标闪烁提醒。
        只在主窗口不在前台时提醒 —— 你正看着的时候不打扰。
      </p>
      <div class="row">
        <span class="value">{{ notifyOnDone ? "已开启" : "已关闭" }}</span>
        <button @click="toggleNotify">{{ notifyOnDone ? "关闭" : "开启" }}</button>
      </div>
    </section>

    <section class="card">
      <div class="card-title">关闭主窗口时</div>
      <div class="row">
        <span class="value">
          {{ closeAction ? (CLOSE_LABEL[closeAction] ?? closeAction) : "每次询问" }}
        </span>
        <button v-if="closeAction" @click="resetClose">改回每次询问</button>
      </div>
    </section>

    <section class="card">
      <div class="card-title">版本与更新</div>

      <template v-if="report">
        <div class="pkg">
          <div class="pkg-head">
            <span>DeepSeek Harness（dsh）</span>
            <span class="value">{{ report.dsh.installed ?? "未安装" }}</span>
          </div>
          <p v-if="report.dsh.upgradable" class="dim small">
            可升级到 {{ report.dsh.target }}
          </p>
          <p v-else-if="report.dsh.note" class="dim small">
            {{ report.dsh.note }}
          </p>
        </div>

        <div class="pkg">
          <div class="pkg-head">
            <span>界面插件（含鲸鱼娘）</span>
            <span class="value">{{ report.bundle.installed ?? "未安装" }}</span>
          </div>
          <p v-if="report.bundle.upgradable" class="dim small">
            重启应用后会自动装到 {{ report.bundle.target }}
          </p>
          <p v-else-if="report.bundle.note" class="dim small">
            {{ report.bundle.note }}
          </p>
          <button
            v-if="report.bundle.latestIsNewer && !serviceStopped"
            class="chip"
            :disabled="upgrading"
            @click="upgradePlugins"
          >
            <template v-if="confirmingPlugins">
              确认升到 {{ report.bundle.latest }}？会断开当前会话
            </template>
            <template v-else>升级到 {{ report.bundle.latest }}</template>
          </button>
        </div>
      </template>
      <p v-else class="dim">尚未检查。dsh 与命令行共用同一份安装。</p>

      <p v-if="upgradeError" class="warn">
        {{ upgradeError }}
        <template v-if="serviceStopped">
          <br />dsh 服务已停止，请重启应用恢复。
        </template>
      </p>
      <p v-else-if="upgraded" class="warn">
        dsh 已升级到 {{ upgraded }}，服务已重启。
      </p>
      <p v-else-if="pluginsUpgraded" class="warn">
        界面插件已升级到 {{ pluginsUpgraded }}，服务已重启。
      </p>

      <div class="row">
        <button :disabled="checking || upgrading" @click="checkUpgrades">
          {{ checking ? "检查中…" : "检查更新" }}
        </button>

        <button
          v-if="serviceStopped"
          class="primary"
          :disabled="upgrading"
          @click="api.restartApp()"
        >
          {{ upgrading ? "升级中…" : "立即重启" }}
        </button>
        <button
          v-else-if="report?.dsh.upgradable"
          class="primary"
          @click="upgradeDsh"
        >
          <template v-if="confirming">确认？会断开当前会话</template>
          <template v-else>升级 dsh</template>
        </button>
      </div>
    </section>
  </div>
</template>

<style scoped>
.page {
  padding: 24px 28px;
  height: 100%;
  overflow-y: auto;
}

.head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin-bottom: 18px;
}

.head-actions {
  display: flex;
  gap: 8px;
}

h1 {
  font-size: 18px;
  margin: 0;
}

.card {
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: 10px;
  padding: 16px 18px;
  margin-bottom: 12px;
}

.card-title {
  font-weight: 600;
  margin-bottom: 10px;
}

.field {
  display: flex;
  gap: 8px;
  margin: 12px 0 10px;
}

.input {
  flex: 1;
  padding: 7px 10px;
  border-radius: 6px;
  border: 1px solid var(--border);
  background: var(--bg);
  color: var(--text);
  font-family: inherit;
  font-size: 13px;
  outline: none;
}

.input:focus {
  border-color: var(--accent);
}

.presets {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
}

.chip {
  padding: 4px 10px;
  font-size: 12px;
  border-radius: 999px;
}

.chip.on {
  border-color: var(--accent);
  color: var(--accent);
}

.row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-top: 14px;
}

.value {
  color: var(--text-dim);
  font-size: 13px;
}

.dim {
  color: var(--text-dim);
  line-height: 1.7;
  margin: 0;
  font-size: 13px;
}

.small {
  font-size: 12px;
}

.pkg {
  padding: 9px 0;
  border-bottom: 1px solid var(--border);
}

.pkg:last-of-type {
  border-bottom: none;
}

.pkg-head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 12px;
  font-size: 13px;
  margin-bottom: 3px;
}

.warn {
  margin: 12px 0 0;
  padding: 9px 11px;
  background: var(--bg);
  border: 1px solid var(--border);
  border-radius: 6px;
  color: var(--text-dim);
  font-size: 12px;
  line-height: 1.7;
}

button:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}
</style>
