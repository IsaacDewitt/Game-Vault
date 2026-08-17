<script setup lang="ts">
import { ref, onMounted, onUnmounted, nextTick } from "vue";

export interface ContextMenuItem {
  label: string;
  icon?: string;
  action: () => void;
  danger?: boolean;
  divider?: boolean;
}

const props = defineProps<{
  items: ContextMenuItem[];
  x: number;
  y: number;
}>();

const emit = defineEmits<{
  close: [];
}>();

const menuRef = ref<HTMLElement | null>(null);
const adjustedX = ref(props.x);
const adjustedY = ref(props.y);
const focusedIndex = ref(-1);

/** 获取可操作（非分割线）的菜单项索引列表 */
function getActionableIndices(): number[] {
  return props.items
    .map((item, i) => (!item.divider ? i : -1))
    .filter((i) => i !== -1);
}

onMounted(() => {
  nextTick(() => {
    if (menuRef.value) {
      const rect = menuRef.value.getBoundingClientRect();
      const windowWidth = window.innerWidth;
      const windowHeight = window.innerHeight;

      if (props.x + rect.width > windowWidth) {
        adjustedX.value = windowWidth - rect.width - 8;
      }
      if (props.y + rect.height > windowHeight) {
        adjustedY.value = windowHeight - rect.height - 8;
      }
      // 防止菜单超出左边界和上边界
      if (adjustedX.value < 8) adjustedX.value = 8;
      if (adjustedY.value < 8) adjustedY.value = 8;

      // 自动聚焦第一个可操作项
      const actionable = getActionableIndices();
      if (actionable.length > 0) {
        focusedIndex.value = actionable[0];
        focusItem(focusedIndex.value);
      }
    }
  });

  document.addEventListener("click", handleOutsideClick);
  document.addEventListener("keydown", handleKeydown);
});

onUnmounted(() => {
  document.removeEventListener("click", handleOutsideClick);
  document.removeEventListener("keydown", handleKeydown);
});

function handleOutsideClick() {
  emit("close");
}

function handleItemClick(item: ContextMenuItem) {
  item.action();
  emit("close");
}

/** 聚焦指定原始索引的菜单项（通过可操作项映射，兼容含分割线的菜单） */
function focusItem(index: number) {
  const actionable = getActionableIndices();
  const pos = actionable.indexOf(index);
  if (pos === -1) return;
  const items = menuRef.value?.querySelectorAll<HTMLElement>("[role='menuitem']");
  items?.[pos]?.focus();
}

function handleKeydown(e: KeyboardEvent) {
  const actionable = getActionableIndices();
  if (actionable.length === 0) return;

  const currentPos = actionable.indexOf(focusedIndex.value);

  switch (e.key) {
    case "Escape":
      e.preventDefault();
      emit("close");
      break;
    case "ArrowDown": {
      e.preventDefault();
      const nextPos = (currentPos + 1) % actionable.length;
      focusedIndex.value = actionable[nextPos];
      focusItem(focusedIndex.value);
      break;
    }
    case "ArrowUp": {
      e.preventDefault();
      const prevPos = (currentPos - 1 + actionable.length) % actionable.length;
      focusedIndex.value = actionable[prevPos];
      focusItem(focusedIndex.value);
      break;
    }
    case "Enter":
    case " ": {
      e.preventDefault();
      if (focusedIndex.value >= 0 && focusedIndex.value < props.items.length) {
        handleItemClick(props.items[focusedIndex.value]);
      }
      break;
    }
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      ref="menuRef"
      class="context-menu"
      role="menu"
      :style="{ left: adjustedX + 'px', top: adjustedY + 'px' }"
      @click.stop
      @contextmenu.stop.prevent
    >
      <template v-for="(item, index) in items" :key="index">
        <div v-if="item.divider" class="context-menu-divider" role="separator" />
        <div
          v-else
          class="context-menu-item"
          :class="{ danger: item.danger }"
          role="menuitem"
          tabindex="-1"
          @click="handleItemClick(item)"
        >
          <span v-if="item.icon" class="context-menu-icon">{{ item.icon }}</span>
          <span class="context-menu-label">{{ item.label }}</span>
        </div>
      </template>
    </div>
  </Teleport>
</template>

<style scoped>
.context-menu {
  position: fixed;
  z-index: 9999;
  min-width: 160px;
  background: #252538;
  border: 1px solid rgba(255, 255, 255, 0.1);
  border-radius: 8px;
  padding: 4px;
  box-shadow: 0 8px 24px rgba(0, 0, 0, 0.5);
  backdrop-filter: blur(12px);
}

.context-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  border-radius: 6px;
  font-size: 13px;
  color: #e0e0e0;
  cursor: pointer;
  transition: background 0.15s;
  user-select: none;
}

.context-menu-item:hover {
  background: rgba(99, 102, 241, 0.2);
}

.context-menu-item.danger {
  color: #ef4444;
}

.context-menu-item.danger:hover {
  background: rgba(239, 68, 68, 0.15);
}

.context-menu-icon {
  font-size: 15px;
  width: 20px;
  text-align: center;
}

.context-menu-divider {
  height: 1px;
  background: rgba(255, 255, 255, 0.08);
  margin: 4px 8px;
}
</style>
