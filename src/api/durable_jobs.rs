//! Durable background jobs.
//!
//! Jobs are launched by the API server rather than by an ephemeral agent shell.
//! That places long-running commands under the server's lifecycle and gives
//! later turns an explicit registry for status, logs, and cancellation.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use axum::{
    extract::{Path as AxumPath, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use uuid::Uuid;

use super::routes::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DurableJobStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableJob {
    pub id: Uuid,
    pub command: String,
    pub cwd: String,
    pub status: DurableJobStatus,
    pub pid: Option<u32>,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub started_by_mission_id: Option<Uuid>,
    pub stdout_log: String,
    pub stderr_log: String,
    pub status_file: String,
}

#[derive(Debug, Deserialize)]
pub struct StartDurableJobRequest {
    pub command: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub started_by_mission_id: Option<Uuid>,
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct JobLogsQuery {
    #[serde(default = "default_tail_bytes")]
    pub tail_bytes: usize,
    #[serde(default)]
    pub stream: Option<String>,
}

fn default_tail_bytes() -> usize {
    16 * 1024
}

#[derive(Debug, Serialize)]
pub struct JobLogsResponse {
    pub job_id: Uuid,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    error: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExitRecord {
    exit_code: Option<i32>,
    signal: Option<i32>,
    finished_at: DateTime<Utc>,
}

fn err(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: message.into(),
        }),
    )
}

fn jobs_root(state: &AppState) -> PathBuf {
    state.config.working_dir.join(".sandboxed-sh/durable-jobs")
}

fn job_dir(state: &AppState, id: Uuid) -> PathBuf {
    jobs_root(state).join(id.to_string())
}

fn job_file(state: &AppState, id: Uuid) -> PathBuf {
    job_dir(state, id).join("job.json")
}

fn resolve_cwd(base: &Path, raw: Option<&str>) -> Result<PathBuf, String> {
    let cwd = match raw {
        Some(value) if !value.trim().is_empty() => {
            let path = PathBuf::from(value.trim());
            if path.is_absolute() {
                path
            } else {
                base.join(path)
            }
        }
        _ => base.to_path_buf(),
    };

    if !cwd.exists() {
        return Err(format!("cwd does not exist: {}", cwd.display()));
    }
    if !cwd.is_dir() {
        return Err(format!("cwd is not a directory: {}", cwd.display()));
    }

    Ok(cwd)
}

async fn write_job(state: &AppState, job: &DurableJob) -> Result<(), String> {
    let path = job_file(state, job.id);
    let parent = path
        .parent()
        .ok_or_else(|| "invalid durable job path".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| format!("failed to create job dir: {}", e))?;
    let bytes = serde_json::to_vec_pretty(job)
        .map_err(|e| format!("failed to serialize job registry entry: {}", e))?;
    tokio::fs::write(path, bytes)
        .await
        .map_err(|e| format!("failed to write job registry entry: {}", e))
}

async fn read_job(state: &AppState, id: Uuid) -> Result<DurableJob, String> {
    let bytes = tokio::fs::read(job_file(state, id))
        .await
        .map_err(|_| format!("durable job not found: {}", id))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("invalid durable job entry: {}", e))
}

