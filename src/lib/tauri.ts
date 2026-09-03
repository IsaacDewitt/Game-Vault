import { invoke } from "@tauri-apps/api/core";

// ==================== 类型定义 ====================

export interface Game {
  id: string;
  name: string;
  install_path: string | null;
  exe_path: string | null;
  exe_name: string | null;
  /** 从 exe 文件读取的版本号 */
  exe_version: string | null;
  cover_local: string | null;
  cover_url: string | null;
  description: string | null;
  developer: string | null;
  publisher: string | null;
  release_date: string | null;
  genres: string[];
  play_time_seconds: number;
  last_played: string | null;
  play_count: number;
  is_favorite: boolean;
  /** 游戏状态: "unplayed", "playing", "completed", "abandoned" */
  status: string;
  added_at: string;
  updated_at: string | null;
  /** HLTB 主线时长（分钟） */
  hltb_main_story: number | null;
  /** HLTB 主线+支线时长（分钟） */
  hltb_main_extra: number | null;
  /** HLTB 完美通关时长（分钟） */
  hltb_completionist: number | null;
  /** 游戏存档路径列表 */
  save_paths: string[];
  /** exe 文件最后修改时间（Unix 时间戳秒数），用于缓存判断 */
  exe_modified_at: number | null;
  /** exe 文件大小（字节），用于缓存判断 */
  exe_file_size: number | null;
}

export interface GameFilter {
  search?: string;
  favorites_only?: boolean;
  status?: string;
  genre?: string;
  sort_by?: string;
  sort_order?: string;
}


export interface PlayStats {
  game_id: string;
  game_name: string;
  total_seconds: number;
  play_count: number;
  last_played: string | null;
}

export interface DailyStats {
  date: string;
  total_seconds: number;
  sessions_count: number;
}

export interface GenreStats {
  genre: string;
  total_seconds: number;
  game_count: number;
}

export interface HeatmapDay {
  date: string;
  total_seconds: number;
}

export interface HourlyStats {
  hour: number;
  weekday: number;
  total_seconds: number;
}

export interface StatusStats {
  unplayed: number;
  playing: number;
  completed: number;
  abandoned: number;
}

export interface Settings {
  theme: string;
  language: string;
  steamgriddb_api_key: string;
  llm_protocol: string;
  llm_api_key: string;
  llm_base_url: string;
  llm_model: string;
  llm_enabled: boolean;
  accent_color: string;
  window_width: number;
  window_height: number;
  /** 截图保存目录（默认 %USERPROFILE%\Videos，与 NVIDIA App 一致） */
  screenshot_dir: string;
  /** 截图快捷键（默认 F12，与 Steam 一致） */
  screenshot_hotkey: string;
}

// ==================== 成就系统 ====================

export interface AchievementDef {
  id: string;
  base_id: string;
  name: string;
  /** "global" | "pergame" */
  scope: string;
  /** "progress" | "collect" | "fun" | "challenge" */
  category: string;
  desc: string;
  icon: string;
  /** 当前等级（从 1 开始） */
  tier: number;
  /** 总等级数 */
  tier_total: number;
  target: number;
}

export interface GlobalAchievementStatus {
  def: AchievementDef;
  unlocked: boolean;
  unlocked_at: string | null;
  progress: number;
  target: number;
}

export interface GameAchievementStatus {
  def: AchievementDef;
  unlocked: boolean;
  unlocked_at: string | null;
  progress: number;
  target: number;
}

export interface GameAchievements {
  game_id: string;
  game_name: string;
  achievements: GameAchievementStatus[];
}

export interface AchievementSummary {
  global: GlobalAchievementStatus[];
  per_game: GameAchievements[];
  total_count: number;
  unlocked_count: number;
}

export interface UnlockEvent {
  def: AchievementDef;
  game_id: string | null;
}

export async function getAchievements(): Promise<AchievementSummary> {
  return invoke("get_achievements");
}

/** 手动触发成就检测，返回本次新解锁的事件列表 */
export async function checkAchievements(): Promise<UnlockEvent[]> {
  return invoke("check_achievements");
}

// ==================== Tauri 命令封装 ====================

export async function getGames(filter?: GameFilter): Promise<Game[]> {
  return invoke("get_games", { filter });
}

export async function getGameDetail(gameId: string): Promise<Game | null> {
  return invoke("get_game_detail", { gameId });
}

export async function launchGame(gameId: string): Promise<void> {
  return invoke("launch_game", { gameId });
}

export async function toggleFavorite(gameId: string): Promise<boolean> {
  return invoke("toggle_favorite", { gameId });
}

export async function deleteGame(gameId: string): Promise<void> {
  return invoke("delete_game", { gameId });
}

export async function addGameManual(name: string, exePath: string): Promise<Game> {
  return invoke("add_game_manual", { name, exePath });
}

