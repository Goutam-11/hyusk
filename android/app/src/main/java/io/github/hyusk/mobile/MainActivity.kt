package io.github.hyusk.mobile

import android.content.Intent
import android.os.Bundle
import android.os.SystemClock
import android.provider.Settings
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.EnterTransition
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.animation.core.tween
import androidx.activity.ComponentActivity
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.Image
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Apps
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Bolt
import androidx.compose.material.icons.filled.CheckCircle
import androidx.compose.material.icons.filled.Devices
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Memory
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Wifi
import androidx.compose.material3.Button
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.windowsizeclass.WindowWidthSizeClass
import androidx.compose.material3.windowsizeclass.calculateWindowSizeClass
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.lifecycle.lifecycleScope
import io.github.hyusk.mobile.security.ProviderSettings
import io.github.hyusk.mobile.security.ProviderSettingsStore
import io.github.hyusk.mobile.security.LocalModelSettingsStore
import io.github.hyusk.mobile.security.LocalModelSettings
import io.github.hyusk.mobile.network.HyuskRpcClient
import io.github.hyusk.mobile.network.PairingState
import io.github.hyusk.mobile.services.EmergencyStop
import io.github.hyusk.mobile.services.HyuskAccessibilityService
import io.github.hyusk.mobile.voice.VoiceController
import io.github.hyusk.mobile.voice.isVoiceStopCommand
import io.github.hyusk.mobile.network.LocalModelClient
import io.github.hyusk.mobile.network.ModelCatalogRepository
import io.github.hyusk.mobile.network.ModelCatalogState
import io.github.hyusk.mobile.data.ModelCatalogItem
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import io.github.hyusk.mobile.automation.DeviceActionExecutor
import io.github.hyusk.mobile.automation.PhoneAgentTaskManager
import io.github.hyusk.mobile.automation.PhoneTaskPhase
import io.github.hyusk.mobile.automation.InstalledApp
import io.github.hyusk.mobile.automation.WorkflowDefinition
import io.github.hyusk.mobile.automation.WorkflowNode
import io.github.hyusk.mobile.automation.executeWorkflowDefinition
import io.github.hyusk.mobile.data.HyuskRepository
import io.github.hyusk.mobile.data.MemorySummary
import io.github.hyusk.mobile.data.ProviderIdentity
import io.github.hyusk.mobile.data.RunSummary
import io.github.hyusk.mobile.data.WorkflowEntity
import io.github.hyusk.mobile.ui.VoiceHomeScreen
import io.github.hyusk.mobile.ui.VoicePhase
import org.json.JSONObject
import java.text.DateFormat
import java.util.Date
import java.util.UUID

private data class PendingInvocation(val id: String, val action: String, val arguments: JSONObject)

private enum class Destination(val label: String, val icon: androidx.compose.ui.graphics.vector.ImageVector) {
    Home("Home", Icons.Default.Bolt), Devices("Devices", Icons.Default.Devices), Workflows("Flows", Icons.Default.PlayArrow),
    Memory("Memory", Icons.Default.Memory), Activity("Runs", Icons.Default.History), Settings("Settings", Icons.Default.Settings)
}

// Secondary destinations use the same visual language as the voice home.
private val UiInk = Color(0xFF050607)
private val UiPanelRaised = Color(0xFF171A1F)
private val UiPearl = Color(0xFFF4F2ED)
private val UiMuted = Color(0xFF92969D)
private val UiLine = Color(0xFF2A2D33)

class MainActivity : ComponentActivity() {
    companion object {
        const val EXTRA_WAKE_LISTENING = "hyusk.wake.listening"
        const val EXTRA_WAKE_COMMAND = "hyusk.wake.command"
    }
    private val _wakeRequest = MutableStateFlow<String?>(null)
    val wakeRequest: StateFlow<String?> = _wakeRequest

