/** 与 Rust 侧 bootstrap 模块一一对应（serde rename_all = "camelCase"） */

export type Stage =
  | "checkingNode"
  | "downloadingNode"
  | "checkingDsh"
  | "installingDsh"
  | "initProfile"
  | "installingPlugins"
  | "verifyingPlugins"
  | "startingDsh"
  | "waitingReady";

export interface Progress {
  stage: Stage;
  label: string;
  detail: string | null;
  /** 仅下载类阶段有值，0.0 ~ 1.0 */
  fraction: number | null;
  index: number;
  total: number;
}

export interface ReadyPayload {
  url: string;
}

export interface ErrorPayload {
  message: string;
  /** 可操作的建议，没有就别硬凑 */
  hint: string | null;
  retryable: boolean;
  /** pending = 阶段未实现，按中性样式渲染，别长得像崩溃 */
  severity: "error" | "pending";
}

/** 关闭主窗口的三个选项 */
export type CloseAction = "quit" | "tray" | "cancel";

/** 单个包的版本状态，与 Rust 的 VersionStatus 对应 */
export interface VersionStatus {
  name: string;
  installed: string | null;
  /** dsh 取 registry 最新版；插件取本应用钉死的版本 */
  target: string | null;
  upgradable: boolean;
  note: string | null;
}

export interface UpgradeReport {
  dsh: VersionStatus;
  bundle: VersionStatus;
}

export const EVENT_PROGRESS = "bootstrap:progress";
export const EVENT_READY = "bootstrap:ready";
export const EVENT_FAILED = "bootstrap:failed";
