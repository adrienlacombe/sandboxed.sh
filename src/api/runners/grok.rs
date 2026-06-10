//! Grok Build CLI turn runner.
//!
//! Moved verbatim from `mission_runner.rs` (Phase 2 of the decomposition).

use std::collections::HashMap;

use uuid::Uuid;

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::agents::{AgentResult, CompletionConfidence, CompletionSignal, TerminalReason};
use crate::api::control::AgentEvent;
use crate::api::mission_runner::*;
use crate::cost::resolve_cost_cents_and_source;
use crate::util::env_var_bool;
use crate::workspace::Workspace;
use crate::workspace_exec::WorkspaceExec;

async fn ensure_grok_cli_available(
    workspace_exec: &WorkspaceExec,
    cwd: &std::path::Path,
    cli_path: &str,
) -> Result<String, String> {
    let program = cli_path.split(' ').next().unwrap_or(cli_path);
    if command_available(workspace_exec, cwd, program).await {
        return Ok(cli_path.to_string());
    }

    let auto_install = env_var_bool("SANDBOXED_SH_AUTO_INSTALL_GROK", true);
    if !auto_install {
        return Err(format!(
            "Grok Build CLI '{}' not found in workspace. Install it with: curl -fsSL https://x.ai/cli/install.sh | bash",
            cli_path
        ));
    }

    if !command_available(workspace_exec, cwd, "curl").await {
        return Err(format!(
            "Grok Build CLI '{}' not found and curl is not available in the workspace. Install curl or install Grok manually.",
            cli_path
        ));
    }

    tracing::info!("Auto-installing Grok Build CLI");
    let output = workspace_exec
        .output(
            cwd,
            "/bin/sh",
            &[
                "-lc".to_string(),
                "curl -fsSL https://x.ai/cli/install.sh | GROK_BIN_DIR=/usr/local/bin bash 2>&1"
                    .to_string(),
            ],
            HashMap::new(),
        )
        .await
        .map_err(|e| format!("Failed to run Grok Build installer: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut message = String::new();
        if !stderr.trim().is_empty() {
            message.push_str(stderr.trim());
        }
        if !stdout.trim().is_empty() {
            if !message.is_empty() {
                message.push_str(" | ");
            }
            message.push_str(stdout.trim());
        }
        if message.is_empty() {
            message = "Grok Build install failed with no output".to_string();
        }
        return Err(format!("Grok Build install failed: {}", message));
    }

    if command_available(workspace_exec, cwd, cli_path).await {
        Ok(cli_path.to_string())
    } else if command_available(workspace_exec, cwd, "/usr/local/bin/grok").await {
        Ok("/usr/local/bin/grok".to_string())
    } else {
        Err(
            "Grok Build install completed but 'grok' is still not available in workspace PATH."
                .to_string(),
        )
    }
}

async fn sync_grok_oauth_auth_file(
    workspace_exec: &WorkspaceExec,
    cwd: &std::path::Path,
) -> Result<bool, String> {
    let auth_path = std::path::PathBuf::from(crate::util::home_dir())
        .join(".grok")
        .join("auth.json");
    if !auth_path.is_file() {
        return Ok(false);
    }

    let auth_json = tokio::fs::read_to_string(&auth_path)
        .await
        .map_err(|e| format!("Failed to read Grok auth file: {}", e))?;
    if auth_json.trim().is_empty() {
        return Ok(false);
    }

    let source_expires_at = grok_auth_file_expires_at(&auth_json);
    if crate::api::ai_providers::oauth_token_expired(source_expires_at) {
        return Err(
            "Host Grok auth file is expired; reconnect xAI or refresh OAuth before syncing"
                .to_string(),
        );
    }
    let existing_output = workspace_exec
        .output(
            cwd,
            "/bin/sh",
            &[
                "-lc".to_string(),
                "test -s \"${HOME:-/root}/.grok/auth.json\" && cat \"${HOME:-/root}/.grok/auth.json\""
                    .to_string(),
            ],
            HashMap::new(),
        )
        .await
        .map_err(|e| format!("Failed to inspect workspace Grok auth file: {}", e))?;
    if existing_output.status.success() {
        let existing_json = String::from_utf8_lossy(&existing_output.stdout);
        let existing_expires_at = grok_auth_file_expires_at(&existing_json);
        if existing_expires_at >= source_expires_at {
            tracing::debug!(
                source_expires_at,
                existing_expires_at,
                "Skipping Grok auth sync because workspace auth is at least as fresh"
            );
            return Ok(false);
        }
    }

    let encoded = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(auth_json.as_bytes())
    };
    let output = workspace_exec
        .output(
            cwd,
            "/bin/sh",
            &[
                "-lc".to_string(),
                format!(
                    "mkdir -p \"${{HOME:-/root}}/.grok\" && printf %s '{}' | base64 -d > \"${{HOME:-/root}}/.grok/auth.json\" && chmod 600 \"${{HOME:-/root}}/.grok/auth.json\"",
                    encoded
                ),
            ],
            HashMap::new(),
        )
        .await
        .map_err(|e| format!("Failed to sync Grok auth file: {}", e))?;
    if output.status.success() {
        Ok(true)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        Err(format!(
            "Failed to sync Grok auth file into workspace: {}{}{}",
            stderr.trim(),
            if stderr.trim().is_empty() || stdout.trim().is_empty() {
                ""
            } else {
                " | "
            },
            stdout.trim()
        ))
    }
}

