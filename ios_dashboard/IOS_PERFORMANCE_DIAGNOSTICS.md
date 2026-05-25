# iOS Performance Diagnostics

Date: 2026-05-25

## Instrumentation Added

- `SandboxedDashboard/Services/ControlPerformanceDiagnostics.swift`
  - Adds `Logger` + `OSSignposter` under subsystem `md.thomas.openagent.dashboard`, category `ControlPerformance`.
  - Records recent slow operations and hot SwiftUI body render counts while the in-app diagnostics overlay is enabled.
- `SandboxedDashboard/Views/Control/ControlView.swift`
  - Existing **Control Diagnostics** menu toggle now also reports slowest measured operation and hottest body render probes.
  - Signposted/timed paths:
    - `control.fetch_snapshot`
    - `control.fetch_current_snapshot`
    - `control.fetch_reload_snapshot`
    - `control.fetch_switch_snapshot`
    - `control.fetch_refresh_snapshot`
    - `control.fetch_earlier`
    - `control.fetch_delta`
    - `control.apply_snapshot`
    - `control.sort_remember_events`
    - `control.replay_events`
    - `control.apply_delta`
    - `control.group_messages`
  - Body render probes:
    - `MessageBubble`
    - `ToolGroupView`
    - `MarkdownView`
- `SandboxedDashboard/Views/Components/MarkdownView.swift`
  - Adds `markdown.parse` timing when Control Diagnostics is enabled.
- `SandboxedDashboard/Services/APIService.swift`
  - Adds whole-app request and decode timing for every JSON API call:
    - `api.request`
    - `api.decode`
  - This covers Control, History, Files, Settings, Workspaces, and other views using `APIService`.

## Simulator / Instruments Notes

Validated locally:

```bash
xcodebuild -project ios_dashboard/SandboxedDashboard.xcodeproj \
  -scheme SandboxedDashboard \
  -destination 'generic/platform=iOS Simulator' \
  build
```

Result: build succeeded. Existing warning remains in `APIService.swift` about generic `T.Type` Sendability; it is unrelated to the diagnostics patch.

Simulator work:

```bash
xcrun simctl create OpenAgentPerf \
  'com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro' \
  'com.apple.CoreSimulator.SimRuntime.iOS-26-4'
xcrun simctl boot <device-id>
xcrun simctl bootstatus <device-id> -b
```

The simulator booted, but `simctl launch` and `simctl listapps` hung after install on this CoreSimulator instance. That blocked a complete automated trace capture in this run. The app now emits signposts usable from Instruments once the simulator can launch the app:

```bash
xcrun xctrace record \
  --template 'SwiftUI' \
  --device <device-id> \
  --launch md.thomas.openagent.dashboard \
  --time-limit 30s \
  --output /tmp/SandboxedDashboard-SwiftUI.trace

xcrun xctrace record \
  --template 'Time Profiler' \
  --device <device-id> \
  --launch md.thomas.openagent.dashboard \
  --time-limit 30s \
  --output /tmp/SandboxedDashboard-TimeProfiler.trace

xcrun xctrace record \
  --template 'Animation Hitches' \
  --device <device-id> \
  --launch md.thomas.openagent.dashboard \
  --time-limit 30s \
  --output /tmp/SandboxedDashboard-Hitches.trace
```

In Console/Instruments, filter for:

```text
subsystem == "md.thomas.openagent.dashboard"
category == "ControlPerformance"
```

## Diagnostics

1. **Chat history replay is still O(events x messages) in practice.**
   `applyViewingMissionWithEvents` replays every stored event through `handleStreamEvent`. Inside the replay, several event cases call `messages.contains`, `messages.firstIndex`, `messages.lastIndex`, and `messages.removeAll`. On long missions this becomes the dominant CPU path after the network snapshot returns.

2. **Assistant markdown is reparsed in SwiftUI body.**
   `MarkdownView.body` calls `MarkdownParser.parse(content)` every time SwiftUI evaluates the row. Large assistant messages, tables, code blocks, and image-rich responses can repeatedly pay regex + line parser + `AttributedString(markdown:)` costs. The new `markdown.parse` signpost should confirm this during scroll and mission load.

