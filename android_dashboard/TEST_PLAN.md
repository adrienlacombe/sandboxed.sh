# Android Dashboard Simulator Test Plan

This plan is written for an LLM or human tester driving the Android app through
an Android Emulator with `adb`. It avoids hard-coded screen coordinates where
possible and treats backend-dependent features separately from app regressions.

## Scope

Validate the native Android dashboard at `android_dashboard/` against a running
Sandboxed.sh backend:

- First-run server configuration and auth gate.
- Bottom-tab navigation: Control, Missions, Terminal, Files, More.
- Secondary screens: Workspaces, Desktop, Tasks, Runs, FIDO approvals, Settings.
- Control flows: mission creation, streaming messages, mission switcher,
  automations, queue/resume/cancel/delete UI where data exists.
- WebSocket/SSE behavior: terminal, control stream, reconnect/error states.
- Persistence, rotation, background/foreground, and crash-free behavior.

Do not use production data unless explicitly authorized. Prefer a disposable
local or staging backend.

## Required Tools

- Android Studio or Android command-line tools.
- One running Android Emulator, API 33 or newer required; API 35 or newer preferred.
- `adb` on `PATH`.
- Java 17.
- A reachable Sandboxed.sh backend.
- Optional but recommended: Docker Compose for a disposable local backend.

## Environment Variables

Set these before testing:

```bash
export APP_ID=sh.sandboxed.dashboard
export RUN_ID="llm-android-$(date +%Y%m%d-%H%M%S)"
export ARTIFACT_DIR="$PWD/android_dashboard/test-artifacts/$RUN_ID"
mkdir -p "$ARTIFACT_DIR"

# Host URL used by curl from the test machine.
export BACKEND_URL_HOST="http://127.0.0.1:3000"

# URL entered inside the Android Emulator. 10.0.2.2 maps to the host machine.
export BACKEND_URL_APP="http://10.0.2.2:3000"

# Optional: only set when a test backend uses password auth.
export SANDBOXED_TEST_PASSWORD=""
```

If testing a remote backend, set both URLs to the remote HTTPS origin.

## Required Preflight And Diagnostics

Run this before building or launching. Save every output file; the final report
must cite the relevant artifact for each failure.

```bash
adb version | tee "$ARTIFACT_DIR/adb-version.txt"
emulator -list-avds | tee "$ARTIFACT_DIR/avd-names.txt" || true
avdmanager list avd | tee "$ARTIFACT_DIR/avd-list.txt" || true

if grep -E "could not be loaded|no longer exists|Error:" "$ARTIFACT_DIR/avd-list.txt"; then
  echo "ENV_BLOCKED: at least one configured AVD is stale or invalid" \
    | tee "$ARTIFACT_DIR/avd-invalid.txt"
fi
```

If testing more than one Android release, run the plan once on the stable AVD
and once on the newest valid AVD. Do not use an AVD reported as invalid by
`avdmanager list avd`; delete and recreate it first.

Before `pm clear` or `am start`, verify the emulator user is unlocked. A locked
Direct Boot user can make Android report a misleading launcher error such as
`Activity class ... MainActivity does not exist` even though the APK manifest is
valid.

```bash
unlock_emulator() {
  adb wait-for-device
  adb shell input keyevent KEYCODE_WAKEUP || true
  adb shell wm dismiss-keyguard || true
  adb shell input swipe 540 2100 540 300 1000 || true
  sleep 2
  adb shell dumpsys user | tee "$ARTIFACT_DIR/user-state.txt"
  adb shell dumpsys window | tee "$ARTIFACT_DIR/window-state.txt"
  if grep -q "RUNNING_LOCKED" "$ARTIFACT_DIR/user-state.txt"; then
    adb exec-out screencap -p > "$ARTIFACT_DIR/locked-user.png"
    echo "ENV_BLOCKED: emulator user is RUNNING_LOCKED; unlock, wipe, or recreate the AVD before app launch" \
      | tee "$ARTIFACT_DIR/locked-user-blocker.txt"
    return 1
  fi
}

collect_launch_diagnostics() {
  adb shell pm path "$APP_ID" | tee "$ARTIFACT_DIR/pm-path.txt" || true
  adb shell dumpsys package "$APP_ID" > "$ARTIFACT_DIR/dumpsys-package.txt" || true
  adb shell dumpsys user > "$ARTIFACT_DIR/dumpsys-user.txt" || true
  adb shell dumpsys window > "$ARTIFACT_DIR/dumpsys-window.txt" || true
  adb shell logcat -d -t 1000 > "$ARTIFACT_DIR/launch-logcat.txt" || true
  if command -v apkanalyzer >/dev/null; then
    apkanalyzer manifest print android_dashboard/app/build/outputs/apk/debug/app-debug.apk \
      > "$ARTIFACT_DIR/apk-manifest.txt" || true
    apkanalyzer dex packages android_dashboard/app/build/outputs/apk/debug/app-debug.apk \
      > "$ARTIFACT_DIR/apk-dex-packages.txt" || true
  fi
}
```

