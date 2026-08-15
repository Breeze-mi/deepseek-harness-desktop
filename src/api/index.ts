import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  EVENT_FAILED,
  EVENT_PROGRESS,
  EVENT_READY,
  type CloseAction,
  type ErrorPayload,
  type Progress,
  type ReadyPayload,
  type UpgradeReport,
} from "@/types";

export const api = {
  /** 启动引导。立刻返回，进度走事件。 */
  startBootstrap: () => invoke<void>("start_bootstrap"),

  resolveClose: (action: CloseAction, remember: boolean) =>
    invoke<void>("resolve_close", { action, remember }),

  openSettings: () => invoke<void>("open_settings"),

  getCloseAction: () => invoke<string | null>("get_close_action"),

  resetCloseAction: () => invoke<void>("reset_close_action"),

  /** 调出/收起主窗口，与全局快捷键同一个动作 */
  toggleMain: () => invoke<void>("toggle_main"),

  getGlobalShortcut: () => invoke<string>("get_global_shortcut"),

  /** 空字符串 = 关闭全局快捷键 */
  setGlobalShortcut: (value: string) =>
    invoke<void>("set_global_shortcut", { value }),

  /** dsh 服务是否已就绪 */
  serviceReady: () => invoke<boolean>("service_ready"),

  getNotifyOnDone: () => invoke<boolean>("get_notify_on_done"),

  /** 任务完成通知开关，改完立刻生效 */
  setNotifyOnDone: (value: boolean) =>
    invoke<void>("set_notify_on_done", { value }),

  /** 查询 dsh 与界面插件的版本状态。走网络，可能要几秒。 */
  checkUpgrades: () => invoke<UpgradeReport>("check_upgrades"),

  /**
   * 升级 dsh 到最新版。
   * 会先停掉 dsh 子进程（原生模块被锁时 npm 覆盖不了），
   * 因此调用前必须让用户确认「会中断当前会话且需要重启应用」。
   * 返回升级后的版本号。
   */
  upgradeDsh: () => invoke<string>("upgrade_dsh"),

  restartApp: () => invoke<void>("restart_app"),
};

export const events = {
  onProgress: (cb: (p: Progress) => void): Promise<UnlistenFn> =>
    listen<Progress>(EVENT_PROGRESS, (e) => cb(e.payload)),

  onReady: (cb: (p: ReadyPayload) => void): Promise<UnlistenFn> =>
    listen<ReadyPayload>(EVENT_READY, (e) => cb(e.payload)),

  onFailed: (cb: (p: ErrorPayload) => void): Promise<UnlistenFn> =>
    listen<ErrorPayload>(EVENT_FAILED, (e) => cb(e.payload)),
};
