//! MCP Server for orchestrating parallel agent missions.
//!
//! Provides boss agents with tools to create, monitor, and manage worker missions.
//! Communicates over stdio using JSON-RPC 2.0.

use std::io::{BufRead, BufReader, Write};
use std::process::Command;
use std::sync::Arc;

use chrono::Utc;
use jsonwebtoken::{EncodingKey, Header};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

// =============================================================================
// JSON-RPC Types (same pattern as automation-manager-mcp)
// =============================================================================

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(rename = "jsonrpc")]
    _jsonrpc: String,
    #[serde(default)]
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// =============================================================================
// MCP Types
// =============================================================================

#[derive(Debug, Serialize)]
struct ToolDefinition {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Serialize)]
struct ServerInfo {
    name: String,
    version: String,
}

// =============================================================================
// Tool Params
// =============================================================================

#[derive(Debug, Deserialize)]
struct CreateWorkerParams {
    title: String,
    #[serde(default)]
    agent: Option<String>,
    /// Backend to use: "claudecode", "codex", "gemini", "opencode", "amp"
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    model_override: Option<String>,
    #[serde(default)]
    model_effort: Option<String>,
    #[serde(default)]
    config_profile: Option<String>,
    #[serde(default)]
    working_directory: Option<String>,
    /// Initial prompt to send to the worker after creation.
    #[serde(default)]
    prompt: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BatchCreateWorkersParams {
    /// Array of worker definitions
    workers: Vec<CreateWorkerParams>,
}

#[derive(Debug, Deserialize)]
struct WaitForAnyWorkerParams {
    /// UUIDs of the worker missions to wait for
    mission_ids: Vec<String>,
    /// Target statuses to wait for (default: completed, failed, interrupted)
    #[serde(default)]
    target_statuses: Vec<String>,
    /// Maximum seconds to wait (default: 600 = 10 minutes)
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
    /// Poll interval in seconds (default: 10)
    #[serde(default = "default_poll_interval")]
    poll_interval_seconds: u64,
}

#[derive(Debug, Deserialize)]
struct GetWorkerStatusParams {
    mission_id: String,
}

#[derive(Debug, Deserialize)]
struct CancelWorkerParams {
    mission_id: String,
}

#[derive(Debug, Deserialize)]
struct SendMessageParams {
    mission_id: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct CreateWorktreeParams {
    /// Path relative to the workspace root where the worktree will be created
    path: String,
    /// Branch name (will be created if it doesn't exist)
    branch: String,
    /// Optional: base branch to create from (defaults to HEAD)
    #[serde(default)]
    base: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RemoveWorktreeParams {
    /// Path of the worktree to remove
    path: String,
}

#[derive(Debug, Deserialize)]
struct WaitForWorkerParams {
    /// UUID of the worker mission to wait for
    mission_id: String,
    /// Target statuses to wait for (default: completed, failed, interrupted)
    #[serde(default)]
    target_statuses: Vec<String>,
    /// Maximum seconds to wait (default: 600 = 10 minutes)
    #[serde(default = "default_timeout")]
    timeout_seconds: u64,
    /// Poll interval in seconds (default: 10)
    #[serde(default = "default_poll_interval")]
    poll_interval_seconds: u64,
}

fn default_timeout() -> u64 {
    600
}

fn default_poll_interval() -> u64 {
    10
}

// =============================================================================
// JWT helpers (lightweight – mirrors auth.rs Claims)
// =============================================================================

#[derive(Debug, Serialize)]
struct JwtClaims {
    sub: String,
    usr: String,
    iat: i64,
    exp: i64,
}

/// Mint a short-lived service JWT using the shared secret.
fn mint_service_jwt(secret: &str) -> Option<String> {
    let now = Utc::now();
    let exp = now + chrono::Duration::hours(24);
    let claims = JwtClaims {
        sub: "orchestrator-mcp".to_string(),
        usr: "orchestrator-mcp".to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .ok()
}

// =============================================================================
// Orchestrator MCP Server
// =============================================================================

struct OrchestratorMcp {
    mission_id: Uuid,
    api_url: String,
    api_token: Option<String>,
    client: reqwest::Client,
}

impl OrchestratorMcp {
    fn new(mission_id: Uuid, api_url: String, api_token: Option<String>) -> Self {
        Self {
            mission_id,
            api_url,
            api_token,
            client: reqwest::Client::new(),
        }
    }

    fn auth_header(&self) -> Option<(String, String)> {
        self.api_token
            .as_ref()
            .map(|t| ("Authorization".to_string(), format!("Bearer {}", t)))
    }

    fn get_tools() -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "create_worker_mission".to_string(),
                description: "Create a new worker mission (child of the current boss mission). The worker will start executing immediately. IMPORTANT: You must set the 'backend' field to match the harness you want (claudecode, codex, gemini, opencode). If omitted, defaults to the workspace default (usually claudecode).".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["title", "prompt"],
                    "properties": {
                        "title": {
                            "type": "string",
                            "description": "Descriptive title for the worker mission"
                        },
                        "backend": {
                            "type": "string",
                            "enum": ["claudecode", "codex", "gemini", "opencode"],
                            "description": "Backend/harness to use. MUST match the model: claudecode for Claude models, codex for OpenAI/GPT models, gemini for Gemini models, opencode for any model via provider routing."
                        },
                        "model_override": {
                            "type": "string",
                            "description": "Model to use. Must match the backend: Claude models (e.g. 'claude-sonnet-4-5-20250929') for claudecode, GPT models (e.g. 'gpt-5.4') for codex, Gemini models for gemini, 'provider/model' format for opencode."
                        },
                        "model_effort": {
                            "type": "string",
                            "enum": ["low", "medium", "high"],
                            "description": "Effort level. Supported by codex and claudecode backends."
                        },
                        "agent": {
                            "type": "string",
                            "description": "Agent name from library (optional, for opencode backend)."
                        },
                        "config_profile": {
                            "type": "string",
                            "description": "Config profile name to use for this worker"
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Working directory for the worker (e.g. a git worktree path). If omitted, uses the boss mission's repo directory."
                        },
                        "prompt": {
                            "type": "string",
                            "description": "Initial prompt/instructions to send to the worker after creation. Must be self-contained."
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "batch_create_workers".to_string(),
                description: "Create multiple worker missions at once. Each worker is created independently — if one fails, others still succeed. Use this to spawn many workers in parallel.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["workers"],
                    "properties": {
                        "workers": {
                            "type": "array",
                            "description": "Array of worker definitions. Each has the same schema as create_worker_mission.",
                            "items": {
                                "type": "object",
                                "required": ["title", "prompt"],
                                "properties": {
                                    "title": { "type": "string" },
                                    "backend": { "type": "string", "enum": ["claudecode", "codex", "gemini", "opencode"] },
                                    "model_override": { "type": "string" },
                                    "model_effort": { "type": "string", "enum": ["low", "medium", "high"] },
                                    "agent": { "type": "string" },
                                    "config_profile": { "type": "string" },
                                    "working_directory": { "type": "string" },
                                    "prompt": { "type": "string" }
                                }
                            }
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "list_worker_missions".to_string(),
                description: "List all worker missions spawned by this boss mission.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "get_worker_status".to_string(),
                description: "Get the current status and details of a specific worker mission.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["mission_id"],
                    "properties": {
                        "mission_id": {
                            "type": "string",
                            "description": "UUID of the worker mission"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "cancel_worker".to_string(),
                description: "Cancel a specific worker mission.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["mission_id"],
                    "properties": {
                        "mission_id": {
                            "type": "string",
                            "description": "UUID of the worker mission to cancel"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "cancel_all_workers".to_string(),
                description: "Cancel all active worker missions.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {}
                }),
            },
            ToolDefinition {
                name: "send_message_to_worker".to_string(),
                description: "Send a text message/instruction to a running worker mission.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["mission_id", "content"],
                    "properties": {
                        "mission_id": {
                            "type": "string",
                            "description": "UUID of the worker mission"
                        },
                        "content": {
                            "type": "string",
                            "description": "Message content to send to the worker"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "create_worktree".to_string(),
                description: "Create a git worktree for a worker to use as an isolated working directory. The worktree will be on its own branch so workers don't conflict.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path", "branch"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Absolute path where the worktree will be created (e.g. /workspaces/mission-xxx/verity-worker-1)"
                        },
                        "branch": {
                            "type": "string",
                            "description": "Branch name for the worktree (will be created if it doesn't exist)"
                        },
                        "base": {
                            "type": "string",
                            "description": "Base branch/commit to create from (defaults to HEAD)"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "remove_worktree".to_string(),
                description: "Remove a git worktree that is no longer needed.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["path"],
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Path of the worktree to remove"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "wait_for_worker".to_string(),
                description: "Block until a single worker mission reaches a terminal status. Use wait_for_any_worker to monitor multiple workers simultaneously.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["mission_id"],
                    "properties": {
                        "mission_id": {
                            "type": "string",
                            "description": "UUID of the worker mission to wait for"
                        },
                        "target_statuses": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Statuses to wait for (default: ['completed', 'failed', 'interrupted'])"
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum seconds to wait (default: 600)"
                        },
                        "poll_interval_seconds": {
                            "type": "integer",
                            "description": "Seconds between status checks (default: 10)"
                        }
                    }
                }),
            },
            ToolDefinition {
                name: "wait_for_any_worker".to_string(),
                description: "Block until ANY of the specified worker missions reaches a terminal status. Returns the first worker that finishes. Use this to monitor a pool of workers and react as each completes.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "required": ["mission_ids"],
                    "properties": {
                        "mission_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "UUIDs of worker missions to monitor"
                        },
                        "target_statuses": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Statuses to wait for (default: ['completed', 'failed', 'interrupted'])"
                        },
                        "timeout_seconds": {
                            "type": "integer",
                            "description": "Maximum seconds to wait (default: 600)"
                        },
                        "poll_interval_seconds": {
                            "type": "integer",
                            "description": "Seconds between status checks (default: 10)"
                        }
                    }
                }),
            },
        ]
    }

    async fn api_get(&self, path: &str) -> Result<reqwest::Response, String> {
        let url = format!("{}{}", self.api_url, path);
        let mut req = self.client.get(&url);
        if let Some((k, v)) = self.auth_header() {
            req = req.header(k, v);
        }
        req.send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))
    }