## Backend Setup

Choose one backend target.

### Option A: Existing Backend

```bash
curl -fsS "$BACKEND_URL_HOST/api/health" | tee "$ARTIFACT_DIR/health.json"
```

Pass criteria:

- JSON includes `status`.
- Note `auth_required`, `auth_mode`, and `github_enabled`; these determine the
  auth cases to run.

If auth is enabled and credentials were provided for the run, also create a
host-side token for API diagnostics:

```bash
if [ -n "${SANDBOXED_TEST_PASSWORD:-}" ]; then
  export BACKEND_TOKEN="$(
    curl -fsS -X POST "$BACKEND_URL_HOST/api/auth/login" \
      -H 'content-type: application/json' \
      -d "{\"password\":\"$SANDBOXED_TEST_PASSWORD\"}" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin).get("token",""))'
  )"
  test -n "$BACKEND_TOKEN" && echo "token acquired" > "$ARTIFACT_DIR/token-state.txt"
fi
```

Do not write credentials into the report or artifact filenames.

### Option B: Disposable Local Docker Backend

```bash
cp -n .env.example .env
docker compose up -d --build
curl -fsS "$BACKEND_URL_HOST/api/health" | tee "$ARTIFACT_DIR/health.json"
```

For the default `.env.example`, `DEV_MODE=true`, so Android should enter the app
after saving the server URL without showing a login form.

## Build And Install

```bash
cd android_dashboard
./gradlew :app:assembleDebug
cd ..

adb wait-for-device
unlock_emulator
adb install -r android_dashboard/app/build/outputs/apk/debug/app-debug.apk \
  | tee "$ARTIFACT_DIR/install.txt"
adb shell pm clear "$APP_ID" | tee "$ARTIFACT_DIR/pm-clear.txt" || true
unlock_emulator
adb shell am start -W -n "$APP_ID/.MainActivity" \
  | tee "$ARTIFACT_DIR/am-start.txt" || true

if grep -E "Error type|does not exist|Exception|Status: error" "$ARTIFACT_DIR/am-start.txt"; then
  collect_launch_diagnostics
  echo "FAIL: launch failed; see launch diagnostics" >&2
fi
```

Capture the initial state:

```bash
adb exec-out screencap -p > "$ARTIFACT_DIR/00-launch.png"
adb logcat -c
```

## ADB UI Protocol For LLMs

After every navigation or submit action:

1. Wait 1-3 seconds.
2. Dump the UI.
3. Search the dump for expected text or content descriptions.
4. Capture a screenshot for failures or visually significant passes.

Use this helper to inspect the current UI:

```bash
dump_ui() {
  adb shell uiautomator dump /sdcard/window.xml >/dev/null
  adb exec-out cat /sdcard/window.xml > "$ARTIFACT_DIR/window.xml"
  python3 - "$ARTIFACT_DIR/window.xml" <<'PY'
import sys, xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
for n in root.iter("node"):
    label = n.attrib.get("text") or n.attrib.get("content-desc")
    if label:
        print(f"{label}\t{n.attrib.get('bounds')}")
PY
}
```

If `uiautomator dump` fails or creates an empty text artifact, do not
immediately classify the app as crashed. Save the dump stderr, wait 2 seconds,
retry once, and also capture focus/lifecycle state:

