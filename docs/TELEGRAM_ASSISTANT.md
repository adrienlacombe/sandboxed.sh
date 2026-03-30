# Telegram Assistant

Connect a Telegram bot to a sandboxed.sh mission so users can chat with an AI assistant directly from Telegram.

## Overview

A **Telegram Assistant** is a persistent mission (mode: `assistant`) that receives messages from a Telegram bot via webhooks and streams responses back. The assistant has full access to the host workspace's MCP servers, tools, and library — just like any other mission.

Assistant missions cannot be started from the dashboard frontend. They are created by attaching a Telegram channel to an existing mission (which automatically sets it to `assistant` mode).

## Setup

### 1. Create a Telegram Bot

1. Open Telegram and message [@BotFather](https://t.me/BotFather)
2. Send `/newbot` and follow the prompts to choose a name and username
3. Copy the **bot token** (e.g. `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`)

### 2. Create a Mission

Create a mission that will serve as the assistant:

```
POST /api/control/missions
Authorization: Bearer <token>

{
  "title": "My Telegram Assistant"
}
```

Note the returned mission `id`.

### 3. Attach a Telegram Channel

```
POST /api/control/missions/:mission_id/telegram-channels
Authorization: Bearer <token>

{
  "bot_token": "123456789:ABCdefGHIjklMNOpqrsTUVwxyz",
  "bot_username": "my_bot",
  "allowed_chat_ids": [12345678],
  "trigger_mode": "all",
  "instructions": "Respond in plain text only. Do not use markdown formatting."
}
```

This will:
- Auto-set the mission to `assistant` mode
- Register a Telegram webhook so the bot receives messages
- Start routing messages to the mission

**Fields:**

| Field | Required | Description |
|---|---|---|
| `bot_token` | Yes | Bot token from BotFather |
| `bot_username` | No | Bot username (for mention detection in groups) |
| `allowed_chat_ids` | No | Restrict to specific chat IDs. Empty = allow all. |
| `trigger_mode` | No | `"all"` (default), `"mention_only"`, or `"private_only"` |
| `instructions` | No | System instructions prepended to every message (e.g. formatting rules) |

### 4. Start Chatting

Message your bot in Telegram. The assistant will respond with streaming updates (typing indicator, progressive edits).

## Configuration

### Instructions

The `instructions` field lets you customize the assistant's behavior per-channel. Common uses:

- `"Respond in plain text only. Do not use markdown formatting."` — Telegram doesn't render full markdown
- `"You are a helpful coding assistant. Keep answers concise."` — Set personality/scope
- `"Always respond in French."` — Language preference

Instructions are prepended to every incoming message as `[Instructions: ...]`.

### Trigger Modes

- **`all`** — Process every message in allowed chats
- **`mention_only`** — Only respond when the bot is @mentioned (useful in group chats)
- **`private_only`** — Only respond in private (1:1) conversations

### Chat ID Restrictions

Set `allowed_chat_ids` to restrict which Telegram chats can interact with the bot. Leave empty to allow all chats. You can find a chat's ID by forwarding a message to [@userinfobot](https://t.me/userinfobot).

## API Reference

### List Assistant Missions

```
GET /api/control/assistants
Authorization: Bearer <token>
```

Returns all missions with `mission_mode: "assistant"`.

### List Telegram Channels for a Mission

```
GET /api/control/missions/:id/telegram-channels
Authorization: Bearer <token>
```

### Toggle Channel Active/Inactive

```
POST /api/control/telegram-channels/:id/toggle
Authorization: Bearer <token>

{ "active": false }
```

Deactivating a channel unregisters the Telegram webhook. Reactivating re-registers it.

### Delete a Channel

```
DELETE /api/control/telegram-channels/:id
Authorization: Bearer <token>
```

## Architecture

```
Telegram → webhook → /api/telegram/webhook/:channel_id
         → TelegramBridge routes to ChannelContext
         → ControlCommand::UserMessage { target_mission_id }
         → MissionRunner (parallel execution)
         → AgentEvent stream
         → TelegramBridge sends response via editMessageText
```

Key design decisions:

- **Parallel execution**: Telegram messages always run in parallel runners, never hijacking the main session. This prevents assistant messages from appearing in the user's dashboard view of other missions.
- **Webhook-based**: Uses Telegram's `setWebhook` API (not polling) for lower latency and simpler infrastructure.
- **Streaming responses**: The bot sends a typing indicator, then the first text chunk as a message, followed by progressive `editMessageText` calls (throttled to every 1.5s) as the AI generates more text.
- **Eager boot**: On server startup, Telegram webhooks are re-registered automatically. No user interaction required.

## Security

- **Bot token**: Stored in the mission database. Not returned in API responses (masked via `skip_serializing`). Consider moving to the encrypted secrets store for production use.
- **Webhook secret**: Each channel gets a unique webhook secret validated via `X-Telegram-Bot-Api-Secret-Token` header. Only Telegram can call the webhook endpoint.
- **Chat ID filtering**: Use `allowed_chat_ids` to restrict which Telegram users/groups can interact with the bot.

## Troubleshooting

**Bot not responding?**
1. Check the channel is active: `GET /api/control/missions/:id/telegram-channels`
2. Toggle the channel off and on to re-register the webhook
3. Check server logs for `boot_from_store` or webhook registration errors

**Messages appearing in wrong mission?**
This was a known bug (fixed). Telegram messages now always use parallel execution, ensuring they stay in their own mission context.

**Webhook not registered after restart?**
The server eagerly boots the default control session on startup, which re-registers all active Telegram webhooks. If this fails, any authenticated API call will trigger lazy boot as a fallback.
