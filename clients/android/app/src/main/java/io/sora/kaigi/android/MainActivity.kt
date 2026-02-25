package io.sora.kaigi.android

import android.annotation.SuppressLint
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.media.AudioAttributes
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Bundle
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            MaterialTheme {
                Surface {
                    MeetingShell()
                }
            }
        }
    }
}

@Composable
private fun MeetingShell(viewModel: MeetingViewModel = viewModel()) {
    val state by viewModel.uiState.collectAsStateWithLifecycle()
    val lifecycleOwner = LocalLifecycleOwner.current
    val context = LocalContext.current

    var showFallback by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(state.fallbackActive) {
        if (state.fallbackActive) {
            showFallback = true
        }
    }

    DisposableEffect(lifecycleOwner, viewModel) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_START -> viewModel.onAppForegrounded()
                Lifecycle.Event.ON_STOP -> viewModel.onAppBackgrounded()
                else -> Unit
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose {
            lifecycleOwner.lifecycle.removeObserver(observer)
        }
    }

    DisposableEffect(context, viewModel) {
        val cm = context.getSystemService(ConnectivityManager::class.java) ?: return@DisposableEffect onDispose { }

        val active = cm.activeNetwork
        val capabilities = cm.getNetworkCapabilities(active)
        val online = capabilities?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        viewModel.onConnectivityChanged(online)

        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                viewModel.onConnectivityChanged(true)
            }

            override fun onLost(network: Network) {
                viewModel.onConnectivityChanged(false)
            }
        }

        runCatching { cm.registerDefaultNetworkCallback(callback) }

        onDispose {
            runCatching { cm.unregisterNetworkCallback(callback) }
        }
    }

    DisposableEffect(context, viewModel) {
        val appContext = context.applicationContext
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(receiverContext: Context?, intent: Intent?) {
                if (intent?.action == AudioManager.ACTION_AUDIO_BECOMING_NOISY) {
                    viewModel.onAudioRouteChanged("becoming_noisy")
                }
            }
        }
        val filter = IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY)
        runCatching { appContext.registerReceiver(receiver, filter) }
        onDispose {
            runCatching { appContext.unregisterReceiver(receiver) }
        }
    }

    DisposableEffect(context, viewModel) {
        val audioManager = context.getSystemService(AudioManager::class.java) ?: return@DisposableEffect onDispose { }
        val focusMapper = AudioFocusInterruptionMapper()
        val focusListener = AudioManager.OnAudioFocusChangeListener { change ->
            when (focusMapper.onFocusChange(change)) {
                AudioInterruptionSignal.Began -> {
                    viewModel.onAudioInterruptionBegan()
                }

                AudioInterruptionSignal.Ended -> {
                    viewModel.onAudioInterruptionEnded(shouldReconnect = true)
                }

                null -> Unit
            }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val focusRequest = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN_TRANSIENT)
                .setAcceptsDelayedFocusGain(true)
                .setOnAudioFocusChangeListener(focusListener)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                        .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                        .build()
                )
                .build()
            runCatching { audioManager.requestAudioFocus(focusRequest) }
            onDispose {
                runCatching { audioManager.abandonAudioFocusRequest(focusRequest) }
            }
        } else {
            @Suppress("DEPRECATION")
            runCatching {
                audioManager.requestAudioFocus(
                    focusListener,
                    AudioManager.STREAM_VOICE_CALL,
                    AudioManager.AUDIOFOCUS_GAIN_TRANSIENT
                )
            }
            onDispose {
                @Suppress("DEPRECATION")
                runCatching { audioManager.abandonAudioFocus(focusListener) }
            }
        }
    }

    val fallbackUrl = state.config.fallbackUriOrNull()?.toString()
    if (showFallback && fallbackUrl != null) {
        FallbackScreen(
            url = fallbackUrl,
            recoveryMode = state.fallbackActive,
            onClose = {
                showFallback = false
                if (state.fallbackActive) {
                    viewModel.recoverFromFallback()
                }
            }
        )
        return
    }

    Box(
        modifier = Modifier
            .fillMaxSize()
            .background(
                brush = Brush.verticalGradient(
                    colors = listOf(Color(0xFF141B3A), Color(0xFF063243))
                )
            )
            .padding(16.dp)
    ) {
        Column(
            modifier = Modifier.fillMaxSize(),
            verticalArrangement = Arrangement.spacedBy(12.dp)
        ) {
            Text(
                text = "Kaigi Android",
                color = Color.White,
                fontSize = 30.sp,
                fontFamily = FontFamily.SansSerif,
                fontWeight = FontWeight.Black
            )
            Text(
                text = "Status: ${state.transportState}",
                color = if (state.connected) Color(0xFF7CFC9D) else Color(0xFFFFC947),
                fontWeight = FontWeight.Bold
            )
            state.lastError?.let {
                Text(
                    text = "Last Error: $it",
                    color = Color(0xFFFF8A80),
                    fontWeight = FontWeight.SemiBold,
                    fontSize = 12.sp
                )
            }
            state.fallbackRtoMs?.let {
                Text(
                    text = "Last fallback recovery: ${it}ms",
                    color = Color(0xFFB3E5FC),
                    fontSize = 12.sp
                )
            }

            ConfigEditor(
                config = state.config,
                onConfigChange = { updater -> viewModel.updateConfig(updater) }
            )

            SessionSnapshotCard(session = state.session)

            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
                Button(
                    onClick = { viewModel.connect() },
                    enabled = state.config.isJoinable(),
                    colors = ButtonDefaults.buttonColors(containerColor = Color(0xFF18A999))
                ) { Text(if (state.connected) "Reconnect" else "Connect") }

                Button(onClick = { viewModel.sendPing() }, enabled = state.connected) { Text("Ping") }
                Button(onClick = { viewModel.disconnect() }, enabled = state.connected) { Text("Disconnect") }
                TextButton(
                    onClick = { showFallback = true },
                    enabled = state.config.fallbackUriOrNull() != null
                ) { Text("Web Fallback") }
            }

            Surface(
                modifier = Modifier.fillMaxSize(),
                shape = RoundedCornerShape(16.dp),
                color = Color(0x33000000)
            ) {
                LazyColumn(modifier = Modifier.padding(12.dp)) {
                    items(state.logs) { log ->
                        val color = when (log.level) {
                            MeetingLogLevel.INFO -> Color.White
                            MeetingLogLevel.WARN -> Color(0xFFFFE082)
                            MeetingLogLevel.ERROR -> Color(0xFFFF8A80)
                        }
                        Text(
                            text = log.formatted(),
                            color = color,
                            fontFamily = FontFamily.Monospace,
                            fontSize = 12.sp,
                            modifier = Modifier.padding(vertical = 2.dp)
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun SessionSnapshotCard(session: ProtocolSessionState) {
    Surface(
        modifier = Modifier.fillMaxWidth(),
        shape = RoundedCornerShape(12.dp),
        color = Color(0x22000000)
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .padding(horizontal = 12.dp, vertical = 10.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp)
        ) {
            Text("Session Policy", color = Color.White, fontWeight = FontWeight.Bold, fontSize = 13.sp)
            Text(
                text = "roomLocked=${yesNo(session.roomLocked)} waitingRoom=${yesNo(session.waitingRoomEnabled)} guestPolicy=${session.guestPolicy.wire}",
                color = Color(0xFFCFD8DC),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace
            )
            Text(
                text = "e2eeRequired=${yesNo(session.e2eeRequired)} maxParticipants=${session.maxParticipants} policyEpoch=${session.policyEpoch}",
                color = Color(0xFFCFD8DC),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace
            )
            Text(
                text = "recording=${session.recordingNotice.wire} media=${session.mediaProfile.preferredProfile.wire}/${session.mediaProfile.negotiatedProfile.wire}",
                color = Color(0xFFCFD8DC),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace
            )
            Text(
                text = "paymentRequired=${yesNo(session.paymentState.required)} settlement=${session.paymentState.settlementStatus.wire}",
                color = Color(0xFFCFD8DC),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace
            )
            session.paymentState.destination?.let { destination ->
                Text(
                    text = "paymentDestination=$destination",
                    color = Color(0xFFCFD8DC),
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace
                )
            }
        }
    }
}

@Composable
private fun ConfigEditor(
    config: MeetingConfig,
    onConfigChange: ((MeetingConfig) -> MeetingConfig) -> Unit
) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .background(Color(0x22FFFFFF), RoundedCornerShape(16.dp))
            .padding(12.dp)
            .verticalScroll(rememberScrollState()),
        verticalArrangement = Arrangement.spacedBy(8.dp)
    ) {
        ConfigField("Signaling URL", config.signalingUrl) {
            onConfigChange { current -> current.copy(signalingUrl = it) }
        }
        ConfigField("Fallback URL", config.fallbackUrl) {
            onConfigChange { current -> current.copy(fallbackUrl = it) }
        }
        ConfigField("Room ID", config.roomId) {
            onConfigChange { current -> current.copy(roomId = it) }
        }
        ConfigField("Participant", config.participant) {
            onConfigChange { current -> current.copy(participant = it) }
        }
        ConfigField("Participant ID (optional)", config.participantId.orEmpty()) {
            onConfigChange { current ->
                current.copy(participantId = it.trim().ifEmpty { null })
            }
        }
        PolicyToggle(
            label = "Require Signed Moderation",
            checked = config.requireSignedModeration,
            onCheckedChange = { checked ->
                onConfigChange { current -> current.copy(requireSignedModeration = checked) }
            }
        )
        PolicyToggle(
            label = "Require Payment Settlement",
            checked = config.requirePaymentSettlement,
            onCheckedChange = { checked ->
                onConfigChange { current -> current.copy(requirePaymentSettlement = checked) }
            }
        )
        PolicyToggle(
            label = "Fallback On Policy Failure",
            checked = config.preferWebFallbackOnPolicyFailure,
            onCheckedChange = { checked ->
                onConfigChange { current -> current.copy(preferWebFallbackOnPolicyFailure = checked) }
            }
        )
    }
}

@Composable
private fun ConfigField(label: String, value: String, onValueChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = onValueChange,
        label = { Text(label) },
        modifier = Modifier.fillMaxWidth(),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.None),
        singleLine = true
    )
}