```bash
adb shell dumpsys activity activities > "$ARTIFACT_DIR/dumpsys-activities.txt" || true
adb shell dumpsys window > "$ARTIFACT_DIR/dumpsys-window-current.txt" || true
adb exec-out screencap -p > "$ARTIFACT_DIR/uiautomator-fallback.png" || true
```

`UiAutomationService ... already registered` in logcat is Android test harness
noise from overlapping `uiautomator` invocations unless the foreground package
or fatal process is `sh.sandboxed.dashboard`.

Optional helper to tap the first node whose text or content description contains
a label. Inspect `dump_ui` first when labels are ambiguous.

```bash
tap_label() {
  local needle="$1"
  dump_ui >/dev/null
  local xy
  xy=$(python3 - "$ARTIFACT_DIR/window.xml" "$needle" <<'PY'
import re, sys, xml.etree.ElementTree as ET
xml_path, needle = sys.argv[1], sys.argv[2].lower()
root = ET.parse(xml_path).getroot()
for n in root.iter("node"):
    label = (n.attrib.get("text") or n.attrib.get("content-desc") or "").lower()
    if needle not in label:
        continue
    m = re.match(r"\[(\d+),(\d+)\]\[(\d+),(\d+)\]", n.attrib.get("bounds", ""))
    if not m:
        continue
    x1, y1, x2, y2 = map(int, m.groups())
    print((x1 + x2) // 2, (y1 + y2) // 2)
    raise SystemExit
raise SystemExit(1)
PY
)
  test -n "$xy" || { echo "label not found: $needle" >&2; return 1; }
  adb shell input tap $xy
}
```

Do not assume coordinates from this document. Use the current `bounds` from
`dump_ui`, then tap the center of the target bounds:

```bash
adb shell input tap X Y
```

For text entry, tap the field, clear existing text with the emulator keyboard if
needed, then type:

```bash
adb shell input text "text-to-enter"
adb shell input keyevent ENTER
```

If `adb shell input text` mishandles punctuation in URLs, type the URL manually
through the emulator keyboard or paste through the emulator UI.

## Investigation Log Requirements

Every failed or blocked test case must add a short entry to
`$ARTIFACT_DIR/investigation.log` with this format:

```text
[CASE] Classification: APP_FAIL | BACKEND_FAIL | ENV_BLOCKED | N/A
Expected:
Actual:
Evidence files:
Likely owner:
Suggested fix:
```

Minimum diagnostics by failure type:

- Launch failure: `am-start.txt`, `dumpsys-user.txt`, `dumpsys-window.txt`,
  `dumpsys-package.txt`, `apk-manifest.txt`, `apk-dex-packages.txt`, and
  `launch-logcat.txt`.
- HTTP failure: request URL, method, status code, response body, and UI
  screenshot/snackbar text.
- WebSocket failure: URL scheme, endpoint, visible error text, and logcat
  excerpt around the connection attempt.
- ADB/UI automation ambiguity: UI dump before and after the action, screenshot,
  whether the soft keyboard was visible, and a manual fallback result if
  available.
- Backend capability failure: health JSON plus the exact backend error shown in
  the app.

## Test Data

Use unique names based on `RUN_ID`:

- Workspace name: `$RUN_ID-workspace`
- Folder name: `$RUN_ID-folder`
- Mission prompt: `Reply with exactly android smoke $RUN_ID`
- Automation command: `Say android automation smoke $RUN_ID`
- Terminal command: `printf 'android terminal smoke $RUN_ID\n'`

Only delete objects created by this test run.

## Test Cases

### 1. First Run Configuration

Steps:

1. Launch the app with cleared data.
2. Verify first screen shows `Sandboxed`, `Server URL`, and `Continue`.
3. Enter `$BACKEND_URL_APP`.
4. Tap `Continue`.

Pass criteria:

- If backend auth is disabled, the app reaches the `Control` tab.
- If backend auth is enabled, the app reaches `Sign in`.
- No crash or endless spinner after 15 seconds.

Evidence:

```bash
dump_ui | tee "$ARTIFACT_DIR/01-config-ui.txt"
adb exec-out screencap -p > "$ARTIFACT_DIR/01-config-result.png"
```

### 2. Auth Gate

Run only the cases supported by `health.json`.

