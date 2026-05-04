package sh.sandboxed.dashboard.ui.control

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.OpenInNew
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Cancel
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Flag
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.core.net.toUri
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.sandboxed.dashboard.data.AppContainer
import sh.sandboxed.dashboard.data.ChatMessage
import sh.sandboxed.dashboard.data.ChatMessageKind
import sh.sandboxed.dashboard.data.QueuedMessage
import sh.sandboxed.dashboard.data.RunningMissionInfo
import sh.sandboxed.dashboard.data.SharedFile
import sh.sandboxed.dashboard.ui.components.ErrorBanner
import sh.sandboxed.dashboard.ui.components.GlassCard
import sh.sandboxed.dashboard.ui.components.StatusBadge
import sh.sandboxed.dashboard.ui.components.ToolUiWidget
import sh.sandboxed.dashboard.ui.theme.Palette
import sh.sandboxed.dashboard.util.Haptics

@Composable
fun ControlScreen(container: AppContainer, onOpenAutomations: (String) -> Unit) {
    val vm = remember { ControlViewModel(container) }
    val state by vm.state.collectAsState()
    val listState = rememberLazyListState()
    val haptics = remember { Haptics(container) }

    LaunchedEffect(state.messages.size) {
        if (state.messages.isNotEmpty()) listState.animateScrollToItem(state.messages.lastIndex)
    }

    Column(Modifier.fillMaxSize()) {
        TopBar(
            mission = state.mission,
            connected = state.isConnected,
            canResume = state.mission?.status?.canResume == true,
            onResume = { haptics.success(); vm.resume() },
            onAutomations = { state.mission?.id?.let(onOpenAutomations) },
        )
        if (state.parallel.isNotEmpty()) {
            ParallelBar(state.parallel, state.mission?.id) { haptics.selection(); vm.switchMission(it) }
        }
        state.goalStatus?.takeIf { it.isNotBlank() }?.let { GoalBanner(it) }
        state.error?.let { Box(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) { ErrorBanner(it) } }
        if (state.queue.isNotEmpty()) QueueBar(state.queue, vm::deleteQueueItem, vm::clearQueue)
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            items(state.messages, key = { it.id }) { msg -> MessageRow(msg) }
        }
        Composer(
            value = state.draft,
            onChange = vm::setDraft,
            onSend = { haptics.medium(); vm.send() },
            onCancel = { haptics.error(); vm.cancel() },
            isSending = state.isSending,
        )
    }
}

@Composable
private fun TopBar(mission: sh.sandboxed.dashboard.data.Mission?, connected: Boolean, canResume: Boolean, onResume: () -> Unit, onAutomations: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.BackgroundSecondary).padding(horizontal = 16.dp, vertical = 12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(mission?.title ?: "New mission", style = MaterialTheme.typography.titleMedium, color = Palette.TextPrimary, maxLines = 1)
                Text(if (connected) "Connected" else "Reconnecting…",
                    style = MaterialTheme.typography.bodySmall,
                    color = if (connected) Palette.Success else Palette.Warning)
            }
            mission?.status?.let { StatusBadge(it) }
            if (canResume) {
                Spacer(Modifier.width(8.dp))
                IconButton(onClick = onResume) { Icon(Icons.Filled.PlayArrow, "Resume", tint = Palette.Accent) }
            }
            if (mission != null) {
                IconButton(onClick = onAutomations) { Icon(Icons.Filled.Settings, "Automations", tint = Palette.TextSecondary) }
            }
        }
        if (mission != null && (mission.metadataModel != null || mission.metadataSource != null || mission.workspaceName != null)) {
            Spacer(Modifier.height(4.dp))
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                mission.metadataModel?.let { Tag(it) }
                mission.metadataSource?.let { Tag(it) }
                mission.workspaceName?.let { Tag(it) }
            }
        }
    }
}

