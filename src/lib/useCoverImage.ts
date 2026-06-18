import { computed, ref, watch, type Ref } from "vue";
import type { Game } from "./tauri";
import { readCoverAsBase64 } from "./tauri";
import { useGamesStore } from "../stores/games";

/**
 * 封面图片逻辑 composable
 * 通过 Tauri 命令读取本地文件为 base64 data URL，绕过 asset protocol
 */
export function useCoverImage(game: Ref<Game>) {
  const store = useGamesStore();
  const imgFailed = ref(false);
  const coverImage = ref<string | null>(null);

  // 计算封面文件路径
  const coverPath = computed(() => {
    const cachedPath = store.coverPaths[game.value.id];
    if (cachedPath) return cachedPath;
    if (game.value.cover_local) return game.value.cover_local;
    if (game.value.cover_url) return game.value.cover_url;
    return null;
  });

  // 当路径变化时，异步加载 base64
  watch(
    coverPath,
    async (newPath) => {
      if (!newPath) {
        coverImage.value = null;
        return;
      }
      try {
        imgFailed.value = false;
        coverImage.value = await readCoverAsBase64(newPath);
      } catch (e) {
        console.error("[CoverImage] 加载封面失败:", e);
        coverImage.value = null;
        imgFailed.value = true;
      }
    },
    { immediate: true }
  );

  function handleImageError() {
    imgFailed.value = true;
  }

  return {
    coverImage,
    imgFailed,
    handleImageError,
  };
}