    @OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class, androidx.compose.material3.windowsizeclass.ExperimentalMaterial3WindowSizeClassApi::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        consumeWakeIntent(intent)
        setContent {
            val width = calculateWindowSizeClass(this).widthSizeClass
            // Hyusk deliberately uses a dark command-deck surface. Keep the
            // Material semantic palette dark even when Android is in light mode
            // so dialogs, fields, and buttons retain readable contrast.
            val colors = if (android.os.Build.VERSION.SDK_INT >= 31) dynamicDarkColorScheme(this)
            else darkColorScheme(primary = Color(0xFFB7C4FF), secondary = Color(0xFFD0BCFF))
            MaterialTheme(colorScheme = colors) {
                HyuskApp(width)
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        consumeWakeIntent(intent)
    }

    fun clearWakeRequest() { _wakeRequest.value = null }

    private fun consumeWakeIntent(intent: Intent?) {
        if (intent?.getBooleanExtra(EXTRA_WAKE_LISTENING, false) == true) {
            _wakeRequest.value = intent.getStringExtra(EXTRA_WAKE_COMMAND).orEmpty()
            intent.removeExtra(EXTRA_WAKE_LISTENING)
            intent.removeExtra(EXTRA_WAKE_COMMAND)
        }
    }
}

@Composable
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
private fun HyuskApp(width: WindowWidthSizeClass) {
    val context = LocalContext.current
    val activity = context as? MainActivity
    val wakeRequest by (activity?.wakeRequest ?: kotlinx.coroutines.flow.flowOf(null)).collectAsState(initial = null)
    val rpc = remember { HyuskRpcClient() }
    val repository = remember { HyuskRepository(context) }
    val executor = remember { DeviceActionExecutor(context.applicationContext) }
    val localModelStore = remember { LocalModelSettingsStore(context.applicationContext) }
    val phoneTasks = remember { PhoneAgentTaskManager.get(context.applicationContext) }
    var pendingInvocation by remember { mutableStateOf<PendingInvocation?>(null) }
    DisposableEffect(rpc) { onDispose { rpc.close() } }
    DisposableEffect(repository) { onDispose { repository.close() } }
    LaunchedEffect(rpc) {
        val saved = ProviderSettingsStore(context).settings.first()
        if (saved.endpoint.isNotBlank()) runCatching { rpc.connect(saved) }
    }
    LaunchedEffect(rpc, executor) {
        rpc.notifications.collect { message ->
            if (message.optString("method") != "device.invoke") return@collect
            val params = message.getJSONObject("params")
            val invocation = PendingInvocation(
                params.getString("invocation_id"),
                params.getString("action"),
                params.optJSONObject("arguments") ?: JSONObject(),
            )
            val locallySafe = executor.risk(invocation.action, invocation.arguments) == io.github.hyusk.mobile.automation.ActionRisk.Safe
            if (params.optString("risk", "confirm") == "safe" && locallySafe) {
                sendInvocationResult(rpc, invocation, executor.execute(invocation.action, invocation.arguments))
            } else pendingInvocation = invocation
        }
    }
    pendingInvocation?.let { invocation ->
        AlertDialog(
            onDismissRequest = { pendingInvocation = null },
            title = { Text("Allow phone action?") },
            text = { Text("Laptop Hyusk wants to run ${invocation.action}. Review the exact action before continuing.") },
            confirmButton = { TextButton(onClick = {
                sendInvocationResult(rpc, invocation, executor.execute(invocation.action, invocation.arguments))
                pendingInvocation = null
            }) { Text("Allow once") } },
            dismissButton = { TextButton(onClick = {
                rpc.call("device.invoke.result", JSONObject().put("invocation_id", invocation.id)
                    .put("success", false).put("error", "Denied on phone"))
                pendingInvocation = null
            }) { Text("Deny") } },
        )
    }
    var destination by remember { mutableStateOf(Destination.Home) }
    val onboarding = remember { context.getSharedPreferences("hyusk_onboarding", android.content.Context.MODE_PRIVATE) }
    var firstRun by remember { mutableStateOf(!onboarding.getBoolean("complete", false)) }
    if (firstRun) {
        val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { }
        Onboarding(onContinue = {
            val requested = buildList {
                add(android.Manifest.permission.RECORD_AUDIO)
                if (android.os.Build.VERSION.SDK_INT >= 33) add(android.Manifest.permission.POST_NOTIFICATIONS)
            }
            permissionLauncher.launch(requested.toTypedArray())
            onboarding.edit().putBoolean("complete", true).apply()
            firstRun = false
        })
        return
    }
    val rail = width != WindowWidthSizeClass.Compact
    val animationsEnabled = remember(context) {
        runCatching {
            Settings.Global.getFloat(context.contentResolver, Settings.Global.TRANSITION_ANIMATION_SCALE, 1f) > 0f
        }.getOrDefault(true)
    }
    val pageContent: @Composable (Modifier) -> Unit = { modifier ->
        AnimatedContent(
            targetState = destination,
            modifier = modifier.fillMaxSize(),
            transitionSpec = {
                if (!animationsEnabled) {
                    EnterTransition.None togetherWith ExitTransition.None
                } else {
                    val direction = if (targetState.ordinal >= initialState.ordinal) 1 else -1
                    (fadeIn(tween(180)) + slideInHorizontally(tween(220)) { direction * it / 10 }) togetherWith
                        (fadeOut(tween(120)) + slideOutHorizontally(tween(170)) { -direction * it / 14 })
                }
            },
            label = "Hyusk destination",
        ) { current ->
            when (current) {
                Destination.Home -> CommandDeck(
                    rpc,
                    phoneTasks,
                    wakeRequest = wakeRequest,
                    onWakeHandled = { activity?.clearWakeRequest() },
                )
                Destination.Devices -> DevicesScreen(rpc)
                Destination.Workflows -> WorkflowsScreen(repository)
                Destination.Memory -> MemoryScreen(repository)
                Destination.Activity -> ActivityScreen(repository)
                Destination.Settings -> SettingsScreen(localModelStore, onOpenDevices = { destination = Destination.Devices })
            }
        }
    }
    Surface(Modifier.fillMaxSize(), color = UiInk, contentColor = UiPearl) {
        Box(Modifier.fillMaxSize()) {
            HyuskBackdrop()
            if (rail) {
                Row(Modifier.fillMaxSize()) {
                    RailNav(destination) { destination = it }
                    pageContent(Modifier.weight(1f))
                }
            } else {
                // A navigation bar slot reserves a full-width strip. Overlay
                // the glass instead so each page remains visible beneath it.
                Box(Modifier.fillMaxSize()) {
                    pageContent(Modifier.fillMaxSize())
                    BottomNav(
                        selected = destination,
                        onSelected = { destination = it },
                        modifier = Modifier.align(Alignment.BottomCenter),
                    )
                }
            }
        }
    }
}

@Composable
private fun HyuskBackdrop() {
    Canvas(Modifier.fillMaxSize()) {
        drawCircle(
            brush = Brush.radialGradient(
                colors = listOf(Color(0xFF82AAFF).copy(alpha = .16f), Color.Transparent),
                center = Offset(size.width * .90f, size.height * .18f),
                radius = size.minDimension * .62f,
            ),
            radius = size.minDimension * .62f,
            center = Offset(size.width * .90f, size.height * .18f),
        )
        drawCircle(
            brush = Brush.radialGradient(
                colors = listOf(Color(0xFF78E6AA).copy(alpha = .08f), Color.Transparent),
                center = Offset(size.width * .08f, size.height * .72f),
                radius = size.minDimension * .48f,
            ),
            radius = size.minDimension * .48f,
            center = Offset(size.width * .08f, size.height * .72f),
        )
        drawCircle(
            color = UiPearl.copy(alpha = .045f),
            radius = size.minDimension * .24f,
            center = Offset(size.width * .86f, size.height * .84f),
        )
    }
}

private fun sendInvocationResult(
    rpc: HyuskRpcClient,
    invocation: PendingInvocation,
    result: io.github.hyusk.mobile.automation.ActionResult,
) {
    runCatching {
        rpc.call("device.invoke.result", JSONObject()
            .put("invocation_id", invocation.id)
            .put("success", result.success)
            .put("output", result.data.put("message", result.message))
            .put("error", if (result.success) JSONObject.NULL else result.message))
    }
}

@Composable
private fun Onboarding(onContinue: () -> Unit) {
    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().padding(28.dp), verticalArrangement = Arrangement.Center) {
            Text("Hyusk", style = MaterialTheme.typography.displaySmall, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(12.dp))
            Text("A calm command deck for your devices.", style = MaterialTheme.typography.headlineSmall)
            Spacer(Modifier.height(12.dp))
            Text("Pair a trusted provider, then give Hyusk only the capabilities your workflows need. You can stop every action instantly.")
            Spacer(Modifier.height(28.dp))
            Button(onClick = onContinue, modifier = Modifier.fillMaxWidth()) { Text("Set up Hyusk") }
        }
    }
}

