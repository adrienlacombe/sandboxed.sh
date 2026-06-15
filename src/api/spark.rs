//! DGX Spark build-offload endpoint.
//!
//! A harness inside a mission workspace calls `POST /api/spark/offload` over the
//! host veth link (`10.88.0.1`) to run a Lean build on the DGX Spark instead of
//! the main box. The HOST holds the Spark credentials (arbiter token + SSH
//! target, from [`crate::config::Config`]), so workspaces never carry them.
//!
//! Flow: rsync the mission workspace to the Spark, submit the build to the
//! arbiter (`dgx-spark-arbiter`, which time-shares the Spark's unified memory
//! against vLLM/step37 by priority), poll to completion, rsync artifacts back,
//! return the result. Responds `503` when Spark config is absent so the
//! in-workspace `spark-build` wrapper transparently falls back to a local build.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::routes::AppState;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/offload", post(offload_build))
}

#[derive(Deserialize)]
struct OffloadRequest {
    /// Host filesystem path of the mission workspace root, injected as
    /// `SPARK_WORKSPACE_HOST_DIR` and echoed back by the wrapper.
    host_dir: String,
    /// Build cwd relative to the workspace root (e.g. `"morpho-verity"`).
    #[serde(default)]
    rel: String,
    /// The build command, e.g. `"lake build"`.
    cmd: String,
    #[serde(default = "default_priority")]
    priority: String,
}

fn default_priority() -> String {
    "P0".to_string()
}

#[derive(Serialize)]
struct OffloadResponse {
    exit_code: i64,
    log: String,
}

/// Run a host subprocess, returning (success, combined output).
async fn run(args: &[&str]) -> (bool, String) {
    match tokio::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .await
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.success(), s)
        }
        Err(e) => (false, e.to_string()),
    }
}

async fn offload_build(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<OffloadRequest>,
) -> axum::response::Response {
    if let Err(resp) = super::proxy::verify_proxy_auth(&headers, &state).await {
        return resp;
    }

    // All three must be set, else tell the caller to build locally.
    let (Some(url), Some(token), Some(ssh)) = (
        state.config.spark_arbiter_url.as_deref(),
        state.config.spark_arbiter_token.as_deref(),
        state.config.spark_ssh_target.as_deref(),
    ) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "spark offload not configured",
        )
            .into_response();
    };

    // Security: only rsync real mission workspace dirs (the host_dir comes from a
    // workspace-authenticated caller, but constrain it anyway). Require an
    // absolute path containing the mission marker and no `..`. The leading-`/`
    // anchor also guarantees host_dir can't start with `-`, which — combined
    // with the `--` separator before the positional rsync paths below — closes
    // the argv flag-smuggling vector (e.g. a `--rsync-path=<cmd>` value that
    // still contains `/workspaces/mission-` would otherwise reach rsync as an
    // option and run a command on the Spark host).
    if !req.host_dir.starts_with('/')
        || !req.host_dir.contains("/workspaces/mission-")
        || req.host_dir.contains("..")
    {
        return (StatusCode::BAD_REQUEST, "invalid host_dir").into_response();
    }
    if req.cmd.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "cmd required").into_response();
    }

    let host_dir = req.host_dir.trim_end_matches('/').to_string();
    let name = host_dir.rsplit('/').next().unwrap_or("build").to_string();
    // Belt-and-suspenders: the derived `name` feeds remote paths; never let it
    // look like a flag even if host_dir's last segment somehow began with `-`.
    if name.starts_with('-') {
        return (StatusCode::BAD_REQUEST, "invalid host_dir").into_response();
    }
    let user = ssh.split('@').next().unwrap_or("th0rgal");
    let remote_rel = format!(".spark-builds/{}", name);
    let remote_cwd = if req.rel.is_empty() {
        format!("/home/{}/{}", user, remote_rel)
    } else {
        format!(
            "/home/{}/{}/{}",
            user,
            remote_rel,
            req.rel.trim_matches('/')
        )
    };

    // 1. Sync the workspace up to the Spark.
    let up = run(&[
        "rsync",
        "-az",
        "--delete",
        "--exclude",
        ".git",
        "-e",
        "ssh",
        "--",
        &format!("{}/", host_dir),
        &format!("{}:{}/", ssh, remote_rel),
    ])
    .await;
    if !up.0 {
        tracing::warn!("spark offload: rsync up failed: {}", up.1);
        return (
            StatusCode::BAD_GATEWAY,
            format!("rsync up failed: {}", up.1),
        )
            .into_response();
    }

    // 2. Submit the build to the arbiter.
    let client = &state.http_client;
    let submit = client
        .post(format!("{}/build", url))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "priority": req.priority, "cmd": req.cmd, "cwd": remote_cwd,
        }))
        .send()
        .await;
    let jid = match submit {
        Ok(r) => match r.json::<serde_json::Value>().await {
            Ok(v) => v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()),
            Err(e) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    format!("arbiter submit parse: {e}"),
                )
                    .into_response()
            }
        },
        Err(e) => {
            return (StatusCode::BAD_GATEWAY, format!("arbiter unreachable: {e}")).into_response()
        }
    };
    let Some(jid) = jid else {
        return (StatusCode::BAD_GATEWAY, "arbiter returned no job id").into_response();
    };

    // 3. Poll to completion (cap ~60 min — Lean builds can be long).
    let mut log = String::new();
    let mut exit_code = -1i64;
    let mut done = false;
    for _ in 0..1200 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        let st = client
            .get(format!("{}/build/{}", url, jid))
            .bearer_auth(token)
            .send()
            .await
            .ok();
        let Some(v) = st else { continue };
        let Ok(v) = v.json::<serde_json::Value>().await else {
            continue;
        };
        log = v
            .get("log_tail")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        match v.get("status").and_then(|x| x.as_str()) {
            Some("done") | Some("failed") => {
                exit_code = v.get("exit_code").and_then(|x| x.as_i64()).unwrap_or(-1);
                done = true;
                break;
            }
            _ => continue,
        }
    }
    if !done {
        return (StatusCode::GATEWAY_TIMEOUT, "build timed out on spark").into_response();
    }

    // 4. Sync artifacts (.olean etc.) back into the workspace.
    let _back = run(&[
        "rsync",
        "-az",
        "-e",
        "ssh",
        "--",
        &format!("{}:{}/", ssh, remote_rel),
        &format!("{}/", host_dir),
    ])
    .await;

    Json(OffloadResponse { exit_code, log }).into_response()
}