Disabled auth:

- Expected: after server configuration, main tabs are visible: `Control`,
  `Missions`, `Terminal`, `Files`, `More`.

Single-tenant auth:

1. Verify `Sign in`, backend URL, and `Password` field.
2. Enter the test password.
3. Tap `Sign in`.

Multi-user auth:

1. Verify `Username` and `Password` fields.
2. Enter test credentials.
3. Tap `Sign in`.

GitHub auth, when `github_enabled=true`:

1. Verify `Sign in with GitHub` is visible.
2. Tap it and verify a browser/custom tab opens.
3. If the backend OAuth app is not configured for the emulator, return to the
   app and mark OAuth completion as blocked by backend config.
4. Separately validate the Android deep link handler:

```bash
adb shell am start \
  -a android.intent.action.VIEW \
  -d "sandboxed://auth/callback?token=synthetic-token&exp=4102444800" \
  -p "$APP_ID"
```

Pass criteria:

- Correct form is shown for the auth mode.
- Valid credentials store a JWT and reveal the main tabs.
- Invalid credentials show an error banner and do not crash.
- Deep link callback does not crash and returns to the app.

### 3. Bottom Navigation Smoke

Steps:

1. Tap each bottom tab: `Control`, `Missions`, `Terminal`, `Files`, `More`.
2. After each tap, dump the UI.

Pass criteria:

- `Control` shows `New mission` or the current mission title and a `Message…`
  composer.
- `Missions` shows title `Missions`, search placeholder
  `Search missions and moments…`, and filter chips when not searching.
- `Terminal` shows title `Terminal`, workspace selector, and connection status.
- `Files` shows title `Files`, path `Workspace root`, and toolbar buttons for
  upload, new folder, refresh.
- `More` shows tiles for `Workspaces`, `Desktop`, `Tasks`, `Runs`,
  `FIDO approvals`, and `Settings`.

### 4. Settings

Steps:

1. More -> `Settings`.
2. Verify `Server`, `Defaults`, and `About`.
3. Tap `Test & save`.
4. Toggle `Skip agent picker` on and off.
5. If backends are listed, select a non-selected backend, then select the
   original backend again.
6. If agents are listed, select a non-selected agent, then select the original
   agent again.
7. If providers or slash commands are listed, verify they render without clipped
   text.
8. Press Home, relaunch app, return to Settings.

Pass criteria:

- `Test & save` reports `Connected (...)`.
- Settings persist across relaunch.
- `Sign out` clears the token and returns to login only when auth is enabled.
- No settings action crashes the app.

### 5. Workspaces

Steps:

1. More -> `Workspaces`.
2. Verify list or `No workspaces` empty state.
3. Tap `Create workspace`.
4. Verify `New workspace`, `Name`, `Container`, `Host`, and `Create`.
5. Select `Host` with an empty path.
6. Verify `Host workspaces require a path.` and disabled create behavior.
7. Cancel.
8. On a disposable backend only, create a container workspace named
   `$RUN_ID-workspace`.

Pass criteria:

- Existing workspaces show name, type, status, and path.
- Host path validation appears.
- Disposable workspace creation either succeeds and appears in the list, or
  shows a backend error without app crash.

### 6. Control: New Mission And Streaming

Prerequisite: at least one workspace and one backend exist. Full assistant
response requires a configured agent/provider.

Steps:

1. Go to `Control`.
2. Tap `New mission`.
3. Verify `New mission` dialog loads `Workspace`, `Agent`, and `Model override`.
4. Select the default workspace, backend, and agent.
5. Leave model override as `Default` unless testing a specific provider model.
6. Tap `Create`.
7. In the composer, enter `Reply with exactly android smoke $RUN_ID`.
8. Tap `Send`.
9. Watch for user bubble, assistant text, tool cards, status changes, or error
   banner.
10. Kill and relaunch the app while the mission is active; return to `Control`.

Pass criteria:

- Dialog can create or attempt to create a mission.
- Composer sends or queues the message; empty composer cannot send.
- SSE content updates without duplicating prior messages after reconnect.
- Relaunch restores the last mission and draft/stream state as expected.
- If backend lacks configured providers, the app shows a readable error rather
  than crashing.

