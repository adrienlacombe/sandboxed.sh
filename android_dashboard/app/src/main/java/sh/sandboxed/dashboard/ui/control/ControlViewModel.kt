package sh.sandboxed.dashboard.ui.control

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.jsonPrimitive
import sh.sandboxed.dashboard.data.AppContainer
import sh.sandboxed.dashboard.data.ChatMessage
import sh.sandboxed.dashboard.data.ChatMessageKind
import sh.sandboxed.dashboard.data.CreateMissionRequest
import sh.sandboxed.dashboard.data.Mission
import sh.sandboxed.dashboard.data.MissionStatus
import sh.sandboxed.dashboard.data.QueuedMessage
import sh.sandboxed.dashboard.data.RunningMissionInfo
import sh.sandboxed.dashboard.data.SharedFile
import sh.sandboxed.dashboard.data.SseEvent
import sh.sandboxed.dashboard.data.ToolUiParser
import java.util.UUID

data class NewMissionOptions(
    val workspaceId: String? = null,
    val agent: String? = null,
    val modelOverride: String? = null,
    val backend: String? = null,
)

data class ControlState(
    val mission: Mission? = null,
    val parallel: List<RunningMissionInfo> = emptyList(),
    val maxParallel: Int = 1,
    val childMissions: List<Mission> = emptyList(),
    val recentMissions: List<Mission> = emptyList(),
    val messages: List<ChatMessage> = emptyList(),
    val queue: List<QueuedMessage> = emptyList(),
    val draft: String = "",
    val isSending: Boolean = false,
    val isConnected: Boolean = false,
    val error: String? = null,
    val goalStatus: String? = null,
    val desktopDisplay: String = ":101",
    val desktopOpenRequest: Long = 0,
    val loadingRecent: Boolean = false,
)

class ControlViewModel(private val container: AppContainer) : ViewModel() {
    private val _state = MutableStateFlow(ControlState())
    val state: StateFlow<ControlState> = _state.asStateFlow()

    private var streamJob: Job? = null
    private var pollJob: Job? = null
    @Volatile private var lastSeq: Long? = null

    init {
        viewModelScope.launch {
            try {
                refreshMission()
                refreshRunning()
                refreshQueue()
            } catch (_: Throwable) {}
            startStream()
            startRunningPoller()
        }
    }

    fun setDraft(text: String) {
        _state.update { it.copy(draft = text) }
        viewModelScope.launch { container.settings.setDraft(text) }
    }

    fun send() {
        val text = _state.value.draft.trim()
        if (text.isEmpty()) return
        _state.update { it.copy(isSending = true) }

        val draftMsg = ChatMessage(kind = ChatMessageKind.User, content = text)
        _state.update { it.copy(messages = it.messages + draftMsg, draft = "") }
        viewModelScope.launch { container.settings.setDraft("") }

        viewModelScope.launch {
            runCatching {
                if (_state.value.mission == null) {
                    val s = container.cached.value
                    val mission = container.api.createMission(CreateMissionRequest(
                        title = text.take(60),
                        agent = s.defaultAgent.takeIf { it.isNotBlank() },
                        backend = s.defaultBackend.takeIf { it.isNotBlank() },
                    ))
                    _state.update { it.copy(mission = mission, childMissions = emptyList()) }
                    container.settings.setLastMission(mission.id)
                }
                container.api.sendMessage(text)
                refreshQueue()
            }.onFailure { e -> _state.update { it.copy(error = e.message) } }
            _state.update { it.copy(isSending = false) }
        }
    }