@Composable
private fun CommandDeck(
    rpc: HyuskRpcClient,
    phoneTasks: PhoneAgentTaskManager,
    wakeRequest: String?,
    onWakeHandled: () -> Unit,
) {
    val context = LocalContext.current
    val voice = remember { VoiceController(context) }
    val voiceState by voice.state.collectAsState()
    var command by remember { mutableStateOf("") }
    var resultMessage by remember { mutableStateOf(phoneTasks.state.value.message.takeIf(String::isNotBlank)) }
    var processing by remember { mutableStateOf(phoneTasks.state.value.phase == PhoneTaskPhase.Working) }
    var lastSubmittedCommand by remember { mutableStateOf("") }
    var lastSubmittedAt by remember { mutableLongStateOf(0L) }
    val phoneTask by phoneTasks.state.collectAsState()
    var lastHandledTaskRevision by remember { mutableLongStateOf(phoneTasks.state.value.revision) }
    var ownsPhoneTask by remember { mutableStateOf(false) }
    var listenAfterReply by remember { mutableStateOf(false) }
    var replySpeechStarted by remember { mutableStateOf(false) }
    DisposableEffect(voice) { onDispose { voice.close() } }
    val stopped by EmergencyStop.engaged.collectAsState()
    val connection by rpc.state.collectAsState()
    LaunchedEffect(rpc) {
        rpc.notifications.collect { event ->
            if (event.optString("method") != "event") return@collect
            val params = event.optJSONObject("params") ?: return@collect
            val status = params.optJSONObject("status")
            val latest = status?.optJSONObject("latest")
            latest?.optString("text")?.takeIf(String::isNotBlank)?.let { text ->
                resultMessage = text
                processing = status.optString("state") in setOf("Waking", "Listening", "Thinking", "Working", "Speaking")
                if (!processing && latest.optString("kind") in setOf("response", "approval", "task")) {
                    listenAfterReply = latest.optBoolean("needs_reply", false)
                    replySpeechStarted = false
                    voice.speak(text)
                }
            }
        }
    }
    LaunchedEffect(phoneTask.revision) {
        if (phoneTask.revision <= lastHandledTaskRevision) return@LaunchedEffect
        lastHandledTaskRevision = phoneTask.revision
        resultMessage = phoneTask.message
        processing = phoneTask.phase == PhoneTaskPhase.Working
        if (!ownsPhoneTask) return@LaunchedEffect
        if (phoneTask.phase == PhoneTaskPhase.Working) {
            if (phoneTask.speakProgress) voice.speak(phoneTask.message)
        } else if (phoneTask.phase != PhoneTaskPhase.Idle) {
            listenAfterReply = phoneTask.needsReply
            replySpeechStarted = false
            voice.speak(phoneTask.message)
            ownsPhoneTask = false
        }
    }
    val submitCommand: (String) -> Unit = { raw ->
        val text = raw.trim()
        val normalized = text.lowercase().replace(Regex("\\s+"), " ")
        if (isVoiceStopCommand(text)) {
            if (connection is PairingState.Paired) runCatching { rpc.submit("go away") }
            phoneTasks.stop()
            voice.stopAll()
            processing = false
            listenAfterReply = false
            replySpeechStarted = false
            command = ""
            resultMessage = "Conversation ended."
        } else {
            val now = SystemClock.elapsedRealtime()
            val duplicate = normalized == lastSubmittedCommand && now - lastSubmittedAt < 4_000L
            if (text.isNotBlank() && !processing && !stopped && !duplicate) {
                lastSubmittedCommand = normalized
                lastSubmittedAt = now
                processing = true
                command = ""
                if (connection is PairingState.Paired) {
                    runCatching { rpc.submit(text) }
                        .onSuccess { resultMessage = "Laptop accepted the task…" }
                        .onFailure { resultMessage = it.message ?: "Laptop connection failed" }
                    processing = false
                } else {
                    ownsPhoneTask = phoneTasks.submit(text)
                    if (!ownsPhoneTask) {
                        processing = phoneTasks.state.value.phase == PhoneTaskPhase.Working
                        resultMessage = "A phone task is already running. Stop it or wait for it to finish."
                    }
                }
            }
        }
    }
    LaunchedEffect(voiceState.speaking, listenAfterReply) {
        if (!listenAfterReply) return@LaunchedEffect
        if (voiceState.speaking) {
            replySpeechStarted = true
        } else if (replySpeechStarted) {
            // Reopen the microphone only after the final/ask-user response has
            // finished speaking. A normal completed task returns to idle.
            listenAfterReply = false
            replySpeechStarted = false
            resultMessage = null
            voice.start()
        }
    }
    LaunchedEffect(voiceState.transcript, voiceState.listening) {
        if (voiceState.listening && voiceState.transcript.isNotBlank()) command = voiceState.transcript
    }
    LaunchedEffect(voiceState.finalResultId) {
        if (voiceState.finalResultId > 0) {
            val finalText = voiceState.transcript.trim()
            if (finalText.isNotBlank()) {
                command = finalText
                submitCommand(finalText)
            }
        }
    }
    LaunchedEffect(wakeRequest) {
        if (wakeRequest != null) {
            resultMessage = null
            if (wakeRequest.isBlank()) voice.start() else submitCommand(wakeRequest)
            onWakeHandled()
        }
    }
    val phase = when {
        stopped -> VoicePhase.Stopped
        voiceState.error != null -> VoicePhase.Error
        voiceState.listening -> VoicePhase.Listening
        processing -> VoicePhase.Thinking
        voiceState.speaking -> VoicePhase.Speaking
        else -> VoicePhase.Ready
    }
    VoiceHomeScreen(
        phase = phase,
        transcript = voiceState.transcript,
        response = resultMessage ?: voiceState.error,
        command = command,
        connectionLabel = if (connection is PairingState.Paired) "Laptop linked" else "Phone agent",
        onCommandChange = { command = it },
        onMic = {
            if (stopped) Unit
            else if (voiceState.listening) voice.stop()
            else { resultMessage = null; voice.start() }
        },
        onSubmit = { submitCommand(command) },
        onStop = {
            phoneTasks.stop()
            processing = false
            listenAfterReply = false
            replySpeechStarted = false
            voice.stopAll()
            if (stopped) EmergencyStop.reset() else EmergencyStop.engage()
        },
    )
}
@Composable private fun EmptyState(title: String, message: String) { Column(Modifier.fillMaxWidth().padding(vertical = 14.dp)) { Text(title, style = MaterialTheme.typography.bodyLarge); Text(message, color = MaterialTheme.colorScheme.onSurfaceVariant) } }