Issue logging:

If the mission fails before model execution, capture the exact backend message
and backend inventory:

```bash
dump_ui | tee "$ARTIFACT_DIR/06-control-error-ui.txt"
if [ -n "${BACKEND_TOKEN:-}" ]; then
  curl -fsS "$BACKEND_URL_HOST/api/backends" \
    -H "authorization: Bearer $BACKEND_TOKEN" \
    | tee "$ARTIFACT_DIR/06-backends.json" || true
  curl -fsS "$BACKEND_URL_HOST/api/workspaces" \
    -H "authorization: Bearer $BACKEND_TOKEN" \
    | tee "$ARTIFACT_DIR/06-workspaces.json" || true
fi
```

For errors like `Claude Code CLI 'claude' not found`, classify as
`BACKEND_FAIL`. Suggested backend fix: install/configure the missing CLI inside
the selected workspace execution environment, set the backend CLI path in
Backend Settings, or select an installed backend before creating the mission.

### 7. Control: Mission Switcher, Queue, Workers

Steps:

1. Tap the `Missions` icon in the Control top bar.
2. Verify mission switcher dialog shows `Missions`, `Search`, `New`, and
   mission sections when data exists.
3. Search for `$RUN_ID` or another visible mission title.
4. Open a mission from the dialog.
5. If a mission is active, send a second message while the backend is busy and
   verify the `Queued` bar if the backend queues it.
6. If child/parallel missions exist, open `Workers` and verify worker rows.

Pass criteria:

- Search filters visible missions.
- Opening a mission returns to Control and updates the top bar.
- Queue controls remove individual queued items and clear all queued items
  without crashing.
- Worker dialog handles empty and populated states.

### 8. Automations

Prerequisite: a current mission exists.

Steps:

1. Control -> top bar `Automations`.
2. Verify `Automations` screen and `Add` button.
3. Tap `Add`.
4. Enter `Say android automation smoke $RUN_ID`.
5. Leave trigger `Interval`, set `Seconds` to `60`, tap `Create`.
6. Verify row label `every 60s`.
7. Toggle the row off and on.
8. Delete the row.
9. Go back to Control.

Pass criteria:

- Create validates non-empty command and positive interval seconds.
- Created automation appears, toggles active state, and can be deleted.
- Backend errors render as banners without app crash.

### 9. Missions History

Steps:

1. Tap bottom `Missions`.
2. Tap each filter chip: `All`, `Active`, `Interrupted`, `Completed`, `Failed`.
3. Search for `$RUN_ID` and for a known non-matching string.
4. If search results include `Missions` or `Moments`, open one result.
5. Tap refresh.
6. On a disposable backend only, test cleanup completed.

Pass criteria:

- Filters and search update the list.
- Empty search states do not crash.
- Opening a result loads that mission into Control.
- Cleanup reports either success or `Nothing to clean`.

### 10. Terminal WebSocket

Steps:

1. Tap bottom `Terminal`.
2. Wait for `connected`; if it remains offline, capture the error and mark
   terminal backend unavailable.
3. Open the workspace selector and verify `default (host)` plus workspaces.
4. Enter:

```text
printf 'android terminal smoke <RUN_ID>\n'
```

5. Dump the UI and save it as `10-terminal-before-send.txt`.
6. Press Back once to hide the soft keyboard, then tap the `Send` content
   description from the latest UI bounds.
7. Dump the UI again as `10-terminal-after-send.txt`.
8. If the draft remains visible or output does not appear, press the keyboard
   Enter/IME action and retry the Send button once. Record both attempts in
   `investigation.log`.
9. If a workspace is available, select it and run:

```text
pwd
```

Pass criteria:

- WebSocket connects or shows a readable reconnect/error state.
- Terminal output includes `android terminal smoke`.
- Workspace selector reconnects without app crash.
- ANSI-colored output remains readable.

Issue logging:

```bash
dump_ui | tee "$ARTIFACT_DIR/10-terminal-before-send.txt"
adb shell input keyevent BACK || true
tap_label "Send" || true
sleep 2
dump_ui | tee "$ARTIFACT_DIR/10-terminal-after-send.txt"
adb logcat -d -t 500 > "$ARTIFACT_DIR/10-terminal-logcat.txt"
```

