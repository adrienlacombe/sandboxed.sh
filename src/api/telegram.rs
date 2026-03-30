//! Telegram bridge service.
//!
//! Manages long-polling bots that connect Telegram chats to assistant missions.
//! Each active `TelegramChannel` spawns a polling task that:
//! 1. Receives messages from Telegram via `getUpdates`
//! 2. Routes them as `ControlCommand::UserMessage` to the linked mission
//! 3. Subscribes to `AgentEvent` broadcasts and sends responses back to Telegram

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

/// Manages all active Telegram bot polling tasks.
pub struct TelegramBridge {
    /// Active polling tasks keyed by channel ID.
    active_channels: RwLock<HashMap<Uuid, tokio::task::JoinHandle<()>>>,
    http: Client,
}

impl TelegramBridge {
    pub fn new() -> Self {
        Self {
            active_channels: RwLock::new(HashMap::new()),
            http: Client::new(),
        }
    }

    /// Start polling for a Telegram channel.
    /// If a poller is already running for this channel, it is stopped first.
    pub async fn start_channel(
        &self,
        channel: TelegramChannel,
        cmd_tx: mpsc::Sender<ControlCommand>,
        events_tx: broadcast::Sender<AgentEvent>,
    ) {
        // Stop existing poller if any
        self.stop_channel(channel.id).await;

        let http = self.http.clone();
        let channel_id = channel.id;

        let handle = tokio::spawn(async move {
            if let Err(e) = run_telegram_poller(channel, cmd_tx, events_tx, http).await {
                tracing::error!("Telegram poller for channel {} failed: {}", channel_id, e);
            }
        });

        self.active_channels.write().await.insert(channel_id, handle);
        tracing::info!("Started Telegram poller for channel {}", channel_id);
    }

    /// Stop polling for a channel.
    pub async fn stop_channel(&self, channel_id: Uuid) {
        if let Some(handle) = self.active_channels.write().await.remove(&channel_id) {
            handle.abort();
            tracing::info!("Stopped Telegram poller for channel {}", channel_id);
        }
    }

    /// Stop all polling tasks.
    pub async fn stop_all(&self) {
        let mut channels = self.active_channels.write().await;
        for (id, handle) in channels.drain() {
            handle.abort();
            tracing::info!("Stopped Telegram poller for channel {}", id);
        }
    }

    /// Check if a channel poller is running.
    pub async fn is_running(&self, channel_id: Uuid) -> bool {
        self.active_channels.read().await.contains_key(&channel_id)
    }

