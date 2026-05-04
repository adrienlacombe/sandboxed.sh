//! Minimal JWT auth for the dashboard (single-tenant).
//!
//! - Dashboard submits a password to `/api/auth/login`
//! - Server returns a JWT valid for ~30 days
//! - When `DEV_MODE=false`, all API endpoints require `Authorization: Bearer <jwt>`
//!
//! # Security notes
//! - This is intentionally minimal; it is NOT multi-tenant and does not implement RLS.
//! - Use a strong `JWT_SECRET` in production.

use axum::{
    body::Body,
    extract::{Query, State},
    http::{Request, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    Extension, Json,
};
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

use super::routes::AppState;
use super::types::{LoginRequest, LoginResponse};
use crate::config::{AuthMode, Config, UserAccount};
use crate::secrets::types::{SecretMetadata, SecretType};
use crate::secrets::SecretsStore;
use crate::util::internal_error;

const GITHUB_OAUTH_REGISTRY: &str = "github-oauth";
const GITHUB_ACCESS_TOKEN_SUFFIX: &str = "access-token";
const GITHUB_REFRESH_TOKEN_SUFFIX: &str = "refresh-token";
const GITHUB_AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const GITHUB_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const GITHUB_USER_URL: &str = "https://api.github.com/user";
const GITHUB_USER_EMAILS_URL: &str = "https://api.github.com/user/emails";
const GITHUB_DEFAULT_SCOPES: &str = "repo workflow read:user user:email";

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Claims {
    /// Subject (we only need a stable sentinel)
    sub: String,
    /// Username (for display/auditing)
    #[serde(default)]
    usr: String,
    /// Issued-at unix seconds
    iat: i64,
    /// Expiration unix seconds
    exp: i64,
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: String,
    pub username: String,
}

#[derive(Debug, Clone)]
pub struct PendingGithubOAuth {
    pub user: AuthUser,
    pub redirect_uri: String,
    pub created_at: chrono::DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct GithubOAuthCredentials {
    pub access_token: String,
    pub login: Option<String>,
    pub github_user_id: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
}

fn configured_single_tenant_user_id() -> Option<String> {
    std::env::var("SANDBOXED_SINGLE_TENANT_USER_ID")
        .or_else(|_| std::env::var("SINGLE_TENANT_USER_ID"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn implicit_single_tenant_user_from_id(
    config: &Config,
    configured_user_id: Option<String>,
) -> AuthUser {
    if config.dev_mode {
        return AuthUser {
            id: "dev".to_string(),
            username: "dev".to_string(),
        };
    }

    let id = configured_user_id.unwrap_or_else(|| "default".to_string());
    AuthUser {
        username: id.clone(),
        id,
    }
}

/// Resolve the effective single-tenant user identity.
///
/// By default, authenticated single-tenant deployments use `default`, while
/// dev-mode sessions use `dev`. Operators can override the production
/// single-tenant identity with `SANDBOXED_SINGLE_TENANT_USER_ID` (or the
/// shorter `SINGLE_TENANT_USER_ID`) to keep using a legacy mission partition.
pub fn implicit_single_tenant_user(config: &Config) -> AuthUser {
    implicit_single_tenant_user_from_id(config, configured_single_tenant_user_id())
}

pub(crate) fn constant_time_eq(a: &str, b: &str) -> bool {
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();
    if a_bytes.len() != b_bytes.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for i in 0..a_bytes.len() {
        diff |= a_bytes[i] ^ b_bytes[i];
    }
    diff == 0
}

/// Hash a password using PBKDF2-SHA256.
/// Returns a string in the format `pbkdf2:100000:<hex_salt>:<hex_hash>`.
pub fn hash_password(password: &str) -> String {
    use hmac::Hmac;
    use pbkdf2::pbkdf2;
    use rand::RngCore;
    use sha2::Sha256;

    let iterations = 100_000u32;
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);

    let mut hash = [0u8; 32];
    pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, iterations, &mut hash)
        .expect("PBKDF2 should not fail");

    format!(
        "pbkdf2:{}:{}:{}",
        iterations,
        hex::encode(salt),
        hex::encode(hash)
    )
}

/// Verify a password against a stored PBKDF2 hash string.
pub fn verify_password_hash(password: &str, stored: &str) -> bool {
    use hmac::Hmac;
    use pbkdf2::pbkdf2;
    use sha2::Sha256;

    let parts: Vec<&str> = stored.split(':').collect();
    if parts.len() != 4 || parts[0] != "pbkdf2" {
        return false;
    }

    let iterations: u32 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => return false,
    };
    let salt = match hex::decode(parts[2]) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let expected_hash = match hex::decode(parts[3]) {
        Ok(h) => h,
        Err(_) => return false,
    };

    let mut computed = vec![0u8; expected_hash.len()];
    if pbkdf2::<Hmac<Sha256>>(password.as_bytes(), &salt, iterations, &mut computed).is_err() {
        return false;
    }

    constant_time_eq(&hex::encode(&computed), &hex::encode(&expected_hash))
}

fn issue_jwt(secret: &str, ttl_days: i64, user: &AuthUser) -> anyhow::Result<(String, i64)> {
    let now = Utc::now();
    let exp = now + Duration::days(ttl_days.max(1));
    let claims = Claims {
        sub: user.id.clone(),
        usr: user.username.clone(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };
    let token = jsonwebtoken::encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok((token, claims.exp))
}

fn verify_jwt(token: &str, secret: &str) -> anyhow::Result<Claims> {
    let validation = Validation::default();
    let token_data = jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )?;
    Ok(token_data.claims)
}

/// Verify a JWT against the server config.
/// Returns true iff:
/// - auth is not required (dev mode), OR
/// - auth is required and the token is valid.
pub fn verify_token_for_config(token: &str, config: &Config) -> bool {
    if !config.auth.auth_required(config.dev_mode) {
        return true;
    }
    let secret = match config.auth.jwt_secret.as_deref() {
        Some(s) => s,
        None => return false,
    };
    let Ok(claims) = verify_jwt(token, secret) else {
        return false;
    };
    match config.auth.auth_mode(config.dev_mode) {
        AuthMode::MultiUser => user_for_claims(&claims, &config.auth.users).is_some(),
        AuthMode::SingleTenant => true,
        AuthMode::Disabled => true,
    }
}

pub async fn login(
    State(state): State<std::sync::Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, String)> {
    let auth_mode = state.config.auth.auth_mode(state.config.dev_mode);
    let user = match auth_mode {
        AuthMode::MultiUser => {
            let username = req.username.as_deref().unwrap_or("").trim();
            if username.is_empty() {
                return Err((StatusCode::UNAUTHORIZED, "Username required".to_string()));
            }
            // Find user and verify password. Use a single generic error message
            // for both invalid username and invalid password to prevent username enumeration.
            let account = state
                .config
                .auth
                .users
                .iter()
                .find(|u| u.username.trim() == username);

            let valid = match account {
                Some(acc) => {
                    !acc.password.trim().is_empty()
                        && constant_time_eq(req.password.trim(), acc.password.trim())
                }
                None => {
                    // Perform a dummy comparison to prevent timing attacks
                    let _ = constant_time_eq(req.password.trim(), "dummy_password_for_timing");
                    false
                }
            };

            if !valid {
                return Err((
                    StatusCode::UNAUTHORIZED,
                    "Invalid username or password".to_string(),
                ));
            }

            // account is guaranteed Some here: the None branch above sets valid=false,
            // and we returned early on !valid.
            let account = account.expect("account must be Some when valid is true");
            let effective_id = effective_user_id(account);

            AuthUser {
                id: effective_id,
                username: account.username.clone(),
            }
        }
        AuthMode::SingleTenant | AuthMode::Disabled => {
            // If dev_mode is enabled, we still allow login, but it won't be required.
            // Check dashboard-managed password hash first, then fall back to env var.
            let stored_auth = state.settings.get_auth_settings().await;
            let valid = if let Some(ref auth_settings) = stored_auth {
                if let Some(ref hash) = auth_settings.password_hash {
                    verify_password_hash(req.password.trim(), hash)
                } else {
                    // No stored hash — fall back to env var
                    let expected = state
                        .config
                        .auth
                        .dashboard_password
                        .as_deref()
                        .unwrap_or("");
                    !expected.is_empty() && constant_time_eq(req.password.trim(), expected)
                }
            } else {
                // No auth settings at all — fall back to env var
                let expected = state
                    .config
                    .auth
                    .dashboard_password
                    .as_deref()
                    .unwrap_or("");
                !expected.is_empty() && constant_time_eq(req.password.trim(), expected)
            };

            if !valid {
                return Err((StatusCode::UNAUTHORIZED, "Invalid password".to_string()));
            }

            implicit_single_tenant_user(&state.config)
        }
    };

    let secret = state.config.auth.jwt_secret.as_deref().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "JWT_SECRET not configured".to_string(),
        )
    })?;

    let (token, exp) =
        issue_jwt(secret, state.config.auth.jwt_ttl_days, &user).map_err(internal_error)?;

    Ok(Json(LoginResponse { token, exp }))
}