3. **Hot redraws are likely row-wide, not cell-local.**
   `MessageBubble` receives full `ChatMessage` values and the parent view owns many unrelated `@State` values. State changes such as polling, queue count, copied id, connection status, diagnostics overlay updates, and running missions can cause visible chat rows to re-evaluate. The render probes will show whether `MessageBubble`/`MarkdownView` counts rise when unrelated toolbar or polling state changes.

4. **The diagnostics overlay itself is intentionally debug-only but can perturb results.**
   When enabled, body probes mutate an in-memory counter and `markdown.parse` timing wraps parsing. Use it to identify suspicious paths, then confirm with Instruments signposts with the overlay hidden.

5. **Mission switching has good cache-first behavior but still performs main-actor decode/replay.**
   Cached mission data is read and decoded synchronously before render. This is good for perceived latency when files are small, but large cached missions still consume main-thread time before the first interactive frame.

6. **Running-mission and child-mission polling can still invalidate ControlView regularly.**
   The code already backs off on failures and gates child-mission fetches, but successful 5-second polling mutates `runningMissions` on the parent view. This can drive redraws in the chat subtree unless the chat list is isolated from polling state.

7. **History, Files, and Settings perform sorted/filter computed properties in view render paths.**
   Examples:
   - `HistoryView.filteredMissions`
   - `FilesView.sortedEntries`
   - mission switcher/search helpers in `ControlView`
   These are probably fine for small lists but should be measured if the whole app feels sluggish outside chat.

8. **Inline/shared image loading has no explicit memory cache.**
   Inline markdown images and shared file images hold decoded `Data` in row state. Rows recreated during navigation or identity changes can refetch/redecode images. Use Instruments Allocations + Network to confirm when image-heavy chats are slow.

9. **Timers in row/sheet components can contribute to unnecessary invalidations.**
   Several control subviews start timers on appear for durations/progress. If many tool rows are visible, timer-driven body refreshes can stack. Render probes around `ToolGroupView` help detect this.

## Recommended Fixes

1. **Replace replay-through-UI-state with a pure reducer.**
   Build `[ChatMessage]` from `[StoredEvent]` in a pure function using dictionaries/sets for ids (`messageById`, `toolById`, active thinking id). Assign `messages` once at the end. This removes repeated array scans and avoids transient SwiftUI invalidations during replay.

2. **Cache parsed markdown per message id/content hash.**
   Move markdown parsing out of `body`, or introduce a `ParsedMarkdown` cache keyed by message id + content hash. For streaming content, debounce parsing to frame cadence or parse only the active message incrementally.

3. **Isolate chat list state from toolbar/polling state.**
   Extract the conversation list into a small view model or child view that only receives `groupedItems`, copy/retry closures, and scroll state. Keep `runningMissions`, queue polling, sheets, and toolbar state out of the row subtree.

4. **Track rendered message ids during replay.**
   Even before a full reducer refactor, maintain a temporary `Set<String>` during historical replay so duplicate checks are O(1) instead of `messages.contains`.

5. **Move cache decode off the first render when the cache file is large.**
   For cache files over a small threshold, render the skeleton immediately and decode in a background task, then publish the decoded snapshot. Keep the current sync fast path for small cache files.

6. **Memoize sorted/filter lists outside view bodies.**
   For History, Files, and mission switcher/search, compute sorted/filter outputs when source arrays or query/filter settings change, not every `body` evaluation.

7. **Use `EquatableView` or narrower Equatable row models for chat rows.**
   Make visible rows skip body work when unrelated state changes. This is especially important for assistant rows with expensive markdown.

8. **Add image cache and downsampling.**
   Use `URLCache`/`NSCache` keyed by resolved download URL and downsample large images before storing in SwiftUI state. This should reduce both network repeat work and memory spikes.

9. **Run the three trace templates on a configured simulator session.**
   After CoreSimulator launch is healthy, capture:
   - SwiftUI: body invalidation and diffing hot spots.
   - Time Profiler: reducer/replay/markdown CPU cost.
   - Animation Hitches: chat load and scroll frame drops.
