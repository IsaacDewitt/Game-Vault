import { computed, ref, watch, type Ref } from "vue";
import type { Game } from "./tauri";
import { useGamesStore } from "../stores/games";

/**
 * 封面图片逻辑 composable
 * 从 store 的批量 base64 缓存中读取封面，避免单独 IPC 调用
 */
export function useCoverImage(game: Ref<Game>) {
  const store = useGamesStore();

  // 标记图片渲染是否失败（base64 数据存在但无法渲染）
  const renderFailed = ref(false);

  // 仅当 game.id 变化时重置渲染失败状态（避免在 computed 内修改 ref 导致死循环）
  watch(() => game.value.id, () => {
    renderFailed.value = false;
  }, { immediate: true });

  // 直接从 store 的 base64 缓存中获取封面
  const coverImage = computed(() => {
    return store.coverBase64Cache[game.value.id] || null;
  });

  // 如果缓存中没有且有封面路径，说明加载失败或正在加载
  const imgFailed = computed(() => {
    const hasPath = store.coverPaths[game.value.id] || game.value.cover_local || game.value.cover_url;
    return !!hasPath && !coverImage.value && !store.coversLoading;
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
