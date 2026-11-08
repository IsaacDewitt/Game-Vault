import { ref } from "vue";

/**
 * 「添加游戏」流程的共享状态（模块单例）。
 *
 * 为什么不在某个视图里：入口搬到侧边栏左下角后，添加游戏是**全局动作**，
 * 而弹窗宿主挂在 `App.vue`（与视图无关，切到设置页也能用）。游戏库的空状态
 * 按钮与右键菜单同样要能唤起同一套流程，因此把状态提到模块作用域，三处共用
 * 一份，避免各自复制弹窗与校验逻辑。
 *
 * 流程：来源选择（本地 / Steam / Epic）→ 本地走选 exe + 取名；平台走扫描导入。
 */
const showPicker = ref(false);
const showName = ref(false);
const showImport = ref(false);
const pendingExePath = ref("");
const gameNameInput = ref("");
const importPlatform = ref("steam");

/** 打开「添加游戏」来源选择弹窗（侧边栏按钮 / 空状态按钮 / 右键菜单共用入口） */
export function openAddGame() {
  showPicker.value = true;
}

/** 关闭并清空流程状态（取消或完成后调用，避免下次打开残留上次的路径/名称） */
export function resetAddGameFlow() {
  showPicker.value = false;
  showName.value = false;
  showImport.value = false;
  pendingExePath.value = "";
  gameNameInput.value = "";
  importPlatform.value = "steam";
}

export function useAddGameFlow() {
  return {
    showPicker,
    showName,
    showImport,
    pendingExePath,
    gameNameInput,
    importPlatform,
  };
}
