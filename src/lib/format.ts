import { TIME_MS } from "./constants";

/**
 * 格式化游戏时长
 */
export function formatPlayTime(seconds: number): string {
  if (seconds <= 0) return "未游玩";

  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  return `${minutes}m`;
}

/**
 * 格式化日期
 */
export function formatDate(dateStr: string | null): string {
  if (!dateStr) return "未知";

  const date = new Date(dateStr);
  // 检查日期是否有效
  if (isNaN(date.getTime())) return "未知";

  const now = new Date();
  const diff = now.getTime() - date.getTime();

  // 负差值表示未来时间，直接显示日期
  if (diff < 0) return date.toLocaleDateString("zh-CN");

  if (diff < TIME_MS.MINUTE) return "刚刚";
  if (diff < TIME_MS.HOUR) return `${Math.floor(diff / TIME_MS.MINUTE)}分钟前`;
  if (diff < TIME_MS.DAY) return `${Math.floor(diff / TIME_MS.HOUR)}小时前`;
  if (diff < TIME_MS.WEEK) return `${Math.floor(diff / TIME_MS.DAY)}天前`;

  return date.toLocaleDateString("zh-CN");
}
