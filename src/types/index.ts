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
	/** 仅下载/安装类阶段有值，0.0 ~ 1.0 */
	fraction: number | null;
	index: number;
	total: number;
	/** 瞬态进度（下载字节数、安装计数）：只刷新「当前活动」，不进已完成列表 */
	transient: boolean;
}

export interface ReadyPayload {
	url: string;
}

/**
 * 非致命问题：引导会继续走完，但有功能没就位。
 *
 */
export interface WarningPayload {
	message: string;
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

/**
 * 设置页里的关闭行为偏好。
 */
export type CloseSetting = "ask" | "quit" | "tray";

/** 单个包的版本状态，与 Rust 的 VersionStatus 对应 */
export interface VersionStatus {
	name: string;
	installed: string | null;
	/** dsh 取 registry 最新版；插件取本应用硬编码的版本 */
	target: string | null;
	/** registry 上的最新版。插件这一行它可能新于 target。 */
	latest: string | null;
	/** latest 是否严格新于 installed。由 Rust 用 semver 算好，别在前端比字符串。 */
	latestIsNewer: boolean;
	upgradable: boolean;
	note: string | null;
}

export interface UpgradeReport {
	app: VersionStatus;
	dsh: VersionStatus;
	bundle: VersionStatus;
}

export const EVENT_PROGRESS = "bootstrap:progress";
export const EVENT_READY = "bootstrap:ready";
export const EVENT_FAILED = "bootstrap:failed";
export const EVENT_WARNING = "bootstrap:warning";

/**
 * 外壳配色跟随 DSH。
 *
 * 值由 Rust 读 `$DSH_HOME/settings.yaml` 的 `ui-theme.preference` 解析而来，
 */
export interface ThemePayload {
	theme: "light" | "dark";
}

export const EVENT_THEME = "app:theme";

/**
 * 标题栏主题按钮的模式。follow = 跟随 DSH（默认）；
 * 显式亮/暗由按钮直切，立即生效，不等轮询。
 */
export type ThemeMode = "follow" | "light" | "dark";