    fun cancel() { viewModelScope.launch { runCatching { container.api.cancelControl() } } }
    fun resume() {
        val id = _state.value.mission?.id ?: return
        resumeMission(id)
    }
    fun resumeMission(id: String) {
        viewModelScope.launch {
            runCatching { container.api.resumeMission(id) }
                .onSuccess { if (id == _state.value.mission?.id) _state.update { st -> st.copy(mission = it) } }
            loadRecentMissions()
            refreshRunning()
        }
    }
    fun cancelMission(id: String) {
        viewModelScope.launch {
            runCatching { container.api.cancelMission(id) }
            loadRecentMissions()
            refreshRunning()
        }
    }
    fun deleteMission(id: String) {
        viewModelScope.launch {
            runCatching { container.api.deleteMission(id) }
            _state.update { st ->
                st.copy(
                    recentMissions = st.recentMissions.filterNot { it.id == id },
                    childMissions = st.childMissions.filterNot { it.id == id },
                    parallel = st.parallel.filterNot { it.missionId == id },
                )
            }
        }
    }
    fun createMission(options: NewMissionOptions = NewMissionOptions()) {
        viewModelScope.launch {
            runCatching {
                container.api.createMission(
                    CreateMissionRequest(
                        workspaceId = options.workspaceId,
                        agent = options.agent,
                        modelOverride = options.modelOverride,
                        backend = options.backend,
                    )
                )
            }.onSuccess { mission ->
                lastSeq = null
                _state.update { it.copy(mission = mission, messages = emptyList(), childMissions = emptyList(), goalStatus = null) }
                container.settings.setLastMission(mission.id)
                refreshRunning()
            }.onFailure { e ->
                _state.update { it.copy(error = e.message) }
            }
        }
    }
    fun createFollowUpMission(source: Mission) {
        viewModelScope.launch {
            runCatching {
                container.api.createMission(
                    CreateMissionRequest(
                        workspaceId = source.workspaceId,
                        agent = source.agent,
                        modelOverride = source.modelOverride,
                        backend = source.backend,
                    )
                )
            }.onSuccess { mission ->
                val title = source.title?.trim().takeUnless { it.isNullOrEmpty() }
                    ?: source.shortDescription?.trim().takeUnless { it.isNullOrEmpty() }
                val prompt = if (title.isNullOrEmpty()) {
                    "Follow up on this mission with the next concrete implementation steps."
                } else {
                    "Follow up on \"$title\" and implement the next concrete steps."
                }
                lastSeq = null
                _state.update {
                    it.copy(
                        mission = mission,
                        messages = emptyList(),
                        childMissions = emptyList(),
                        draft = prompt,
                        goalStatus = null,
                    )
                }
                container.settings.setLastMission(mission.id)
                container.settings.setDraft(prompt)
                refreshRunning()
            }.onFailure { e ->
                _state.update { it.copy(error = e.message) }
            }
        }
    }
    fun deleteQueueItem(id: String) {
        viewModelScope.launch { runCatching { container.api.deleteQueueItem(id); refreshQueue() } }
    }
    fun clearQueue() {
        viewModelScope.launch { runCatching { container.api.clearQueue(); refreshQueue() } }
    }

    fun switchMission(missionId: String) {
        viewModelScope.launch {
            runCatching {
                val mission = container.api.loadMission(missionId)
                _state.update { it.copy(mission = mission, messages = mapHistory(mission), goalStatus = null) }
                container.settings.setLastMission(mission.id)
                lastSeq = null
                runCatching {
                    val (_, max) = container.api.missionEvents(mission.id, latest = true, limit = 1)
                    lastSeq = max
                }
                refreshChildMissions(mission.id)
            }
        }
    }

    fun loadRecentMissions() {
        viewModelScope.launch {
            _state.update { it.copy(loadingRecent = true) }
            runCatching { container.api.listMissions(limit = 200) }
                .onSuccess { missions ->
                    _state.update {
                        it.copy(
                            recentMissions = missions.sortedByDescending { m -> m.updatedAt },
                            loadingRecent = false,
                        )
                    }
                }
                .onFailure { e -> _state.update { it.copy(error = e.message, loadingRecent = false) } }
        }
    }

    private suspend fun refreshMission() {
        val cur = container.api.currentMission() ?: return
        // Fetch event seq high-water-mark for delta resume on stream reconnect
        runCatching {
            val (_, max) = container.api.missionEvents(cur.id, latest = true, limit = 1)
            lastSeq = max
        }
        _state.update {
            it.copy(
                mission = cur,
                messages = mapHistory(cur),
            )
        }
        refreshChildMissions(cur.id)
    }

    private suspend fun refreshQueue() {
        runCatching { container.api.getQueue() }.onSuccess { q -> _state.update { it.copy(queue = q) } }
    }

    private fun startStream() {
        streamJob?.cancel()
        streamJob = viewModelScope.launch {
            var attempt = 0
            while (true) {
                try {
                    // Replay any events we missed since last seq before opening live stream.
                    val mid = _state.value.mission?.id
                    val sinceSeq = lastSeq
                    if (mid != null && sinceSeq != null) {
                        runCatching {
                            val (events, max) = container.api.missionEvents(mid, sinceSeq = sinceSeq, limit = 200)
                            events.forEach { ev ->
                                handle(storedEventToSse(ev), live = false)
                            }
                            if (max != null) lastSeq = max
                        }
                    }

                    container.sse.stream()
                        .catch { e -> _state.update { it.copy(isConnected = false, error = e.message) } }
                        .collect { evt ->
                            attempt = 0
                            _state.update { it.copy(isConnected = true, error = null) }
                            handle(evt, live = true)
                        }
                } catch (_: Throwable) {
                    _state.update { it.copy(isConnected = false) }
                }
                attempt += 1
                val backoff = (1000L shl minOf(attempt, 5)).coerceAtMost(30_000L)
                delay(backoff)
            }
        }
    }

