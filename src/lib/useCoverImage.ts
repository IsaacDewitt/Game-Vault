import { computed, type Ref } from "vue";
import type { Game } from "./tauri";
import { useGamesStore } from "../stores/games";

/**
 * 封面图片逻辑 composable
 * 从 store 的批量 base64 缓存中读取封面，避免单独 IPC 调用
 */
export function useCoverImage(game: Ref<Game>) {
  const store = useGamesStore();

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
    // 图片加载失败时的处理（base64 data URL 通常不会加载失败）
    console.warn("[CoverImage] 图片渲染失败:", game.value.name);
  }

  return {
    coverImage,
    imgFailed,
    handleImageError,
  };
}
