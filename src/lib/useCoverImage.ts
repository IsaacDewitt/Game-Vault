import { computed, ref, watch, type Ref } from "vue";
import type { Game } from "./tauri";
import { useGamesStore } from "../stores/games";

/**
 * 封面图片逻辑 composable
 * 通过 Tauri asset 协议（convertFileSrc）直接加载本地封面文件，
 * 避免全量 base64 传输；文件不存在或加载失败时回退到占位符。
 */
export function useCoverImage(game: Ref<Game>) {
  const store = useGamesStore();

  // 标记图片渲染是否失败（asset URL 存在但无法渲染）
  const renderFailed = ref(false);

  // 仅当 game.id 变化时重置渲染失败状态（避免在 computed 内修改 ref 导致死循环）
  watch(() => game.value.id, () => {
    renderFailed.value = false;
  }, { immediate: true });

  // 直接从 store 的封面路径映射生成 asset URL
  const coverImage = computed(() => {
    return store.coverSrc(game.value.id);
  });

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

  return {
    coverImage,
    imgFailed,
    showPlaceholder,
    handleImageError,
  };
}