pub async fn require_auth(
    State(state): State<std::sync::Arc<AppState>>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    // Dev mode => no auth checks.
    if state.config.dev_mode {
        req.extensions_mut()
            .insert(implicit_single_tenant_user(&state.config));
        return next.run(req).await;
    }

    // If auth isn't configured, fail closed in non-dev mode.
    let secret = match state.config.auth.jwt_secret.as_deref() {
        Some(s) => s,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "JWT_SECRET not configured",
            )
                .into_response();
        }
    };

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let token = auth_header
        .strip_prefix("Bearer ")
        .or_else(|| auth_header.strip_prefix("bearer "))
        .unwrap_or("");

    if token.is_empty() {
        return (StatusCode::UNAUTHORIZED, "Missing Authorization header").into_response();
    }

    match verify_jwt(token, secret) {
        Ok(claims) => {
            let user = match state.config.auth.auth_mode(state.config.dev_mode) {
                AuthMode::MultiUser => match user_for_claims(&claims, &state.config.auth.users) {
                    Some(u) => u,
                    None => {
                        return (StatusCode::UNAUTHORIZED, "Invalid user").into_response();
                    }
                },
                AuthMode::SingleTenant => AuthUser {
                    id: claims.sub,
                    username: claims.usr,
                },
                AuthMode::Disabled => implicit_single_tenant_user(&state.config),
            };
            req.extensions_mut().insert(user);
            next.run(req).await
        }
        Err(_) => (StatusCode::UNAUTHORIZED, "Invalid or expired token").into_response(),
    }
}

