# Sandboxed.sh Android Dashboard

Native Android client for Sandboxed.sh, with feature parity against the iOS dashboard. SwiftUI ↔ Jetpack Compose port; talks to the same `/api/...` backend.

## Install

The Android app is published on Zapstore:

https://zapstore.dev/apps/sh.sandboxed.dashboard

## What's in v0.2.0

### Bottom-tab screens

| Tab | Purpose |
| --- | --- |
| **Control** | Streaming chat with the agent, parallel-mission switcher, message queue, goal-mode banner, mission metadata |
| **Missions** | Mission history with status filters, full-text search across missions and per-message moments, pull-to-refresh, cleanup-completed |
| **Terminal** | WebSocket terminal with ANSI-color rendering and per-workspace shell selector |
| **Files** | Server file browser: list, upload (`GetContent`), download (FileProvider + `ACTION_VIEW`), mkdir, delete |
| **More** | Hub linking to Workspaces, Tasks, Runs, FIDO approvals, Settings |

### Reachable from More

- **Workspaces** — list / create (host or container), status badges, skill chips
- **Tasks** — subtasks from `/api/tasks` with status colours
- **Runs** — cost-tracked invocations from `/api/runs`, header total in dollars
- **FIDO approvals** — auto-approval rules (All SSH / Hostname / Fingerprint), per-rule expiry (1h / 24h / 7d / never), per-rule and global biometric requirement
- **Settings** — server URL test + save, sign-out, default backend / agent, providers list, built-in slash commands library

### Reachable from Control

- **Automations** (per mission) — list / create / toggle active / delete, with `interval` / `agent_finished` / `webhook` triggers

### Global overlays

- **Auth gate** — probes `/api/health`, supports `disabled`, `single_tenant`, `multi_user`; JWT stored in DataStore; auto-shown when not configured or unauthenticated
- **FIDO approval dialog** — surfaces non-auto-approved sign requests; on Approve, runs `BiometricPrompt` with `BIOMETRIC_WEAK | DEVICE_CREDENTIAL` and POSTs `/api/fido/respond`

## Tech stack

| Layer | Choice |
| --- | --- |
| Language | Kotlin 2.0.21 |
| Build | AGP 8.9.1, Gradle 8.11.1 |
| SDK | `compileSdk` 36, `targetSdk` 36, `minSdk` 26 (Android 8.0) |
| UI | Jetpack Compose (BOM 2024.12.01), Material 3 with `material-icons-extended` |
| Navigation | `androidx.navigation:navigation-compose` 2.9.8 |
| State | ViewModel + StateFlow, Compose `collectAsState` |
| Persistence | `androidx.datastore:datastore-preferences` 1.2.1 |
| Networking | OkHttp 4.12 (HTTP, SSE via `okhttp-sse`, WebSocket built-in) |
| JSON | `kotlinx-serialization-json` 1.7.3 |
| Coroutines | `kotlinx-coroutines-android` 1.9.0 |
| Auth | `androidx.biometric:biometric` 1.1.0 hosted by `FragmentActivity` (`androidx.fragment:fragment-ktx` 1.8.9) |
| Images | `coil-compose` 2.7.0 |
| DI | Hand-rolled — single `AppContainer` held by `Application` |

## Project layout

```
android_dashboard/
├── build.gradle.kts                  root project file
├── settings.gradle.kts
├── gradle.properties
├── local.properties                  (sdk.dir; not committed in real repos)
├── gradle/wrapper/                   wrapper jar fetched on first sync
├── keys/release.jks                  throwaway dev keystore (replace before publishing)
└── app/
    ├── build.gradle.kts              app module
    ├── proguard-rules.pro
    └── src/main/
        ├── AndroidManifest.xml
        ├── res/                      themes, colors, network_security_config, file_paths,
        │                              data_extraction_rules, backup_rules, adaptive launcher
        └── java/sh/sandboxed/dashboard/
            ├── SandboxedDashboardApp.kt   Application — owns AppContainer
            ├── MainActivity.kt            FragmentActivity host (needed by BiometricPrompt)
            ├── data/
            │   ├── AppContainer.kt        manual DI graph
            │   ├── Settings.kt            DataStore-backed AppSettings
            │   ├── Models.kt              Mission, FileEntry, Workspace, Backend, Provider,
            │   │                          Run, TaskState, Automation, FidoSignRequest,
            │   │                          AutoApprovalRule, ToolUiContent + ToolUiParser, …
            │   ├── ChatMessage.kt         UI-side ChatMessage / ChatMessageKind sealed types
            │   ├── FidoChannel.kt         global SSE listener, applies FIDO rules
            │   └── api/
            │       ├── ApiClient.kt       OkHttp clients + JSON config
            │       ├── ApiService.kt      every /api endpoint used
            │       ├── SseClient.kt       /api/control/stream EventSource wrapper
            │       └── TerminalSocket.kt  WebSocket terminal protocol
            ├── ui/
            │   ├── theme/                 Palette + MaterialTheme overrides (dark-first)
            │   ├── components/            GlassCard, StatusBadge, ErrorBanner, ToolUiWidgets
            │   ├── nav/AppRoot.kt         AuthGate + bottom tabs + nav graph + FidoOverlay
            │   ├── auth/AuthGate.kt       health probe → config / login / authenticated
            │   ├── control/               Control screen + ControlViewModel
            │   ├── history/               Missions list with search and pull-to-refresh
            │   ├── terminal/              Terminal screen with ANSI rendering
            │   ├── files/                 Files screen with upload/download/mkdir/delete
            │   ├── workspaces/            Workspaces screen
            │   ├── tasks/                 Tasks screen
            │   ├── runs/                  Runs screen
            │   ├── automations/           Automations CRUD (per mission)
            │   ├── fido/                  FidoOverlay + FidoRulesScreen
            │   ├── more/                  More hub
            │   └── settings/              Settings screen
            └── util/
                ├── Ansi.kt                SGR escape sequence parser → AnnotatedString
                └── Haptics.kt             VibrationEffect-based haptics
```