async fn write_terminal_job_state(
    state: &AppState,
    mut job: DurableJob,
    status: DurableJobStatus,
    exit_code: Option<i32>,
    signal: Option<i32>,
    updated_at: DateTime<Utc>,
) -> DurableJob {
    if let Ok(latest) = read_job(state, job.id).await {
        if latest.status == DurableJobStatus::Cancelled {
            return latest;
        }
        job = latest;
    }

    job.status = status;
    job.exit_code = exit_code;
    job.signal = signal;
    job.updated_at = updated_at;
    let _ = write_job(state, &job).await;
    job
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn process_alive(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
fn terminate_process_group(pid: u32) {
    unsafe {
        let pgid = libc::getpgid(pid as libc::pid_t);
        if pgid > 0 {
            let _ = libc::kill(-pgid, libc::SIGTERM);
        } else {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
    }
}

#[cfg(not(unix))]
fn terminate_process_group(_pid: u32) {}

async fn refresh_job(state: &AppState, mut job: DurableJob) -> DurableJob {
    if matches!(
        job.status,
        DurableJobStatus::Running | DurableJobStatus::Unknown
    ) {
        if let Ok(bytes) = tokio::fs::read(&job.status_file).await {
            if let Ok(exit) = serde_json::from_slice::<ExitRecord>(&bytes) {
                let status = if exit.exit_code == Some(0) {
                    DurableJobStatus::Completed
                } else {
                    DurableJobStatus::Failed
                };
                return write_terminal_job_state(
                    state,
                    job,
                    status,
                    exit.exit_code,
                    exit.signal,
                    exit.finished_at,
                )
                .await;
            }
        }

        if let Some(pid) = job.pid {
            if !process_alive(pid) {
                job.status = DurableJobStatus::Unknown;
                job.updated_at = Utc::now();
                let _ = write_job(state, &job).await;
            }
        }
    }
    job
}

pub async fn start_job(
    State(state): State<Arc<AppState>>,
    Json(req): Json<StartDurableJobRequest>,
) -> Result<Json<DurableJob>, (StatusCode, Json<ErrorResponse>)> {
    let command = req.command.trim();
    if command.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "command is required"));
    }

    let cwd = resolve_cwd(&state.config.working_dir, req.cwd.as_deref())
        .map_err(|e| err(StatusCode::BAD_REQUEST, e))?;

    let id = Uuid::new_v4();
    let dir = job_dir(&state, id);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stdout_log = dir.join("stdout.log");
    let stderr_log = dir.join("stderr.log");
    let status_file = dir.join("exit.json");

    let stdout = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stdout_log)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let stderr = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_log)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let wrapper = format!(
        "cd \"$SANDBOXED_SH_DURABLE_CWD\" && {{ {}; }}\ncode=$?\nprintf '{{\"exit_code\":%s,\"signal\":null,\"finished_at\":\"%s\"}}\\n' \"$code\" \"$(date -u +%Y-%m-%dT%H:%M:%SZ)\" > \"$SANDBOXED_SH_DURABLE_STATUS\"\nexit \"$code\"\n",
        command
    );

    let mut child = Command::new("setsid");
    child
        .arg("/bin/sh")
        .arg("-lc")
        .arg(wrapper)
        .current_dir(&cwd)
        .envs(req.env)
        .env("SANDBOXED_SH_DURABLE_CWD", &cwd)
        .env("SANDBOXED_SH_DURABLE_STATUS", &status_file)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));

    let child = child
        .spawn()
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let pid = child.id();

    let now = Utc::now();
    let job = DurableJob {
        id,
        command: command.to_string(),
        cwd: cwd.to_string_lossy().to_string(),
        status: DurableJobStatus::Running,
        pid,
        exit_code: None,
        signal: None,
        created_at: now,
        updated_at: now,
        started_by_mission_id: req.started_by_mission_id,
        stdout_log: stdout_log.to_string_lossy().to_string(),
        stderr_log: stderr_log.to_string_lossy().to_string(),
        status_file: status_file.to_string_lossy().to_string(),
    };
    write_job(&state, &job)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;

    let watcher_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut child = child;
        if let Ok(status) = child.wait().await {
            if let Ok(job) = read_job(&watcher_state, id).await {
                let job_status = if status.success() {
                    DurableJobStatus::Completed
                } else {
                    DurableJobStatus::Failed
                };
                #[cfg(unix)]
                let signal = {
                    use std::os::unix::process::ExitStatusExt;
                    status.signal()
                };
                #[cfg(not(unix))]
                let signal = None;
                let _ = write_terminal_job_state(
                    &watcher_state,
                    job,
                    job_status,
                    status.code(),
                    signal,
                    Utc::now(),
                )
                .await;
            }
        }
    });

    Ok(Json(job))
}

pub async fn list_jobs(State(state): State<Arc<AppState>>) -> Json<Vec<DurableJob>> {
    let mut jobs = Vec::new();
    let root = jobs_root(&state);
    if let Ok(mut entries) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path().join("job.json");
            if let Ok(bytes) = tokio::fs::read(path).await {
                if let Ok(job) = serde_json::from_slice::<DurableJob>(&bytes) {
                    jobs.push(refresh_job(&state, job).await);
                }
            }
        }
    }
    jobs.sort_by_key(|job| std::cmp::Reverse(job.created_at));
    Json(jobs)
}

pub async fn get_job(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Json<DurableJob>, (StatusCode, Json<ErrorResponse>)> {
    let job = read_job(&state, id)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, e))?;
    Ok(Json(refresh_job(&state, job).await))
}

async fn tail_file(path: &str, max_bytes: usize) -> String {
    let Ok(bytes) = tokio::fs::read(path).await else {
        return String::new();
    };
    let keep = max_bytes.clamp(1, 256 * 1024);
    let start = bytes.len().saturating_sub(keep);
    String::from_utf8_lossy(&bytes[start..]).to_string()
}

pub async fn job_logs(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<Uuid>,
    axum::extract::Query(query): axum::extract::Query<JobLogsQuery>,
) -> Result<Json<JobLogsResponse>, (StatusCode, Json<ErrorResponse>)> {
    let job = read_job(&state, id)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, e))?;

    let stdout = if query.stream.as_deref() == Some("stderr") {
        String::new()
    } else {
        tail_file(&job.stdout_log, query.tail_bytes).await
    };
    let stderr = if query.stream.as_deref() == Some("stdout") {
        String::new()
    } else {
        tail_file(&job.stderr_log, query.tail_bytes).await
    };

    Ok(Json(JobLogsResponse {
        job_id: id,
        stdout,
        stderr,
    }))
}

pub async fn cancel_job(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<Uuid>,
) -> Result<Json<DurableJob>, (StatusCode, Json<ErrorResponse>)> {
    let mut job = read_job(&state, id)
        .await
        .map_err(|e| err(StatusCode::NOT_FOUND, e))?;
    job = refresh_job(&state, job).await;
    if job.status == DurableJobStatus::Running {
        if let Some(pid) = job.pid {
            terminate_process_group(pid);
        }
        job.status = DurableJobStatus::Cancelled;
        job.updated_at = Utc::now();
        write_job(&state, &job)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    }
    Ok(Json(job))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(list_jobs).post(start_job))
        .route("/:id", get(get_job))
        .route("/:id/logs", get(job_logs))
        .route("/:id/cancel", post(cancel_job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_cwd_defaults_to_base() {
        let base = std::env::current_dir().unwrap();
        assert_eq!(resolve_cwd(&base, None).unwrap(), base);
    }

    #[test]
    fn resolve_cwd_rejects_missing_path() {
        let base = std::env::current_dir().unwrap();
        let result = resolve_cwd(&base, Some("__definitely_missing_durable_job_cwd__"));
        assert!(result.is_err());
    }
}
