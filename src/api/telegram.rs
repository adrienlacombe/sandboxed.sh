//! Telegram bridge service.
//!
//! Connects Telegram bots to assistant missions using webhooks (instant delivery)
//! and streaming responses via `sendChatAction` + `editMessageText`.
//!
//! Flow:
//! 1. On channel creation, registers a Telegram webhook pointing at our public endpoint
//! 2. Telegram POSTs updates instantly to `/api/telegram/webhook/:channel_id`
//! 3. The webhook handler routes the message as `ControlCommand::UserMessage`
//! 4. A response task streams `TextDelta` events back via `editMessageText`

use crate::api::control::{AgentEvent, ControlCommand};
use crate::api::mission_store::{MissionStore, TelegramChannel, TelegramTriggerMode};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, RwLock};
use uuid::Uuid;

/// Shared handle to the Telegram bridge manager.
pub type SharedTelegramBridge = Arc<TelegramBridge>;

/// Manages Telegram webhook registrations and channel routing context.
pub struct TelegramBridge {
    /// Routing context for each active channel (needed to forward webhook messages).
    active_channels: RwLock<HashMap<Uuid, ChannelContext>>,
    http: Client,
}

/// Context needed to route incoming webhook messages to a mission.
#[derive(Clone)]
pub struct ChannelContext {
    pub channel: TelegramChannel,
    pub bot_username: String,
    pub cmd_tx: mpsc::Sender<ControlCommand>,
    pub events_tx: broadcast::Sender<AgentEvent>,
}

impl TelegramBridge {
    pub fn new() -> Self {
        Self {
            active_channels: RwLock::new(HashMap::new()),
            http: Client::new(),
        }
    }

    /// Register a webhook for a Telegram channel and store routing context.
    pub async fn start_channel(
        &self,
        channel: TelegramChannel,
        cmd_tx: mpsc::Sender<ControlCommand>,
        events_tx: broadcast::Sender<AgentEvent>,
        public_base_url: &str,
    ) {
        self.stop_channel(channel.id).await;

        let base_url = format!("https://api.telegram.org/bot{}", channel.bot_token);

        // Resolve bot username
        let bot_username = if let Some(ref u) = channel.bot_username {
            u.clone()
        } else {
            get_bot_username(&self.http, &base_url)
                .await
                .unwrap_or_default()
        };

        // Register the webhook with Telegram
        let webhook_url = format!(
            "{}/api/telegram/webhook/{}",
            public_base_url.trim_end_matches('/'),
            channel.id
        );

        if let Err(e) = set_webhook(
            &self.http,
            &base_url,
            &webhook_url,
            channel.webhook_secret.as_deref(),
        )
        .await
        {
            tracing::error!(
                "Failed to set Telegram webhook for channel {}: {}",
                channel.id,
                e
            );
            return;
        }

        tracing::info!(
            "Registered Telegram webhook for channel {} (bot: @{}, mission: {}, url: {})",
            channel.id,
            bot_username,
            channel.mission_id,
            webhook_url,
        );

        let ctx = ChannelContext {
            channel,
            bot_username,
            cmd_tx,
            events_tx,
        };

        self.active_channels.write().await.insert(ctx.channel.id, ctx);
    }

    /// Remove webhook and routing context for a channel.
    pub async fn stop_channel(&self, channel_id: Uuid) {
        if let Some(ctx) = self.active_channels.write().await.remove(&channel_id) {
            let base_url = format!("https://api.telegram.org/bot{}", ctx.channel.bot_token);
            if let Err(e) = delete_webhook(&self.http, &base_url).await {
                tracing::warn!(
                    "Failed to delete Telegram webhook for channel {}: {}",
                    channel_id,
                    e
                );
            }
            tracing::info!("Stopped Telegram channel {}", channel_id);
        }
    }

    /// Stop all channels.
    pub async fn stop_all(&self) {
        let channels: Vec<_> = self.active_channels.write().await.drain().collect();
        for (id, ctx) in channels {
            let base_url = format!("https://api.telegram.org/bot{}", ctx.channel.bot_token);
            let _ = delete_webhook(&self.http, &base_url).await;
            tracing::info!("Stopped Telegram channel {}", id);
        }
    }

