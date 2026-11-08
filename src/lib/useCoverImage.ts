import { computed, ref, watch, type Ref } from "vue";
import type { Game } from "./tauri";
import { useGamesStore } from "../stores/games";

/**
 * 封面图片逻辑 composable
 * 通过 Tauri asset 协议（convertFileSrc）直接加载本地封面文件，
 * 避免全量 base64 传输；文件不存在或加载失败时回退到占位符。
 *
 * @param preferThumb 小尺寸场景（卡片网格）传 true 用缩略图；大图场景（详情页）传 false 用原图
 */
export function useCoverImage(game: Ref<Game>, preferThumb = true) {
  const store = useGamesStore();

  // 标记图片渲染是否失败（asset URL 存在但无法渲染）
  const renderFailed = ref(false);

  // 直接从 store 的封面路径映射生成 asset URL
  const coverImage = computed(() => {
    return store.coverSrc(game.value.id, preferThumb);
  });

  // 【2026-11-08】换游戏 **或换封面** 都要重置失败态。
  // 旧版只 watch game.id：同一游戏换封面（asset URL 靠 `?t=` 变化）时
  // renderFailed 仍是 true，新图被占位符一直挡到组件重挂。
  // 注意 watch 必须放在 coverImage 之后，否则 immediate 会撞上 TDZ。
  watch(
    [() => game.value.id, coverImage],
    () => {
      renderFailed.value = false;
    },
    { immediate: true }
  );

  // 如果配置了封面路径但没有可用的 asset URL，说明文件失效（删除/损坏）
  const imgFailed = computed(() => {
    const hasPath = store.coverPaths[game.value.id] || game.value.cover_local || game.value.cover_url;
    return !!hasPath && !coverImage.value;
  });

  function handleImageError() {
    console.warn("[CoverImage] 图片渲染失败:", game.value.name);
    renderFailed.value = true;
  }

  // 最终是否应该显示占位符（无封面 或 加载失败 或 渲染失败）
  const showPlaceholder = computed(() => {
    return !coverImage.value || imgFailed.value || renderFailed.value;
  });

  // `imgFailed` 只供内部 showPlaceholder 判定，不再对外导出：
  // 消费方统一用 showPlaceholder，避免又有人只判「有路径没 URL」而漏掉 @error。
  return {
    coverImage,
    showPlaceholder,
    handleImageError,
  };
}