/// Returns the effective user ID (id if non-empty, otherwise username).
fn effective_user_id(user: &UserAccount) -> String {
    if user.id.is_empty() {
        user.username.clone()
    } else {
        user.id.clone()
    }
}

fn user_for_claims(claims: &Claims, users: &[UserAccount]) -> Option<AuthUser> {
    users
        .iter()
        .find(|u| effective_user_id(u) == claims.sub)
        .map(|u| AuthUser {
            id: effective_user_id(u),
            username: u.username.clone(),
        })
}

// ─── Auth status & password change endpoints ─────────────────────────────

#[derive(Debug, serde::Serialize)]
pub struct AuthStatusResponse {
    pub auth_mode: String,
    pub password_source: String, // "dashboard", "environment", "none"
    pub password_changed_at: Option<String>,
    pub dev_mode: bool,
}

pub async fn auth_status(
    State(state): State<std::sync::Arc<AppState>>,
) -> Json<AuthStatusResponse> {
    let auth_mode = match state.config.auth.auth_mode(state.config.dev_mode) {
        AuthMode::Disabled => "disabled",
        AuthMode::SingleTenant => "single_tenant",
        AuthMode::MultiUser => "multi_user",
    };

    let stored_auth = state.settings.get_auth_settings().await;
    let has_stored_hash = stored_auth
        .as_ref()
        .and_then(|a| a.password_hash.as_ref())
        .is_some();
    let has_env_password = state.config.auth.dashboard_password.is_some();

    let password_source = if has_stored_hash {
        "dashboard"
    } else if has_env_password {
        "environment"
    } else {
        "none"
    };

    let password_changed_at = stored_auth.and_then(|a| a.password_changed_at);

    Json(AuthStatusResponse {
        auth_mode: auth_mode.to_string(),
        password_source: password_source.to_string(),
        password_changed_at,
        dev_mode: state.config.dev_mode,
    })
}