@Composable
private fun Tag(text: String) {
    Text(
        text,
        color = Palette.TextTertiary,
        style = MaterialTheme.typography.labelSmall,
        modifier = Modifier
            .background(Palette.BackgroundTertiary, RoundedCornerShape(4.dp))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
private fun GoalBanner(status: String) {
    val color = when (status) {
        "complete" -> Palette.Success
        "paused", "budgetLimited" -> Palette.Warning
        "active" -> Palette.Info
        else -> Palette.TextTertiary
    }
    Row(
        Modifier.fillMaxWidth().background(color.copy(alpha = 0.12f)).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Filled.Flag, null, tint = color, modifier = Modifier.size(16.dp))
        Spacer(Modifier.width(8.dp))
        Text("/goal · $status", color = color, style = MaterialTheme.typography.labelMedium)
    }
}

@Composable
private fun QueueBar(queue: List<QueuedMessage>, onDelete: (String) -> Unit, onClear: () -> Unit) {
    Column(Modifier.fillMaxWidth().background(Palette.BackgroundSecondary).padding(horizontal = 12.dp, vertical = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Schedule, null, tint = Palette.AccentLight, modifier = Modifier.size(14.dp))
            Spacer(Modifier.width(6.dp))
            Text("Queued · ${queue.size}", color = Palette.AccentLight, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f))
            IconButton(onClick = onClear) { Icon(Icons.Filled.Close, "Clear queue", tint = Palette.TextTertiary) }
        }
        LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.fillMaxWidth()) {
            items(queue, key = { it.id }) { q ->
                Row(
                    modifier = Modifier
                        .background(Palette.Card, RoundedCornerShape(8.dp))
                        .border(1.dp, Palette.Border, RoundedCornerShape(8.dp))
                        .padding(horizontal = 8.dp, vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(q.displayContent, color = Palette.TextPrimary, style = MaterialTheme.typography.bodySmall, maxLines = 1)
                    Spacer(Modifier.width(6.dp))
                    Icon(Icons.Filled.Close, "Remove", tint = Palette.TextTertiary, modifier = Modifier.size(14.dp).clickable { onDelete(q.id) })
                }
            }
        }
    }
}

@Composable
private fun ParallelBar(running: List<RunningMissionInfo>, currentId: String?, onSwitch: (String) -> Unit) {
    LazyRow(
        modifier = Modifier
            .fillMaxWidth()
            .background(Palette.BackgroundSecondary)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        items(running, key = { it.missionId }) { r ->
            val color = when {
                r.isSeverelyStalled -> Palette.Error
                r.isStalled -> Palette.Warning
                r.isRunning -> Palette.Success
                else -> Palette.TextTertiary
            }
            val active = r.missionId == currentId
            Row(
                modifier = Modifier
                    .background(if (active) Palette.Accent.copy(alpha = 0.16f) else Palette.Card, RoundedCornerShape(999.dp))
                    .border(1.dp, if (active) Palette.Accent else Palette.Border, RoundedCornerShape(999.dp))
                    .clickable { onSwitch(r.missionId) }
                    .padding(horizontal = 10.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(8.dp).background(color, RoundedCornerShape(4.dp)))
                Spacer(Modifier.width(6.dp))
                Text(r.title?.take(20) ?: r.missionId.take(8), style = MaterialTheme.typography.labelMedium, color = Palette.TextPrimary)
            }
        }
    }
}

@Composable
private fun MessageRow(msg: ChatMessage) {
    when (val k = msg.kind) {
        ChatMessageKind.User -> Bubble(msg.content, mine = true)
        is ChatMessageKind.Assistant -> AssistantBubble(msg.content, k)
        is ChatMessageKind.Thinking -> SystemNote(if (k.done) "thinking complete" else "thinking…", muted = true, body = msg.content)
        is ChatMessageKind.Phase -> SystemNote("phase: ${k.phase}${k.detail?.let { " — $it" } ?: ""}")
        is ChatMessageKind.ToolCall -> ToolCallRow(k.name, k.isActive, msg.content)
        is ChatMessageKind.ToolUi -> ToolUiWidget(k.content)
        is ChatMessageKind.Goal -> SystemNote("goal · iter ${k.iteration} · ${k.status}", body = k.objective.takeIf { it.isNotBlank() })
        ChatMessageKind.SystemNote -> SystemNote(msg.content)
        ChatMessageKind.ErrorMsg -> ErrorBanner(msg.content)
    }
}

@Composable
private fun Bubble(text: String, mine: Boolean) {
    val bg = if (mine) Palette.Accent else Palette.Card
    val fg = if (mine) Color(0xFFFFFFFF) else Palette.TextPrimary
    Row(Modifier.fillMaxWidth(), horizontalArrangement = if (mine) Arrangement.End else Arrangement.Start) {
        Column(
            Modifier
                .widthIn(max = 320.dp)
                .background(bg, RoundedCornerShape(16.dp))
                .padding(horizontal = 12.dp, vertical = 10.dp),
        ) {
            Text(text, color = fg, style = MaterialTheme.typography.bodyMedium)
        }
    }
}

