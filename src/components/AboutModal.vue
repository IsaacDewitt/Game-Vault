<script setup lang="ts">
import { changelog } from "../data/changelog";

defineProps<{
  show: boolean;
  version: string;
}>();

const emit = defineEmits<{
  (e: "close"): void;
}>();
</script>

<template>
  <Teleport to="body">
    <Transition name="fade">
      <div v-if="show" class="about-overlay" @click.self="emit('close')">
        <div class="about-modal">
          <div class="about-header">
            <h2 class="about-title">Game Vault</h2>
            <span class="about-version">v{{ version }}</span>
          </div>
          <div class="about-body">
            <div class="changelog-title">更新日志</div>
            <div class="changelog-list">
              <div v-for="entry in changelog" :key="entry.version" class="changelog-entry">
                <div class="changelog-entry-header">
                  <span class="changelog-version">v{{ entry.version }}</span>
                  <span class="changelog-date">{{ entry.date }}</span>
                </div>
                <ul class="changelog-changes">
                  <li v-for="(change, idx) in entry.changes" :key="idx">{{ change }}</li>
                </ul>
              </div>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.about-overlay {
  position: fixed;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  background: rgba(0, 0, 0, 0.6);
  display: flex;
  align-items: center;
  justify-content: center;
  z-index: 9999;
}

.about-modal {
  background: #1a1a2e;
  border-radius: 12px;
  width: 420px;
  max-height: 70vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  border: 1px solid rgba(255, 255, 255, 0.1);
}

.light-theme .about-modal {
  background: #fff;
  border: 1px solid rgba(0, 0, 0, 0.1);
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.15);
}

.about-header {
  padding: 20px 24px 16px;
  border-bottom: 1px solid rgba(255, 255, 255, 0.1);
  display: flex;
  align-items: baseline;
  gap: 12px;
}

.light-theme .about-header {
  border-bottom: 1px solid rgba(0, 0, 0, 0.1);
}

.about-title {
  font-size: 20px;
  font-weight: 700;
  color: #e0e0e0;
}

.light-theme .about-title {
  color: #333;
}

.about-version {
  font-size: 14px;
  color: rgba(255, 255, 255, 0.5);
}

.light-theme .about-version {
  color: rgba(0, 0, 0, 0.4);
}

.about-body {
  padding: 16px 24px 20px;
  overflow-y: auto;
  flex: 1;
}

.about-body::-webkit-scrollbar {
  width: 6px;
}

.about-body::-webkit-scrollbar-track {
  background: transparent;
}

.about-body::-webkit-scrollbar-thumb {
  background: rgba(255, 255, 255, 0.2);
  border-radius: 3px;
}

.light-theme .about-body::-webkit-scrollbar-thumb {
  background: rgba(0, 0, 0, 0.2);
}

.changelog-title {
  font-size: 15px;
  font-weight: 600;
  color: #e0e0e0;
  margin-bottom: 12px;
}

.light-theme .changelog-title {
  color: #333;
}

.changelog-entry {
  margin-bottom: 16px;
}

.changelog-entry:last-child {
  margin-bottom: 0;
}

.changelog-entry-header {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 8px;
}

.changelog-version {
  font-size: 14px;
  font-weight: 600;
  color: var(--accent-color, #4fc3f7);
}

.changelog-date {
  font-size: 12px;
  color: rgba(255, 255, 255, 0.4);
}

.light-theme .changelog-date {
  color: rgba(0, 0, 0, 0.4);
}

.changelog-changes {
  list-style: none;
  padding: 0;
  margin: 0;
}

.changelog-changes li {
  font-size: 13px;
  color: rgba(255, 255, 255, 0.7);
  padding: 3px 0 3px 16px;
  position: relative;
  line-height: 1.5;
}

.light-theme .changelog-changes li {
  color: rgba(0, 0, 0, 0.65);
}

.changelog-changes li::before {
  content: "";
  position: absolute;
  left: 0;
  top: 10px;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: var(--accent-color, #4fc3f7);
  opacity: 0.6;
}

.fade-enter-active,
.fade-leave-active {
  transition: opacity 0.2s ease;
}

.fade-enter-from,
.fade-leave-to {
  opacity: 0;
}
</style>