## Backend contract

The client targets the same Sandboxed.sh HTTP/SSE/WebSocket contract as the iOS app.

### HTTP (`ApiService`)

| Area | Endpoints |
| --- | --- |
| Health / Auth | `GET /api/health` · `POST /api/auth/login` |
| Missions | `GET/POST /api/control/missions` · `GET/POST /api/control/missions/{id}` · `…/load` · `…/status` · `…/resume` · `…/cancel` · `DELETE …` · `…/cleanup` · `…/current` |
| Mission events | `GET /api/control/missions/{id}/events?since_seq=&limit=&latest=&types=` (returns `X-Max-Sequence`) |
| Search | `GET /api/control/missions/search` · `…/search/moments` |
| Chat / queue | `POST /api/control/message` · `…/cancel` · `GET /api/control/queue` · `DELETE /api/control/queue/{id}` · `DELETE /api/control/queue` |
| Parallel | `GET /api/control/running` · `…/parallel/config` · `POST /api/control/missions/{id}/parallel` |
| Files | `GET /api/fs/list?path=` · `POST /api/fs/mkdir` · `POST /api/fs/rm` · `GET /api/fs/download?path=` · `POST /api/fs/upload?path=` (multipart `file`) |
| Workspaces | `GET /api/workspaces` · `GET /api/workspaces/{id}` · `POST /api/workspaces` |
| Backends | `GET /api/backends` · `…/{id}/agents` |
| Providers / library | `GET /api/providers?include_all=` · `GET /api/library/builtin-commands` |
| Tasks / Runs | `GET /api/tasks` · `GET /api/runs?limit=&offset=` |
| FIDO | `POST /api/fido/respond` |
| Automations | `GET/POST /api/control/missions/{id}/automations` · `PATCH /api/control/automations/{id}` · `DELETE …` |

### SSE — `GET /api/control/stream`

Wrapped by `SseClient` (OkHttp `EventSources`). Exponential reconnect (1s → 30s) is implemented inside the consumers (`ControlViewModel`, `FidoChannel`).

`ControlViewModel.handle(SseEvent)` maps event types:

| Event type | UI effect |
| --- | --- |
| `user_message` | Append user bubble |
| `assistant_message` | Append assistant bubble (`SharedFile` chips, model + cost footer with cost-source icon) |
| `text_delta` | Concatenate to last assistant bubble |
| `thinking` | Upsert collapsible thinking note (with `done` flag) |
| `agent_phase` | Inline phase note |
| `tool_call` / `tool_result` | Tool invocation card with active spinner |
| `tool_ui` | Parsed by `ToolUiParser` and rendered via `ToolUiWidgets` (data table / option list / progress / alert / code block / unknown fallback) |
| `goal_iteration` | Goal iteration row in chat |
| `goal_status` | Goal banner above chat (`active` / `paused` / `budgetLimited` / `complete` / `cleared`) |
| `mission_status_changed` / `mission_title_changed` | Update mission top-bar |
| `fido_sign_request` | Routed to `FidoChannel` for rule-matching / overlay |
| `error` | Red banner |

#### Delta resume

On every reconnect the ViewModel calls `GET /api/control/missions/{id}/events?since_seq=N` first to replay missed events, then opens the live SSE stream. The high-water-mark `N` comes from the `X-Max-Sequence` response header.

### WebSocket terminal

`TerminalSocket` connects to:

- `wss://<base>/api/console/ws` (default host workspace), or
- `wss://<base>/api/workspaces/{id}/shell` when a workspace is selected.