#[derive(Debug, serde::Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: Option<String>,
    pub new_password: String,
}

pub async fn change_password(
    State(state): State<std::sync::Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    // Multi-user mode: passwords are managed via SANDBOXED_USERS env var
    if state.config.auth.auth_mode(state.config.dev_mode) == AuthMode::MultiUser {
        return Err((
            StatusCode::BAD_REQUEST,
            "Password change is not available in multi-user mode. Manage passwords via the SANDBOXED_USERS environment variable.".to_string(),
        ));
    }

    // Determine whether a current password exists (stored hash or env var)
    let stored_auth = state.settings.get_auth_settings().await;
    let has_stored_hash = stored_auth
        .as_ref()
        .and_then(|a| a.password_hash.as_ref())
        .is_some();
    let has_env_password = state
        .config
        .auth
        .dashboard_password
        .as_ref()
        .map(|p| !p.is_empty())
        .unwrap_or(false);
    let has_existing_password = has_stored_hash || has_env_password;

    // If a password exists and auth is not disabled (dev mode), require the current password
    if has_existing_password && !state.config.dev_mode {
        let current = req.current_password.as_deref().unwrap_or("").trim();
        if current.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Current password is required".to_string(),
            ));
        }

        let current_valid = if has_stored_hash {
            let hash = stored_auth
                .as_ref()
                .expect("stored_auth must be Some when has_stored_hash is true")
                .password_hash
                .as_ref()
                .expect("password_hash must be Some when has_stored_hash is true");
            verify_password_hash(current, hash)
        } else {
            let expected = state
                .config
                .auth
                .dashboard_password
                .as_deref()
                .unwrap_or("");
            constant_time_eq(current, expected)
        };

        if !current_valid {
            return Err((
                StatusCode::UNAUTHORIZED,
                "Current password is incorrect".to_string(),
            ));
        }
    }

    // Validate new password
    let new_password = req.new_password.trim();
    if new_password.len() < 8 {
        return Err((
            StatusCode::BAD_REQUEST,
            "New password must be at least 8 characters".to_string(),
        ));
    }

    // Hash and persist
    let hashed = hash_password(new_password);
    let now = Utc::now().to_rfc3339();

    let auth_settings = crate::settings::AuthSettings {
        password_hash: Some(hashed),
        password_changed_at: Some(now.clone()),
    };

    state
        .settings
        .set_auth_settings(auth_settings)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to save password: {}", e),
            )
        })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "password_changed_at": now
    })))
}

// ─── GitHub OAuth for user-scoped bot/mission git access ───────────────────