    /// Check if a channel is active.
    pub async fn is_running(&self, channel_id: Uuid) -> bool {
        self.active_channels.read().await.contains_key(&channel_id)
    }

    /// Get the routing context for a channel (used by webhook handler).
    pub async fn get_channel_context(&self, channel_id: Uuid) -> Option<ChannelContext> {
        self.active_channels.read().await.get(&channel_id).cloned()
    }

    /// Boot all active channels from the store.
    pub async fn boot_from_store(
        &self,
        store: &Arc<dyn MissionStore>,
        cmd_tx: mpsc::Sender<ControlCommand>,
        events_tx: broadcast::Sender<AgentEvent>,
        public_base_url: &str,
    ) {
        match store.list_all_active_telegram_channels().await {
            Ok(channels) => {
                if !channels.is_empty() {
                    tracing::info!(
                        "Booting {} active Telegram channel(s) from store",
                        channels.len()
                    );
                }
                for channel in channels {
                    self.start_channel(
                        channel,
                        cmd_tx.clone(),
                        events_tx.clone(),
                        public_base_url,
                    )
                    .await;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load Telegram channels from store: {}", e);
            }
        }
    }

    /// Get a reference to the HTTP client (for use in webhook handler).
    pub fn http(&self) -> &Client {
        &self.http
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Bot API types (minimal subset)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TelegramResponse<T> {
    pub ok: bool,
    #[allow(dead_code)]
    pub result: Option<T>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    #[allow(dead_code)]
    pub update_id: i64,
    pub message: Option<Message>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub chat: Chat,
    pub from: Option<User>,
    pub text: Option<String>,
    pub reply_to_message: Option<Box<Message>>,
    pub entities: Option<Vec<MessageEntity>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub chat_type: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct User {
    pub id: i64,
    pub first_name: String,
    pub last_name: Option<String>,
    pub username: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageEntity {
    #[serde(rename = "type")]
    pub entity_type: String,
    pub offset: i64,
    pub length: i64,
}

#[derive(Debug, Serialize)]
struct SendMessageRequest<'a> {
    chat_id: i64,
    text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to_message_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct SendMessageResponse {
    message_id: i64,
}

#[derive(Debug, Serialize)]
struct EditMessageRequest<'a> {
    chat_id: i64,
    message_id: i64,
    text: &'a str,
}

// ─────────────────────────────────────────────────────────────────────────────
// Webhook management
// ─────────────────────────────────────────────────────────────────────────────

/// Register a webhook URL with Telegram.
async fn set_webhook(
    http: &Client,
    base_url: &str,
    webhook_url: &str,
    secret_token: Option<&str>,
) -> Result<(), String> {
    let url = format!("{}/setWebhook", base_url);
    let mut params = serde_json::json!({
        "url": webhook_url,
        "allowed_updates": ["message"],
        "drop_pending_updates": true,
    });
    if let Some(secret) = secret_token {
        params["secret_token"] = serde_json::Value::String(secret.to_string());
    }
    let response = http
        .post(&url)
        .json(&params)
        .send()
        .await
        .map_err(|e| format!("setWebhook request failed: {}", e))?;

    let body: TelegramResponse<bool> = response
        .json()
        .await
        .map_err(|e| format!("setWebhook parse failed: {}", e))?;

    if body.ok {
        Ok(())
    } else {
        Err(format!(
            "setWebhook API error: {}",
            body.description.unwrap_or_default()
        ))
    }
}

/// Remove the webhook for a bot.
async fn delete_webhook(http: &Client, base_url: &str) -> Result<(), String> {
    let url = format!("{}/deleteWebhook", base_url);
    let response = http
        .post(&url)
        .send()
        .await
        .map_err(|e| format!("deleteWebhook request failed: {}", e))?;

    let body: TelegramResponse<bool> = response
        .json()
        .await
        .map_err(|e| format!("deleteWebhook parse failed: {}", e))?;

    if body.ok {
        Ok(())
    } else {
        Err(format!(
            "deleteWebhook API error: {}",
            body.description.unwrap_or_default()
        ))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Message processing (used by webhook handler)
// ─────────────────────────────────────────────────────────────────────────────

/// Process an incoming Telegram message from a webhook.
/// Called by the axum route handler.
pub async fn process_webhook_message(
    ctx: &ChannelContext,
    msg: &Message,
    http: &Client,
) {
    let text = match &msg.text {
        Some(t) => t,
        None => return,
    };

    if !should_process_message(&ctx.channel, msg, &ctx.bot_username) {
        return;
    }

    let sender_name = msg
        .from
        .as_ref()
        .map(|u| {
            if let Some(ref un) = u.username {
                format!("@{}", un)
            } else if let Some(ref last) = u.last_name {
                format!("{} {}", u.first_name, last)
            } else {
                u.first_name.clone()
            }
        })
        .unwrap_or_else(|| "Unknown".to_string());

    let clean_text = strip_bot_mention(text, &ctx.bot_username);

    let content = format!(
        "[Telegram from {} in chat {}] {}",
        sender_name, msg.chat.id, clean_text
    );

    tracing::info!(
        "Telegram webhook message for mission {} from {}: {}",
        ctx.channel.mission_id,
        sender_name,
        &clean_text[..clean_text.len().min(100)]
    );

    // Send to mission
    let msg_id = Uuid::new_v4();
    let (queued_tx, _queued_rx) = tokio::sync::oneshot::channel();
    let _ = ctx
        .cmd_tx
        .send(ControlCommand::UserMessage {
            id: msg_id,
            content,
            agent: None,
            target_mission_id: Some(ctx.channel.mission_id),
            respond: queued_tx,
        })
        .await;

    // Spawn a task to stream the response back to Telegram
    let events_rx = ctx.events_tx.subscribe();
    let http_clone = http.clone();
    let bot_token = ctx.channel.bot_token.clone();
    let chat_id = msg.chat.id;
    let reply_to = msg.message_id;
    let mission_id = ctx.channel.mission_id;

    tokio::spawn(async move {
        if let Err(e) =
            stream_response(events_rx, &http_clone, &bot_token, chat_id, reply_to, mission_id)
                .await
        {
            tracing::warn!(
                "Failed to stream Telegram reply for mission {}: {}",
                mission_id,
                e
            );
        }
    });
}

/// Check if a message should be processed based on trigger mode and allowed chats.
fn should_process_message(channel: &TelegramChannel, msg: &Message, bot_username: &str) -> bool {
    // Check allowed chat IDs
    if !channel.allowed_chat_ids.is_empty() && !channel.allowed_chat_ids.contains(&msg.chat.id) {
        return false;
    }

    let is_private = msg.chat.chat_type == "private";
    let is_mention = msg
        .entities
        .as_ref()
        .map(|entities| {
            entities.iter().any(|e| {
                if e.entity_type == "mention" {
                    if let Some(ref text) = msg.text {
                        let mention = &text[e.offset as usize..(e.offset + e.length) as usize];
                        mention.eq_ignore_ascii_case(&format!("@{}", bot_username))
                    } else {
                        false
                    }
                } else {
                    false
                }
            })
        })
        .unwrap_or(false);
    let is_reply = msg
        .reply_to_message
        .as_ref()
        .and_then(|r| r.from.as_ref())
        .map(|u| {
            u.username
                .as_ref()
                .map(|un| un.eq_ignore_ascii_case(bot_username))
                .unwrap_or(false)
        })
        .unwrap_or(false);

    match channel.trigger_mode {
        TelegramTriggerMode::DirectMessage => is_private,
        TelegramTriggerMode::BotMention => is_mention,
        TelegramTriggerMode::Reply => is_reply,
        TelegramTriggerMode::All => is_private || is_mention || is_reply,
    }
}

/// Strip @bot_username from the beginning of a message.
fn strip_bot_mention(text: &str, bot_username: &str) -> String {
    let mention = format!("@{}", bot_username);
    let trimmed = text.trim();
    if let Some(rest) = trimmed.strip_prefix(&mention) {
        rest.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Streaming response (typing indicator + progressive edits)
// ─────────────────────────────────────────────────────────────────────────────

/// Stream the agent response back to Telegram with typing indicator and progressive edits.
///
/// 1. Sends `sendChatAction(typing)` immediately
/// 2. On first `TextDelta`, sends an initial message and captures `message_id`
/// 3. Accumulates subsequent deltas and calls `editMessageText` every ~1s
/// 4. On `AssistantMessage`, sends final edit with full content
async fn stream_response(
    mut events_rx: broadcast::Receiver<AgentEvent>,
    http: &Client,
    bot_token: &str,
    chat_id: i64,
    reply_to: i64,
    mission_id: Uuid,
) -> Result<(), String> {
    let base_url = format!("https://api.telegram.org/bot{}", bot_token);
    let timeout = tokio::time::Duration::from_secs(300);
    let deadline = tokio::time::Instant::now() + timeout;

    // Send typing indicator immediately
    send_chat_action(http, &base_url, chat_id).await;

    let mut sent_message_id: Option<i64> = None;
    let mut accumulated_text = String::new();
    let mut last_edit = tokio::time::Instant::now();
    let edit_interval = tokio::time::Duration::from_millis(1500);
    let mut typing_interval = tokio::time::interval(tokio::time::Duration::from_secs(4));
    typing_interval.tick().await; // consume the first immediate tick

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            // If we sent a partial message, finalize it
            if sent_message_id.is_some() && !accumulated_text.is_empty() {
                let final_text = format!("{}...\n\n_(timed out)_", accumulated_text);
                let _ = edit_message(http, &base_url, chat_id, sent_message_id.unwrap(), &final_text).await;
            }
            return Err("Timeout waiting for agent response".to_string());
        }

        tokio::select! {
            event = events_rx.recv() => {
                match event {
                    Ok(AgentEvent::TextDelta {
                        content,
                        mission_id: Some(mid),
                        ..
                    }) if mid == mission_id => {
                        accumulated_text = content;

                        if sent_message_id.is_none() {
                            // Send initial message
                            match send_message(http, &base_url, chat_id, &accumulated_text, Some(reply_to)).await {
                                Ok(msg_id) => {
                                    sent_message_id = Some(msg_id);
                                    last_edit = tokio::time::Instant::now();
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to send initial Telegram message: {}", e);
                                }
                            }
                        } else if last_edit.elapsed() >= edit_interval {
                            // Throttled edit
                            let display = truncate_for_telegram(&accumulated_text);
                            let _ = edit_message(http, &base_url, chat_id, sent_message_id.unwrap(), &display).await;
                            last_edit = tokio::time::Instant::now();
                        }
                    }
                    Ok(AgentEvent::AssistantMessage {
                        content,
                        mission_id: Some(mid),
                        ..
                    }) if mid == mission_id => {
                        // Final response — send or edit with complete text
                        if let Some(msg_id) = sent_message_id {
                            // Edit existing message with final content
                            let display = truncate_for_telegram(&content);
                            let _ = edit_message(http, &base_url, chat_id, msg_id, &display).await;
                            // If content exceeds 4096 chars, send overflow as new messages
                            send_overflow_chunks(http, &base_url, chat_id, &content).await;
                        } else {
                            // No streaming happened, send the full response directly
                            send_chunked_message(http, &base_url, chat_id, &content, Some(reply_to)).await?;
                        }
                        return Ok(());
                    }
                    Ok(AgentEvent::Error {
                        message,
                        mission_id: Some(mid),
                        ..
                    }) if mid == mission_id => {
                        let error_msg = format!("Error: {}", message);
                        if let Some(msg_id) = sent_message_id {
                            let final_text = if accumulated_text.is_empty() {
                                error_msg
                            } else {
                                format!("{}\n\n_{}_", accumulated_text, error_msg)
                            };
                            let _ = edit_message(http, &base_url, chat_id, msg_id, &final_text).await;
                        } else {
                            let _ = send_message(http, &base_url, chat_id, &error_msg, Some(reply_to)).await;
                        }
                        return Ok(());
                    }
                    Ok(AgentEvent::Thinking {
                        mission_id: Some(mid),
                        ..
                    }) if mid == mission_id => {
                        // Keep sending typing indicator while agent is thinking
                        send_chat_action(http, &base_url, chat_id).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!("Telegram response listener lagged by {} events", n);
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err("Event channel closed".to_string());
                    }
                    _ => {
                        // Not our event, keep listening
                    }
                }
            }
            _ = typing_interval.tick() => {
                // Keep typing indicator alive every 4s while waiting
                if sent_message_id.is_none() {
                    send_chat_action(http, &base_url, chat_id).await;
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram API helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Send `sendChatAction(typing)` to show typing indicator.
async fn send_chat_action(http: &Client, base_url: &str, chat_id: i64) {
    let url = format!("{}/sendChatAction", base_url);
    let _ = http
        .post(&url)
        .json(&serde_json::json!({
            "chat_id": chat_id,
            "action": "typing"
        }))
        .send()
        .await;
}

/// Send a message and return the message_id.
async fn send_message(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<i64, String> {
    let display = truncate_for_telegram(text);
    let body = SendMessageRequest {
        chat_id,
        text: &display,
        reply_to_message_id: reply_to,
        parse_mode: None,
    };

    let url = format!("{}/sendMessage", base_url);
    let response = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("sendMessage failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        return Err(format!("sendMessage API error {}: {}", status, body_text));
    }

    let parsed: TelegramResponse<SendMessageResponse> = response
        .json()
        .await
        .map_err(|e| format!("sendMessage parse failed: {}", e))?;

    parsed
        .result
        .map(|r| r.message_id)
        .ok_or_else(|| "sendMessage returned no result".to_string())
}

/// Edit an existing message's text.
async fn edit_message(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    message_id: i64,
    text: &str,
) -> Result<(), String> {
    if text.is_empty() {
        return Ok(());
    }
    let body = EditMessageRequest {
        chat_id,
        message_id,
        text,
    };

    let url = format!("{}/editMessageText", base_url);
    let response = http
        .post(&url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("editMessageText failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body_text = response.text().await.unwrap_or_default();
        // "message is not modified" is not a real error (same content)
        if body_text.contains("message is not modified") {
            return Ok(());
        }
        return Err(format!(
            "editMessageText API error {}: {}",
            status, body_text
        ));
    }

    Ok(())
}

/// Truncate text to fit Telegram's 4096 character limit.
fn truncate_for_telegram(text: &str) -> String {
    if text.len() <= 4096 {
        text.to_string()
    } else {
        // Truncate to 4090 chars + "..." indicator
        let truncated = &text[..text.floor_char_boundary(4090)];
        format!("{}...", truncated)
    }
}

/// Send overflow chunks (content beyond 4096 chars) as separate messages.
async fn send_overflow_chunks(http: &Client, base_url: &str, chat_id: i64, text: &str) {
    if text.len() <= 4096 {
        return;
    }
    // The first 4096 chars were already sent via edit. Send the rest in chunks.
    let rest = &text[text.floor_char_boundary(4090)..];
    if rest.is_empty() {
        return;
    }
    let _ = send_chunked_message(http, base_url, chat_id, rest, None).await;
}

/// Send a long message split into multiple chunks.
async fn send_chunked_message(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<(), String> {
    let max_len = 4000;
    if text.len() <= max_len {
        let _ = send_message(http, base_url, chat_id, text, reply_to).await?;
        return Ok(());
    }

    let mut remaining = text;
    let mut first = true;
    while !remaining.is_empty() {
        let boundary = if remaining.len() <= max_len {
            remaining.len()
        } else {
            remaining.floor_char_boundary(max_len)
        };
        let chunk = &remaining[..boundary];
        remaining = &remaining[boundary..];

        let reply = if first { reply_to } else { None };
        first = false;
        let _ = send_message(http, base_url, chat_id, chunk, reply).await;
    }
    Ok(())
}

/// Fetch the bot's username via getMe.
pub async fn get_bot_username(http: &Client, base_url: &str) -> Result<String, String> {
    let url = format!("{}/getMe", base_url);
    let response = http
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("getMe failed: {}", e))?;

    #[derive(Deserialize)]
    struct GetMeResult {
        username: Option<String>,
    }

    let body: TelegramResponse<GetMeResult> = response
        .json()
        .await
        .map_err(|e| format!("getMe parse error: {}", e))?;

    body.result
        .and_then(|r| r.username)
        .ok_or_else(|| "Bot has no username".to_string())
}
