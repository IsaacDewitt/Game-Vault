import { invoke } from "@tauri-apps/api/core";

// ==================== 类型定义 ====================

export interface Game {
  id: string;
  name: string;
  install_path: string | null;
  exe_path: string | null;
  exe_name: string | null;
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
}

export interface GameFilter {
  search?: string;
  favorites_only?: boolean;
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
  llm_provider: string;
  llm_protocol: string;
  llm_api_key: string;
  llm_base_url: string;
  llm_model: string;
  llm_enabled: boolean;
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

export async function getAllCovers(): Promise<Record<string, string>> {
  return invoke("get_all_covers");
}

export async function readCoverAsBase64(path: string): Promise<string> {
  return invoke("read_cover_as_base64", { path });
}

export async function readCoversBatchAsBase64(paths: string[]): Promise<Record<string, string>> {
  return invoke("read_covers_batch_as_base64", { paths });
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