fn github_oauth_client_id() -> Option<String> {
    std::env::var("GITHUB_OAUTH_CLIENT_ID")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn github_oauth_client_secret() -> Option<String> {
    std::env::var("GITHUB_OAUTH_CLIENT_SECRET")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn github_oauth_scopes() -> String {
    std::env::var("GITHUB_OAUTH_SCOPES")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| GITHUB_DEFAULT_SCOPES.to_string())
}

fn trim_trailing_slash(value: &str) -> String {
    value.trim_end_matches('/').to_string()
}

fn github_oauth_redirect_uri(config: &Config) -> String {
    if let Ok(uri) = std::env::var("GITHUB_OAUTH_REDIRECT_URI") {
        let trimmed = uri.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }

    if let Ok(public_url) = std::env::var("SANDBOXED_PUBLIC_URL") {
        let trimmed = public_url.trim();
        if !trimmed.is_empty() {
            return format!("{}/api/auth/github/callback", trim_trailing_slash(trimmed));
        }
    }

    format!(
        "http://{}:{}/api/auth/github/callback",
        config.host, config.port
    )
}

fn github_secret_key(user_id: &str, suffix: &str) -> String {
    format!(
        "{}-{}",
        crate::api::mission_store::sanitize_filename(user_id),
        suffix
    )
}

fn github_oauth_configured() -> bool {
    github_oauth_client_id().is_some() && github_oauth_client_secret().is_some()
}

async fn ensure_secrets_ready(secrets: &SecretsStore) -> Result<(), String> {
    if !secrets.is_initialized().await {
        secrets
            .initialize("default")
            .await
            .map_err(|e| format!("Failed to initialize secrets store: {}", e))?;
    }
    if !secrets.can_decrypt().await {
        return Err(
            "Secrets store is locked. Unlock it or set SANDBOXED_SECRET_PASSPHRASE before connecting GitHub."
                .to_string(),
        );
    }
    Ok(())
}

#[derive(Debug, serde::Serialize)]
pub struct GithubOAuthStatusResponse {
    pub configured: bool,
    pub connected: bool,
    pub can_decrypt: bool,
    pub login: Option<String>,
    pub github_user_id: Option<String>,
    pub name: Option<String>,
    pub email: Option<String>,
    pub scopes: Option<String>,
    pub connected_at: Option<String>,
    pub expires_at: Option<i64>,
    pub is_expired: bool,
    pub message: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct GithubOAuthAuthorizeResponse {
    pub url: String,
    pub state: String,
    pub redirect_uri: String,
    pub scopes: String,
}

pub async fn github_oauth_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Json<GithubOAuthStatusResponse> {
    let Some(secrets) = state.secrets.as_ref() else {
        return Json(GithubOAuthStatusResponse {
            configured: github_oauth_configured(),
            connected: false,
            can_decrypt: false,
            login: None,
            github_user_id: None,
            name: None,
            email: None,
            scopes: None,
            connected_at: None,
            expires_at: None,
            is_expired: false,
            message: Some("Secrets store is not available".to_string()),
        });
    };

    let can_decrypt = secrets.can_decrypt().await;
    let key = github_secret_key(&user.id, GITHUB_ACCESS_TOKEN_SUFFIX);
    let secret_info = secrets
        .list_secrets(GITHUB_OAUTH_REGISTRY)
        .await
        .ok()
        .and_then(|secrets| secrets.into_iter().find(|secret| secret.key == key));

    let Some(secret_info) = secret_info else {
        return Json(GithubOAuthStatusResponse {
            configured: github_oauth_configured(),
            connected: false,
            can_decrypt,
            login: None,
            github_user_id: None,
            name: None,
            email: None,
            scopes: None,
            connected_at: None,
            expires_at: None,
            is_expired: false,
            message: None,
        });
    };

    let labels = secret_info.labels;
    Json(GithubOAuthStatusResponse {
        configured: github_oauth_configured(),
        connected: !secret_info.is_expired,
        can_decrypt,
        login: labels.get("github_login").cloned(),
        github_user_id: labels.get("github_user_id").cloned(),
        name: labels.get("github_name").cloned(),
        email: labels.get("github_email").cloned(),
        scopes: labels.get("scopes").cloned(),
        connected_at: labels.get("connected_at").cloned(),
        expires_at: secret_info.expires_at,
        is_expired: secret_info.is_expired,
        message: secret_info
            .is_expired
            .then_some("GitHub OAuth token is expired; reconnect GitHub.".to_string()),
    })
}

pub async fn github_oauth_authorize(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<Json<GithubOAuthAuthorizeResponse>, (StatusCode, String)> {
    let client_id = github_oauth_client_id().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "GITHUB_OAUTH_CLIENT_ID is not configured".to_string(),
        )
    })?;
    let _client_secret = github_oauth_client_secret().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "GITHUB_OAUTH_CLIENT_SECRET is not configured".to_string(),
        )
    })?;

    let secrets = state.secrets.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Secrets store is not available".to_string(),
        )
    })?;
    ensure_secrets_ready(secrets)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let state_token = Uuid::new_v4().to_string();
    let redirect_uri = github_oauth_redirect_uri(&state.config);
    let scopes = github_oauth_scopes();

    {
        let mut pending = state.pending_github_oauth.write().await;
        let cutoff = Utc::now() - Duration::minutes(15);
        pending.retain(|_, value| value.created_at >= cutoff);
        pending.insert(
            state_token.clone(),
            PendingGithubOAuth {
                user,
                redirect_uri: redirect_uri.clone(),
                created_at: Utc::now(),
            },
        );
    }

    let mut url = url::Url::parse(GITHUB_AUTHORIZE_URL).map_err(internal_error)?;
    url.query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("scope", &scopes)
        .append_pair("state", &state_token)
        .append_pair("allow_signup", "true");

    Ok(Json(GithubOAuthAuthorizeResponse {
        url: url.to_string(),
        state: state_token,
        redirect_uri,
        scopes,
    }))
}