    private fun startRunningPoller() {
        pollJob?.cancel()
        pollJob = viewModelScope.launch {
            while (true) {
                runCatching { refreshRunning() }
                delay(3_000)
            }
        }
    }

    private suspend fun refreshRunning() {
        val running = container.api.running()
        val cfg = runCatching { container.api.parallelConfig() }.getOrNull()
        _state.update {
            it.copy(parallel = running, maxParallel = cfg?.maxParallel ?: it.maxParallel)
        }
        _state.value.mission?.id?.let { refreshChildMissions(it) }
    }

    private fun handle(evt: SseEvent, live: Boolean) {
        val obj = (evt.data as? JsonObject) ?: return
        fun s(k: String): String? = obj[k]?.jsonPrimitive?.content
        fun b(k: String): Boolean? = obj[k]?.jsonPrimitive?.booleanOrNull
        fun i(k: String): Int? = obj[k]?.jsonPrimitive?.intOrNull
        val missionId = s("mission_id") ?: _state.value.mission?.id
        if (missionId != null && missionId != _state.value.mission?.id) return

        when (evt.type) {
            "user_message" -> appendMessage(ChatMessage(kind = ChatMessageKind.User, content = s("content") ?: return))
            "assistant_message" -> {
                val content = s("content") ?: return
                val cost = i("cost_cents") ?: 0
                val source = s("cost_source") ?: "actual"
                val model = s("model")
                val files = parseSharedFiles(obj["shared_files"])
                appendMessage(ChatMessage(
                    kind = ChatMessageKind.Assistant(costCents = cost, costSource = source, model = model, sharedFiles = files),
                    content = content,
                ))
            }
            "text_delta" -> { val content = s("content") ?: return; setStreamingAssistant(content) }
            "thinking" -> {
                val text = s("content") ?: ""
                val done = b("done") == true
                upsertThinking(text, done)
            }
            "agent_phase" -> {
                val phase = s("phase") ?: return
                appendMessage(ChatMessage(kind = ChatMessageKind.Phase(phase, s("detail"), s("agent")), content = ""))
            }
            "tool_call" -> {
                val name = s("name") ?: return
                val args = obj["args"]
                val toolUi = ToolUiParser.parse(name, args)
                if (toolUi !is sh.sandboxed.dashboard.data.ToolUiContent.Unknown) {
                    appendMessage(ChatMessage(kind = ChatMessageKind.ToolUi(name, toolUi), content = ""))
                } else {
                    appendMessage(ChatMessage(kind = ChatMessageKind.ToolCall(name, true), content = args.displayText()))
                }
            }
            "tool_result" -> {
                val name = s("name") ?: ""
                val isError = b("is_error") == true
                parseDesktopDisplay(name, obj["result"])?.let { display ->
                    _state.update {
                        it.copy(
                            desktopDisplay = display,
                            desktopOpenRequest = if (live) it.desktopOpenRequest + 1 else it.desktopOpenRequest,
                        )
                    }
                }
                appendMessage(ChatMessage(
                    kind = if (isError) ChatMessageKind.ErrorMsg else ChatMessageKind.ToolCall(name, false),
                    content = obj["result"].displayText(),
                ))
            }
            "tool_ui" -> {
                val name = s("name") ?: "ui"
                val content = ToolUiParser.parse(name, obj["args"])
                appendMessage(ChatMessage(kind = ChatMessageKind.ToolUi(name, content), content = ""))
            }
            "goal_iteration" -> {
                val iter = i("iteration") ?: 0
                val status = s("status") ?: ""
                val obj0 = s("objective") ?: ""
                appendMessage(ChatMessage(kind = ChatMessageKind.Goal(iter, status, obj0), content = ""))
            }
            "goal_status" -> _state.update { it.copy(goalStatus = s("status")) }
            "mission_status_changed" -> {
                val status = s("status") ?: return
                _state.update { it.copy(mission = it.mission?.copy(status = parseStatus(status))) }
                viewModelScope.launch { refreshRunning() }
            }
            "mission_title_changed" -> {
                val t = s("title") ?: return
                _state.update { it.copy(mission = it.mission?.copy(title = t)) }
            }
            "status" -> {}
            "error" -> _state.update { it.copy(error = s("message")) }
        }
    }

