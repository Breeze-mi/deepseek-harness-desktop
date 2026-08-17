import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
	EVENT_FAILED,
	EVENT_PROGRESS,
	EVENT_READY,
	EVENT_THEME,
	EVENT_WARNING,
	type CloseAction,
	type CloseSetting,
	type ErrorPayload,
	type Progress,
	type ReadyPayload,
	type ThemeMode,
	type ThemePayload,
	type UpgradeReport,
	type WarningPayload,
} from "@/types";

export const api = {
	/** 启动引导。立刻返回，进度走事件。 */
	startBootstrap: () => invoke<void>("start_bootstrap"),

	resolveClose: (action: CloseAction, remember: boolean) =>
		invoke<void>("resolve_close", { action, remember }),

	openSettings: () => invoke<void>("open_settings"),

	getCloseAction: () => invoke<string | null>("get_close_action"),

	/** 设置页用：指定关闭行为。"ask" = 恢复每次询问。 */
	setCloseAction: (action: CloseSetting) =>
		invoke<void>("set_close_action", { action }),

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
	 * 因此调用前让用户确认。
	 * 返回升级后的版本号。
	 */
	upgradeDsh: () => invoke<string>("upgrade_dsh"),

	/**
	 * 把界面插件装成指定版本，然后重启 dsh。
	 * 同样会中断当前会话（插件里有原生模块，运行中覆盖不了）。
	 */
	upgradePlugins: (version: string) =>
		invoke<void>("upgrade_plugins", { version }),

	restartApp: () => invoke<void>("restart_app"),

	/** 用系统默认程序打开日志文件。 */
	openLog: () => invoke<void>("open_log"),

	/**
	 * 重启 dsh 服务（不重启应用）。要几秒到几十秒。
	 * 成功后 Rust 会广播 ready 事件，外壳据此换到新地址。
	 */
	restartDsh: () => invoke<string>("restart_dsh"),

	/** 当前应跟随的主题，取自 DSH 的设置 */
	getTheme: () => invoke<"light" | "dark">("get_theme"),

	getThemeMode: () => invoke<ThemeMode>("get_theme_mode"),

	/**
	 * 改主题。亮/暗会写进 DSH 的外观设置（它是唯一真值，页面实时翻转），
	 * 成功后模式回到 follow；写不进去才降级为外壳独立。返回最终模式。
	 */
	setThemeMode: (value: ThemeMode) =>
		invoke<ThemeMode>("set_theme_mode", { value }),
};

export const events = {
	onProgress: (cb: (p: Progress) => void): Promise<UnlistenFn> =>
		listen<Progress>(EVENT_PROGRESS, (e) => cb(e.payload)),

	onReady: (cb: (p: ReadyPayload) => void): Promise<UnlistenFn> =>
		listen<ReadyPayload>(EVENT_READY, (e) => cb(e.payload)),

	onFailed: (cb: (p: ErrorPayload) => void): Promise<UnlistenFn> =>
		listen<ErrorPayload>(EVENT_FAILED, (e) => cb(e.payload)),

	onWarning: (cb: (p: WarningPayload) => void): Promise<UnlistenFn> =>
		listen<WarningPayload>(EVENT_WARNING, (e) => cb(e.payload)),

	/** DSH 主题变化。每个窗口都该听，配色才会一起跟着走。 */
	onTheme: (cb: (p: ThemePayload) => void): Promise<UnlistenFn> =>
		listen<ThemePayload>(EVENT_THEME, (e) => cb(e.payload)),
};