If Send does not clear the draft or send bytes while the UI says `connected`,
classify as `APP_FAIL` unless a manual tap succeeds. Suggested app fixes to
record: add `ImeAction.Send` / `KeyboardActions(onSend = vm.submit())`, expose a
stable test tag or larger semantic button target, and log `sendInput` failures
when the WebSocket is null.

If Send clears the draft but terminal output does not include the marker, first
verify the backend WebSocket directly from the host with the same token and
protocols. A direct WebSocket pass plus an Android no-output result is an
`APP_FAIL`; likely client owner is stale socket routing where `sendInput()`
targets an older open WebSocket while the UI is collecting another connection,
or terminal JSON frames are malformed. Confirm Android sends tagged frames such
as `{"t":"i","d":"..."}` and `{"t":"r","c":80,"r":24}`; kotlinx serialization
omits default-valued fields unless configured or explicitly passed. Record
direct WebSocket transcript evidence as `10-terminal-direct-ws.txt`.

### 11. Files

Steps:

1. Tap bottom `Files`.
2. Verify root path `Workspace root`, refresh, upload, new-folder, and up
   controls.
3. Tap `New folder`.
4. Enter `$RUN_ID-folder`, tap `Create`.
5. Verify the folder appears.
6. Tap the folder and verify path changes.
7. Tap up and verify path returns to `Workspace root`.
8. Delete `$RUN_ID-folder` and confirm.

Optional upload/download check:

1. Create a host file:

```bash
printf 'android upload smoke %s\n' "$RUN_ID" > "$ARTIFACT_DIR/upload.txt"
adb push "$ARTIFACT_DIR/upload.txt" /sdcard/Download/upload-$RUN_ID.txt
```

2. In Files, tap Upload and choose the file from Android DocumentsUI.
3. Verify `Uploaded` snackbar and a file row.
4. Tap Download on the file and verify Android opens an `Open ...` chooser or a
   compatible viewer.
5. Delete the uploaded file.

Pass criteria:

- Directory creation, navigation, and deletion work.
- File operations show snackbar/errors and do not crash.
- Up button is disabled at `Workspace root`.

Issue logging:

If `New folder` returns HTTP 403, capture the backend contract directly:

```bash
if [ -n "${BACKEND_TOKEN:-}" ]; then
  curl -i -sS -X POST "$BACKEND_URL_HOST/api/fs/mkdir" \
    -H "authorization: Bearer $BACKEND_TOKEN" \
    -H 'content-type: application/json' \
    -d "{\"path\":\"/$RUN_ID-folder\"}" \
    | tee "$ARTIFACT_DIR/11-fs-mkdir-root-http.txt"
  curl -i -sS "$BACKEND_URL_HOST/api/fs/list?path=." \
    -H "authorization: Bearer $BACKEND_TOKEN" \
    | tee "$ARTIFACT_DIR/11-fs-list-dot-http.txt"
fi
dump_ui | tee "$ARTIFACT_DIR/11-files-after-mkdir.txt"
```

Classify a 403 for `/...` as a backend/client contract issue, not an Android
crash. The backend only allows writes under configured workspace/context roots;
the Android client should create folders under the displayed `Workspace root`
with relative paths, or the backend should expose allowed roots for the client
to display.

### 12. More Screens: Tasks And Runs

Tasks:

1. More -> `Tasks`.
2. Verify list rows or `No subtasks running`.
3. Tap refresh.

Runs:

1. More -> `Runs`.
2. Verify list rows or `No runs recorded`.
3. If costs exist, verify total dollars in the header equals visible row totals
   rounded to cents.
4. Tap refresh.

Pass criteria:

- Empty and populated states render.
- Refresh does not crash.
- Long task/run text is bounded and does not overlap controls.

### 13. FIDO Approvals

Steps:

1. More -> `FIDO approvals`.
2. Verify `Always require biometric` and either `No rules` or existing rules.
3. Toggle `Always require biometric` on, then off.
4. Tap `Add rule`.
5. With `All SSH` selected and `24h` selected, tap `Add`.
6. Verify row `Any SSH` and expiry label.
7. Delete that rule.
8. Add `Host` rule with value `example.com`, expiry `1h`, and
   `Require biometric` on; verify row; delete it.
