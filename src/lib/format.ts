import { TIME_MS } from "./constants";

/**
 * 格式化游戏时长
 * - < 60 秒: 显示秒数（如 "30s"），避免截断为 "0m"
 * - < 1 小时: 显示分钟（如 "5m"）
 * - >= 1 小时: 显示小时+分钟（如 "2h 30m"）
 */
export function formatPlayTime(seconds: number): string {
  if (seconds <= 0) return "未游玩";

  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const secs = seconds % 60;

  if (hours > 0) {
    return `${hours}h ${minutes}m`;
  }
  if (minutes > 0) {
    return `${minutes}m`;
  }
  // 子分钟级别显示秒数，避免进程匹配失败导致 10s 会话显示为 "0m"
  return `${secs}s`;
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