/** 启动时批量刷新所有游戏的 exe 版本号，返回更新数量 */
export async function refreshExeVersions(): Promise<number> {
  return invoke("refresh_exe_versions");
}

export async function setGameCover(gameId: string, coverPath: string): Promise<void> {
  return invoke("set_game_cover", { gameId, coverPath });
}

export async function removeGameCover(gameId: string): Promise<void> {
  return invoke("remove_game_cover", { gameId });
}

export interface CoverFetchResult {
  fetched: number;
  total: number;
  errors: string[];
}

export async function fetchMissingCovers(): Promise<CoverFetchResult> {
  return invoke("fetch_missing_covers");
}

/** 批量获取缺失游戏信息的游戏 */
export async function fetchMissingGameInfo(): Promise<CoverFetchResult> {
  return invoke("fetch_missing_game_info");
}

export async function getAllCovers(): Promise<Record<string, string>> {
  return invoke("get_all_covers");
}

/** 检查单个游戏的存档路径是否存在（编辑后局部刷新用） */
export async function checkSavePathsForGame(gameId: string): Promise<boolean> {
  return invoke("check_save_paths_for_game", { gameId });
}

export interface CoverOption {
  thumb_url: string;
  url: string;
  width: number;
  height: number;
  style: string;
}

export async function fetchCoverOptions(gameId: string): Promise<CoverOption[]> {
  return invoke("fetch_cover_options", { gameId });
}

export async function setGameCoverFromUrl(gameId: string, url: string): Promise<void> {
  return invoke("set_game_cover_from_url", { gameId, url });
}


export async function getPlayStats(limit?: number): Promise<PlayStats[]> {
  return invoke("get_play_stats", { limit });
}

export async function getDailyStats(days?: number): Promise<DailyStats[]> {
  return invoke("get_daily_stats", { days });
}

export async function getOverviewStats(): Promise<{
  game_count: number;
  total_play_time: number;
  monthly_play_time: number;
  today_play_time: number;
}> {
  return invoke("get_overview_stats");
}

export async function getGenreStats(): Promise<GenreStats[]> {
  return invoke("get_genre_stats");
}

export async function getHeatmapStats(days?: number): Promise<HeatmapDay[]> {
  return invoke("get_heatmap_stats", { days });
}

export async function getHourlyStats(): Promise<HourlyStats[]> {
  return invoke("get_hourly_stats");
}

export async function getStatusStats(): Promise<StatusStats> {
  return invoke("get_status_stats");
}

export async function setGameStatus(gameId: string, status: string): Promise<void> {
  return invoke("set_game_status", { gameId, status });
}

export async function fetchGameInfoLlm(gameId: string): Promise<Game> {
  return invoke("fetch_game_info_llm", { gameId });
}

export async function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  return invoke("save_settings", { settings });
}

export async function exportGameData(): Promise<string> {
  return invoke("export_game_data");
}

export async function renameGame(gameId: string, newName: string): Promise<void> {
  return invoke("rename_game", { gameId, newName });
}

export async function updateExePath(gameId: string, newExePath: string): Promise<Game> {
  return invoke("update_exe_path", { gameId, newExePath });
}

export async function importGameData(jsonData: string): Promise<{ imported_games: number; settings_restored: boolean }> {
  return invoke("import_game_data", { jsonData });
}

export async function getAllGenres(): Promise<string[]> {
  return invoke("get_all_genres");
}

export async function openSavePath(path: string): Promise<void> {
  return invoke("open_save_path", { path });
}

export async function updateSavePaths(gameId: string, savePaths: string[]): Promise<void> {
  return invoke("update_save_paths", { gameId, savePaths });
}

/** 检查所有游戏的存档路径是否存在，返回 game_id -> 存在与否 的映射 */
export async function checkSavePaths(): Promise<Record<string, boolean>> {
  return invoke("check_save_paths");
}

export interface GameMetaInput {
  description?: string | null;
  developer?: string | null;
  publisher?: string | null;
  release_date?: string | null;
  genres?: string[] | null;
  hltb_main_story?: number | null;
  hltb_main_extra?: number | null;
  hltb_completionist?: number | null;
  save_paths?: string[] | null;
}

export async function updateGameMeta(gameId: string, meta: GameMetaInput): Promise<Game> {
  return invoke("update_game_meta", { gameId, ...meta });
}

export interface SavesBackupResult {
  exported: number;
  errors: string[];
}

export async function exportSavesBackup(exportPath: string): Promise<SavesBackupResult> {
  return invoke("export_saves_backup", { exportPath });
}

export interface SavesRestoreResult {
  restored: number;
  errors: string[];
}

export async function importSavesBackup(zipPath: string): Promise<SavesRestoreResult> {
  return invoke("import_saves_backup", { zipPath });
}

