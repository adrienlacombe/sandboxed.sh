pub mod client;

use anyhow::Error;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};
use tokio::task::JoinHandle;
use tracing::debug;

use crate::backend::events::ExecutionEvent;
use crate::backend::{AgentInfo, Backend, Session, SessionConfig};

use client::{GeminiClient, GeminiConfig, GeminiEvent};

/// Gemini CLI backend that spawns the Gemini CLI for mission execution.
pub struct GeminiBackend {
    id: String,
    name: String,
    config: Arc<RwLock<GeminiConfig>>,
    workspace_exec: Option<crate::workspace_exec::WorkspaceExec>,
}

impl GeminiBackend {
    pub fn new() -> Self {
        Self {
            id: "gemini".to_string(),
            name: "Gemini CLI".to_string(),
            config: Arc::new(RwLock::new(GeminiConfig::default())),
            workspace_exec: None,
        }
    }

    pub fn with_config(config: GeminiConfig) -> Self {
        Self {
            id: "gemini".to_string(),
            name: "Gemini CLI".to_string(),
            config: Arc::new(RwLock::new(config)),
            workspace_exec: None,
        }
    }

    pub fn with_config_and_workspace(
        config: GeminiConfig,
        workspace_exec: crate::workspace_exec::WorkspaceExec,
    ) -> Self {
        Self {
            id: "gemini".to_string(),
            name: "Gemini CLI".to_string(),
            config: Arc::new(RwLock::new(config)),
            workspace_exec: Some(workspace_exec),
        }
    }

    /// Update the backend configuration.
    pub async fn update_config(&self, config: GeminiConfig) {
        let mut cfg = self.config.write().await;
        *cfg = config;
    }

    /// Get the current configuration.
    pub async fn get_config(&self) -> GeminiConfig {
        self.config.read().await.clone()
    }
}

impl Default for GeminiBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Backend for GeminiBackend {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    async fn list_agents(&self) -> Result<Vec<AgentInfo>, Error> {
        // Gemini CLI doesn't have separate agent types
        // Return a single general-purpose agent
        Ok(vec![AgentInfo {
            id: "default".to_string(),
            name: "Gemini Agent".to_string(),
        }])
    }

    async fn create_session(&self, config: SessionConfig) -> Result<Session, Error> {
        let client = GeminiClient::new();
        Ok(Session {
            id: client.create_session_id(),
            directory: config.directory,
            model: config.model,
            agent: config.agent,
        })
    }

    async fn send_message_streaming(
        &self,
        session: &Session,
        message: &str,
    ) -> Result<(mpsc::Receiver<ExecutionEvent>, JoinHandle<()>), Error> {
        let config = self.config.read().await.clone();
        let client = GeminiClient::with_config(config);
        let workspace_exec = self.workspace_exec.as_ref();

        let (mut gemini_rx, gemini_handle) = client
            .execute_message(
                &session.directory,
                message,
                session.model.as_deref(),
                Some(&session.id),
                session.agent.as_deref(),
                workspace_exec,
            )
            .await?;

        let (tx, rx) = mpsc::channel(256);
        let session_id = session.id.clone();

        // Spawn event conversion task
        let handle = tokio::spawn(async move {
            while let Some(event) = gemini_rx.recv().await {
                let exec_events = convert_gemini_event(event);

                for exec_event in exec_events {
                    if tx.send(exec_event).await.is_err() {
                        debug!("ExecutionEvent receiver dropped");
                        break;
                    }
                }
            }

            // Ensure MessageComplete is sent
            let _ = tx
                .send(ExecutionEvent::MessageComplete {
                    session_id: session_id.clone(),
                })
                .await;

            // Drop the gemini handle to clean up
            drop(gemini_handle);
        });

        Ok((rx, handle))
    }
}

/// Convert a Gemini CLI event to backend-agnostic ExecutionEvents.
fn convert_gemini_event(event: GeminiEvent) -> Vec<ExecutionEvent> {
    let mut results = vec![];

    match event {
        GeminiEvent::Init { session_id, model } => {
            debug!(
                "Gemini session init: session_id={:?}, model={:?}",
                session_id, model
            );
        }

        GeminiEvent::Message {
            role,
            content,
            delta: _,
        } => {
            // Only emit assistant messages as text deltas
            if role.as_deref() == Some("assistant") {
                if let Some(text) = content {
                    if !text.is_empty() {
                        results.push(ExecutionEvent::TextDelta { content: text });
                    }
                }
            }
        }

        GeminiEvent::ToolUse {
            tool_name,
            tool_id,
            parameters,
        } => {
            if let Some(name) = tool_name {
                let id = tool_id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                let args = parameters
                    .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
                results.push(ExecutionEvent::ToolCall { id, name, args });
            }
        }

        GeminiEvent::ToolResult {
            tool_id,
            status: _,
            output,
            error,
        } => {
            if let Some(id) = tool_id {
                // Build result value, including error if present
                let result = if let Some(err) = error {
                    serde_json::json!({
                        "error": {
                            "type": err.error_type,
                            "message": err.message,
                        },
                        "output": output,
                    })
                } else {
                    output.unwrap_or(serde_json::Value::Null)
                };

                // Use tool_id as name fallback since Gemini tool_result doesn't repeat the name
                results.push(ExecutionEvent::ToolResult {
                    id: id.clone(),
                    name: id,
                    result,
                });
            }
        }

        GeminiEvent::Error { severity, message } => {
            // Treat warnings as debug logs, errors as execution errors
            if severity.as_deref() == Some("warning") {
                debug!("Gemini warning: {}", message);
            } else {
                results.push(ExecutionEvent::Error { message });
            }
        }

        GeminiEvent::Result { status: _, stats } => {
            // Extract token usage from final stats
            if let Some(stats) = stats {
                let input = stats.total_input_tokens.unwrap_or(0);
                let output = stats.total_output_tokens.unwrap_or(0);
                if input > 0 || output > 0 {
                    results.push(ExecutionEvent::Usage {
                        input_tokens: input,
                        output_tokens: output,
                    });
                }
            }
        }

        GeminiEvent::Unknown => {
            debug!("Unknown Gemini event type");
        }
    }

    results
}