#[derive(Debug, serde::Deserialize)]
pub struct GithubOAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubTokenResponse {
    access_token: Option<String>,
    token_type: Option<String>,
    scope: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubUserResponse {
    id: i64,
    login: String,
    name: Option<String>,
    email: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct GithubEmailResponse {
    email: String,
    primary: bool,
    verified: bool,
}

pub async fn github_oauth_callback(
    State(state): State<Arc<AppState>>,
    Query(query): Query<GithubOAuthCallbackQuery>,
) -> Result<Html<String>, (StatusCode, String)> {
    if let Some(error) = query.error {
        let description = query.error_description.unwrap_or_default();
        return Ok(github_callback_html(
            "GitHub connection cancelled",
            &format!("{} {}", error, description).trim(),
            false,
        ));
    }

    let code = query
        .code
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing OAuth code".to_string()))?;
    let state_token = query
        .state
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing OAuth state".to_string()))?;

    let pending = state
        .pending_github_oauth
        .write()
        .await
        .remove(&state_token)
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                "OAuth state is invalid or expired".to_string(),
            )
        })?;

    let client_id = github_oauth_client_id().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "GITHUB_OAUTH_CLIENT_ID is not configured".to_string(),
        )
    })?;
    let client_secret = github_oauth_client_secret().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "GITHUB_OAUTH_CLIENT_SECRET is not configured".to_string(),
        )
    })?;
    let secrets = state.secrets.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Secrets store is not available".to_string(),
        )
    })?;
    ensure_secrets_ready(secrets)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    let token_response = state
        .http_client
        .post(GITHUB_TOKEN_URL)
        .header("Accept", "application/json")
        .header("User-Agent", "sandboxed.sh")
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("code", code.as_str()),
            ("redirect_uri", pending.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(internal_error)?;

    if !token_response.status().is_success() {
        let status = token_response.status();
        let text = token_response.text().await.unwrap_or_default();
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("GitHub token exchange failed ({}): {}", status, text),
        ));
    }

    let token_data: GithubTokenResponse = token_response.json().await.map_err(internal_error)?;
    if let Some(error) = token_data.error {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!(
                "GitHub token exchange failed: {} {}",
                error,
                token_data.error_description.unwrap_or_default()
            ),
        ));
    }
    let access_token = token_data.access_token.ok_or_else(|| {
        (
            StatusCode::BAD_GATEWAY,
            "GitHub did not return an access token".to_string(),
        )
    })?;

    let github_user = fetch_github_user(&state.http_client, &access_token)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    let email = if github_user
        .email
        .as_deref()
        .is_some_and(|v| !v.trim().is_empty())
    {
        github_user.email.clone()
    } else {
        fetch_github_primary_email(&state.http_client, &access_token)
            .await
            .ok()
            .flatten()
    };

    store_github_oauth(
        secrets,
        &pending.user,
        &access_token,
        token_data.refresh_token.as_deref(),
        token_data.expires_in,
        token_data.refresh_token_expires_in,
        token_data.scope.as_deref(),
        token_data.token_type.as_deref(),
        &github_user,
        email.as_deref(),
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    Ok(github_callback_html(
        "GitHub connected",
        "You can close this window and return to sandboxed.sh.",
        true,
    ))
}

