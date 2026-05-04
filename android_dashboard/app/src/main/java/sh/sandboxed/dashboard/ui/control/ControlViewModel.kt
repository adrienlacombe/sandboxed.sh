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

data class ControlState(
    val mission: Mission? = null,
    val parallel: List<RunningMissionInfo> = emptyList(),
    val maxParallel: Int = 1,
    val messages: List<ChatMessage> = emptyList(),
    val queue: List<QueuedMessage> = emptyList(),
    val draft: String = "",
    val isSending: Boolean = false,
    val isConnected: Boolean = false,
    val error: String? = null,
    val goalStatus: String? = null,
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
                    _state.update { it.copy(mission = mission) }
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
        viewModelScope.launch { runCatching { container.api.resumeMission(id) } }
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
                _state.update { it.copy(mission = mission, messages = mission.history.map { entry ->
                    ChatMessage(
                        kind = if (entry.role == "user") ChatMessageKind.User else ChatMessageKind.Assistant(),
                        content = entry.content,
                    )
                }) }
                container.settings.setLastMission(mission.id)
                lastSeq = null
            }
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
                messages = cur.history.map { entry ->
                    ChatMessage(
                        kind = if (entry.role == "user") ChatMessageKind.User else ChatMessageKind.Assistant(),
                        content = entry.content,
                    )
                },
            )
        }
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
                                handle(storedEventToSse(ev))
                            }
                            if (max != null) lastSeq = max
                        }
                    }

                    container.sse.stream()
                        .catch { e -> _state.update { it.copy(isConnected = false, error = e.message) } }
                        .collect { evt ->
                            attempt = 0
                            _state.update { it.copy(isConnected = true, error = null) }
                            handle(evt)
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
    }

    private fun handle(evt: SseEvent) {
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
