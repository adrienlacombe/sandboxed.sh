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
use crate::api::mission_store::{
    MissionMode, MissionStore, TelegramChannel, TelegramChatMission, TelegramTriggerMode,
};
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
    pub mission_store: Arc<dyn MissionStore>,
}

impl Default for TelegramBridge {
    fn default() -> Self {
        Self::new()
    }
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
        mission_store: Arc<dyn MissionStore>,
        public_base_url: &str,
    ) -> Result<(), String> {
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

        set_webhook(
            &self.http,
            &base_url,
            &webhook_url,
            channel.webhook_secret.as_deref(),
        )
        .await
        .map_err(|e| {
            let msg = format!(
                "Failed to set Telegram webhook for channel {}: {}",
                channel.id, e
            );
            tracing::error!("{}", msg);
            msg
        })?;

        let mode_label = if channel.auto_create_missions {
            "auto-create".to_string()
        } else {
            format!("mission: {}", channel.mission_id)
        };
        tracing::info!(
            "Registered Telegram webhook for channel {} (bot: @{}, {}, url: {})",
            channel.id,
            bot_username,
            mode_label,
            webhook_url,
        );

        let ctx = ChannelContext {
            channel,
            bot_username,
            cmd_tx,
            events_tx,
            mission_store,
        };

        self.active_channels
            .write()
            .await
            .insert(ctx.channel.id, ctx);

        Ok(())
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
                    let ch_id = channel.id;
                    if let Err(e) = self
                        .start_channel(
                            channel,
                            cmd_tx.clone(),
                            events_tx.clone(),
                            store.clone(),
                            public_base_url,
                        )
                        .await
                    {
                        tracing::warn!("Failed to boot Telegram channel {}: {}", ch_id, e);
                    }
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
    /// Caption for media messages (photos, documents, etc.)
    pub caption: Option<String>,
    pub reply_to_message: Option<Box<Message>>,
    pub entities: Option<Vec<MessageEntity>>,
    /// Entities in the caption (for media messages with @mentions in captions)
    pub caption_entities: Option<Vec<MessageEntity>>,
    /// Document attachment (PDF, ZIP, etc.)
    pub document: Option<TelegramDocument>,
    /// Photo attachment (array of sizes, last is largest)
    pub photo: Option<Vec<PhotoSize>>,
    /// Voice message
    pub voice: Option<TelegramFile>,
    /// Audio file
    pub audio: Option<TelegramFile>,
    /// Video file
    pub video: Option<TelegramFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramDocument {
    pub file_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PhotoSize {
    pub file_id: String,
    pub width: i64,
    pub height: i64,
    #[serde(default)]
    pub file_size: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramFile {
    pub file_id: String,
    pub file_name: Option<String>,
    pub mime_type: Option<String>,
    #[serde(default)]
    pub file_size: Option<i64>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    parse_mode: Option<&'a str>,
}

/// Response from the Telegram `getFile` API.
#[derive(Debug, Deserialize)]
struct GetFileResponse {
    file_path: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// File download helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Download a file from Telegram by file_id and save it to a local directory.
/// Returns the local file path on success.
async fn download_telegram_file(
    http: &Client,
    bot_token: &str,
    file_id: &str,
    filename: &str,
    dest_dir: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let base_url = format!("https://api.telegram.org/bot{}", bot_token);

    // Step 1: Get file path from Telegram
    let url = format!("{}/getFile", base_url);
    let response = http
        .post(&url)
        .json(&serde_json::json!({ "file_id": file_id }))
        .send()
        .await
        .map_err(|e| format!("getFile request failed: {}", e))?;

    let body: TelegramResponse<GetFileResponse> = response
        .json()
        .await
        .map_err(|e| format!("getFile parse failed: {}", e))?;

    let tg_file_path = body
        .result
        .and_then(|r| r.file_path)
        .ok_or_else(|| "getFile returned no file_path".to_string())?;

    // Step 2: Download the file
    let download_url = format!(
        "https://api.telegram.org/file/bot{}/{}",
        bot_token, tg_file_path
    );
    let download_response = http
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("File download failed: {}", e))?;
    if !download_response.status().is_success() {
        return Err(format!(
            "File download HTTP error: {}",
            download_response.status()
        ));
    }
    // Enforce a 50 MB size limit to prevent OOM from large files
    const MAX_FILE_SIZE: u64 = 50 * 1024 * 1024;
    if let Some(content_length) = download_response.content_length() {
        if content_length > MAX_FILE_SIZE {
            return Err(format!(
                "File too large: {} bytes (limit: {} bytes)",
                content_length, MAX_FILE_SIZE
            ));
        }
    }
    let file_bytes = download_response
        .bytes()
        .await
        .map_err(|e| format!("File read failed: {}", e))?;
    if file_bytes.len() as u64 > MAX_FILE_SIZE {
        return Err(format!(
            "File too large: {} bytes (limit: {} bytes)",
            file_bytes.len(),
            MAX_FILE_SIZE
        ));
    }

    // Step 3: Save to destination
    tokio::fs::create_dir_all(dest_dir)
        .await
        .map_err(|e| format!("Failed to create upload dir: {}", e))?;

    let safe_name = filename.replace(['/', '\\', '\0'], "_");
    let dest_path = dest_dir.join(&safe_name);
    tokio::fs::write(&dest_path, &file_bytes)
        .await
        .map_err(|e| format!("Failed to write file: {}", e))?;

    tracing::info!(
        "Downloaded Telegram file {} ({} bytes) to {}",
        safe_name,
        file_bytes.len(),
        dest_path.display()
    );

    Ok(dest_path)
}

/// Extract file info from a Telegram message. Returns (file_id, filename, mime_type).
fn extract_file_info(msg: &Message) -> Option<(String, String, String)> {
    if let Some(ref doc) = msg.document {
        let name = doc.file_name.clone().unwrap_or_else(|| {
            format!(
                "document_{}",
                doc.file_id.chars().take(8).collect::<String>()
            )
        });
        let mime = doc
            .mime_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        return Some((doc.file_id.clone(), name, mime));
    }
    if let Some(ref photos) = msg.photo {
        if let Some(largest) = photos.last() {
            let name = format!(
                "photo_{}.jpg",
                largest.file_id.chars().take(8).collect::<String>()
            );
            return Some((largest.file_id.clone(), name, "image/jpeg".to_string()));
        }
    }
    if let Some(ref voice) = msg.voice {
        let name = voice
            .file_name
            .clone()
            .unwrap_or_else(|| "voice_message.ogg".to_string());
        let mime = voice
            .mime_type
            .clone()
            .unwrap_or_else(|| "audio/ogg".to_string());
        return Some((voice.file_id.clone(), name, mime));
    }
    if let Some(ref audio) = msg.audio {
        let name = audio
            .file_name
            .clone()
            .unwrap_or_else(|| "audio.mp3".to_string());
        let mime = audio
            .mime_type
            .clone()
            .unwrap_or_else(|| "audio/mpeg".to_string());
        return Some((audio.file_id.clone(), name, mime));
    }
    if let Some(ref video) = msg.video {
        let name = video
            .file_name
            .clone()
            .unwrap_or_else(|| "video.mp4".to_string());
        let mime = video
            .mime_type
            .clone()
            .unwrap_or_else(|| "video/mp4".to_string());
        return Some((video.file_id.clone(), name, mime));
    }
    None
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

/// Resolve or auto-create a mission for a Telegram chat.
async fn resolve_or_create_mission(
    ctx: &ChannelContext,
    chat_id: i64,
    sender_name: &str,
) -> Option<Uuid> {
    // 1. Look up existing mapping
    if let Ok(Some(mapping)) = ctx
        .mission_store
        .get_telegram_chat_mission(ctx.channel.id, chat_id)
        .await
    {
        return Some(mapping.mission_id);
    }

    // 2. Create a new mission via ControlCommand
    let (tx, rx) = tokio::sync::oneshot::channel();
    let title = Some(format!("Telegram: {}", sender_name));

    // Normalize agent name: strip parenthetical suffixes like "(Ultraworker)"
    // and lowercase to get the config key (e.g. "Sisyphus (Ultraworker)" → "sisyphus")
    let agent = ctx.channel.default_agent.as_ref().map(|a| {
        let name = if let Some(idx) = a.find('(') {
            a[..idx].trim()
        } else {
            a.trim()
        };
        name.to_lowercase()
    });

    let _ = ctx
        .cmd_tx
        .send(ControlCommand::CreateMission {
            title,
            workspace_id: ctx.channel.default_workspace_id,
            agent,
            model_override: ctx.channel.default_model_override.clone(),
            model_effort: ctx.channel.default_model_effort.clone(),
            backend: ctx.channel.default_backend.clone(),
            config_profile: ctx.channel.default_config_profile.clone(),
            parent_mission_id: None,
            working_directory: None,
            respond: tx,
        })
        .await;

    match rx.await {
        Ok(Ok(mission)) => {
            let mission_id = mission.id;

            // Set to assistant mode
            let _ = ctx
                .mission_store
                .update_mission_mode(mission_id, MissionMode::Assistant)
                .await;

            // Store the mapping
            let mapping = TelegramChatMission {
                id: Uuid::new_v4(),
                channel_id: ctx.channel.id,
                chat_id,
                mission_id,
                chat_title: None,
                created_at: crate::api::mission_store::now_string(),
            };
            // Handle race condition: if another message already created the mapping, look it up
            match ctx
                .mission_store
                .create_telegram_chat_mission(mapping)
                .await
            {
                Ok(_) => {}
                Err(e) if e.contains("UNIQUE constraint") => {
                    // Another concurrent message already created the mapping
                    if let Ok(Some(existing)) = ctx
                        .mission_store
                        .get_telegram_chat_mission(ctx.channel.id, chat_id)
                        .await
                    {
                        return Some(existing.mission_id);
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to store chat-mission mapping: {}", e);
                    return None;
                }
            }

            tracing::info!(
                "Auto-created mission {} for Telegram chat {} on channel {}",
                mission_id,
                chat_id,
                ctx.channel.id
            );

            Some(mission_id)
        }
        _ => {
            tracing::error!(
                "Failed to create mission for chat {} on channel {}",
                chat_id,
                ctx.channel.id
            );
            None
        }
    }
}

/// Process an incoming Telegram message from a webhook.
/// Called by the axum route handler.
pub async fn process_webhook_message(ctx: &ChannelContext, msg: &Message, http: &Client) {
    // Accept text, caption (on media), or file-only messages
    let text = msg.text.as_deref().or(msg.caption.as_deref()).unwrap_or("");

    let has_file = extract_file_info(msg).is_some();
    if text.is_empty() && !has_file {
        return;
    }

    let should_respond = should_process_message(&ctx.channel, msg, &ctx.bot_username);

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

    // Resolve target mission: auto-create per chat or legacy single-mission
    // For context-only messages (should_respond=false), only look up existing
    // missions — don't create new ones just to store context.
    let target_mission_id = if ctx.channel.auto_create_missions {
        if should_respond {
            match resolve_or_create_mission(ctx, msg.chat.id, &sender_name).await {
                Some(id) => id,
                None => {
                    let base_url = format!("https://api.telegram.org/bot{}", ctx.channel.bot_token);
                    let _ = send_message(
                        http,
                        &base_url,
                        msg.chat.id,
                        "Sorry, I couldn't start a new conversation. Please try again.",
                        Some(msg.message_id),
                    )
                    .await;
                    return;
                }
            }
        } else {
            // Context-only: look up existing mission for this chat, skip if none
            match ctx
                .mission_store
                .get_telegram_chat_mission(ctx.channel.id, msg.chat.id)
                .await
            {
                Ok(Some(mapping)) => mapping.mission_id,
                _ => return, // No existing mission for this chat — nothing to store context in
            }
        }
    } else {
        ctx.channel.mission_id
    };

    // Download attached file if present
    let file_annotation = if let Some((file_id, filename, mime)) = extract_file_info(msg) {
        let upload_dir =
            std::path::PathBuf::from("/tmp/telegram-uploads").join(target_mission_id.to_string());
        match download_telegram_file(
            http,
            &ctx.channel.bot_token,
            &file_id,
            &filename,
            &upload_dir,
        )
        .await
        {
            Ok(local_path) => Some(format!(
                "[Attached file: {} ({}), saved to: {}]",
                filename,
                mime,
                local_path.display()
            )),
            Err(e) => {
                tracing::warn!("Failed to download Telegram file: {}", e);
                Some(format!(
                    "[Attached file: {} ({}) — download failed: {}]",
                    filename, mime, e
                ))
            }
        }
    } else {
        None
    };

    // Build message content with optional system instructions and file info
    let mut parts = Vec::new();
    parts.push(format!(
        "[Telegram from {} in chat {}]",
        sender_name, msg.chat.id
    ));
    if let Some(ref instructions) = ctx.channel.instructions {
        parts.push(format!("[Instructions: {}]", instructions));
    }
    if let Some(ref file_info) = file_annotation {
        parts.push(file_info.clone());
    }
    if !clean_text.is_empty() {
        parts.push(clean_text.clone());
    }
    let content = parts.join(" ");

    if !should_respond {
        // Context-only: store the message in mission history without triggering
        // the agent. This lets the agent see full chat context when it IS triggered.
        tracing::debug!(
            "Storing Telegram context message for mission {} from {}: {}",
            target_mission_id,
            sender_name,
            &clean_text[..clean_text.floor_char_boundary(100)]
        );
        let _ = ctx
            .mission_store
            .log_event(
                target_mission_id,
                &AgentEvent::UserMessage {
                    id: Uuid::new_v4(),
                    content: content.clone(),
                    queued: false,
                    mission_id: Some(target_mission_id),
                },
            )
            .await;
        return;
    }

    tracing::info!(
        "Telegram webhook message for mission {} from {}: {}",
        target_mission_id,
        sender_name,
        &clean_text[..clean_text.floor_char_boundary(100)]
    );

    // Subscribe to events BEFORE sending the command to avoid race conditions
    // where the response arrives before the subscription is active.
    let events_rx = ctx.events_tx.subscribe();

    // Send to mission
    let msg_id = Uuid::new_v4();
    let (queued_tx, _queued_rx) = tokio::sync::oneshot::channel();
    let _ = ctx
        .cmd_tx
        .send(ControlCommand::UserMessage {
            id: msg_id,
            content,
            agent: None,
            target_mission_id: Some(target_mission_id),
            respond: queued_tx,
        })
        .await;

    // Spawn a task to stream the response back to Telegram
    let http_clone = http.clone();
    let bot_token = ctx.channel.bot_token.clone();
    let chat_id = msg.chat.id;
    let reply_to = msg.message_id;
    let mission_id = target_mission_id;

    tokio::spawn(async move {
        if let Err(e) = stream_response(
            events_rx,
            &http_clone,
            &bot_token,
            chat_id,
            reply_to,
            mission_id,
        )
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

    // Check for @bot_username mentions in both text entities and caption entities
    let mention_target = format!("@{}", bot_username);
    let has_mention_in = |entities: &Option<Vec<MessageEntity>>, text: &Option<String>| -> bool {
        entities
            .as_ref()
            .map(|ents| {
                ents.iter().any(|e| {
                    if e.entity_type == "mention" {
                        if let Some(ref t) = text {
                            let utf16_units: Vec<u16> = t.encode_utf16().collect();
                            let start = e.offset as usize;
                            let end = (e.offset + e.length) as usize;
                            if end <= utf16_units.len() {
                                if let Ok(mention) = String::from_utf16(&utf16_units[start..end]) {
                                    return mention.eq_ignore_ascii_case(&mention_target);
                                }
                            }
                            false
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                })
            })
            .unwrap_or(false)
    };
    let is_mention = has_mention_in(&msg.entities, &msg.text)
        || has_mention_in(&msg.caption_entities, &msg.caption);
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
        TelegramTriggerMode::MentionOrDm => is_private || is_mention || is_reply,
        TelegramTriggerMode::BotMention => is_mention,
        TelegramTriggerMode::Reply => is_reply,
        TelegramTriggerMode::DirectMessage => is_private,
        TelegramTriggerMode::Always => true,
    }
}

/// Strip @bot_username from the beginning of a message (case-insensitive).
fn strip_bot_mention(text: &str, bot_username: &str) -> String {
    let mention = format!("@{}", bot_username);
    let trimmed = text.trim();
    // Use char-aware comparison to avoid panics on non-ASCII usernames
    if let Some(rest) = trimmed.get(..mention.len()) {
        if rest.eq_ignore_ascii_case(&mention) {
            return trimmed[mention.len()..].trim().to_string();
        }
    }
    trimmed.to_string()
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
pub async fn stream_response(
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
            if let Some(msg_id) = sent_message_id {
                if !accumulated_text.is_empty() {
                    let final_text = format!("{}...\n\n_(timed out)_", accumulated_text);
                    let display = truncate_for_telegram(&final_text);
                    let _ = edit_message(http, &base_url, chat_id, msg_id, &display.html).await;
                }
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

                        if let Some(msg_id) = sent_message_id {
                            if last_edit.elapsed() >= edit_interval {
                                // Throttled edit
                                let display = truncate_for_telegram(&accumulated_text);
                                if let Err(e) = edit_message(http, &base_url, chat_id, msg_id, &display.html).await {
                                    tracing::warn!(
                                        mission_id = %mission_id,
                                        "Failed to edit Telegram message during streaming: {}",
                                        e
                                    );
                                }
                                last_edit = tokio::time::Instant::now();
                            }
                        } else {
                            // Send initial message
                            let reply = if reply_to > 0 { Some(reply_to) } else { None };
                            match send_message(http, &base_url, chat_id, &accumulated_text, reply).await {
                                Ok(msg_id) => {
                                    sent_message_id = Some(msg_id);
                                    last_edit = tokio::time::Instant::now();
                                }
                                Err(e) => {
                                    tracing::warn!("Failed to send initial Telegram message: {}", e);
                                }
                            }
                        }
                    }
                    Ok(AgentEvent::AssistantMessage {
                        content,
                        mission_id: Some(mid),
                        shared_files,
                        ..
                    }) if mid == mission_id => {
                        // Final response — send or edit with complete text
                        if let Some(msg_id) = sent_message_id {
                            // Edit existing message with final content
                            let display = truncate_for_telegram(&content);
                            if let Err(e) = edit_message(http, &base_url, chat_id, msg_id, &display.html).await {
                                tracing::warn!(
                                    mission_id = %mission_id,
                                    "Failed to edit Telegram message with final response, sending as new message: {}",
                                    e
                                );
                                // Fallback: send as a new message if edit fails
                                let _ = send_chunked_message(http, &base_url, chat_id, &content, None).await;
                            }
                            // If content exceeds 4096 chars, send overflow as new messages
                            send_overflow_chunks(
                                http,
                                &base_url,
                                chat_id,
                                &content,
                                display.source_boundary,
                            )
                            .await;
                        } else {
                            // No streaming happened, send the full response directly
                            send_chunked_message(http, &base_url, chat_id, &content, Some(reply_to)).await?;
                        }

                        // Send shared files as Telegram documents/photos
                        if let Some(files) = shared_files {
                            for file in &files {
                                if let Err(e) = send_file_to_telegram(http, &base_url, chat_id, file).await {
                                    tracing::warn!("Failed to send file {} to Telegram: {}", file.name, e);
                                }
                            }
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
                            let display = truncate_for_telegram(&final_text);
                            let _ = edit_message(http, &base_url, chat_id, msg_id, &display.html).await;
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

/// Send a file to a Telegram chat via sendDocument or sendPhoto.
/// The file is read from the URL in SharedFile (which is a local file:// or http:// path).
async fn send_file_to_telegram(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    file: &crate::api::control::SharedFile,
) -> Result<(), String> {
    use reqwest::multipart;

    // Read the file from the URL (which could be a relative workspace path or absolute)
    let file_path = if file.url.starts_with("http://") || file.url.starts_with("https://") {
        // Download from URL first (cap at 50MB to prevent OOM)
        const MAX_DOWNLOAD: usize = 50 * 1024 * 1024;
        let resp = http
            .get(&file.url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch file from URL: {}", e))?;
        if !resp.status().is_success() {
            return Err(format!("File fetch HTTP error {}", resp.status(),));
        }
        if let Some(len) = resp.content_length() {
            if len as usize > MAX_DOWNLOAD {
                return Err(format!(
                    "File too large: {} bytes (max {})",
                    len, MAX_DOWNLOAD
                ));
            }
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read file bytes: {}", e))?;
        if bytes.len() > MAX_DOWNLOAD {
            return Err(format!(
                "File too large: {} bytes (max {})",
                bytes.len(),
                MAX_DOWNLOAD
            ));
        }
        // Sanitize filename to prevent path traversal
        let safe_name = file
            .name
            .replace(['/', '\\', '\0'], "_")
            .trim_start_matches('.')
            .to_string();
        let safe_name = if safe_name.is_empty() {
            "file".to_string()
        } else {
            safe_name
        };
        let tmp_path = std::path::PathBuf::from("/tmp/telegram-outbound").join(&safe_name);
        if let Some(parent) = tmp_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        tokio::fs::write(&tmp_path, &bytes)
            .await
            .map_err(|e| format!("Failed to write temp file: {}", e))?;
        tmp_path
    } else {
        // Local file path — must be under a workspace directory
        let path = std::path::PathBuf::from(&file.url);
        let canonical = path
            .canonicalize()
            .map_err(|e| format!("Failed to resolve file path: {}", e))?;
        let allowed_roots = ["/root/workspaces/", "/tmp/"];
        if !allowed_roots.iter().any(|r| canonical.starts_with(r)) {
            return Err(format!(
                "File path outside allowed directories: {}",
                canonical.display()
            ));
        }
        canonical
    };

    if !file_path.exists() {
        return Err(format!("File not found: {}", file_path.display()));
    }

    let file_bytes = tokio::fs::read(&file_path)
        .await
        .map_err(|e| format!("Failed to read file: {}", e))?;

    let is_image = file.content_type.starts_with("image/") && !file.content_type.contains("svg");

    let (endpoint, field_name) = if is_image {
        ("sendPhoto", "photo")
    } else {
        ("sendDocument", "document")
    };

    let url = format!("{}/{}", base_url, endpoint);
    let file_part = multipart::Part::bytes(file_bytes)
        .file_name(file.name.clone())
        .mime_str(&file.content_type)
        .map_err(|e| format!("Invalid MIME type: {}", e))?;

    let form = multipart::Form::new()
        .text("chat_id", chat_id.to_string())
        .part(field_name, file_part);

    let response = http
        .post(&url)
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("{} request failed: {}", endpoint, e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("{} API error {}: {}", endpoint, status, body));
    }

    tracing::info!("Sent file {} to Telegram chat {}", file.name, chat_id);
    Ok(())
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
        text: &display.html,
        reply_to_message_id: reply_to,
        parse_mode: Some("HTML"),
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
    html: &str,
) -> Result<(), String> {
    if html.is_empty() {
        return Ok(());
    }
    let body = EditMessageRequest {
        chat_id,
        message_id,
        text: html,
        parse_mode: Some("HTML"),
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

/// Find the byte index that includes at most `max_chars` characters.
fn char_boundary_at(text: &str, max_chars: usize) -> usize {
    text.char_indices()
        .nth(max_chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

/// Convert markdown to Telegram HTML for rich rendering.
/// Handles **bold**, *italic*, `code`, ```blocks```, # headers, [links](url).
#[allow(clippy::while_let_on_iterator)]
pub fn markdown_to_telegram_html(text: &str) -> String {
    // Escape HTML special chars first
    let escaped = text
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;");

    let mut result = String::with_capacity(escaped.len());
    let mut chars = escaped.chars().peekable();
    let mut at_line_start = true;

    while let Some(ch) = chars.next() {
        match ch {
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut content = String::new();
                while let Some(c) = chars.next() {
                    if c == '*' && chars.peek() == Some(&'*') {
                        chars.next();
                        break;
                    }
                    content.push(c);
                }
                result.push_str("<b>");
                result.push_str(&content);
                result.push_str("</b>");
                at_line_start = false;
            }
            '*' => {
                let mut content = String::new();
                while let Some(c) = chars.next() {
                    if c == '*' {
                        break;
                    }
                    content.push(c);
                }
                if content.is_empty() {
                    result.push('*');
                } else {
                    result.push_str("<i>");
                    result.push_str(&content);
                    result.push_str("</i>");
                }
                at_line_start = false;
            }
            '`' if chars.peek() == Some(&'`') => {
                chars.next();
                if chars.peek() == Some(&'`') {
                    chars.next();
                    // Skip language tag
                    while chars.peek().map(|c| *c != '\n').unwrap_or(false) {
                        chars.next();
                    }
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    let mut code = String::new();
                    while let Some(c) = chars.next() {
                        if c == '`' && chars.peek() == Some(&'`') {
                            chars.next();
                            if chars.peek() == Some(&'`') {
                                chars.next();
                            }
                            break;
                        }
                        code.push(c);
                    }
                    result.push_str("<pre>");
                    result.push_str(code.trim_end());
                    result.push_str("</pre>");
                } else {
                    let mut code = String::new();
                    while let Some(c) = chars.next() {
                        if c == '`' && chars.peek() == Some(&'`') {
                            chars.next();
                            break;
                        }
                        code.push(c);
                    }
                    result.push_str("<code>");
                    result.push_str(&code);
                    result.push_str("</code>");
                }
                at_line_start = false;
            }
            '`' => {
                let mut code = String::new();
                while let Some(c) = chars.next() {
                    if c == '`' {
                        break;
                    }
                    code.push(c);
                }
                result.push_str("<code>");
                result.push_str(&code);
                result.push_str("</code>");
                at_line_start = false;
            }
            '#' if at_line_start => {
                while chars.peek() == Some(&'#') {
                    chars.next();
                }
                if chars.peek() == Some(&' ') {
                    chars.next();
                }
                let mut header = String::new();
                while chars.peek().map(|c| *c != '\n').unwrap_or(false) {
                    header.push(chars.next().unwrap());
                }
                result.push_str("<b>");
                result.push_str(&header);
                result.push_str("</b>");
                at_line_start = false;
            }
            '[' => {
                let mut link_text = String::new();
                let mut found_link = false;
                while let Some(c) = chars.next() {
                    if c == ']' {
                        if chars.peek() == Some(&'(') {
                            chars.next();
                            let mut url = String::new();
                            while let Some(c) = chars.next() {
                                if c == ')' {
                                    break;
                                }
                                url.push(c);
                            }
                            result.push_str("<a href=\"");
                            result.push_str(&url);
                            result.push_str("\">");
                            result.push_str(&link_text);
                            result.push_str("</a>");
                            found_link = true;
                        }
                        break;
                    }
                    link_text.push(c);
                }
                if !found_link {
                    result.push('[');
                    result.push_str(&link_text);
                    result.push(']');
                }
                at_line_start = false;
            }
            '\n' => {
                result.push('\n');
                at_line_start = true;
            }
            _ => {
                result.push(ch);
                at_line_start = false;
            }
        }
    }
    result
}

struct TelegramRenderChunk {
    html: String,
    source_boundary: usize,
}

fn render_telegram_chunk(
    text: &str,
    max_chars: usize,
    truncated_suffix: Option<&str>,
) -> TelegramRenderChunk {
    let html = markdown_to_telegram_html(text);
    if html.chars().count() <= max_chars {
        return TelegramRenderChunk {
            html,
            source_boundary: text.len(),
        };
    }

    let suffix = truncated_suffix.unwrap_or("");
    let suffix_chars = suffix.chars().count();
    let available_chars = max_chars.saturating_sub(suffix_chars);
    let total_chars = text.chars().count();
    let mut low = 0usize;
    let mut high = total_chars;
    let mut best_chars = 0usize;

    while low <= high {
        let mid = (low + high) / 2;
        let boundary = char_boundary_at(text, mid);
        let candidate = markdown_to_telegram_html(&text[..boundary]);

        if candidate.chars().count() <= available_chars {
            best_chars = mid;
            low = mid.saturating_add(1);
        } else if mid == 0 {
            break;
        } else {
            high = mid - 1;
        }
    }

    let source_boundary = char_boundary_at(text, best_chars);
    let mut html = markdown_to_telegram_html(&text[..source_boundary]);
    html.push_str(suffix);

    TelegramRenderChunk {
        html,
        source_boundary,
    }
}

fn truncate_for_telegram(text: &str) -> TelegramRenderChunk {
    render_telegram_chunk(text, 4096, Some("..."))
}

/// Send overflow chunks (content beyond 4096 chars) as separate messages.
async fn send_overflow_chunks(
    http: &Client,
    base_url: &str,
    chat_id: i64,
    text: &str,
    source_boundary: usize,
) {
    if source_boundary >= text.len() {
        return;
    }
    let rest = &text[source_boundary..];
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
    let mut remaining = text;
    let mut first = true;
    while !remaining.is_empty() {
        let rendered = render_telegram_chunk(remaining, 4096, None);
        let reply = if first { reply_to } else { None };
        first = false;
        send_message(
            http,
            base_url,
            chat_id,
            &remaining[..rendered.source_boundary],
            reply,
        )
        .await?;
        remaining = &remaining[rendered.source_boundary..];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{markdown_to_telegram_html, render_telegram_chunk, truncate_for_telegram};

    #[test]
    fn truncate_for_telegram_preserves_valid_html_boundaries() {
        let text = "**bold** ".repeat(700);
        let rendered = truncate_for_telegram(&text);

        assert!(rendered.html.chars().count() <= 4096);
        assert!(rendered.source_boundary < text.len());
        assert!(
            rendered.html.ends_with("...</b>...")
                || rendered.html.ends_with("</b>...")
                || rendered.html.ends_with("...")
        );
        assert!(!rendered.html.contains("&lt;b&gt;"));
    }

    #[test]
    fn render_chunk_tracks_consumed_source_before_html_limit() {
        let text = "[label](https://example.com) ".repeat(300);
        let rendered = render_telegram_chunk(&text, 4096, None);
        let full_html = markdown_to_telegram_html(&text);

        assert!(rendered.html.chars().count() <= 4096);
        assert!(rendered.source_boundary < text.len());
        assert!(full_html.chars().count() > 4096);
        assert_eq!(
            rendered.html,
            markdown_to_telegram_html(&text[..rendered.source_boundary])
        );
    }
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