Subprotocols header: `sandboxed, jwt.<token>`. Frames:

| Direction | Shape |
| --- | --- |
| Client → server | `{"t":"i","d":"<input>"}` (input) · `{"t":"r","c":<cols>,"r":<rows>}` (resize) |
| Server → client | UTF-8 text or binary (passed through ANSI parser) |

Resize is sent on connect and whenever `LocalWindowInfo.containerSize` changes (rotation / split-screen).

## FIDO approvals

Two layers:

1. **Server-driven prompts** — every `fido_sign_request` SSE event is captured by `FidoChannel`. If a non-expired `AutoApprovalRule` matches and neither `rule.requireBiometric` nor the global `fidoRequireBiometricAll` is on, the channel silently POSTs `/api/fido/respond {approved:true}` without showing UI. Otherwise the request is enqueued.

2. **`FidoOverlay`** — a global Compose dialog that shows the next pending request (origin, hostname, workspace, key type, fingerprint). Tapping **Approve** runs `BiometricPrompt` (Weak biometric or device credential); on success, POSTs `…approved:true`. Tapping **Deny** posts `…approved:false`. Both then call `FidoChannel.resolve(requestId)` to clear the queue.

`FidoRulesScreen` (More → FIDO approvals) is the management UI: add / delete rules, toggle global biometric. Rules persist as a JSON array under DataStore key `fido_auto_approval_rules` (same key as iOS).

## Auth flow

1. App launches, reads `AppSettings` from DataStore.
2. If `baseUrl` is blank → `ConfigSheet` (server URL).
3. Otherwise `GET /api/health`. If `auth_required=false` or `auth_mode=disabled` → straight in.
4. Otherwise show `LoginScreen` (username + password if `multi_user`, password only if `single_tenant`).
5. `POST /api/auth/login` returns `{token, exp}`; token is stored in DataStore and sent as `Authorization: Bearer <token>` on subsequent requests.

`Settings → Sign out` clears the token.

### Sign in with GitHub (Android side; backend stub pending)

The Android client supports a "Sign in with GitHub" button on the login screen, gated on the server reporting `github_enabled: true` from `/api/health`. The client side is wired end-to-end; the matching server routes need to be added to `src/api/auth.rs` to make it functional.

**Backend contract** (Android assumes this — implement to match):

| Endpoint | Behaviour |
| --- | --- |
| `GET /api/health` | Add `github_enabled: bool` to the response when a GitHub OAuth App is configured. |
| `GET /api/auth/github/start?redirect=<uri>` | Validate that `redirect` matches an allow-list (the only entry the app sends is `sandboxed://auth/callback`), set a state cookie, and 302 to GitHub's `/login/oauth/authorize?client_id=…&state=…&redirect_uri=…&scope=read:user`. |
| `GET /api/auth/github/callback?code=&state=` | Verify state against the cookie, exchange `code` with GitHub for an access token, fetch `/user`, look up or provision a `UserAccount` (optionally gate on a configured `github_login_allowlist`), issue a JWT, then 302 to the saved `redirect` with `?token=<jwt>&exp=<unix_ts>` (or `?error=<message>` on failure). |

**Android side** (already implemented):

- `AndroidManifest.xml` declares an intent-filter for `sandboxed://auth/callback` on `MainActivity` (`launchMode="singleTask"` so deep links route via `onNewIntent`).
- `util/GitHubAuth.kt` opens the Custom Tab pointed at `<baseUrl>/api/auth/github/start?redirect=sandboxed%3A%2F%2Fauth%2Fcallback`.
- `MainActivity.handleAuthIntent` parses the callback URI's `token` query parameter and writes it to DataStore — `AuthGate` observes settings and switches the phase to `AUTHENTICATED` automatically.
- The button is rendered by `LoginScreen` only when `health.github_enabled` is `true`, so deployments without the OAuth App configured see the password flow unchanged.

**Permissions / scopes**: the only GitHub OAuth scope the client needs is `read:user` (display name, login). Add `user:email` if you want the verified email on the server.

**Allowlist**: it's worth gating the callback by a configurable list of GitHub usernames or org membership before issuing a JWT, otherwise the OAuth route becomes a public sign-up endpoint. The Android client doesn't care — it just receives a JWT or an error message.

## Persistent settings (DataStore)

Defined in `Settings.kt`:

| Key | Type | Purpose |
| --- | --- | --- |
| `api_base_url` | String | Server URL |
| `jwt_token` | String? | Bearer token |
| `last_username` | String | Multi-user mode remembered username |
| `default_agent` | String | Sent on `createMission` |
| `default_backend` | String | Sent on `createMission` |
| `skip_agent_selection` | Boolean | Reserved for an inline agent picker |
| `control_draft_text` | String | Composer draft persistence |
| `control_last_mission_id` | String? | Last viewed mission |
| `fido_auto_approval_rules` | JSON list | `AutoApprovalRule` records |
| `fido_require_biometric_all` | Boolean | Global biometric gate |