    async fn api_post(&self, path: &str, body: Value) -> Result<reqwest::Response, String> {
        let url = format!("{}{}", self.api_url, path);
        let mut req = self.client.post(&url).json(&body);
        if let Some((k, v)) = self.auth_header() {
            req = req.header(k, v);
        }
        req.send()
            .await
            .map_err(|e| format!("HTTP request failed: {}", e))
    }

    async fn create_worker(&self, params: CreateWorkerParams) -> Result<Value, String> {
        let body = json!({
            "title": params.title,
            "agent": params.agent,
            "backend": params.backend,
            "model_override": params.model_override,
            "model_effort": params.model_effort,
            "config_profile": params.config_profile,
            "parent_mission_id": self.mission_id.to_string(),
            "working_directory": params.working_directory,
        });

        let response = self.api_post("/api/control/missions", body).await?;
        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to create worker mission: {}", text));
        }

        let mission: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let worker_id = mission["id"].as_str().unwrap_or("");

        // If a prompt was provided, send it as the first message
        if let Some(prompt) = params.prompt {
            if !prompt.trim().is_empty() && !worker_id.is_empty() {
                let msg_body = json!({
                    "content": prompt,
                    "mission_id": worker_id,
                });
                if let Err(e) = self.api_post("/api/control/message", msg_body).await {
                    eprintln!("[orchestrator-mcp] Warning: created mission but failed to send initial prompt: {}", e);
                }
            }
        }