9. Add `Fingerprint` rule with value `SHA256:testfingerprint`, expiry `never`;
   verify row; delete it.

Optional live prompt check:

- Trigger a real backend FIDO signing request. Verify the global dialog
  `Approve signing request?`, `Approve`, and `Deny`.
- On an emulator without enrolled biometrics, approval may fall back to device
  credential or fail according to emulator configuration; record the behavior.

Pass criteria:

- Rule validation blocks missing Host/Fingerprint values.
- Rules persist after app relaunch.
- Delete removes only the selected rule.
- Prompt handling posts approval/denial when backend request exists.

### 14. Desktop Stream

This depends on backend desktop streaming being enabled.

Steps:

1. More -> `Desktop`.
2. Verify header `Desktop`, display chips `:99`, `:100`, `:101`, `:102`,
   FPS and Quality controls, text entry, quick keys, and scroll controls.
3. If a stream is available, wait for frame count to increase.
4. Tap pause, verify `Paused`; tap play, verify `Live`.
5. Change FPS and Quality using sliders or +/- controls.
6. Type `android desktop smoke $RUN_ID`, tap `Type`.
7. Tap quick keys `Return`, `Esc`, `Ctrl+L`, `Tab`.
8. Tap reconnect.

Pass criteria:

- With stream enabled, frames render and frame count increases.
- With stream unavailable, the app shows an error plus `Retry`.
- Controls are usable and do not overlap in portrait or landscape.

Issue logging:

```bash
dump_ui | tee "$ARTIFACT_DIR/14-desktop-ui.txt"
adb logcat -d -t 800 > "$ARTIFACT_DIR/14-desktop-logcat.txt"
if [ -n "${BACKEND_TOKEN:-}" ]; then
  curl -i -sS "$BACKEND_URL_HOST/api/desktop/sessions" \
    -H "authorization: Bearer $BACKEND_TOKEN" \
    | tee "$ARTIFACT_DIR/14-desktop-sessions-http.txt" || true
fi
```

If the visible error is `Expected URL scheme 'http' or 'https' but was 'wss'`,
classify as `APP_FAIL`. The Android desktop client is building the stream URL by
passing a `ws://` or `wss://` string into `toHttpUrl()`, which only accepts
HTTP(S). Suggested fix: build query parameters on the HTTP(S) URL first, then
convert the final string to `ws://` or `wss://` only when constructing the
OkHttp `Request`.

### 15. Rotation, Resize, And App Lifecycle

Steps:

```bash
adb shell settings put system accelerometer_rotation 0
adb shell settings put system user_rotation 1
sleep 2
adb exec-out screencap -p > "$ARTIFACT_DIR/landscape.png"
adb shell settings put system user_rotation 0
adb shell input keyevent HOME
sleep 2
adb shell am start -n "$APP_ID/.MainActivity"
```

Repeat this on Control, Terminal, Files, and Desktop.

Pass criteria:

- No important text overlaps or is clipped.
- Composer remains accessible with keyboard shown.
- Terminal sends a resize frame and remains connected or reconnects.
- App returns to the same logical screen after foregrounding.

### 16. Network And Error Resilience

Steps:

1. With the app open, stop the backend or block network.
2. Navigate Control, Terminal, Files, Missions.
3. Restart/unblock the backend.
4. Tap refresh/retry where available.

Pass criteria:

- Screens show readable errors or reconnect indicators.
- No crash, ANR, or infinite modal.
- App recovers after backend returns.

### 17. Crash And Log Check

Run after the full pass:

```bash
adb logcat -d -t 3000 > "$ARTIFACT_DIR/logcat.txt"
grep -E -C 8 "FATAL EXCEPTION|Application Not Responding|ANR in|Process .* has died|Process:" \
  "$ARTIFACT_DIR/logcat.txt" > "$ARTIFACT_DIR/logcat-crash-context.txt" || true
if grep -E "ANR in sh\\.sandboxed\\.dashboard|Process: sh\\.sandboxed\\.dashboard|Process sh\\.sandboxed\\.dashboard .* has died" "$ARTIFACT_DIR/logcat.txt"; then
  echo "FAIL: app crash markers found"
else
  echo "PASS: no app crash markers"
fi
adb exec-out screencap -p > "$ARTIFACT_DIR/final.png"
```