export interface PlaySessionDetail {
  id: number;
  game_id: string;
  game_name: string;
  start_time: string;
  end_time: string | null;
  duration_seconds: number;
}

export async function getPlaySessions(gameId?: string, limit?: number, offset?: number): Promise<PlaySessionDetail[]> {
  return invoke("get_play_sessions", { gameId, limit, offset });
}

export async function quitApp(): Promise<void> {
  return invoke("quit_app");
}

export async function getAutostartEnabled(): Promise<boolean> {
  return invoke("get_autostart_enabled");
}

export async function setAutostartEnabled(enabled: boolean): Promise<void> {
  return invoke("set_autostart_enabled", { enabled });
}

export async function setWindowSize(width: number, height: number): Promise<void> {
  return invoke("set_window_size", { width, height });
}

// ==================== 截图 ====================

/** 截图触发结果（后端事件 screenshot-taken 的 payload） */
export type ScreenshotOutcome =
  | {
      outcome: "captured";
      path: string;
      process_name: string;
      width: number;
      height: number;
      tone_map_path: string;
    }
  | { outcome: "no_active_game" }
  | { outcome: "foreground_mismatch" }
  | { outcome: "failed"; message: string };

/** 打开某个游戏的截图文件夹（在文件管理器中） */
export async function openScreenshotDir(gameId: string): Promise<void> {
  return invoke("open_screenshot_dir", { gameId });
}

/** 查询截图热键注册状态：null=注册成功，字符串=注册失败原因 */
export async function getScreenshotHotkeyStatus(): Promise<string | null> {
  return invoke("get_screenshot_hotkey_status");
}

// ==================== 游戏手账 ====================

export interface Review {
  id: string;
  name: string;
  /** 本地封面缓存文件路径 */
  cover_local: string | null;
  cover_url: string | null;
  description: string | null;
  developer: string | null;
  publisher: string | null;
  release_date: string | null;
  genres: string[];
  /** HLTB 主线时长（分钟） */
  hltb_main_story: number | null;
  /** HLTB 主线+支线时长（分钟） */
  hltb_main_extra: number | null;
  /** HLTB 完美通关时长（分钟） */
  hltb_completionist: number | null;
  /** 我的评分（0-10 分制，null 表示未评分） */
  rating: number | null;
  /** 我的评价 */
  review: string | null;
  /** 状态: "wishlist" | "playing" | "completed" | "abandoned" */
  status: string;
  added_at: string;
  updated_at: string | null;
}

export interface ReviewFilter {
  search?: string;
  status?: string;
  genre?: string;
  sort_by?: string;
  sort_order?: string;
}

export async function getReviews(filter?: ReviewFilter): Promise<Review[]> {
  return invoke("get_reviews", { filter });
}

export async function getReviewDetail(reviewId: string): Promise<Review | null> {
  return invoke("get_review_detail", { reviewId });
}

export async function addReview(name: string): Promise<Review> {
  return invoke("add_review", { name });
}

/** 刷新手账条目：LLM 拉元数据 + SteamGridDB 拉封面 */
export async function refreshReviewInfo(reviewId: string): Promise<Review> {
  return invoke("refresh_review_info", { reviewId });
}

export interface ReviewMetaInput {
  name?: string | null;
  description?: string | null;
  developer?: string | null;
  publisher?: string | null;
  release_date?: string | null;
  genres?: string[] | null;
  hltb_main_story?: number | null;
  hltb_main_extra?: number | null;
  hltb_completionist?: number | null;
}

export async function updateReviewMeta(reviewId: string, meta: ReviewMetaInput): Promise<Review> {
  return invoke("update_review_meta", { reviewId, meta });
}

export async function setReviewRating(reviewId: string, rating: number | null): Promise<Review> {
  return invoke("set_review_rating", { reviewId, rating });
}

export async function setReviewReview(reviewId: string, reviewText: string | null): Promise<Review> {
  return invoke("set_review_review", { reviewId, reviewText });
}

export async function setReviewStatus(reviewId: string, status: string): Promise<Review> {
  return invoke("set_review_status", { reviewId, status });
}

export async function deleteReview(reviewId: string): Promise<void> {
  return invoke("delete_review", { reviewId });
}

export async function fetchReviewCoverOptions(reviewId: string): Promise<CoverOption[]> {
  return invoke("fetch_review_cover_options", { reviewId });
}

export async function setReviewCoverFromUrl(reviewId: string, url: string): Promise<void> {
  return invoke("set_review_cover_from_url", { reviewId, url });
}

/** 从游戏库导入到游戏手账 */
export async function importReviewFromGame(gameId: string): Promise<Review> {
  return invoke("import_review_from_game", { gameId });
}