@Composable
private fun PolicyToggle(label: String, checked: Boolean, onCheckedChange: (Boolean) -> Unit) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.SpaceBetween
    ) {
        Text(label, color = Color.White, fontSize = 13.sp)
        Switch(checked = checked, onCheckedChange = onCheckedChange)
    }
}

private fun yesNo(value: Boolean): String = if (value) "yes" else "no"

@SuppressLint("SetJavaScriptEnabled")
@Composable
private fun FallbackScreen(url: String, recoveryMode: Boolean, onClose: () -> Unit) {
    Column(modifier = Modifier.fillMaxSize()) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(8.dp),
            horizontalArrangement = Arrangement.SpaceBetween,
            verticalAlignment = Alignment.CenterVertically
        ) {
            Text("Web Fallback", fontWeight = FontWeight.Bold)
            Button(onClick = onClose) {
                Text(if (recoveryMode) "Recover Native" else "Close")
            }
        }

        AndroidView(
            modifier = Modifier
                .fillMaxWidth()
                .height(1.dp)
                .weight(1f),
            factory = { context ->
                WebView(context).apply {
                    webViewClient = WebViewClient()
                    settings.javaScriptEnabled = true
                    loadUrl(url)
                }
            },
            update = { view ->
                if (view.url != url) {
                    view.loadUrl(url)
                }
            }
        )
    }
}
