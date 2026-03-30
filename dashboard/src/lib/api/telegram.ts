/**
 * Telegram Channels API - manage Telegram bot integrations for assistant missions.
 */

import { apiGet, apiPost, apiPatch, apiDel } from "./core";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export type TelegramTriggerMode = "bot_mention" | "reply" | "direct_message" | "all";

export interface TelegramChannel {
  id: string;
  mission_id: string;
  bot_username: string | null;
  allowed_chat_ids: number[];
  trigger_mode: TelegramTriggerMode;
  active: boolean;
  instructions: string | null;
  created_at: string;
  updated_at: string;
}

export interface CreateTelegramChannelInput {
  bot_token: string;
  bot_username?: string;
  allowed_chat_ids?: number[];
  trigger_mode?: TelegramTriggerMode;
  instructions?: string;
}

export interface UpdateTelegramChannelInput {
  active?: boolean;
  trigger_mode?: TelegramTriggerMode;
  allowed_chat_ids?: number[];
  instructions?: string;
}

// ---------------------------------------------------------------------------
// API Functions
// ---------------------------------------------------------------------------

export async function listTelegramChannels(missionId: string): Promise<TelegramChannel[]> {
  return apiGet<TelegramChannel[]>(
    `/api/control/missions/${missionId}/telegram/channels`,
    "Failed to fetch Telegram channels"
  );
}

export async function createTelegramChannel(
  missionId: string,
  input: CreateTelegramChannelInput
): Promise<TelegramChannel> {
  return apiPost<TelegramChannel>(
    `/api/control/missions/${missionId}/telegram/channels`,
    input,
    "Failed to create Telegram channel"
  );
}

export async function updateTelegramChannel(
  channelId: string,
  updates: UpdateTelegramChannelInput
): Promise<TelegramChannel> {
  return apiPatch<TelegramChannel>(
    `/api/control/telegram/channels/${channelId}`,
    updates,
    "Failed to update Telegram channel"
  );
}

export async function deleteTelegramChannel(channelId: string): Promise<void> {
  await apiDel(`/api/control/telegram/channels/${channelId}`, "Failed to delete Telegram channel");
}

export async function listAssistantMissions(): Promise<unknown[]> {
  return apiGet<unknown[]>(
    `/api/control/assistants`,
    "Failed to fetch assistant missions"
  );
}

export async function setMissionMode(
  missionId: string,
  mode: "task" | "assistant"
): Promise<void> {
  await apiPost(
    `/api/control/missions/${missionId}/mode`,
    { mode },
    "Failed to set mission mode"
  );
}