fn github_callback_html(title: &str, message: &str, success: bool) -> Html<String> {
    let color = if success { "#34d399" } else { "#f87171" };
    let title = escape_html(title);
    let message = escape_html(message);
    Html(format!(
        r#"<!doctype html>
<html>
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{title}</title></head>
<body style="margin:0;background:#121214;color:white;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;display:grid;min-height:100vh;place-items:center">
  <main style="max-width:440px;padding:32px;text-align:center">
    <div style="width:48px;height:48px;border-radius:999px;background:{color};margin:0 auto 20px"></div>
    <h1 style="font-size:22px;margin:0 0 10px">{title}</h1>
    <p style="color:rgba(255,255,255,.68);line-height:1.5">{message}</p>
    <button onclick="window.close()" style="margin-top:18px;border:1px solid rgba(255,255,255,.14);background:rgba(255,255,255,.06);color:white;border-radius:8px;padding:10px 14px">Close</button>
  </main>
</body>
</html>"#
    ))
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

async fn fetch_github_user(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<GithubUserResponse, String> {
    let resp = client
        .get(GITHUB_USER_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "sandboxed.sh")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch GitHub user: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("GitHub user fetch failed ({}): {}", status, text));
    }

    resp.json()
        .await
        .map_err(|e| format!("Failed to parse GitHub user response: {}", e))
}

async fn fetch_github_primary_email(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<Option<String>, String> {
    let resp = client
        .get(GITHUB_USER_EMAILS_URL)
        .bearer_auth(access_token)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "sandboxed.sh")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch GitHub emails: {}", e))?;

    if !resp.status().is_success() {
        return Ok(None);
    }

    let emails: Vec<GithubEmailResponse> = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse GitHub emails response: {}", e))?;
    Ok(emails
        .into_iter()
        .find(|email| email.primary && email.verified)
        .map(|email| email.email))
}

#[allow(clippy::too_many_arguments)]
async fn store_github_oauth(
    secrets: &SecretsStore,
    user: &AuthUser,
    access_token: &str,
    refresh_token: Option<&str>,
    expires_in: Option<i64>,
    refresh_token_expires_in: Option<i64>,
    scopes: Option<&str>,
    token_type: Option<&str>,
    github_user: &GithubUserResponse,
    email: Option<&str>,
) -> Result<(), String> {
    let now = Utc::now();
    let expires_at = expires_in.map(|seconds| (now + Duration::seconds(seconds)).timestamp());
    let refresh_expires_at =
        refresh_token_expires_in.map(|seconds| (now + Duration::seconds(seconds)).timestamp());
    let connected_at = now.to_rfc3339();

    let mut labels = HashMap::new();
    labels.insert("provider".to_string(), "github".to_string());
    labels.insert("user_id".to_string(), user.id.clone());
    labels.insert("username".to_string(), user.username.clone());
    labels.insert("github_login".to_string(), github_user.login.clone());
    labels.insert("github_user_id".to_string(), github_user.id.to_string());
    labels.insert("connected_at".to_string(), connected_at);
    if let Some(name) = github_user.name.as_deref().filter(|v| !v.trim().is_empty()) {
        labels.insert("github_name".to_string(), name.to_string());
    }
    if let Some(email) = email.filter(|v| !v.trim().is_empty()) {
        labels.insert("github_email".to_string(), email.to_string());
    }
    if let Some(scopes) = scopes.filter(|v| !v.trim().is_empty()) {
        labels.insert("scopes".to_string(), scopes.to_string());
    }
    if let Some(token_type) = token_type.filter(|v| !v.trim().is_empty()) {
        labels.insert("token_type".to_string(), token_type.to_string());
    }

    secrets
        .set_secret(
            GITHUB_OAUTH_REGISTRY,
            &github_secret_key(&user.id, GITHUB_ACCESS_TOKEN_SUFFIX),
            access_token,
            Some(SecretMetadata {
                secret_type: Some(SecretType::OAuthAccessToken),
                expires_at,
                labels,
            }),
        )
        .await
        .map_err(|e| format!("Failed to store GitHub access token: {}", e))?;

    if let Some(refresh_token) = refresh_token.filter(|v| !v.trim().is_empty()) {
        let mut refresh_labels = HashMap::new();
        refresh_labels.insert("provider".to_string(), "github".to_string());
        refresh_labels.insert("user_id".to_string(), user.id.clone());
        refresh_labels.insert("github_login".to_string(), github_user.login.clone());
        secrets
            .set_secret(
                GITHUB_OAUTH_REGISTRY,
                &github_secret_key(&user.id, GITHUB_REFRESH_TOKEN_SUFFIX),
                refresh_token,
                Some(SecretMetadata {
                    secret_type: Some(SecretType::OAuthRefreshToken),
                    expires_at: refresh_expires_at,
                    labels: refresh_labels,
                }),
            )
            .await
            .map_err(|e| format!("Failed to store GitHub refresh token: {}", e))?;
    }

    Ok(())
}