@Composable
private fun DevicesScreen(client: HyuskRpcClient) {
    val context = LocalContext.current
    val state by client.state.collectAsState()
    val store = remember { ProviderSettingsStore(context.applicationContext) }
    val saved by store.settings.collectAsState(initial = ProviderSettings())
    val scope = rememberCoroutineScope()
    var pairingCode by remember { mutableStateOf("") }
    var message by remember { mutableStateOf<String?>(null) }
    var savingPairing by remember { mutableStateOf(false) }
    Screen("Devices", scrollContent = true) {
        Text("Laptop connection", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
        Text(when (val current = state) {
            PairingState.Disconnected -> "Not connected"
            PairingState.Connecting -> "Connecting securely…"
            PairingState.Authenticating -> "Authenticating this phone…"
            is PairingState.Paired -> "Paired · ${current.deviceId}"
            is PairingState.Failed -> "Connection failed · ${current.message}"
        }, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (saved.endpoint.isBlank()) {
            Card(colors = CardDefaults.cardColors(containerColor = UiPanelRaised), modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text("1 · Enable the laptop link once", color = UiPearl, style = MaterialTheme.typography.titleMedium)
                    Text("On the laptop, open the Hyusk butterfly menu → Connect phone → Enable laptop link.", color = UiMuted)
                    Text("2 · Get a fresh code", color = UiPearl, style = MaterialTheme.typography.titleMedium)
                    Text("Open the Hyusk butterfly menu on the laptop → Connect phone → Generate fresh code → Copy pairing code.", color = UiMuted)
                    Text("3 · Paste it below", color = UiPearl, style = MaterialTheme.typography.titleMedium)
                    Text("Transfer the code to this phone privately. It expires in five minutes and works once.", color = UiMuted)
                }
            }
        } else {
            Card(colors = CardDefaults.cardColors(containerColor = UiPanelRaised), modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("Saved laptop", color = UiPearl, style = MaterialTheme.typography.titleMedium)
                    Text(saved.endpoint, color = UiMuted)
                    Text("Identity ${saved.deviceId.take(8)}…", color = UiMuted, style = MaterialTheme.typography.bodySmall)
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(
                    onClick = { runCatching { client.connect(saved) }.onFailure { message = it.message } },
                    modifier = Modifier.weight(1f),
                    enabled = state !is PairingState.Connecting && state !is PairingState.Authenticating && state !is PairingState.Paired,
                ) { Icon(Icons.Default.Wifi, null); Spacer(Modifier.width(8.dp)); Text("Connect") }
                OutlinedButton(onClick = { client.close(); message = "Disconnected" }, modifier = Modifier.weight(1f)) { Text("Disconnect") }
            }
        }
        HorizontalDivider()
        Text(if (saved.endpoint.isBlank()) "Pair this phone" else "Replace pairing", color = UiPearl, style = MaterialTheme.typography.titleLarge)
        OutlinedTextField(
            pairingCode,
            { pairingCode = it },
            Modifier.fillMaxWidth(),
            label = { Text("One-time code from laptop") },
            supportingText = { Text("The code contains a pinned TLS identity and expires after five minutes.") },
            minLines = 4,
            colors = hyuskTextFieldColors(),
        )
        OutlinedButton(
            onClick = {
                val clipboard = context.getSystemService(android.content.ClipboardManager::class.java)
                pairingCode = clipboard?.primaryClip?.getItemAt(0)?.coerceToText(context)?.toString().orEmpty()
                if (pairingCode.isBlank()) message = "The clipboard does not contain a pairing code"
            },
            modifier = Modifier.fillMaxWidth(),
        ) { Text("Paste from clipboard") }
        Button(
            onClick = {
                scope.launch {
                    savingPairing = true
                    runCatching {
                        val next = ProviderSettings.fromPairingPayload(pairingCode, saved, replaceIdentity = saved.endpoint.isNotBlank())
                        store.save(next)
                        client.connect(next)
                    }.onSuccess {
                        pairingCode = ""
                        message = "Pairing saved. Connecting securely…"
                    }.onFailure { message = it.message ?: "Could not pair this phone" }
                    savingPairing = false
                }
            },
            enabled = pairingCode.isNotBlank() && !savingPairing,
            modifier = Modifier.fillMaxWidth(),
        ) { Text(if (savingPairing) "Pairing…" else if (saved.endpoint.isBlank()) "Pair and connect" else "Replace and reconnect") }
        if (saved.endpoint.isNotBlank()) {
            TextButton(onClick = {
                scope.launch {
                    client.close()
                    store.clear()
                    message = "Saved laptop and phone link identity removed"
                }
            }, modifier = Modifier.fillMaxWidth()) { Text("Forget laptop", color = MaterialTheme.colorScheme.error) }
        }
        message?.let { Text(it, color = if (it.contains("failed", true) || it.contains("expired", true) || it.contains("could not", true)) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary) }
    }
}
private data class WorkflowDraft(
    val id: String? = null,
    val name: String = "",
    val definition: WorkflowDefinition = WorkflowDefinition(),
    val voicePhrases: String = "",
    val scope: String = "global",
    val originDevice: String = "this-device",
    val enabled: Boolean = true,
)

@Composable
private fun WorkflowsScreen(repository: HyuskRepository) {
    val context = LocalContext.current
    val workflows by repository.workflows.collectAsState(initial = emptyList())
    val scope = rememberCoroutineScope()
    var search by remember { mutableStateOf("") }
    var draft by remember { mutableStateOf<WorkflowDraft?>(null) }
    var editorOpen by remember { mutableStateOf(false) }
    var deleteTarget by remember { mutableStateOf<WorkflowEntity?>(null) }
    var saveStatus by remember { mutableStateOf("Ready") }
    var deviceId by remember { mutableStateOf("this-device") }
    var definitions by remember { mutableStateOf<Map<String, WorkflowDefinition>>(emptyMap()) }
    val executor = remember { DeviceActionExecutor(context.applicationContext) }

    LaunchedEffect(Unit) {
        deviceId = ProviderSettingsStore(context).settings.first().deviceId.ifBlank { "this-device" }
    }

    LaunchedEffect(workflows) {
        definitions = withContext(Dispatchers.IO) {
            workflows.mapNotNull { workflow ->
                runCatching {
                    workflow.id to WorkflowDefinition.parse(repository.decryptWorkflowDefinition(workflow))
                }.getOrNull()
            }.toMap()
        }
    }

    val visibleWorkflows = workflows.filter { workflow ->
        search.isBlank() || workflow.name.contains(search, ignoreCase = true) ||
            workflow.scope.contains(search, ignoreCase = true) ||
            workflow.originDevice.contains(search, ignoreCase = true) ||
            definitions[workflow.id]?.voiceTriggers?.any { it.contains(search, ignoreCase = true) } == true
    }

    Screen("Workflows") {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("${visibleWorkflows.size} shown", color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            Button(onClick = {
                draft = WorkflowDraft(originDevice = deviceId)
                editorOpen = true
                saveStatus = "Ready to save"
            }) { Text("New workflow") }
        }
        OutlinedTextField(
            value = search,
            onValueChange = { search = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Search workflows") },
            singleLine = true,
            colors = hyuskTextFieldColors(),
        )
        Text(saveStatus, color = if (saveStatus.startsWith("Save failed")) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary)
        if (visibleWorkflows.isEmpty()) {
            EmptyState(
                if (workflows.isEmpty()) "No workflows" else "No matching workflows",
                if (workflows.isEmpty()) "Create a shortcut, add actions, and run it by voice—no JSON needed." else "Try a different name, voice phrase, scope, or device.",
            )
        } else {
            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(visibleWorkflows, key = { it.id }) { workflow ->
                    WorkflowCard(
                        workflow = workflow,
                        definition = definitions[workflow.id],
                        onRun = {
                            scope.launch {
                                val runId = UUID.randomUUID().toString()
                                repository.saveAgentRun(
                                    runId,
                                    JSONObject()
                                        .put("kind", "workflow")
                                        .put("label", workflow.name)
                                        .put("prompt", "Run ${workflow.name}")
                                        .put("completed_steps", 0),
                                )
                                val outcome = runCatching {
                                    executeWorkflowDefinition(repository.decryptWorkflowDefinition(workflow), executor)
                                }
                                saveStatus = outcome.fold(
                                    onSuccess = {
                                        val message = "Completed: $it"
                                        repository.completeAgentRun(
                                            runId,
                                            message,
                                            completedSteps = definitions[workflow.id]?.nodes?.size,
                                        )
                                        message
                                    },
                                    onFailure = {
                                        val message = "Run failed: ${it.message ?: "invalid workflow"}"
                                        repository.completeAgentRun(runId, message, "failed")
                                        message
                                    },
                                )
                            }
                        },
                        onEdit = {
                            scope.launch {
                                try {
                                    val parsed = WorkflowDefinition.parse(repository.decryptWorkflowDefinition(workflow))
                                    draft = WorkflowDraft(
                                        id = workflow.id,
                                        name = workflow.name,
                                        definition = parsed,
                                        voicePhrases = parsed.voiceTriggers.joinToString(", "),
                                        scope = workflow.scope,
                                        originDevice = workflow.originDevice,
                                        enabled = workflow.enabled,
                                    )
                                    editorOpen = true
                                    saveStatus = "Ready to save"
                                } catch (error: Exception) {
                                    saveStatus = "Save failed: ${error.message ?: "Unable to decrypt definition"}"
                                }
                            }
                        },
                        onDelete = { deleteTarget = workflow },
                    )
                }
            }
        }
    }

    if (editorOpen && draft != null) {
        WorkflowEditor(
            draft = draft!!,
            saving = saveStatus == "Saving…",
            status = saveStatus,
            onDraftChange = { draft = it },
            onDismiss = { editorOpen = false },
            onSave = {
                val toSave = draft!!
                scope.launch {
                    saveStatus = "Saving…"
                    try {
                        repository.saveWorkflow(
                            id = toSave.id,
                            name = toSave.name,
                            definition = toSave.definition.copy(
                                voiceTriggers = toSave.voicePhrases.split(',')
                                    .map(String::trim)
                                    .filter(String::isNotBlank)
                                    .distinctBy(String::lowercase),
                            ).also { it.validate() }.toJson(),
                            scope = toSave.scope,
                            originDevice = toSave.originDevice,
                            enabled = toSave.enabled,
                        )
                        saveStatus = "Saved locally"
                        editorOpen = false
                    } catch (error: Exception) {
                        saveStatus = "Save failed: ${error.message ?: "Check the fields"}"
                    }
                }
            },
        )
    }

    deleteTarget?.let { workflow ->
        AlertDialog(
            onDismissRequest = { deleteTarget = null },
            title = { Text("Delete workflow?") },
            text = { Text("${workflow.name} will be hidden locally. Its encrypted record remains as a tombstone for sync.") },
            confirmButton = {
                TextButton(onClick = {
                    scope.launch {
                        try {
                            repository.deleteWorkflow(workflow.id)
                            saveStatus = "Deleted locally"
                        } catch (error: Exception) {
                            saveStatus = "Save failed: ${error.message ?: "Unable to delete workflow"}"
                        }
                        deleteTarget = null
                    }
                }) { Text("Delete") }
            },
            dismissButton = { TextButton(onClick = { deleteTarget = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun WorkflowCard(workflow: WorkflowEntity, definition: WorkflowDefinition?, onRun: () -> Unit, onEdit: () -> Unit, onDelete: () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(workflow.name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Text(if (workflow.enabled) "Enabled" else "Disabled", color = MaterialTheme.colorScheme.primary)
            }
            Text("Scope: ${workflow.scope} · Device: ${workflow.originDevice}", color = MaterialTheme.colorScheme.onSurfaceVariant)
            definition?.let {
                Text(
                    "${it.nodes.size} action${if (it.nodes.size == 1) "" else "s"}" +
                        it.voiceTriggers.firstOrNull()?.let { phrase -> " · Say “$phrase”" }.orEmpty(),
                    color = UiPearl,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            Text("Updated ${formatRecordTime(workflow.updatedAt)} · Encrypted on this phone", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = onRun, modifier = Modifier.weight(1f), enabled = workflow.enabled) { Icon(Icons.Default.PlayArrow, null); Spacer(Modifier.width(4.dp)); Text("Run") }
                OutlinedButton(onClick = onEdit, modifier = Modifier.weight(1f)) { Text("Edit") }
                TextButton(onClick = onDelete, modifier = Modifier.weight(1f)) { Text("Delete") }
            }
        }
    }
}

@Composable
private fun WorkflowEditor(
    draft: WorkflowDraft,
    saving: Boolean,
    status: String,
    onDraftChange: (WorkflowDraft) -> Unit,
    onDismiss: () -> Unit,
    onSave: () -> Unit,
) {
    val context = LocalContext.current
    val executor = remember { DeviceActionExecutor(context.applicationContext) }
    var choosingAction by remember { mutableStateOf(false) }
    var choosingAppFor by remember { mutableStateOf<Int?>(null) }
    fun updateNode(index: Int, node: WorkflowNode) {
        val nodes = draft.definition.nodes.toMutableList()
        nodes[index] = node
        onDraftChange(draft.copy(definition = draft.definition.copy(nodes = nodes)))
    }
    fun addNode(node: WorkflowNode) {
        onDraftChange(draft.copy(definition = draft.definition.copy(nodes = draft.definition.nodes + node)))
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().background(UiInk), color = UiInk) {
            Column(
                Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(22.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onDismiss, enabled = !saving) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = UiPearl) }
                    Text(if (draft.id == null) "New shortcut" else "Edit shortcut", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
                }
                Text("Build a repeatable command once, then run it by voice or from Tools.", color = UiMuted)
                OutlinedTextField(
                    draft.name,
                    { onDraftChange(draft.copy(name = it)) },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Shortcut name") },
                    placeholder = { Text("Morning routine") },
                    singleLine = true,
                    colors = hyuskTextFieldColors(),
                )
                OutlinedTextField(
                    value = draft.voicePhrases,
                    onValueChange = { onDraftChange(draft.copy(voicePhrases = it)) },
                    modifier = Modifier.fillMaxWidth(),
                    label = { Text("Voice phrases (optional)") },
                    placeholder = { Text("start my morning, begin study mode") },
                    supportingText = { Text("Separate phrases with commas. The shortcut name always works too.") },
                    minLines = 2,
                    colors = hyuskTextFieldColors(),
                )
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Actions", color = UiPearl, style = MaterialTheme.typography.titleMedium)
                        Text("They run from top to bottom.", color = UiMuted, style = MaterialTheme.typography.bodySmall)
                    }
                    Button(onClick = { choosingAction = true }) { Icon(Icons.Default.Add, null); Spacer(Modifier.width(6.dp)); Text("Add action") }
                }
                if (draft.definition.nodes.isEmpty()) {
                    EmptyState("No actions yet", "Add an app, website, timer, wait, or system action. No JSON is required.")
                }
                draft.definition.nodes.forEachIndexed { index, node ->
                    WorkflowNodeCard(
                        index = index,
                        node = node,
                        count = draft.definition.nodes.size,
                        onChange = { updateNode(index, it) },
                        onChooseApp = { choosingAppFor = index },
                        onMove = { direction ->
                            val target = index + direction
                            if (target in draft.definition.nodes.indices) {
                                val nodes = draft.definition.nodes.toMutableList()
                                val moved = nodes.removeAt(index)
                                nodes.add(target, moved)
                                onDraftChange(draft.copy(definition = draft.definition.copy(nodes = nodes)))
                            }
                        },
                        onRemove = {
                            onDraftChange(draft.copy(definition = draft.definition.copy(nodes = draft.definition.nodes.filterIndexed { itemIndex, _ -> itemIndex != index })))
                        },
                    )
                    if (index < draft.definition.nodes.lastIndex) Text("↓", color = UiMuted, modifier = Modifier.align(Alignment.CenterHorizontally))
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(checked = draft.enabled, onCheckedChange = { onDraftChange(draft.copy(enabled = it)) })
                    Text("Enabled", color = UiPearl)
                }
                Text(status, color = if (status.startsWith("Save failed")) MaterialTheme.colorScheme.error else UiMuted)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    TextButton(onClick = onDismiss, enabled = !saving) { Text("Cancel", color = UiMuted) }
                    Button(onClick = onSave, enabled = !saving && draft.name.isNotBlank() && draft.definition.nodes.isNotEmpty()) {
                        Text(if (saving) "Saving…" else "Save shortcut")
                    }
                }
            }
        }
    }

    if (choosingAction) {
        AlertDialog(
            onDismissRequest = { choosingAction = false },
            title = { Text("Add an action") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    listOf(
                        "apps.launch" to "Open an installed app",
                        "url.open" to "Open a website",
                        "web.search" to "Search the web",
                        "timer.create" to "Start a timer",
                        "workflow.delay" to "Wait before continuing",
                        "media.play_pause" to "Play or pause media",
                        "system.home" to "Go to the Home screen",
                        "system.notifications" to "Open notifications",
                    ).forEach { (action, label) ->
                        TextButton(onClick = {
                            val args = when (action) {
                                "timer.create" -> JSONObject().put("seconds", 1_800).put("label", "Hyusk timer")
                                "workflow.delay" -> JSONObject().put("milliseconds", 5_000)
                                else -> JSONObject()
                            }
                            addNode(WorkflowNode(action = action, arguments = args))
                            choosingAction = false
                            if (action == "apps.launch") choosingAppFor = draft.definition.nodes.size
                        }, modifier = Modifier.fillMaxWidth()) { Text(label, modifier = Modifier.fillMaxWidth()) }
                    }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = { choosingAction = false }) { Text("Cancel") } },
        )
    }
    choosingAppFor?.let { index ->
        AppPicker(
            loadApps = { executor.installedApps() },
            onDismiss = { choosingAppFor = null },
            onSelect = { app ->
                val current = draft.definition.nodes.getOrNull(index)
                if (current != null) updateNode(index, current.copy(arguments = JSONObject().put("package", app.packageName).put("label", app.label)))
                choosingAppFor = null
            },
        )
    }
}