@Composable
private fun AssistantBubble(text: String, a: ChatMessageKind.Assistant) {
    Column(Modifier.fillMaxWidth()) {
        Bubble(text, mine = false)
        if (a.sharedFiles.isNotEmpty()) {
            Spacer(Modifier.height(6.dp))
            LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                items(a.sharedFiles, key = { it.url }) { f -> SharedFileChip(f) }
            }
        }
        formatAssistantFooter(a)?.let {
            Spacer(Modifier.height(4.dp))
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(costSourceIcon(a.costSource), null, tint = Palette.TextTertiary, modifier = Modifier.size(12.dp))
                Spacer(Modifier.width(4.dp))
                Text(it, color = Palette.TextTertiary, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

private fun costSourceIcon(source: String): ImageVector = when (source) {
    "actual" -> Icons.Filled.PlayArrow
    "estimated" -> Icons.Filled.Schedule
    else -> Icons.Filled.PlayArrow
}

@Composable
private fun SharedFileChip(f: SharedFile) {
    val ctx = LocalContext.current
    Row(
        modifier = Modifier
            .background(Palette.Card, RoundedCornerShape(8.dp))
            .border(1.dp, Palette.Border, RoundedCornerShape(8.dp))
            .padding(horizontal = 8.dp, vertical = 6.dp)
            .clickable {
                val intent = android.content.Intent(android.content.Intent.ACTION_VIEW, f.url.toUri())
                runCatching { ctx.startActivity(intent) }
            },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Filled.AttachFile, null, tint = Palette.AccentLight, modifier = Modifier.size(14.dp))
        Spacer(Modifier.width(6.dp))
        Text(f.name.ifBlank { "file" }, color = Palette.TextPrimary, style = MaterialTheme.typography.labelMedium)
        Spacer(Modifier.width(6.dp))
        Icon(Icons.AutoMirrored.Filled.OpenInNew, null, tint = Palette.TextTertiary, modifier = Modifier.size(12.dp))
    }
}

private fun formatAssistantFooter(a: ChatMessageKind.Assistant): String? {
    val parts = buildList<String> {
        a.model?.let { add(it) }
        if (a.costCents > 0) add("$" + "%.2f".format(a.costCents / 100.0))
        if (a.costSource == "estimated") add("est.")
    }
    return parts.takeIf { it.isNotEmpty() }?.joinToString(" • ")
}

@Composable
private fun SystemNote(label: String, body: String? = null, muted: Boolean = false) {
    GlassCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Text(label, color = if (muted) Palette.TextTertiary else Palette.TextSecondary, style = MaterialTheme.typography.labelMedium)
            if (!body.isNullOrBlank()) {
                Spacer(Modifier.height(4.dp))
                Text(body, color = Palette.TextSecondary, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun ToolCallRow(name: String, active: Boolean, args: String) {
    GlassCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (active) CircularProgressIndicator(strokeWidth = 2.dp, modifier = Modifier.size(14.dp), color = Palette.Accent)
                if (active) Spacer(Modifier.width(8.dp))
                Text("tool: $name", color = Palette.AccentLight, style = MaterialTheme.typography.labelLarge)
            }
            if (args.isNotBlank()) {
                Spacer(Modifier.height(4.dp))
                Text(args.take(400), color = Palette.TextTertiary, style = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 12.sp))
            }
        }
    }
}

@Composable
private fun Composer(value: String, onChange: (String) -> Unit, onSend: () -> Unit, onCancel: () -> Unit, isSending: Boolean) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .background(Palette.BackgroundSecondary)
            .padding(12.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        Box(
            Modifier
                .weight(1f)
                .heightIn(min = 44.dp)
                .background(Palette.Card, RoundedCornerShape(20.dp))
                .border(1.dp, Palette.Border, RoundedCornerShape(20.dp))
                .padding(horizontal = 14.dp, vertical = 10.dp),
        ) {
            BasicTextField(
                value = value,
                onValueChange = onChange,
                cursorBrush = SolidColor(Palette.Accent),
                textStyle = MaterialTheme.typography.bodyMedium.copy(color = Palette.TextPrimary),
                modifier = Modifier.fillMaxWidth(),
            )
            if (value.isEmpty()) {
                Text("Message…", color = Palette.TextMuted, style = MaterialTheme.typography.bodyMedium)
            }
        }
        Spacer(Modifier.width(8.dp))
        IconButton(onClick = if (isSending) onCancel else onSend, enabled = isSending || value.isNotBlank()) {
            Icon(
                if (isSending) Icons.Filled.Cancel else Icons.AutoMirrored.Filled.Send,
                contentDescription = if (isSending) "Cancel" else "Send",
                tint = if (isSending) Palette.Error else Palette.Accent,
            )
        }
    }
}
