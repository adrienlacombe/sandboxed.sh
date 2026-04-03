/**
 * Telegram Channels API - manage Telegram bot integrations for assistant missions.
 */

import { apiGet, apiPost, apiPatch, apiDel } from "./core";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type TelegramTriggerMode = "mention_or_dm" | "bot_mention" | "reply" | "direct_message" | "always";

export interface TelegramChannel {
  id: string;
  mission_id: string;
  bot_username: string | null;
  allowed_chat_ids: number[];
  trigger_mode: TelegramTriggerMode;
  active: boolean;
  instructions: string | null;
  auto_create_missions: boolean;
  default_backend: string | null;
  default_model_override: string | null;
  default_model_effort: string | null;
  default_workspace_id: string | null;
  default_config_profile: string | null;
  default_agent: string | null;
  created_at: string;
  updated_at: string;
}

export interface TelegramChatMission {
  id: string;
  channel_id: string;
  chat_id: number;
  mission_id: string;
  chat_title: string | null;
  created_at: string;
}

export interface CreateTelegramChannelInput {
  bot_token: string;
  bot_username?: string;
  allowed_chat_ids?: number[];
  trigger_mode?: TelegramTriggerMode;
  instructions?: string;
}

export interface CreateTelegramBotInput {
  bot_token: string;
  bot_username?: string;
  allowed_chat_ids?: number[];
  trigger_mode?: TelegramTriggerMode;
  instructions?: string;
  default_backend?: string;
  default_model_override?: string;
  default_model_effort?: string;
  default_workspace_id?: string;
  default_config_profile?: string;
  default_agent?: string;
}

export interface UpdateTelegramChannelInput {
  active?: boolean;
  trigger_mode?: TelegramTriggerMode;
  allowed_chat_ids?: number[];
  instructions?: string;
  default_backend?: string;
  default_model_override?: string;
  default_model_effort?: string;
  default_workspace_id?: string;
  default_config_profile?: string;
  default_agent?: string;
}

// ---------------------------------------------------------------------------
// Legacy per-mission API Functions
// ---------------------------------------------------------------------------

export async function listTelegramChannels(missionId: string): Promise<TelegramChannel[]> {
  return apiGet<TelegramChannel[]>(
    `/api/control/missions/${missionId}/telegram-channels`,
    "Failed to fetch Telegram channels"
  );
}

export async function createTelegramChannel(
  missionId: string,
  input: CreateTelegramChannelInput
): Promise<TelegramChannel> {
  return apiPost<TelegramChannel>(
    `/api/control/missions/${missionId}/telegram-channels`,
    input,
    "Failed to create Telegram channel"
  );
}

export async function updateTelegramChannel(
  channelId: string,
  updates: UpdateTelegramChannelInput
): Promise<TelegramChannel> {
  return apiPatch<TelegramChannel>(
    `/api/control/telegram-channels/${channelId}`,
    updates,
    "Failed to update Telegram channel"
  );
}

export async function deleteTelegramChannel(channelId: string): Promise<void> {
  await apiDel(`/api/control/telegram-channels/${channelId}`, "Failed to delete Telegram channel");
}

// ---------------------------------------------------------------------------
// Standalone Bot API Functions (auto-create missions per chat)
// ---------------------------------------------------------------------------

export async function listTelegramBots(): Promise<TelegramChannel[]> {
  return apiGet<TelegramChannel[]>(
    `/api/control/telegram/bots`,
    "Failed to fetch Telegram bots"
  );
}

export async function createTelegramBot(
  input: CreateTelegramBotInput
): Promise<TelegramChannel> {
  return apiPost<TelegramChannel>(
    `/api/control/telegram/bots`,
    input,
    "Failed to create Telegram bot"
  );
}

export async function listBotChats(botId: string): Promise<TelegramChatMission[]> {
  return apiGet<TelegramChatMission[]>(
    `/api/control/telegram/bots/${botId}/chats`,
    "Failed to fetch bot chats"
  );
}