fn grok_auth_file_expires_at(contents: &str) -> i64 {
    const GROK_OAUTH_CLIENT_KEY: &str = "https://auth.x.ai::b1a00492-073a-47ea-816f-4c329264a828";

    serde_json::from_str::<serde_json::Value>(contents)
        .ok()
        .and_then(|auth| auth.get(GROK_OAUTH_CLIENT_KEY).cloned())
        .and_then(|entry| {
            entry.get("expires_at").and_then(|value| {
                if let Some(expires_at) = value.as_i64() {
                    return Some(expires_at);
                }
                let text = value.as_str()?.trim();
                if let Ok(expires_at) = text.parse::<i64>() {
                    return Some(expires_at);
                }
                chrono::DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|dt| dt.timestamp_millis())
            })
        })
        .unwrap_or(0)
}

fn grok_event_is_reasoning_type(value: &serde_json::Value) -> bool {
    value.get("type").and_then(|v| v.as_str()).is_some_and(|t| {
        let lower = t.to_ascii_lowercase();
        lower == "reasoning" || lower == "thinking" || lower == "reasoning_delta"
    })
}

pub(crate) fn grok_event_text(value: &serde_json::Value) -> Option<String> {
    if grok_event_is_reasoning_type(value) {
        return None;
    }

    if let Some(text) = value
        .get("delta")
        .and_then(|delta| delta.get("text").or_else(|| delta.get("content")))
        .and_then(|v| v.as_str())
    {
        return Some(text.to_string());
    }

    if value
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.eq_ignore_ascii_case("text"))
    {
        if let Some(text) = value.get("data").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }

    if let Some(content) = value.get("content") {
        if let Some(text) = content.as_str() {
            return Some(text.to_string());
        }
        if let Some(text) = content.get("text").and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }

    if let Some(text) = value.get("message").and_then(|message| {
        message.as_str().map(str::to_string).or_else(|| {
            message.get("content").and_then(|content| {
                content.as_str().map(str::to_string).or_else(|| {
                    content.as_array().map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|block| block.get("text").and_then(|v| v.as_str()))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                })
            })
        })
    }) {
        if !text.is_empty() {
            return Some(text);
        }
    }

    for key in ["text", "answer", "result", "output"] {
        if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
            return Some(text.to_string());
        }
    }

    None
}