        Ok(mission)
    }

    async fn list_workers(&self) -> Result<Value, String> {
        // List all missions and filter for children of this boss mission
        let response = self
            .api_get("/api/control/missions?limit=100&offset=0")
            .await?;

        if !response.status().is_success() {
            return Err(format!("API returned error: {}", response.status()));
        }

        let missions: Vec<Value> = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        // Filter to only child missions of this boss
        let boss_id = self.mission_id.to_string();
        let workers: Vec<&Value> = missions
            .iter()
            .filter(|m| m["parent_mission_id"].as_str() == Some(&boss_id))
            .collect();

        Ok(json!({
            "boss_mission_id": boss_id,
            "worker_count": workers.len(),
            "workers": workers,
        }))
    }

    async fn get_worker_status(&self, params: GetWorkerStatusParams) -> Result<Value, String> {
        let id = Uuid::parse_str(&params.mission_id)
            .map_err(|_| "Invalid mission ID format".to_string())?;

        let response = self
            .api_get(&format!("/api/control/missions/{}", id))
            .await?;

        if !response.status().is_success() {
            return Err(format!("Worker mission not found: {}", response.status()));
        }

        let mission: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        Ok(mission)
    }

    async fn cancel_worker(&self, params: CancelWorkerParams) -> Result<Value, String> {
        let id = Uuid::parse_str(&params.mission_id)
            .map_err(|_| "Invalid mission ID format".to_string())?;

        let response = self
            .api_post(&format!("/api/control/missions/{}/cancel", id), json!({}))
            .await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to cancel worker: {}", text));
        }

        Ok(json!({"success": true, "cancelled": id.to_string()}))
    }

    async fn cancel_all_workers(&self) -> Result<Value, String> {
        let list = self.list_workers().await?;
        let workers = list["workers"].as_array().cloned().unwrap_or_default();
        let mut cancelled = Vec::new();
        let mut errors = Vec::new();

        for worker in &workers {
            let status = worker["status"].as_str().unwrap_or("");
            if status == "completed"
                || status == "failed"
                || status == "interrupted"
                || status == "not_feasible"
            {
                continue;
            }
            let id = worker["id"].as_str().unwrap_or("");
            if id.is_empty() {
                continue;
            }
            match self
                .cancel_worker(CancelWorkerParams {
                    mission_id: id.to_string(),
                })
                .await
            {
                Ok(_) => cancelled.push(id.to_string()),
                Err(e) => errors.push(format!("{}: {}", id, e)),
            }
        }

        Ok(json!({
            "cancelled": cancelled,
            "errors": errors,
        }))
    }

    async fn send_message(&self, params: SendMessageParams) -> Result<Value, String> {
        let id = Uuid::parse_str(&params.mission_id)
            .map_err(|_| "Invalid mission ID format".to_string())?;

        let body = json!({
            "content": params.content,
            "mission_id": id.to_string(),
        });

        let response = self.api_post("/api/control/message", body).await?;

        if !response.status().is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(format!("Failed to send message: {}", text));
        }

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        Ok(result)
    }

    fn create_worktree(&self, params: CreateWorktreeParams) -> Result<Value, String> {
        let path = &params.path;
        let branch = &params.branch;

        // Check if branch exists
        let branch_exists = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let output = if branch_exists {
            // Branch exists, just create worktree on it
            Command::new("git")
                .args(["worktree", "add", path, branch])
                .output()
                .map_err(|e| format!("Failed to run git worktree add: {}", e))?
        } else {
            // Create new branch from base
            let base = params.base.as_deref().unwrap_or("HEAD");
            Command::new("git")
                .args(["worktree", "add", "-b", branch, path, base])
                .output()
                .map_err(|e| format!("Failed to run git worktree add: {}", e))?
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git worktree add failed: {}", stderr));
        }

        Ok(json!({
            "success": true,
            "path": path,
            "branch": branch,
            "message": format!("Worktree created at {} on branch {}", path, branch),
        }))
    }

    fn remove_worktree(&self, params: RemoveWorktreeParams) -> Result<Value, String> {
        let output = Command::new("git")
            .args(["worktree", "remove", "--force", &params.path])
            .output()
            .map_err(|e| format!("Failed to run git worktree remove: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("git worktree remove failed: {}", stderr));
        }

        Ok(json!({
            "success": true,
            "path": params.path,
            "message": format!("Worktree removed at {}", params.path),
        }))
    }

    async fn batch_create_workers(
        &self,
        params: BatchCreateWorkersParams,
    ) -> Result<Value, String> {
        let mut results = Vec::new();
        let mut errors = Vec::new();

        for (i, worker_params) in params.workers.into_iter().enumerate() {
            match self.create_worker(worker_params).await {
                Ok(mission) => results.push(json!({
                    "index": i,
                    "success": true,
                    "mission": mission,
                })),
                Err(e) => {
                    errors.push(json!({
                        "index": i,
                        "success": false,
                        "error": e,
                    }));
                }
            }
        }

        Ok(json!({
            "created": results.len(),
            "failed": errors.len(),
            "results": results,
            "errors": errors,
        }))
    }

    async fn wait_for_any_worker(&self, params: WaitForAnyWorkerParams) -> Result<Value, String> {
        let mut ids = Vec::new();
        let mut invalid_ids = Vec::new();
        for s in &params.mission_ids {
            match Uuid::parse_str(s) {
                Ok(id) => ids.push(id),
                Err(_) => invalid_ids.push(s.clone()),
            }
        }

        if !invalid_ids.is_empty() {
            return Err(format!(
                "Invalid mission ID format: {}",
                invalid_ids.join(", ")
            ));
        }

        if ids.is_empty() {
            return Err("No mission IDs provided".to_string());
        }

        let target_statuses = if params.target_statuses.is_empty() {
            vec![
                "completed".to_string(),
                "failed".to_string(),
                "interrupted".to_string(),
                "not_feasible".to_string(),
            ]
        } else {
            params.target_statuses
        };

        let timeout = std::time::Duration::from_secs(params.timeout_seconds);
        let interval = std::time::Duration::from_secs(params.poll_interval_seconds);
        let start = std::time::Instant::now();
        let mut consecutive_errors: u32 = 0;

        loop {
            for id in &ids {
                let response = self.api_get(&format!("/api/control/missions/{}", id)).await;

                match response {
                    Ok(resp) if resp.status().is_success() => {
                        consecutive_errors = 0;
                        if let Ok(mission) = resp.json::<Value>().await {
                            let status = mission["status"].as_str().unwrap_or("");
                            if target_statuses.iter().any(|s| s == status) {
                                return Ok(json!({
                                    "reached_target": true,
                                    "mission_id": id.to_string(),
                                    "status": status,
                                    "elapsed_seconds": start.elapsed().as_secs(),
                                    "mission": mission,
                                }));
                            }
                        }
                    }
                    Ok(resp) => {
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            return Err(format!(
                                "Mission {} returned HTTP {} after {} consecutive errors",
                                id,
                                resp.status(),
                                consecutive_errors
                            ));
                        }
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        if consecutive_errors >= 3 {
                            return Err(format!(
                                "API request failed for mission {}: {} ({} consecutive errors)",
                                id, e, consecutive_errors
                            ));
                        }
                    }
                }
            }

            if start.elapsed() > timeout {
                return Ok(json!({
                    "reached_target": false,
                    "elapsed_seconds": start.elapsed().as_secs(),
                    "timeout": true,
                }));
            }

            tokio::time::sleep(interval).await;
        }
    }

    async fn wait_for_worker(&self, params: WaitForWorkerParams) -> Result<Value, String> {
        let id = Uuid::parse_str(&params.mission_id)
            .map_err(|_| "Invalid mission ID format".to_string())?;

        let target_statuses = if params.target_statuses.is_empty() {
            vec![
                "completed".to_string(),
                "failed".to_string(),
                "interrupted".to_string(),
                "not_feasible".to_string(),
            ]
        } else {
            params.target_statuses
        };

        let timeout = std::time::Duration::from_secs(params.timeout_seconds);
        let interval = std::time::Duration::from_secs(params.poll_interval_seconds);
        let start = std::time::Instant::now();

        loop {
            // Check status
            let response = self
                .api_get(&format!("/api/control/missions/{}", id))
                .await?;

            if !response.status().is_success() {
                return Err(format!("Worker mission not found: {}", response.status()));
            }

            let mission: Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse response: {}", e))?;

            let status = mission["status"].as_str().unwrap_or("");
            if target_statuses.iter().any(|s| s == status) {
                return Ok(json!({
                    "reached_target": true,
                    "status": status,
                    "elapsed_seconds": start.elapsed().as_secs(),
                    "mission": mission,
                }));
            }

            // Check timeout
            if start.elapsed() > timeout {
                return Ok(json!({
                    "reached_target": false,
                    "status": status,
                    "elapsed_seconds": start.elapsed().as_secs(),
                    "timeout": true,
                    "mission": mission,
                }));
            }

            tokio::time::sleep(interval).await;
        }
    }

    async fn handle_call(&self, method: &str, params: Value) -> Result<Value, String> {
        match method {
            "create_worker_mission" => {
                let params: CreateWorkerParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.create_worker(params).await
            }
            "batch_create_workers" => {
                let params: BatchCreateWorkersParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.batch_create_workers(params).await
            }
            "list_worker_missions" => self.list_workers().await,
            "get_worker_status" => {
                let params: GetWorkerStatusParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.get_worker_status(params).await
            }
            "cancel_worker" => {
                let params: CancelWorkerParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.cancel_worker(params).await
            }
            "cancel_all_workers" => self.cancel_all_workers().await,
            "send_message_to_worker" => {
                let params: SendMessageParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.send_message(params).await
            }
            "create_worktree" => {
                let params: CreateWorktreeParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.create_worktree(params)
            }
            "remove_worktree" => {
                let params: RemoveWorktreeParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.remove_worktree(params)
            }
            "wait_for_worker" => {
                let params: WaitForWorkerParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.wait_for_worker(params).await
            }
            "wait_for_any_worker" => {
                let params: WaitForAnyWorkerParams =
                    serde_json::from_value(params).map_err(|e| format!("Invalid params: {}", e))?;
                self.wait_for_any_worker(params).await
            }
            _ => Err(format!("Unknown method: {}", method)),
        }
    }

    async fn handle_request(&self, req: JsonRpcRequest) -> JsonRpcResponse {
        match req.method.as_str() {
            "initialize" => {
                let info = ServerInfo {
                    name: "orchestrator".to_string(),
                    version: "0.1.0".to_string(),
                };
                JsonRpcResponse::success(
                    req.id,
                    json!({
                        "protocolVersion": "2024-11-05",
                        "serverInfo": info,
                        "capabilities": {
                            "tools": {}
                        }
                    }),
                )
            }
            "tools/list" => {
                let tools = Self::get_tools();
                JsonRpcResponse::success(req.id, json!({ "tools": tools }))
            }
            "tools/call" => {
                let params = match req.params.as_object() {
                    Some(p) => p,
                    None => {
                        return JsonRpcResponse::error(req.id, -32602, "Invalid params");
                    }
                };
                let method = match params.get("name").and_then(|n| n.as_str()) {
                    Some(m) => m,
                    None => {
                        return JsonRpcResponse::error(req.id, -32602, "Missing tool name");
                    }
                };
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);

                match self.handle_call(method, arguments).await {
                    Ok(result) => JsonRpcResponse::success(
                        req.id,
                        json!({
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&result).unwrap()
                            }]
                        }),
                    ),
                    Err(e) => JsonRpcResponse::error(req.id, -32000, e),
                }
            }
            "notifications/initialized" => {
                // Notification, no response needed but we return empty for safety
                JsonRpcResponse::success(req.id, json!(null))
            }
            _ => JsonRpcResponse::error(req.id, -32601, format!("Unknown method: {}", req.method)),
        }
    }
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
async fn main() {
    let mission_id = std::env::var("MISSION_ID")
        .or_else(|_| std::env::var("SANDBOXED_SH_MISSION_ID"))
        .ok()
        .and_then(|id| Uuid::parse_str(&id).ok())
        .expect("MISSION_ID environment variable not set or invalid");

    let api_url = std::env::var("API_URL")
        .or_else(|_| std::env::var("SANDBOXED_SH_API_URL"))
        .unwrap_or_else(|_| "http://localhost:3000".to_string());
    let api_token = std::env::var("API_TOKEN")
        .or_else(|_| std::env::var("SANDBOXED_SH_API_TOKEN"))
        .ok()
        .or_else(|| {
            // Mint a service JWT from the shared secret when no explicit token is set.
            std::env::var("JWT_SECRET")
                .ok()
                .and_then(|s| mint_service_jwt(&s))
        });

    let server = Arc::new(OrchestratorMcp::new(mission_id, api_url, api_token));

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let reader = BufReader::new(stdin);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let error_resp =
                    JsonRpcResponse::error(Value::Null, -32700, format!("Parse error: {}", e));
                if let Ok(json) = serde_json::to_string(&error_resp) {
                    writeln!(stdout, "{}", json).ok();
                }
                stdout.flush().ok();
                continue;
            }
        };

        // Skip notifications (id is null)
        if request.id.is_null() && request.method.starts_with("notifications/") {
            continue;
        }

        let response = server.handle_request(request).await;
        if let Ok(json) = serde_json::to_string(&response) {
            writeln!(stdout, "{}", json).ok();
        }
        stdout.flush().ok();
    }
}
