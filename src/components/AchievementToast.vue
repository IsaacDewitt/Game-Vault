<script setup lang="ts">
import { ref, onMounted, onUnmounted } from "vue";
import { listen } from "@tauri-apps/api/event";
import type { UnlockEvent } from "../lib/tauri";

interface ToastItem {
  id: number;
  def: UnlockEvent["def"];
  gameId: string | null;
}

const toasts = ref<ToastItem[]>([]);
let nextId = 1;
let unlisten: (() => void) | null = null;
// 同时最多展示的 toast 数，超出时移除最早的，避免批量解锁时堆满屏幕
const MAX_TOASTS = 4;

onMounted(async () => {
  try {
    unlisten = await listen<UnlockEvent[]>("achievement-unlocked", (event) => {
      for (const ev of event.payload) {
        pushToast(ev);
      }
    });
  } catch (e) {
    console.error("监听成就解锁事件失败:", e);
  }
});

function pushToast(ev: UnlockEvent) {
  const item: ToastItem = { id: nextId++, def: ev.def, gameId: ev.game_id };
  toasts.value.push(item);
  while (toasts.value.length > MAX_TOASTS) {
    toasts.value.shift();
  }
  // 4 秒后自动消失
  setTimeout(() => removeToast(item.id), 4200);
}

function removeToast(id: number) {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

onUnmounted(() => {
  unlisten?.();
});
</script>

<template>
  <Teleport to="body">
    <div class="achv-toast-container">
      <TransitionGroup name="toast">
        <div v-for="toast in toasts" :key="toast.id" class="achv-toast">
          <span class="toast-icon">{{ toast.def.icon }}</span>
          <div class="toast-body">
            <div class="toast-title">
              <span class="toast-flag">🏆 成就解锁</span>
              <span class="toast-tier" v-if="toast.def.tier_total > 1">
                · {{ toast.def.tier }}/{{ toast.def.tier_total }}
              </span>
            </div>
            <div class="toast-name">{{ toast.def.name }}</div>
            <div v-if="toast.gameId" class="toast-game">单游戏成就</div>
          </div>
          <div class="toast-shine"></div>
        </div>
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.achv-toast-container {
  position: fixed;
  top: 48px;
  right: 20px;
  z-index: 9999;
  display: flex;
  flex-direction: column;
  gap: 10px;
  pointer-events: none;
}

.achv-toast {
  position: relative;
  display: flex;
  align-items: center;
  gap: 12px;
  min-width: 240px;
  max-width: 320px;
  padding: 14px 18px;
  border-radius: 12px;
  background: linear-gradient(135deg, #1c1a2e 0%, #241d33 100%);
  border: 1px solid rgba(251, 191, 36, 0.45);
  box-shadow: 0 8px 28px rgba(0, 0, 0, 0.45), 0 0 24px rgba(251, 191, 36, 0.08);
  overflow: hidden;
}

.toast-icon {
  font-size: 28px;
  flex-shrink: 0;
}

.toast-body {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.toast-title {
  display: flex;
  align-items: center;
  gap: 4px;
}

.toast-flag {
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 1px;
  color: #fbbf24;
}

.toast-tier {
  font-size: 11px;
  color: #fbbf24;
  opacity: 0.8;
}

.toast-name {
  font-size: 16px;
  font-weight: 700;
  color: #f5f0e1;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.toast-game {
  font-size: 11px;
  color: rgba(245, 240, 225, 0.55);
}

/* 顶部扫光动画 */
.toast-shine {
  position: absolute;
  top: 0;
  left: -60%;
  width: 40%;
  height: 100%;
  background: linear-gradient(90deg, transparent, rgba(255, 255, 255, 0.14), transparent);
  animation: shine 2.4s ease-in-out infinite;
}

@keyframes shine {
  0% { left: -60%; }
  60%, 100% { left: 120%; }
}

/* 进出场动画 */
.toast-enter-active,
.toast-leave-active {
  transition: all 0.35s cubic-bezier(0.22, 0.9, 0.36, 1);
}

.toast-enter-from {
  opacity: 0;
  transform: translateX(40px) scale(0.92);
}

.toast-leave-to {
  opacity: 0;
  transform: translateX(40px) scale(0.92);
}
</style>