/// Extract Grok / xAI reasoning text from a streamed JSONL event.
///
/// The Grok Build CLI mostly mirrors the xAI Chat Completions stream, which
/// puts chain-of-thought in `delta.reasoning_content` (some builds) or
/// `delta.reasoning` (others), and sometimes wraps it as a typed event
/// (`type: "reasoning" | "thinking"` with `data` or `text`). Field name
/// discovery is conservative — return None if no known key is present so a
/// CLI version bump doesn't accidentally show user-visible noise as
/// reasoning.
pub(crate) fn grok_event_reasoning(value: &serde_json::Value) -> Option<String> {
    let is_reasoning_type = grok_event_is_reasoning_type(value);

    if let Some(delta) = value.get("delta") {
        for key in ["reasoning_content", "reasoning", "thinking"] {
            if let Some(text) = delta.get(key).and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
        if is_reasoning_type {
            for key in ["text", "content"] {
                if let Some(text) = delta.get(key).and_then(|v| v.as_str()) {
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
        }
    }

    if is_reasoning_type {
        for key in ["data", "text", "content", "reasoning"] {
            if let Some(text) = value.get(key).and_then(|v| v.as_str()) {
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
        }
    }

    if let Some(text) = value
        .get("message")
        .and_then(|m| m.get("reasoning_content").or_else(|| m.get("reasoning")))
        .and_then(|v| v.as_str())
    {
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }

    None
}

fn grok_event_session_id(value: &serde_json::Value) -> Option<String> {
    value
        .get("session_id")
        .or_else(|| value.get("sessionId"))
        .or_else(|| value.get("session").and_then(|session| session.get("id")))
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

fn grok_event_model(value: &serde_json::Value) -> Option<String> {
    value
        .get("model")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("model"))
        })
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}
pub(crate) fn grok_event_usage(value: &serde_json::Value) -> Option<crate::cost::TokenUsage> {
    let usage = value
        .get("usage")
        .or_else(|| value.get("tokenUsage"))
        .or_else(|| value.get("token_usage"))
        .or_else(|| value.get("response").and_then(|r| r.get("usage")))
        .or_else(|| value.get("message").and_then(|m| m.get("usage")))?;

    let raw_input_tokens = usage_value_tokens(
        usage,
        &[
            "input_tokens",
            "inputTokens",
            "prompt_tokens",
            "promptTokens",
        ],
    );
    let output_tokens = usage_value_tokens(
        usage,
        &[
            "output_tokens",
            "outputTokens",
            "completion_tokens",
            "completionTokens",
        ],
    );
    let cache_creation_tokens = usage_value_tokens(
        usage,
        &[
            "cache_creation_input_tokens",
            "cacheCreationInputTokens",
            "cache_write_input_tokens",
            "cacheWriteInputTokens",
        ],
    );
    let explicit_cache_read_tokens = usage_value_tokens(
        usage,
        &[
            "cache_read_input_tokens",
            "cacheReadInputTokens",
            "cached_tokens",
            "cachedTokens",
        ],
    );
    let nested_cached_tokens =
        nested_usage_value_tokens(usage, &["input_tokens_details", "cached_tokens"])
            .saturating_add(nested_usage_value_tokens(
                usage,
                &["prompt_tokens_details", "cached_tokens"],
            ));
    let cache_read_tokens = explicit_cache_read_tokens.saturating_add(nested_cached_tokens);
    // xAI/OpenAI-compatible usage reports usually include cached prompt
    // tokens inside the prompt/input total. Internally we store billable
    // non-cached input separately from discounted cache-read input, so the
    // two buckets can be summed for display without double counting and
    // priced at their respective rates.
    let input_tokens = raw_input_tokens.saturating_sub(cache_read_tokens);
    let token_usage = crate::cost::TokenUsage {
        input_tokens,
        output_tokens,
        cache_creation_input_tokens: Some(cache_creation_tokens),
        cache_read_input_tokens: Some(cache_read_tokens),
    };
    token_usage.has_usage().then_some(token_usage)
}

fn grok_event_is_error(value: &serde_json::Value) -> bool {
    value
        .get("type")
        .and_then(|v| v.as_str())
        .is_some_and(|t| t.eq_ignore_ascii_case("error"))
        || value.get("error").is_some()
}

/// Detect the Grok CLI's interactive sign-in prompt. The CLI prints these to
/// stderr when it can't authenticate non-interactively, then blocks on a local
/// OAuth callback that never arrives in a headless mission. Matching any of
/// these lets the runner fail fast instead of hanging.
fn grok_line_requests_interactive_login(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("signing in with grok")
        || lower.contains("open this url to sign in")
        || lower.contains("oauth2/authorize")
}

pub(crate) fn grok_stdout_line_requests_interactive_login(line: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(line).is_err()
        && grok_line_requests_interactive_login(line)
}

/// Execute a turn using the Grok Build CLI backend.
#[allow(clippy::too_many_arguments)]
pub async fn run_grok_turn(
    workspace: &Workspace,
    work_dir: &std::path::Path,
    message: &str,
    model: Option<&str>,
    mission_id: Uuid,
    events_tx: broadcast::Sender<AgentEvent>,
    cancel: CancellationToken,
    app_working_dir: &std::path::Path,
    session_id: Option<&str>,
    is_continuation: bool,
) -> AgentResult {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let workspace_exec = WorkspaceExec::new(workspace.clone());

    if workspace.id == crate::workspace::DEFAULT_WORKSPACE_ID && !work_dir.join(".git").exists() {
        let file_count = std::fs::read_dir(work_dir)
            .map(|mut d| {
                d.by_ref()
                    .filter(|e| {
                        e.as_ref()
                            .map(|e| {
                                let n = e.file_name();
                                let n = n.to_string_lossy();
                                !n.starts_with('.') && n != "output"
                            })
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0);
        if file_count == 0 && !is_continuation {
            let dir_display = work_dir.display();
            tracing::warn!(
                mission_id = %mission_id,
                work_dir = %dir_display,
                "Grok mission running in empty host workspace with no git repo — goal loop will hallucinate edits"
            );
            let msg = format!(
                "The mission workspace ({dir_display}) is empty and has no git repository. \
                 Grok cannot edit files or push changes without a project checkout. \
                 Create this mission on a workspace that contains the target repository, \
                 or clone the repo into the workspace first.",
            );
            // Return a failure result so the control loop emits a single
            // `AssistantMessage { success: false }` and marks the mission
            // `Failed` (Bugbot f4a7a2d8). Emitting a manual AssistantMessage
            // and then returning success:true caused the control loop to
            // emit a SECOND assistant message with success:true and record
            // automations as successful, despite the workspace being
            // unusable. LlmError is the right terminal reason: this is a
            // "can't run" error, not a clean turn boundary.
            return AgentResult::failure(msg, 0).with_terminal_reason(TerminalReason::LlmError);
        }
    }

    let cli_path =
        get_backend_string_setting("grok", "cli_path").unwrap_or_else(|| "grok".to_string());
    let cli_path = match ensure_grok_cli_available(&workspace_exec, work_dir, &cli_path).await {
        Ok(cli_path) => cli_path,
        Err(err_msg) => {
            return AgentResult::failure(err_msg, 0).with_terminal_reason(TerminalReason::LlmError);
        }
    };

    let mut args = Vec::new();
    // Use `-s/--session-id` for both first-turn and continuation when we
    // already have a session id from the mission store. Per grok headless
    // docs, `--session-id` has upsert semantics — loads the session if it
    // exists, creates one with that id otherwise — so it self-heals the
    // "orphan session" case where the first turn failed before grok could
    // persist the session and `--resume <sid>` would error with "Session
    // does not exist". `--resume` is strict-existence-only; we only fall
    // through to `--continue` when we have no session id at all.
    if let Some(sid) = session_id {
        args.push("--session-id".to_string());
        args.push(sid.to_string());
    } else if is_continuation {
        args.push("--continue".to_string());
    }
    args.push("-p".to_string());
    args.push(message.to_string());
    args.push("--output-format".to_string());
    args.push("streaming-json".to_string());
    args.push("--always-approve".to_string());
    args.push("--cwd".to_string());
    args.push(workspace_exec.translate_path_for_container(work_dir));
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.push("--model".to_string());
        args.push(model.to_string());
    }

    // The Grok CLI authenticates non-interactively via the XAI_API_KEY env var.
    // The xAI OAuth access token works as a bearer key against api.x.ai, so we
    // capture the freshest one here and inject it below. Without it the CLI
    // falls back to an interactive browser sign-in that never completes in a
    // headless mission — the run then hangs forever ("Agent is working").
    let mut oauth_access_token: Option<String> = None;
    if let Some(entry) =
        crate::api::ai_providers::read_oauth_token_entry(crate::ai_providers::ProviderType::Xai)
    {
        if crate::api::ai_providers::oauth_token_expired(entry.expires_at) {
            match crate::api::ai_providers::refresh_oauth_token_with_lock(
                crate::ai_providers::ProviderType::Xai,
                entry.expires_at,
            )
            .await
            {
                Ok((access, _refresh, expires_at)) => {
                    oauth_access_token = Some(access);
                    tracing::info!(
                        mission_id = %mission_id,
                        expires_at,
                        "Refreshed xAI OAuth token before starting Grok Build"
                    );
                }
                Err(crate::api::ai_providers::OAuthRefreshError::InvalidGrant(err)) => {
                    return AgentResult::failure(
                        format!(
                            "Grok Build xAI OAuth refresh token is expired or revoked. Reconnect the xAI provider, then retry the mission. {}",
                            err
                        ),
                        0,
                    )
                    .with_terminal_reason(TerminalReason::LlmError);
                }
                Err(err) => {
                    return AgentResult::failure(
                        format!(
                            "Failed to refresh xAI OAuth before starting Grok Build: {}",
                            err
                        ),
                        0,
                    )
                    .with_terminal_reason(TerminalReason::LlmError);
                }
            }
        } else {
            oauth_access_token = Some(entry.access_token.clone());
            if let Err(err) = crate::api::ai_providers::write_grok_oauth_auth_file(
                &entry.refresh_token,
                &entry.access_token,
                entry.expires_at,
            ) {
                tracing::warn!(
                    mission_id = %mission_id,
                    error = %err,
                    "Failed to materialize fresh xAI OAuth token into Grok auth file"
                );
            }
        }
    }

    if let Err(err) = sync_grok_oauth_auth_file(&workspace_exec, work_dir).await {
        tracing::warn!(mission_id = %mission_id, error = %err, "Failed to sync Grok OAuth auth file");
    }

    // Authenticate the Grok CLI non-interactively via XAI_API_KEY. Priority:
    // an explicit xAI API key, then the captured OAuth access token, then any
    // ambient env key. Setting this is what prevents the interactive-sign-in
    // hang; the CLI prints "You are using XAI_API_KEY" and goes straight to
    // api.x.ai.
    let mut env = HashMap::new();
    let xai_api_key = crate::api::ai_providers::get_xai_api_key_for_grok(app_working_dir)
        .or_else(|| oauth_access_token.clone())
        .or_else(|| {
            std::env::var("XAI_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty())
        })
        .or_else(|| {
            std::env::var("GROK_CODE_XAI_API_KEY")
                .ok()
                .filter(|k| !k.trim().is_empty())
        });
    if let Some(key) = xai_api_key {
        // Newer Grok CLIs read XAI_API_KEY; keep GROK_CODE_XAI_API_KEY for
        // backward compatibility with older builds.
        env.insert("XAI_API_KEY".to_string(), key.clone());
        env.insert("GROK_CODE_XAI_API_KEY".to_string(), key);
    }

    let mut child = match workspace_exec
        .spawn_streaming(work_dir, &cli_path, &args, env)
        .await
    {
        Ok(child) => child,
        Err(e) => {
            return AgentResult::failure(format!("Failed to start Grok Build CLI: {}", e), 0)
                .with_terminal_reason(TerminalReason::LlmError);
        }
    };
    drop(child.stdin.take());

    let stdout = match child.stdout.take() {
        Some(stdout) => stdout,
        None => {
            return AgentResult::failure("Failed to capture Grok stdout".to_string(), 0)
                .with_terminal_reason(TerminalReason::LlmError);
        }
    };
    let stderr = child.stderr.take();
    let stderr_capture = std::sync::Arc::new(tokio::sync::Mutex::new(String::new()));
    let stderr_capture_clone = stderr_capture.clone();
    // The Grok CLI prints its interactive sign-in prompt to STDERR, then blocks
    // on a local OAuth callback. Watch for it here and signal the main loop to
    // abort so the mission fails fast instead of hanging forever.
    let auth_fail = CancellationToken::new();
    let auth_fail_signal = auth_fail.clone();
    let mut stderr_handle = stderr.map(|stderr| {
        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if grok_line_requests_interactive_login(trimmed) {
                    auth_fail_signal.cancel();
                }
                let mut captured = stderr_capture_clone.lock().await;
                if !captured.is_empty() {
                    captured.push('\n');
                }
                captured.push_str(trimmed);
            }
        })
    });

    let mut final_result = String::new();
    let mut had_error = false;
    let mut model_used = model.map(str::to_string);
    let mut last_streamed_len = 0usize;
    let mut text_delta_coalescer = TextDeltaCoalescer::new();
    let mut token_usage = crate::cost::TokenUsage::default();
    // Accumulate Grok's reasoning deltas into a cumulative buffer and
    // throttle Thinking emissions the same way text deltas are throttled.
    // Grok's CLI delivers reasoning as incremental tokens, mirroring the
    // text path.
    let mut reasoning_buffer = String::new();
    let mut last_reasoning_len = 0usize;
    let mut reasoning_delta_coalescer = TextDeltaCoalescer::new();
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    let mut cancelled = false;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                if let Some(handle) = stderr_handle.take() {
                    handle.abort();
                }
                cancelled = true;
                break;
            }
            _ = auth_fail.cancelled() => {
                // Grok CLI emitted an interactive sign-in prompt (it can't
                // authenticate non-interactively). Kill it and fail fast.
                let _ = child.kill().await;
                if let Some(handle) = stderr_handle.take() {
                    handle.abort();
                }
                return AgentResult::failure(
                    "Grok Build could not authenticate non-interactively (the CLI requested a browser sign-in). Reconnect the xAI / Grok Build provider in Settings → Providers, then retry the mission.".to_string(),
                    0,
                )
                .with_terminal_reason(TerminalReason::LlmError);
            }
            line_result = lines.next_line() => {
                match line_result {
                    Ok(Some(line)) => {
                        if line.trim().is_empty() {
                            continue;
                        }
                        let value: serde_json::Value = match serde_json::from_str(&line) {
                            Ok(value) => value,
                            Err(_) => {
                                // Fail fast on raw interactive sign-in prompts.
                                // Valid streaming-json events may contain these
                                // substrings as assistant/tool text, so only
                                // inspect stdout after JSON parsing fails.
                                if grok_stdout_line_requests_interactive_login(&line) {
                                    let _ = child.kill().await;
                                    if let Some(handle) = stderr_handle.take() {
                                        handle.abort();
                                    }
                                    return AgentResult::failure(
                                        "Grok Build could not authenticate non-interactively (the CLI requested a browser sign-in). Reconnect the xAI / Grok Build provider in Settings → Providers, then retry the mission.".to_string(),
                                        0,
                                    )
                                    .with_terminal_reason(TerminalReason::LlmError);
                                }
                                if final_result.is_empty() {
                                    final_result.push_str(&line);
                                } else {
                                    final_result.push('\n');
                                    final_result.push_str(&line);
                                }
                                continue;
                            }
                        };
                        if let Some(sid) = grok_event_session_id(&value) {
                            let _ = events_tx.send(AgentEvent::SessionIdUpdate {
                                session_id: sid,
                                mission_id,
                            });
                        }
                        if model_used.is_none() {
                            model_used = grok_event_model(&value);
                        }
                        if let Some(usage) = grok_event_usage(&value) {
                            token_usage.input_tokens =
                                token_usage.input_tokens.max(usage.input_tokens);
                            token_usage.output_tokens =
                                token_usage.output_tokens.max(usage.output_tokens);
                            token_usage.cache_creation_input_tokens = Some(
                                token_usage
                                    .cache_creation_input_tokens
                                    .unwrap_or(0)
                                    .max(usage.cache_creation_input_tokens.unwrap_or(0)),
                            );
                            token_usage.cache_read_input_tokens = Some(
                                token_usage
                                    .cache_read_input_tokens
                                    .unwrap_or(0)
                                    .max(usage.cache_read_input_tokens.unwrap_or(0)),
                            );
                        }
                        if grok_event_is_error(&value) {
                            had_error = true;
                            if let Some(text) = grok_event_text(&value) {
                                final_result = text;
                            } else {
                                final_result = value.to_string();
                            }
                            continue;
                        }
                        if let Some(reasoning) = grok_event_reasoning(&value) {
                            if !reasoning.is_empty() {
                                merge_stream_fragment(&mut reasoning_buffer, &reasoning);
                                // Mirror the TextDelta coalescing strategy:
                                // emit cumulative snapshots throttled to ~50ms.
                                if reasoning_buffer.len() > last_reasoning_len
                                    && reasoning_delta_coalescer.should_emit()
                                {
                                    last_reasoning_len = reasoning_buffer.len();
                                    let _ = events_tx.send(AgentEvent::Thinking {
                                        content: reasoning_buffer.clone(),
                                        done: false,
                                        mission_id: Some(mission_id),
                                    });
                                }
                            }
                        }
                        if let Some(text) = grok_event_text(&value) {
                            if !text.is_empty() {
                                // The first non-reasoning content marks the
                                // boundary between thinking and answer; flush
                                // a final Thinking { done: true } so the
                                // dashboard collapses the reasoning panel
                                // before streaming text deltas.
                                if !reasoning_buffer.is_empty() {
                                    let _ = events_tx.send(thinking_final_event(
                                        std::mem::take(&mut reasoning_buffer),
                                        mission_id,
                                    ));
                                    last_reasoning_len = 0;
                                }
                                if value
                                    .get("delta")
                                    .is_some()
                                    || value.get("type").and_then(|v| v.as_str()).is_some_and(|t| {
                                    t.contains("delta") || t.contains("chunk") || t == "text"
                                    })
                                {
                                    merge_stream_fragment(&mut final_result, &text);
                                } else {
                                    final_result = text;
                                }
                                // P3-#21: rate-limit TextDelta emissions
                                // to at most one per ~50ms per turn. Grok
                                // bursts can hit ~100 tokens/sec; without
                                // this every token becomes its own SSE
                                // frame even though the dashboard rAF
                                // coalesces them into a single render.
                                // The cumulative-buffer semantics mean
                                // skipping intermediate frames loses no
                                // content — each emit replaces the prior.
                                if final_result.len() > last_streamed_len
                                    && text_delta_coalescer.should_emit()
                                {
                                    last_streamed_len = final_result.len();
                                    let _ = events_tx.send(AgentEvent::TextDelta {
                                        content: final_result.clone(),
                                        mission_id: Some(mission_id),
                                    });
                                }
                            }
                        }
                    }
                    Ok(None) => break,
                    Err(e) => {
                        had_error = true;
                        final_result = format!("Error reading Grok stdout: {}", e);
                        break;
                    }
                }
            }
        }
    }

    let exit_status = child.wait().await;
    if let Some(handle) = stderr_handle {
        let _ = handle.await;
    }

    // P3-#21 final flush: the coalescer may have dropped the very last
    // delta within the trailing 50ms window. Always emit one more
    // TextDelta carrying the full buffer so the dashboard sees the
    // closing tokens; the AssistantMessage that follows will replace it.
    if final_result.len() > last_streamed_len {
        let _ = events_tx.send(AgentEvent::TextDelta {
            content: final_result.clone(),
            mission_id: Some(mission_id),
        });
        last_streamed_len = final_result.len();
    }
    let _ = last_streamed_len; // silence "unused after final assignment"

    let reasoning_for_fallback = if reasoning_buffer.trim().is_empty() {
        None
    } else {
        Some(reasoning_buffer.clone())
    };

    // Flush any remaining reasoning that never got followed by a text
    // delta (e.g., reasoning-only turns or the trailing coalescer window).
    // Emit done: true so the dashboard finalizes the thinking block in the
    // event store.
    if !reasoning_buffer.is_empty() {
        let _ = events_tx.send(thinking_final_event(
            std::mem::take(&mut reasoning_buffer),
            mission_id,
        ));
    }
    let _ = last_reasoning_len;

    let cancel_marker = if cancelled {
        Some(cancel_or_shutdown_failure())
    } else {
        None
    };

    if final_result.trim().is_empty() {
        let stderr_content = stderr_capture.lock().await;
        if let Some(reasoning) = reasoning_for_fallback {
            final_result = reasoning;
        } else if let Some(marker) = cancel_marker.as_ref() {
            final_result = marker.output.clone();
        } else if !stderr_content.trim().is_empty() {
            final_result = format!(
                "Grok Build error: {}",
                stderr_content
                    .lines()
                    .take(5)
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            had_error = true;
        } else {
            final_result = "Grok Build produced no output. Run `grok login` or configure an xAI provider for Grok Build.".to_string();
            had_error = true;
        }
    }

    let success = exit_status.map(|status| status.success()).unwrap_or(false) && !had_error;
    let model_for_cost = model_used.as_deref().or(Some("grok-build"));
    let (cost_cents, cost_source) =
        resolve_cost_cents_and_source(None, model_for_cost, &token_usage);
    let mut result = if success {
        AgentResult::success(final_result, cost_cents)
            .with_cost_source(cost_source)
            .with_terminal_reason(TerminalReason::TurnComplete)
    } else if let Some(marker) = cancel_marker {
        AgentResult::failure(final_result, cost_cents)
            .with_cost_source(cost_source)
            .with_terminal_reason(marker.terminal_reason.unwrap_or(TerminalReason::Cancelled))
    } else {
        AgentResult::failure(final_result, cost_cents)
            .with_cost_source(cost_source)
            .with_terminal_reason(TerminalReason::LlmError)
    };
    let success_signal = CompletionSignal::ProcessExit;
    let success_confidence = CompletionConfidence::Low;
    let outcome = turn_outcome_for_result(&result, success_signal, success_confidence);
    result = result.with_turn_outcome(outcome);
    if token_usage.has_usage() {
        result = result.with_usage(token_usage);
    }
    result = result.with_model(model_used.unwrap_or_else(|| "grok-build".to_string()));
    result
}