pub async fn github_oauth_disconnect(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    let secrets = state.secrets.as_ref().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "Secrets store is not available".to_string(),
        )
    })?;

    for suffix in [GITHUB_ACCESS_TOKEN_SUFFIX, GITHUB_REFRESH_TOKEN_SUFFIX] {
        let key = github_secret_key(&user.id, suffix);
        if let Err(e) = secrets.delete_secret(GITHUB_OAUTH_REGISTRY, &key).await {
            tracing::debug!("GitHub OAuth secret {} was not deleted: {}", key, e);
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

pub async fn github_oauth_credentials_for_user(
    secrets: Option<&Arc<SecretsStore>>,
    user_id: &str,
) -> Option<GithubOAuthCredentials> {
    let secrets = secrets?;
    let key = github_secret_key(user_id, GITHUB_ACCESS_TOKEN_SUFFIX);
    let info = secrets
        .list_secrets(GITHUB_OAUTH_REGISTRY)
        .await
        .ok()?
        .into_iter()
        .find(|secret| secret.key == key)?;
    if info.is_expired {
        return None;
    }
    let access_token = secrets.get_secret(GITHUB_OAUTH_REGISTRY, &key).await.ok()?;
    let labels = info.labels;
    Some(GithubOAuthCredentials {
        access_token,
        login: labels.get("github_login").cloned(),
        github_user_id: labels.get("github_user_id").cloned(),
        name: labels.get("github_name").cloned(),
        email: labels.get("github_email").cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AuthConfig, ContextConfig};
    use std::path::PathBuf;

    fn test_config(dev_mode: bool) -> Config {
        Config {
            default_model: None,
            working_dir: PathBuf::from("/tmp"),
            host: "127.0.0.1".to_string(),
            port: 3000,
            max_iterations: 50,
            stale_mission_hours: 0,
            max_parallel_missions: 1,
            dev_mode,
            auth: AuthConfig::default(),
            context: ContextConfig::default(),
            opencode_base_url: "http://127.0.0.1:4096".to_string(),
            opencode_agent: None,
            opencode_permissive: false,
            library_path: PathBuf::from("/tmp/library"),
            default_backend: None,
            automations_enabled: true,
            max_concurrent_tasks: 5,
        }
    }

    #[test]
    fn implicit_single_tenant_user_defaults_to_default_outside_dev_mode() {
        let user = implicit_single_tenant_user_from_id(&test_config(false), None);
        assert_eq!(user.id, "default");
        assert_eq!(user.username, "default");
    }

    #[test]
    fn implicit_single_tenant_user_honors_override() {
        let user =
            implicit_single_tenant_user_from_id(&test_config(false), Some("legacy-prod".into()));
        assert_eq!(user.id, "legacy-prod");
        assert_eq!(user.username, "legacy-prod");
    }
}