@Composable
private fun WorkflowNodeCard(
    index: Int,
    node: WorkflowNode,
    count: Int,
    onChange: (WorkflowNode) -> Unit,
    onChooseApp: () -> Unit,
    onMove: (Int) -> Unit,
    onRemove: () -> Unit,
) {
    Card(colors = CardDefaults.cardColors(containerColor = UiPanelRaised), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("${index + 1}", color = UiInk, modifier = Modifier.background(UiPearl, RoundedCornerShape(20.dp)).padding(horizontal = 10.dp, vertical = 5.dp))
                Spacer(Modifier.width(10.dp))
                Column(Modifier.weight(1f)) {
                    Text(node.title(), color = UiPearl, style = MaterialTheme.typography.titleMedium)
                    Text(node.summary(), color = UiMuted, style = MaterialTheme.typography.bodySmall)
                }
                IconButton(onClick = { onMove(-1) }, enabled = index > 0) { Icon(Icons.Default.ArrowUpward, "Move up") }
                IconButton(onClick = { onMove(1) }, enabled = index + 1 < count) { Icon(Icons.Default.ArrowDownward, "Move down") }
                IconButton(onClick = onRemove) { Icon(Icons.Default.Delete, "Remove action", tint = MaterialTheme.colorScheme.error) }
            }
            when (node.action) {
                "apps.launch" -> OutlinedButton(onClick = onChooseApp, modifier = Modifier.fillMaxWidth()) {
                    Icon(Icons.Default.Apps, null); Spacer(Modifier.width(8.dp)); Text(if (node.arguments.optString("package").isBlank()) "Choose installed app" else "Change app")
                }
                "url.open" -> WorkflowArgumentField(node, "url", "Website URL", "https://example.com", onChange)
                "web.search" -> WorkflowArgumentField(node, "query", "Search query", "weather in Jaipur", onChange)
                "timer.create" -> WorkflowNumberField(node, "seconds", "Timer seconds", onChange)
                "workflow.delay" -> WorkflowNumberField(node, "milliseconds", "Wait milliseconds", onChange)
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Checkbox(node.continueOnError, { onChange(node.copy(continueOnError = it)) })
                Text("Continue if this action fails", color = UiMuted, style = MaterialTheme.typography.bodySmall)
            }
        }
    }
}