## Building

### From Android Studio

Open the `android_dashboard/` directory in Android Studio (Hedgehog or newer). The first sync downloads the Gradle wrapper jar and dependencies automatically.

### From the CLI

A keystore is included for development. To build a signed release APK:

```bash
cd android_dashboard
export RELEASE_KEYSTORE=$(pwd)/keys/release.jks
export RELEASE_KEYSTORE_PASSWORD=android
export RELEASE_KEY_ALIAS=sandboxed
export RELEASE_KEY_PASSWORD=android
./gradlew :app:assembleRelease
```

Output: `app/build/outputs/apk/release/app-release.apk` (~2.4 MB after R8 + resource shrink).

For a debug APK that just installs:

```bash
./gradlew :app:assembleDebug
# app/build/outputs/apk/debug/app-debug.apk
```

The release `signingConfig` only kicks in if `RELEASE_KEYSTORE` is set; without it, `assembleRelease` produces an unsigned APK.

## Release to Zapstore

Zapstore metadata lives in `zapstore.yaml`. The published app page is:

https://zapstore.dev/apps/sh.sandboxed.dashboard

### Prerequisites

- `~/go/bin/zsp` is installed.
- The release APK exists at `app/build/outputs/apk/release/app-release.apk`.
- The zsp bunker pairing from Oubli is present locally. The paired bunker pubkey is:
  `7ebbce1843a17cd778a5e169e3d2f679f5ac7b5125d1c43d265e190f7b27538c`

zsp stores the local client key for that bunker under the user config directory
(`~/Library/Application Support/zsp/bunker-keys/` on macOS). Do not commit bunker
URLs that include a `secret=` parameter or any Nostr private key.

### Publish

Build the signed release APK first:

```bash
cd android_dashboard
source keys/release-secrets.env
./gradlew :app:assembleRelease
```

Validate that zsp can read the APK and config:

```bash
~/go/bin/zsp publish --check zapstore.yaml
```

Publish with the same bunker signer used by Oubli:

```bash
SIGN_WITH="bunker://7ebbce1843a17cd778a5e169e3d2f679f5ac7b5125d1c43d265e190f7b27538c?relay=wss://relay.nsec.app" \
  ~/go/bin/zsp publish -y --skip-preview --skip-certificate-linking zapstore.yaml
```

Approve the signing requests in the remote signer if prompted. A successful run
ends with:

```text
Published sh.sandboxed.dashboard <version> to wss://relay.zapstore.dev
```

If you need to republish the same version after changing metadata or assets, add
`--overwrite-release`.

### Lint

```bash
./gradlew :app:lintDebug
# app/build/reports/lint-results-debug.{txt,html}
```

`abortOnError = false` is set so lint never blocks a build, but the current source is at **0 errors / 0 warnings**.

## Replacing the dev keystore

The keystore at `keys/release.jks` is throwaway (alias `sandboxed`, store/key password `android`, valid 100 years). For Play Store distribution, generate your own:

```bash
keytool -genkeypair -v \
  -keystore release.jks -alias sandboxed \
  -keyalg RSA -keysize 2048 -validity 9125 \
  -dname "CN=Your Org, ..."
```

Then export the matching `RELEASE_*` env vars and `assembleRelease` will pick up your config (the build script reads from env, never hard-codes secrets). Keep the keystore out of source control.

## Network security

`res/xml/network_security_config.xml` permits cleartext (`http://`) and trusts user-installed CAs — both intentional, for self-hosted servers on a LAN or with self-signed certs. The corresponding lint warnings are suppressed via `tools:ignore="InsecureBaseConfiguration,AcceptsUserCertificates"` with an explanatory comment.

## Design system

- Dark-first, `#121214` background
- `#6366F1` indigo accent (matches iOS)
- Glass-morphism cards (`GlassCard` component) on `#1C1C1C` with a 6 % white border
- Semantic colors: `#22C55E` success, `#EAB308` warning, `#EF4444` error, `#3B82F6` info
- Typography: SF Pro analog (Compose default sans-serif) for UI, monospace for terminal / tool args / fingerprints

All tokens live in `ui/theme/Color.kt`.

## Known gaps vs iOS

- Interactive `/goal` controls (pause / resume / clear) — banner reflects status but no buttons yet.
- "Sign in with GitHub" — Android side is wired (Custom Tab + deep-link callback handler); the matching `/api/auth/github/{start,callback}` routes still need to be added to the Rust backend.

## License

Same as the parent Sandboxed.sh project.

---

_Generated documentation; please verify before publishing externally._