/// Create a registry entry for the Gemini backend.
pub fn registry_entry() -> Arc<dyn Backend> {
    Arc::new(GeminiBackend::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_list_agents() {
        let backend = GeminiBackend::new();
        let agents = backend.list_agents().await.unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].id, "default");
    }

    #[tokio::test]
    async fn test_create_session() {
        let backend = GeminiBackend::new();
        let session = backend
            .create_session(SessionConfig {
                directory: "/tmp".to_string(),
                title: Some("Test".to_string()),
                model: Some("gemini-2.5-flash".to_string()),
                agent: None,
            })
            .await
            .unwrap();
        assert!(!session.id.is_empty());
        assert_eq!(session.directory, "/tmp");
    }

    #[test]
    fn convert_gemini_event_init_no_events() {
        let event = GeminiEvent::Init {
            session_id: Some("s1".to_string()),
            model: Some("gemini-2.5-flash".to_string()),
        };
        let events = convert_gemini_event(event);
        assert!(events.is_empty(), "Init should produce no execution events");
    }

    #[test]
    fn convert_gemini_event_assistant_message() {
        let event = GeminiEvent::Message {
            role: Some("assistant".to_string()),
            content: Some("Hello world".to_string()),
            delta: Some(true),
        };
        let events = convert_gemini_event(event);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ExecutionEvent::TextDelta { content } => {
                assert_eq!(content, "Hello world");
            }
            other => panic!("Expected TextDelta, got {:?}", other),
        }
    }

    #[test]
    fn convert_gemini_event_user_message_ignored() {
        let event = GeminiEvent::Message {
            role: Some("user".to_string()),
            content: Some("User message".to_string()),
            delta: Some(false),
        };
        let events = convert_gemini_event(event);
        assert!(events.is_empty(), "User messages should be ignored");
    }

    #[test]
    fn convert_gemini_event_tool_use() {
        let event = GeminiEvent::ToolUse {
            tool_name: Some("read_file".to_string()),
            tool_id: Some("tc1".to_string()),
            parameters: Some(serde_json::json!({"path": "/tmp/test.txt"})),
        };
        let events = convert_gemini_event(event);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ExecutionEvent::ToolCall { id, name, args } => {
                assert_eq!(id, "tc1");
                assert_eq!(name, "read_file");
                assert_eq!(args["path"], "/tmp/test.txt");
            }
            other => panic!("Expected ToolCall, got {:?}", other),
        }
    }

    #[test]
    fn convert_gemini_event_tool_result_success() {
        let event = GeminiEvent::ToolResult {
            tool_id: Some("tc1".to_string()),
            status: Some("success".to_string()),
            output: Some(serde_json::json!("file contents")),
            error: None,
        };
        let events = convert_gemini_event(event);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ExecutionEvent::ToolResult { id, result, .. } => {
                assert_eq!(id, "tc1");
                assert_eq!(result, "file contents");
            }
            other => panic!("Expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn convert_gemini_event_error() {
        let event = GeminiEvent::Error {
            severity: Some("error".to_string()),
            message: "Something failed".to_string(),
        };
        let events = convert_gemini_event(event);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ExecutionEvent::Error { message } => {
                assert_eq!(message, "Something failed");
            }
            other => panic!("Expected Error, got {:?}", other),
        }
    }

    #[test]
    fn convert_gemini_event_warning_ignored() {
        let event = GeminiEvent::Error {
            severity: Some("warning".to_string()),
            message: "Just a warning".to_string(),
        };
        let events = convert_gemini_event(event);
        assert!(events.is_empty(), "Warnings should not produce events");
    }

    #[test]
    fn convert_gemini_event_result_with_usage() {
        let event = GeminiEvent::Result {
            status: Some("success".to_string()),
            stats: Some(client::GeminiStats {
                total_input_tokens: Some(1500),
                total_output_tokens: Some(300),
                models: None,
            }),
        };
        let events = convert_gemini_event(event);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ExecutionEvent::Usage {
                input_tokens,
                output_tokens,
            } => {
                assert_eq!(*input_tokens, 1500);
                assert_eq!(*output_tokens, 300);
            }
            other => panic!("Expected Usage, got {:?}", other),
        }
    }

    #[test]
    fn convert_gemini_event_result_zero_usage() {
        let event = GeminiEvent::Result {
            status: Some("success".to_string()),
            stats: Some(client::GeminiStats {
                total_input_tokens: Some(0),
                total_output_tokens: Some(0),
                models: None,
            }),
        };
        let events = convert_gemini_event(event);
        assert!(events.is_empty(), "Zero usage should not emit Usage event");
    }

    #[test]
    fn convert_gemini_event_unknown_no_events() {
        let event = GeminiEvent::Unknown;
        let events = convert_gemini_event(event);
        assert!(events.is_empty());
    }
}
