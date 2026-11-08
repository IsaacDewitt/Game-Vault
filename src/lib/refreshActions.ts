import { computed, ref } from "vue";
import { useMessage } from "naive-ui";
import { useGamesStore } from "../stores/games";

/**
 * 「刷新」三个动作 + 结果提示的唯一实现。
 *
 * 为什么抽出来：刷新按钮已搬到侧边栏左下角，而游戏库右键菜单仍保留
 * 「刷新封面 / 刷新游戏信息」两项，两处必须共用同一份逻辑与 loading 状态
 * （模块作用域的两个 ref），否则同一动作会有两份转圈状态、互相不知情。
 *
 * 必须在组件 setup 内调用（`useMessage` 依赖上层 `NMessageProvider`）。
 */
const refreshingCovers = ref(false);
const refreshingInfo = ref(false);

export function useRefreshActions() {
  const store = useGamesStore();
  const message = useMessage();

  async function refreshCovers() {
    refreshingCovers.value = true;
    try {
      const result = await store.fetchCovers();

      // 检查是否有 API Key 认证失败
      const authError = result.errors.find((e: string) =>
        e.includes("API Key 无效") || e.includes("401") || e.includes("403")
      );
      // 后端因网络全断而中止整批时会给这条："网络异常，已中止：剩余 N 个游戏未尝试…"
      // 它必须优先于「未找到」分支显示——旧顺序里 total>0 恒成立，网络故障永远被
      // 说成「SteamGridDB 未找到」（2026-10-28 审查指出）。
      const aborted = result.errors.find((e: string) => e.includes("已中止"));

      if (authError) {
        message.error("SteamGridDB API Key 无效，请在设置中重新配置");
      } else if (aborted) {
        message.error(aborted);
        result.errors
          .filter((e: string) => e !== aborted)
          .slice(0, 3)
          .forEach((e: string) => console.warn("封面获取:", e));
      } else if (result.fetched > 0) {
        const rest = result.errors.length;
        message.success(
          `已获取 ${result.fetched} 个游戏的封面${rest ? `，另有 ${rest} 个没成功（详见控制台）` : ""}`
        );
      } else if (result.total > 0) {
        message.warning(
          `${result.total} 个游戏缺少封面，自动抓取未找到可用图片。可尝试手动设置封面`
        );
        result.errors.forEach((e: string) => console.warn("封面获取:", e));
      } else if (result.errors.length === 0) {
        message.info("所有游戏封面已是最新");
      } else {
        message.error(result.errors[0]);
        result.errors.slice(1).forEach((e: string) => console.warn("封面获取:", e));
      }
    } catch (e) {
      message.error("获取封面失败");
    } finally {
      refreshingCovers.value = false;
    }
  }

  async function refreshAllInfo() {
    refreshingInfo.value = true;
    try {
      const result = await store.fetchGameInfo();

      if (result.fetched > 0) {
        message.success(`已获取 ${result.fetched} 个游戏的信息`);
      } else if (result.total > 0) {
        message.warning(`${result.total} 个游戏缺少信息，但获取失败。请检查 LLM 配置`);
        result.errors.forEach((e: string) => console.warn("游戏信息获取:", e));
      } else if (result.errors.length === 0) {
        message.info("所有游戏信息已是最新");
      } else {
        message.error(result.errors[0]);
        result.errors.slice(1).forEach((e: string) => console.warn("游戏信息获取:", e));
      }
    } catch (e) {
      message.error("获取游戏信息失败，请检查 LLM 配置");
    } finally {
      refreshingInfo.value = false;
    }
  }

  /** 检查存档：结果写在 store.savePathStatus，卡片/详情自行读，无需提示 */
  async function checkSaves() {
    await store.checkSavePaths();
  }

  /** 任一项刷新进行中 → 侧边栏按钮转圈（store 里的存档检查也计入） */
  const busy = computed(
    () => refreshingCovers.value || refreshingInfo.value || store.checkingSavePaths
  );

  return { busy, refreshingCovers, refreshingInfo, refreshCovers, refreshAllInfo, checkSaves };
}