    private fun parseSharedFiles(el: JsonElement?): List<SharedFile> {
        val arr = el as? JsonArray ?: return emptyList()
        return arr.mapNotNull { e ->
            val o = e as? JsonObject ?: return@mapNotNull null
            SharedFile(
                name = o["name"]?.jsonPrimitive?.content.orEmpty(),
                url = o["url"]?.jsonPrimitive?.content.orEmpty(),
                contentType = o["content_type"]?.jsonPrimitive?.content.orEmpty(),
                sizeBytes = o["size_bytes"]?.jsonPrimitive?.content?.toLongOrNull(),
            )
        }
    }

    private fun parseStatus(s: String): MissionStatus = runCatching {
        MissionStatus.valueOf(s.uppercase())
    }.getOrDefault(MissionStatus.UNKNOWN)

    private fun mapHistory(mission: Mission): List<ChatMessage> =
        mission.history.map { entry ->
            ChatMessage(
                kind = if (entry.role == "user") ChatMessageKind.User else ChatMessageKind.Assistant(),
                content = entry.content,
            )
        }

    private suspend fun refreshChildMissions(parentId: String) {
        runCatching { container.api.childMissions(parentId) }
            .onSuccess { workers ->
                if (_state.value.mission?.id == parentId) {
                    _state.update { it.copy(childMissions = workers) }
                }
            }
    }

    private fun parseDesktopDisplay(name: String, result: JsonElement?): String? {
        if (!name.contains("desktop_start_session")) return null
        val obj = when (result) {
            is JsonObject -> result
            is JsonPrimitive -> runCatching {
                sh.sandboxed.dashboard.data.api.Net.json.parseToJsonElement(result.content) as? JsonObject
            }.getOrNull()
            else -> null
        }
        return obj?.get("display")?.jsonPrimitive?.content
    }

    private fun appendMessage(m: ChatMessage) { _state.update { it.copy(messages = it.messages + m) } }

    private fun storedEventToSse(ev: sh.sandboxed.dashboard.data.StoredEvent): SseEvent {
        val data = ev.metadata.toMutableMap()
        data["mission_id"] = JsonPrimitive(ev.missionId)
        if (ev.content.isNotBlank()) data["content"] = JsonPrimitive(ev.content)
        ev.toolCallId?.let { data["tool_call_id"] = JsonPrimitive(it) }
        ev.toolName?.let { data["name"] = JsonPrimitive(it) }
        when (ev.eventType) {
            "tool_call" -> data["args"] = parseJsonOrString(ev.content)
            "tool_result" -> data["result"] = parseJsonOrString(ev.content)
        }
        return SseEvent(ev.eventType, JsonObject(data))
    }

    private fun parseJsonOrString(value: String): JsonElement =
        runCatching { sh.sandboxed.dashboard.data.api.Net.json.parseToJsonElement(value) }
            .getOrElse { JsonPrimitive(value) }

    private fun JsonElement?.displayText(): String = when (this) {
        null -> ""
        is JsonPrimitive -> content
        else -> toString()
    }

    private fun setStreamingAssistant(content: String) {
        _state.update { st ->
            val msgs = st.messages.toMutableList()
            val last = msgs.lastOrNull()
            if (last?.kind is ChatMessageKind.Assistant) {
                msgs[msgs.lastIndex] = last.copy(content = content)
            } else {
                msgs += ChatMessage(kind = ChatMessageKind.Assistant(), content = content)
            }
            st.copy(messages = msgs)
        }
    }

    private fun upsertThinking(text: String, done: Boolean) {
        _state.update { st ->
            val msgs = st.messages.toMutableList()
            val idx = msgs.indexOfLast { it.kind is ChatMessageKind.Thinking }
            if (idx == -1) {
                msgs += ChatMessage(kind = ChatMessageKind.Thinking(done = done), content = text, id = UUID.randomUUID().toString())
            } else {
                val cur = msgs[idx]
                val kind = (cur.kind as ChatMessageKind.Thinking).copy(done = done)
                val merged = if (text.startsWith(cur.content)) text else cur.content + text
                msgs[idx] = cur.copy(kind = kind, content = merged)
            }
            st.copy(messages = msgs)
        }
    }
}
