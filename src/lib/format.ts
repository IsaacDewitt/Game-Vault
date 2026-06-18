/**
 * 格式化游戏时长
 */
export function formatPlayTime(seconds: number): string {
  if (seconds === 0) return "未游玩";

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
  const now = new Date();
  const diff = now.getTime() - date.getTime();

  // 小于1分钟
  if (diff < 60000) return "刚刚";
  // 小于1小时
  if (diff < 3600000) return `${Math.floor(diff / 60000)}分钟前`;
  // 小于24小时
  if (diff < 86400000) return `${Math.floor(diff / 3600000)}小时前`;
  // 小于7天
  if (diff < 604800000) return `${Math.floor(diff / 86400000)}天前`;

  // 超过7天显示日期
  return date.toLocaleDateString("zh-CN");
}