@Composable
private fun WorkflowArgumentField(
    node: WorkflowNode,
    key: String,
    label: String,
    placeholder: String,
    onChange: (WorkflowNode) -> Unit,
) {
    OutlinedTextField(
        value = node.arguments.optString(key),
        onValueChange = { value -> onChange(node.copy(arguments = JSONObject(node.arguments.toString()).put(key, value))) },
        modifier = Modifier.fillMaxWidth(),
        label = { Text(label) },
        placeholder = { Text(placeholder) },
        singleLine = true,
        colors = hyuskTextFieldColors(),
    )
}

@Composable
private fun WorkflowNumberField(
    node: WorkflowNode,
    key: String,
    label: String,
    onChange: (WorkflowNode) -> Unit,
) {
    OutlinedTextField(
        value = node.arguments.optLong(key).takeIf { it > 0 }?.toString().orEmpty(),
        onValueChange = { value ->
            val number = value.filter(Char::isDigit).toLongOrNull() ?: 0L
            onChange(node.copy(arguments = JSONObject(node.arguments.toString()).put(key, number)))
        },
        modifier = Modifier.fillMaxWidth(),
        label = { Text(label) },
        singleLine = true,
        colors = hyuskTextFieldColors(),
    )
}

@Composable
private fun AppPicker(loadApps: () -> List<InstalledApp>, onDismiss: () -> Unit, onSelect: (InstalledApp) -> Unit) {
    var query by remember { mutableStateOf("") }
    var apps by remember { mutableStateOf(emptyList<InstalledApp>()) }
    var loading by remember { mutableStateOf(true) }
    var refreshToken by remember { mutableIntStateOf(0) }
    LaunchedEffect(refreshToken) {
        loading = true
        apps = withContext(Dispatchers.Default) { loadApps() }
        loading = false
    }
    val visible = apps.filter { query.isBlank() || it.label.contains(query, true) || it.packageName.contains(query, true) }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = UiInk) {
            Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onDismiss) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                    Text("Choose an installed app", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
                }
                OutlinedTextField(query, { query = it }, Modifier.fillMaxWidth(), label = { Text("Search apps") }, singleLine = true, colors = hyuskTextFieldColors())
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    Text(if (loading) "Loading installed apps…" else "${visible.size} launchable apps", color = UiMuted, modifier = Modifier.weight(1f))
                    TextButton(onClick = { refreshToken++ }, enabled = !loading) { Text("Refresh") }
                }
                LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    items(visible, key = { it.packageName }) { app ->
                        TextButton(onClick = { onSelect(app) }, modifier = Modifier.fillMaxWidth()) {
                            Column(Modifier.fillMaxWidth()) {
                                Text(app.label, color = UiPearl, style = MaterialTheme.typography.bodyLarge)
                                Text(app.packageName, color = UiMuted, style = MaterialTheme.typography.bodySmall)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun MemoryScreen(repository: HyuskRepository) {
    val memories by repository.memories.collectAsState(initial = emptyList())
    var search by remember { mutableStateOf("") }
    val visibleMemories = memories.filter { memory ->
        search.isBlank() || memory.kind.contains(search, ignoreCase = true) ||
            memory.content.contains(search, ignoreCase = true) ||
            memory.scope.contains(search, ignoreCase = true) ||
            memory.originDevice.contains(search, ignoreCase = true)
    }

    Screen("Memory") {
        OutlinedTextField(
            value = search,
            onValueChange = { search = it },
            modifier = Modifier.fillMaxWidth(),
            label = { Text("Search encrypted memory") },
            singleLine = true,
            colors = hyuskTextFieldColors(),
        )
        Text("${visibleMemories.size} saved on this device · encrypted at rest", color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (visibleMemories.isEmpty()) {
            EmptyState(
                if (memories.isEmpty()) "Memory is empty" else "No matching memory",
                "Say “remember that …” or tell Hyusk a stable preference to save it here.",
            )
        } else {
            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(visibleMemories, key = { it.id }) { memory -> MemoryCard(memory) }
            }
        }
    }
}

@Composable
private fun MemoryCard(memory: MemorySummary) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(memory.kind.ifBlank { "Memory" }, style = MaterialTheme.typography.titleMedium)
            Text(memory.content, color = UiPearl, style = MaterialTheme.typography.bodyLarge)
            Text("Encrypted on disk", color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelMedium)
            Text("Scope: ${memory.scope} · Device: ${memory.originDevice}", color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("Created ${formatRecordTime(memory.createdAt)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

private fun formatRecordTime(timestamp: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(timestamp))
@Composable
private fun ActivityScreen(repository: HyuskRepository) {
    val runs by repository.runs.collectAsState(initial = emptyList())
    LaunchedEffect(repository) { repository.markStaleRunsInterrupted() }
    Screen("Runs") {
        Text("Recent activity", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
        Text("${runs.size} run${if (runs.size == 1) "" else "s"} stored locally", color = UiMuted)
        if (runs.isEmpty()) {
            EmptyState("Nothing recorded", "Workflow runs and agent tasks will appear here.")
        } else {
            LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                items(runs, key = { it.id }) { run -> RunCard(run) }
            }
        }
    }
}

@Composable
private fun RunCard(run: RunSummary) {
    val statusColor = when (run.status) {
        "completed" -> Color(0xFF78E6AA)
        "failed" -> MaterialTheme.colorScheme.error
        "active" -> Color(0xFF82AAFF)
        else -> UiMuted
    }
    Card(Modifier.fillMaxWidth(), colors = CardDefaults.cardColors(containerColor = UiPanelRaised)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(run.label, color = UiPearl, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Text(run.status.replaceFirstChar(Char::uppercase), color = statusColor, style = MaterialTheme.typography.labelMedium)
            }
            if (run.prompt.isNotBlank()) Text(run.prompt, color = UiMuted, maxLines = 2, overflow = TextOverflow.Ellipsis)
            if (run.result.isNotBlank()) Text(run.result, color = UiPearl, style = MaterialTheme.typography.bodyMedium)
            Text(
                "${run.completedSteps} verified action${if (run.completedSteps == 1) "" else "s"} · ${formatRecordTime(run.updatedAt)}",
                color = UiMuted,
                style = MaterialTheme.typography.bodySmall,
            )
        }
    }
}

@Composable private fun SettingsScreen(localModelStore: LocalModelSettingsStore, onOpenDevices: () -> Unit) {
    val context = LocalContext.current
    val accessibilityConnected by HyuskAccessibilityService.connected.collectAsState()
    var saveMessage by remember { mutableStateOf<String?>(null) }
    var localBaseUrl by remember { mutableStateOf("https://openrouter.ai/api/v1") }
    var localModel by remember { mutableStateOf("openai/gpt-4o-mini") }
    var localApiKey by remember { mutableStateOf("") }
    var activeProvider by remember { mutableStateOf("openrouter") }
    var testMessage by remember { mutableStateOf<String?>(null) }
    var modelPickerOpen by remember { mutableStateOf(false) }
    val modelCatalog = remember { ModelCatalogRepository(context.applicationContext) }
    val catalogState by modelCatalog.state.collectAsState()
    val settingsScope = rememberCoroutineScope()
    val bottomClearance = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 108.dp
    LaunchedEffect(localModelStore) {
        localModelStore.settings.collect {
            localBaseUrl = it.baseUrl
            localModel = it.model
            localApiKey = it.apiKey
            activeProvider = providerForUrl(it.baseUrl)
            if (it.apiKey.isNotBlank()) modelCatalog.load(it) else modelCatalog.clear()
        }
    }
    Column(
        Modifier.fillMaxSize().statusBarsPadding().verticalScroll(rememberScrollState())
            .padding(start = 20.dp, top = 20.dp, end = 20.dp, bottom = bottomClearance),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("Settings", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
        Text("Laptop connection", color = UiPearl, style = MaterialTheme.typography.titleLarge)
        Text("Pair, reconnect, or forget a laptop from the Devices page. Pairing and model credentials are encrypted with Android Keystore.", color = UiMuted)
        OutlinedButton(onClick = onOpenDevices, modifier = Modifier.fillMaxWidth()) {
            Icon(Icons.Default.Devices, null)
            Spacer(Modifier.width(8.dp))
            Text("Open Devices")
        }
        Text("Phone model", color = UiPearl, style = MaterialTheme.typography.titleLarge)
        Text("The API key is encrypted on this phone and is only sent to the OpenAI-compatible endpoint below.", color = UiMuted)
        listOf(
            listOf("openrouter" to "Router", "openai" to "OpenAI"),
            listOf("bedrock" to "Bedrock", "custom" to "Custom"),
        ).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                row.forEach { (profile, label) ->
                    OutlinedButton(onClick = {
                        if (profile != activeProvider) settingsScope.launch {
                            modelCatalog.clear()
                            modelPickerOpen = false
                            runCatching {
                                localModelStore.saveProfile(activeProvider, LocalModelSettings(localBaseUrl, localModel, localApiKey))
                                val defaults = when (profile) {
                                    "openai" -> LocalModelSettings("https://api.openai.com/v1", "", "")
                                    "bedrock" -> LocalModelSettings("https://bedrock-mantle.us-east-1.api.aws/v1", "openai.gpt-oss-20b", "")
                                    "custom" -> LocalModelSettings("https://", "", "")
                                    else -> LocalModelSettings("https://openrouter.ai/api/v1", "", "")
                                }
                                localModelStore.loadProfile(profile, defaults)
                            }.onSuccess { selected ->
                                activeProvider = profile
                                localBaseUrl = selected.baseUrl
                                localModel = selected.model
                                localApiKey = selected.apiKey
                                testMessage = null
                                saveMessage = null
                                if (selected.apiKey.isNotBlank()) modelCatalog.load(selected) else modelCatalog.clear()
                            }.onFailure { saveMessage = it.message ?: "Could not switch provider" }
                        }
                    }, modifier = Modifier.weight(1f), enabled = profile != activeProvider) { Text(label) }
                }
            }
        }
        OutlinedTextField(localBaseUrl, { localBaseUrl = it; modelCatalog.clear() }, Modifier.fillMaxWidth(), label = { Text(if (activeProvider == "bedrock") "Bedrock Mantle URL (edit region)" else "OpenAI-compatible base URL") }, singleLine = true, colors = hyuskTextFieldColors())
        OutlinedTextField(localApiKey, { localApiKey = it; modelCatalog.clear() }, Modifier.fillMaxWidth(), label = { Text("Phone API key") }, singleLine = true, visualTransformation = PasswordVisualTransformation(), colors = hyuskTextFieldColors())
        Text(
            if (localApiKey.isBlank()) "No API key entered" else "API key entered · ${localApiKey.trim().replace(Regex("^Bearer\\s+", RegexOption.IGNORE_CASE), "").takeLast(4).padStart(4, '•')}",
            color = if (localApiKey.isBlank()) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
            style = MaterialTheme.typography.bodySmall,
        )
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(onClick = { modelPickerOpen = true }, modifier = Modifier.weight(1f), enabled = catalogModels(catalogState).isNotEmpty()) {
                Text(if (localModel.isBlank()) "Choose model" else localModel.take(28))
            }
            OutlinedButton(onClick = {
                settingsScope.launch { modelCatalog.refresh(LocalModelSettings(localBaseUrl, localModel, localApiKey)) }
            }, enabled = localApiKey.isNotBlank() && catalogState !is ModelCatalogState.Loading) { Text(if (catalogState is ModelCatalogState.Loading) "Refreshing…" else "Refresh") }
        }
        OutlinedTextField(localModel, { localModel = it }, Modifier.fillMaxWidth(), label = { Text("Model ID (manual fallback)") }, singleLine = true, colors = hyuskTextFieldColors())
        if (activeProvider == "bedrock") Text(
            "Mantle model IDs look like openai.gpt-oss-20b; the -1:0 suffix belongs to the separate bedrock-runtime endpoint. Test the selected model before saving.",
            color = UiMuted,
            style = MaterialTheme.typography.bodySmall,
        )
        when (val catalog = catalogState) {
            is ModelCatalogState.Ready -> Text("${catalog.snapshot.models.size} models · updated ${formatRecordTime(catalog.snapshot.fetchedAtMillis)}", color = UiMuted, style = MaterialTheme.typography.bodySmall)
            is ModelCatalogState.Error -> Text("${catalog.message}${if (catalog.staleSnapshot != null) " · showing cached models" else ""}", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            is ModelCatalogState.Loading -> Text("Loading provider models…", color = UiMuted, style = MaterialTheme.typography.bodySmall)
            ModelCatalogState.Idle -> Text("Save credentials, then refresh the provider model list.", color = UiMuted, style = MaterialTheme.typography.bodySmall)
        }
        Button(onClick = { settingsScope.launch {
            val requested = LocalModelSettings(localBaseUrl, localModel, localApiKey)
            runCatching {
                localModelStore.save(requested)
                localModelStore.saveProfile(activeProvider, requested)
                val persisted = localModelStore.settings.first()
                require(ProviderIdentity.cacheKey(persisted) == ProviderIdentity.cacheKey(requested)) {
                    "Saved profile could not be verified"
                }
                persisted
            }.onSuccess { persisted ->
                saveMessage = "Phone model settings saved and verified."
                if (persisted.apiKey.isNotBlank()) modelCatalog.refresh(persisted) else modelCatalog.clear()
            }.onFailure { saveMessage = it.message ?: "Could not save settings" }
        } }, modifier = Modifier.fillMaxWidth()) { Text("Save phone model") }
        OutlinedButton(onClick = { settingsScope.launch { testMessage = runCatching { LocalModelClient().testConnection(LocalModelSettings(localBaseUrl, localModel, localApiKey)) }.fold({ "Model replied: ${it.take(80)}" }, { "Model test failed: ${it.message}" }) } }, modifier = Modifier.fillMaxWidth()) { Text("Test selected model") }
        testMessage?.let { Text(it, color = if (it.startsWith("Model replied")) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error) }
        HorizontalDivider()
        Text("Permissions", color = UiPearl, style = MaterialTheme.typography.titleLarge)
        PermissionRow("Accessibility actions", accessibilityConnected) { context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) }
        PermissionRow("Notification replies", isNotificationAccessGranted(context)) { context.startActivity(Intent("android.settings.ACTION_NOTIFICATION_LISTENER_SETTINGS")) }
        PermissionRow("Task notifications", android.os.Build.VERSION.SDK_INT < 33 || context.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == android.content.pm.PackageManager.PERMISSION_GRANTED) { if (android.os.Build.VERSION.SDK_INT >= 33) (context as? MainActivity)?.requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 43) }
        PermissionRow("Microphone", context.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) { (context as? MainActivity)?.requestPermissions(arrayOf(android.Manifest.permission.RECORD_AUDIO), 42) }
        PermissionRow("Default assistant", isDefaultAssistant(context)) { openAssistantSettings(context) }
        Text(
            "Choose Hyusk under Digital assistant app. Then use your phone’s assistant gesture or power-button shortcut to open Hyusk.",
            style = MaterialTheme.typography.bodySmall,
            color = UiMuted,
        )
    }
    if (modelPickerOpen) {
        ModelPickerDialog(catalogModels(catalogState), localModel, { modelPickerOpen = false }) {
            localModel = it.id
            modelPickerOpen = false
        }
    }
}

private fun providerForUrl(url: String): String = when {
    url.contains("bedrock-mantle", ignoreCase = true) -> "bedrock"
    url.contains("api.openai.com", ignoreCase = true) -> "openai"
    url.contains("openrouter.ai", ignoreCase = true) -> "openrouter"
    else -> "custom"
}

private fun catalogModels(state: ModelCatalogState): List<ModelCatalogItem> = when (state) {
    is ModelCatalogState.Ready -> state.snapshot.models
    is ModelCatalogState.Error -> state.staleSnapshot?.models.orEmpty()
    is ModelCatalogState.Loading -> state.previous?.models.orEmpty()
    ModelCatalogState.Idle -> emptyList()
}

@Composable
private fun ModelPickerDialog(models: List<ModelCatalogItem>, selected: String, onDismiss: () -> Unit, onSelect: (ModelCatalogItem) -> Unit) {
    var query by remember { mutableStateOf("") }
    val visible = models.filter { query.isBlank() || it.id.contains(query, true) || it.name.contains(query, true) || it.ownedBy.contains(query, true) }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = UiInk) {
            Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    IconButton(onClick = onDismiss) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                    Text("Choose a model", color = UiPearl, style = MaterialTheme.typography.headlineSmall)
                }
                OutlinedTextField(query, { query = it }, Modifier.fillMaxWidth(), label = { Text("Search ${models.size} models") }, singleLine = true, colors = hyuskTextFieldColors())
                LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    items(visible, key = { it.id }) { model ->
                        TextButton(onClick = { onSelect(model) }, modifier = Modifier.fillMaxWidth()) {
                            Column(Modifier.fillMaxWidth()) {
                                Text(model.name.ifBlank { model.id }, color = if (model.id == selected) MaterialTheme.colorScheme.primary else UiPearl)
                                Text(model.id, color = UiMuted, style = MaterialTheme.typography.bodySmall)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun hyuskTextFieldColors() = OutlinedTextFieldDefaults.colors(
    focusedTextColor = UiPearl,
    unfocusedTextColor = UiPearl,
    disabledTextColor = UiMuted,
    focusedLabelColor = UiPearl,
    unfocusedLabelColor = UiMuted,
    focusedPlaceholderColor = UiMuted,
    unfocusedPlaceholderColor = UiMuted,
    cursorColor = UiPearl,
    focusedBorderColor = UiPearl.copy(alpha = .72f),
    unfocusedBorderColor = UiLine,
    focusedSupportingTextColor = UiMuted,
    unfocusedSupportingTextColor = UiMuted,
)

@Composable private fun PermissionRow(label: String, granted: Boolean, onOpen: () -> Unit) { Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) { Icon(if (granted) Icons.Default.CheckCircle else Icons.Default.Lock, null, tint = if (granted) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error); Spacer(Modifier.width(12.dp)); Text(label, Modifier.weight(1f), color = UiPearl); OutlinedButton(onClick = onOpen) { Text(if (granted) "Manage" else "Enable") } } }
private fun isNotificationAccessGranted(context: android.content.Context): Boolean = Settings.Secure.getString(context.contentResolver, "enabled_notification_listeners").orEmpty().contains(context.packageName)
private fun isDefaultAssistant(context: android.content.Context): Boolean =
    context.getSystemService(android.app.role.RoleManager::class.java)?.isRoleHeld(android.app.role.RoleManager.ROLE_ASSISTANT) == true
private fun openAssistantSettings(context: android.content.Context) {
    val voiceSettings = Intent(Settings.ACTION_VOICE_INPUT_SETTINGS)
    val intent = if (voiceSettings.resolveActivity(context.packageManager) != null) {
        voiceSettings
    } else {
        Intent(Settings.ACTION_MANAGE_DEFAULT_APPS_SETTINGS)
    }
    context.startActivity(intent)
}
@Composable
private fun Screen(
    title: String,
    scrollContent: Boolean = false,
    content: @Composable ColumnScope.() -> Unit,
) {
    val bottomClearance = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 108.dp
    Column(
        Modifier.fillMaxSize().statusBarsPadding()
            .padding(start = 20.dp, top = 14.dp, end = 20.dp, bottom = bottomClearance),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Image(
                painterResource(R.drawable.hyusk_butterfly_mark),
                contentDescription = "Hyusk",
                modifier = Modifier.size(28.dp),
            )
            Spacer(Modifier.width(10.dp))
            Text("H Y U S K", color = UiPearl, style = MaterialTheme.typography.titleMedium, letterSpacing = 4.sp)
            Spacer(Modifier.weight(1f))
            Text(title, color = UiMuted, style = MaterialTheme.typography.labelLarge)
        }
        val contentModifier = if (scrollContent) {
            Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 8.dp)
        } else {
            Modifier.fillMaxSize()
        }
        Column(contentModifier, verticalArrangement = Arrangement.spacedBy(14.dp), content = content)
    }
}

@Composable
private fun BottomNav(
    selected: Destination,
    onSelected: (Destination) -> Unit,
    modifier: Modifier = Modifier,
) {
    val shape = RoundedCornerShape(28.dp)
    val glass = Brush.verticalGradient(
        listOf(
            Color(0xE03A4658),
            Color(0xC5222A36),
            Color(0xB8171D26),
        ),
    )
    Surface(
        modifier = modifier.fillMaxWidth().navigationBarsPadding().padding(horizontal = 12.dp, vertical = 10.dp)
            .height(76.dp).shadow(20.dp, shape).background(glass, shape)
            .border(1.dp, UiPearl.copy(alpha = .24f), shape),
        shape = shape,
        color = Color.Transparent,
        contentColor = UiPearl,
        tonalElevation = 0.dp,
    ) {
        Row(
            Modifier.fillMaxSize().selectableGroup().padding(horizontal = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Destination.values().forEach { item ->
                val isSelected = selected == item
                Column(
                    Modifier.weight(1f).fillMaxHeight().selectable(
                        selected = isSelected,
                        onClick = { onSelected(item) },
                        role = Role.Tab,
                    ),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    Surface(
                        modifier = Modifier.width(42.dp).height(30.dp),
                        shape = RoundedCornerShape(16.dp),
                        color = if (isSelected) UiPearl else Color.Transparent,
                        contentColor = if (isSelected) UiInk else UiMuted,
                    ) {
                        Box(contentAlignment = Alignment.Center) {
                            Icon(item.icon, item.label, modifier = Modifier.size(21.dp))
                        }
                    }
                    Spacer(Modifier.height(3.dp))
                    Text(
                        item.label,
                        color = if (isSelected) UiPearl else UiMuted,
                        style = MaterialTheme.typography.labelSmall,
                        maxLines = 1,
                        overflow = TextOverflow.Clip,
                    )
                }
            }
        }
    }
}
@Composable private fun RailNav(selected: Destination, onSelected: (Destination) -> Unit) { NavigationRail { Spacer(Modifier.height(8.dp)); Destination.values().forEach { item -> NavigationRailItem(selected == item, { onSelected(item) }, icon = { Icon(item.icon, item.label) }, label = { Text(item.label) }) } } }