    /// Boot all active channels from the store.
    pub async fn boot_from_store(
        &self,
        store: &Arc<dyn MissionStore>,
        cmd_tx: mpsc::Sender<ControlCommand>,
        events_tx: broadcast::Sender<AgentEvent>,
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
                    self.start_channel(channel, cmd_tx.clone(), events_tx.clone())
                        .await;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to load Telegram channels from store: {}", e);
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Telegram Bot API types (minimal subset)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: Option<T>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    message_id: i64,
    chat: Chat,
    from: Option<User>,
    text: Option<String>,
    reply_to_message: Option<Box<Message>>,
    entities: Option<Vec<MessageEntity>>,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
    #[serde(rename = "type")]
    chat_type: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct User {
    id: i64,
    first_name: String,
    last_name: Option<String>,
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MessageEntity {
    #[serde(rename = "type")]
    entity_type: String,
    offset: i64,
    length: i64,
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

// ─────────────────────────────────────────────────────────────────────────────
// Poller implementation
// ─────────────────────────────────────────────────────────────────────────────

/// Long-polling loop for a single Telegram bot.
async fn run_telegram_poller(
    channel: TelegramChannel,
    cmd_tx: mpsc::Sender<ControlCommand>,
    events_tx: broadcast::Sender<AgentEvent>,
    http: Client,
) -> Result<(), String> {
    let base_url = format!("https://api.telegram.org/bot{}", channel.bot_token);
    let mut offset: i64 = 0;

    // Resolve bot username if not set
    let bot_username = if let Some(ref u) = channel.bot_username {
        u.clone()
    } else {
        get_bot_username(&http, &base_url).await.unwrap_or_default()
    };

    tracing::info!(
        "Telegram poller started for channel {} (bot: @{}, mission: {})",
        channel.id,
        bot_username,
        channel.mission_id,
    );

    loop {
        // Long-poll with 30s timeout
        let url = format!(
            "{}/getUpdates?offset={}&timeout=30&allowed_updates=[\"message\"]",
            base_url, offset
        );

        let response = match http.get(&url).send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Telegram poll error for channel {}: {}", channel.id, e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        let body: TelegramResponse<Vec<Update>> = match response.json().await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("Telegram parse error for channel {}: {}", channel.id, e);
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            }
        };

        if !body.ok {
            tracing::error!(
                "Telegram API error for channel {}: {:?}",
                channel.id,
                body.description
            );
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            continue;
        }

        let updates = body.result.unwrap_or_default();
        for update in updates {
            offset = update.update_id + 1;

            if let Some(msg) = update.message {
                if let Some(text) = &msg.text {
                    if should_process_message(&channel, &msg, &bot_username) {
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

                        // Strip bot mention from the text if present
                        let clean_text = strip_bot_mention(text, &bot_username);

                        let content = format!(
                            "[Telegram from {} in chat {}] {}",
                            sender_name, msg.chat.id, clean_text
                        );

                        tracing::info!(
                            "Telegram message for mission {} from {}: {}",
                            channel.mission_id,
                            sender_name,
                            &clean_text[..clean_text.len().min(100)]
                        );

                        // Send to mission
                        let msg_id = Uuid::new_v4();
                        let (queued_tx, _queued_rx) = tokio::sync::oneshot::channel();
                        let _ = cmd_tx
                            .send(ControlCommand::UserMessage {
                                id: msg_id,
                                content,
                                agent: None,
                                target_mission_id: Some(channel.mission_id),
                                respond: queued_tx,
                            })
                            .await;

                        // Spawn a task to listen for the response and send it back
                        let events_rx = events_tx.subscribe();
                        let http_clone = http.clone();
                        let base_url_clone = base_url.clone();
                        let chat_id = msg.chat.id;
                        let reply_to = msg.message_id;
                        let mission_id = channel.mission_id;

                        tokio::spawn(async move {
                            if let Err(e) = wait_and_reply(
                                events_rx,
                                &http_clone,
                                &base_url_clone,
                                chat_id,
                                reply_to,
                                mission_id,
                            )
                            .await
                            {
                                tracing::warn!(
                                    "Failed to send Telegram reply for mission {}: {}",
                                    mission_id,
                                    e
                                );
                            }
                        });
                    }
                }
            }
        }
    }
}

/// Check if a message should be processed based on trigger mode and allowed chats.
fn should_process_message(channel: &TelegramChannel, msg: &Message, bot_username: &str) -> bool {
    // Check allowed chat IDs
    if !channel.allowed_chat_ids.is_empty()
        && !channel.allowed_chat_ids.contains(&msg.chat.id)
    {
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
                        let mention =
                            &text[e.offset as usize..(e.offset + e.length) as usize];
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

/// Wait for the agent to finish its turn and send the response to Telegram.
async fn wait_and_reply(
    mut events_rx: broadcast::Receiver<AgentEvent>,
    http: &Client,
    base_url: &str,
    chat_id: i64,
    reply_to: i64,
    mission_id: Uuid,
) -> Result<(), String> {
    let timeout = tokio::time::Duration::from_secs(300); // 5 minute max wait
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err("Timeout waiting for agent response".to_string());
        }

        match tokio::time::timeout(remaining, events_rx.recv()).await {
            Ok(Ok(AgentEvent::AssistantMessage {
                content,
                mission_id: Some(mid),
                ..
            })) if mid == mission_id => {
                // Send the response back to Telegram
                send_telegram_message(http, base_url, chat_id, &content, Some(reply_to)).await?;
                return Ok(());
            }
            Ok(Ok(AgentEvent::Error {
                message,
                mission_id: Some(mid),
                ..
            })) if mid == mission_id => {
                let error_msg = format!("Error: {}", message);
                send_telegram_message(http, base_url, chat_id, &error_msg, Some(reply_to)).await?;
                return Ok(());
            }
            Ok(Err(broadcast::error::RecvError::Lagged(n))) => {
                tracing::warn!("Telegram response listener lagged by {} events", n);
            }
            Ok(Err(broadcast::error::RecvError::Closed)) => {
                return Err("Event channel closed".to_string());
            }
            Err(_) => {
                return Err("Timeout waiting for agent response".to_string());
            }
            _ => {
                // Not our event, keep listening
            }
        }
    }
}

/// Send a message via the Telegram Bot API.
async fn send_telegram_message(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    text: &str,
    reply_to: Option<i64>,
) -> Result<(), String> {
    // Telegram has a 4096 character limit per message.
    // Split long messages into chunks.
    let max_len = 4000; // Leave some margin
    let chunks: Vec<&str> = if text.len() <= max_len {
        vec![text]
    } else {
        text.as_bytes()
            .chunks(max_len)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or("[encoding error]"))
            .collect()
    };

    for (i, chunk) in chunks.iter().enumerate() {
        let body = SendMessageRequest {
            chat_id,
            text: chunk,
            reply_to_message_id: if i == 0 { reply_to } else { None },
            parse_mode: None, // Plain text to avoid Markdown parsing issues
        };

        let url = format!("{}/sendMessage", base_url);
        let response = http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Failed to send Telegram message: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            tracing::warn!(
                "Telegram sendMessage failed ({}): {}",
                status,
                body_text
            );
            // If first chunk fails, return error. Otherwise log and continue.
            if i == 0 {
                return Err(format!("Telegram API error {}: {}", status, body_text));
            }
        }
    }

    Ok(())
}

/// Fetch the bot's username via getMe.
async fn get_bot_username(http: &Client, base_url: &str) -> Result<String, String> {
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