Pass criteria:

- No fatal exceptions or ANR markers for `sh.sandboxed.dashboard`.

Do not fail on generic `AndroidRuntime` lines by themselves; `uiautomator` and
other shell tools also log under `AndroidRuntime`.

## Known Findings From 2026-05-25 Remote Run

These findings should be rechecked on the next run and either closed or kept in
the report with fresh evidence:

- API 36 launch failure: the APK installed and `dumpsys package` registered
  `sh.sandboxed.dashboard/.MainActivity`, but `am start` returned
  `Activity class ... does not exist` while `dumpsys user` showed
  `State: RUNNING_LOCKED`. Treat this as `ENV_BLOCKED` unless it reproduces on
  an unlocked/wiped valid API 36 AVD.
- Invalid AVDs: local `Pixel_9` and `Pixel_9_Pro` profiles were stale because
  their device definitions no longer existed. Recreate them before using them
  as “latest Android” coverage.
- Desktop stream: the Android client originally showed
  `Expected URL scheme 'http' or 'https' but was 'wss'`. Retest that URL
  construction now reaches the backend and either streams frames or shows a
  backend availability error.
- Files: creating a folder under `/` returned backend HTTP 403 in the original
  run. Retest that the app now starts at `Workspace root` and sends relative
  mkdir/upload paths.
- Terminal: WebSocket connected and accepted a draft, but the ADB-driven Send
  tap did not submit in the original run. In the Pixel_10_Pro_API36_1 rerun,
  the command text was present, Send cleared the draft, and a direct host
  WebSocket to `/api/console/ws` echoed the same marker, but the Android UI did
  not display the marker. Root causes fixed in the Android client: terminal
  frames now always include the `t` discriminator required by the backend,
  stale terminal WebSockets cannot receive new sends, and PTY output is
  normalized so OSC title updates and carriage returns do not corrupt visible
  text. Recheck by asserting a full terminal marker appears in the UI dump.
- Mission execution: the backend reported a missing Claude CLI for the selected
  backend/workspace. This blocks model execution but is not an Android crash.
- API 36.1 AVD: local SDK tools did not include an official `pixel_10_pro`
  hardware profile. The closest valid current AVD used a `pixel_9_pro` profile
  with the Android 36.1 Google Play image and was named `Pixel_10_Pro_API36_1`.

## Cleanup

Only clean up smoke data created by this run:

- Delete `$RUN_ID-folder` and uploaded smoke files from Files.
- Delete test automations from the mission.
- Delete test missions only if running on a disposable backend.
- Clear emulator app data if desired:

```bash
adb shell pm clear "$APP_ID"
```

If using the disposable Docker backend:

```bash
docker compose down
```

## Result Report Template

```text
Android Dashboard Simulator Test Report
Run ID:
Date:
App version:
Backend URL:
Backend health auth_mode:
Emulator model/API:

Summary:
- Passed:
- Failed:
- Blocked by backend capability:

Failures:
1. Screen / case:
   Expected:
   Actual:
   Evidence:
   Log excerpt:
   Classification:
   Likely owner:
   Suggested fix:

Artifacts:
- Screenshots:
- UI dumps:
- logcat:
- investigation.log:

Cleanup performed:
```

## Failure Classification

Use these labels consistently:

- `APP_FAIL`: Android UI crash, incorrect state, broken navigation, persistence
  bug, unreadable layout, or unhandled client exception.
- `BACKEND_FAIL`: backend endpoint returns invalid data, 5xx, auth mismatch, or
  missing capability required by the feature.
- `CONTRACT_FAIL`: Android client and backend both behave consistently with
  their own assumptions, but the API contract is ambiguous or mismatched.
- `ENV_BLOCKED`: emulator, credentials, provider auth, desktop stream, FIDO
  request source, or file picker unavailable.
- `N/A`: feature intentionally not enabled for this backend.

Every non-pass must have a matching entry in `investigation.log`; a run is not
complete until the tester has captured the diagnostics listed in
`Investigation Log Requirements`.
