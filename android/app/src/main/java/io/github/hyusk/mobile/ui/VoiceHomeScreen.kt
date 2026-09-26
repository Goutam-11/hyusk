package io.github.hyusk.mobile.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import io.github.hyusk.mobile.R
import kotlin.math.sin

enum class VoicePhase { Ready, Listening, Thinking, Speaking, Error, Stopped }

private val Ink = Color(0xFF050607)
private val Pearl = Color(0xFFF4F2ED)
private val Muted = Color(0xFF8E9299)
private val Hairline = Color(0xFF25282D)

@Composable
fun VoiceHomeScreen(
    phase: VoicePhase,
    transcript: String,
    response: String?,
    command: String,
    connectionLabel: String,
    onCommandChange: (String) -> Unit,
    onMic: () -> Unit,
    onSubmit: () -> Unit,
    onStop: () -> Unit,
) {
    val bottomClearance = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 108.dp
    val label = when (phase) {
        VoicePhase.Ready -> "Ready when you are"
        VoicePhase.Listening -> "Listening…"
        VoicePhase.Thinking -> "Thinking…"
        VoicePhase.Speaking -> "Speaking…"
        VoicePhase.Error -> "Needs attention"
        VoicePhase.Stopped -> "Hyusk is paused"
    }
    val helper = response?.takeIf(String::isNotBlank)
        ?: transcript.takeIf(String::isNotBlank)
        ?: when (phase) {
            VoicePhase.Listening -> "Go ahead, sir."
            VoicePhase.Thinking -> "Working through your request."
            VoicePhase.Speaking -> ""
            VoicePhase.Stopped -> "Tap resume when you are ready."
            else -> "Ask me to open an app, set a timer, search, or control your phone."
        }

    Box(Modifier.fillMaxSize().statusBarsPadding().imePadding()) {
        AmbientBackground(phase)
        Column(
            Modifier.fillMaxSize()
                .padding(start = 22.dp, top = 14.dp, end = 22.dp, bottom = bottomClearance),
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Image(
                    painterResource(R.drawable.hyusk_butterfly_mark),
                    contentDescription = null,
                    modifier = Modifier.size(30.dp),
                )
                Spacer(Modifier.width(10.dp))
                Text(
                    "HYUSK",
                    color = Pearl,
                    fontWeight = FontWeight.Medium,
                    letterSpacing = 5.sp,
                    style = MaterialTheme.typography.labelLarge,
                )
                Spacer(Modifier.weight(1f))
                Text(connectionLabel.uppercase(), color = Pearl.copy(alpha = .86f), style = MaterialTheme.typography.labelSmall, letterSpacing = 1.2.sp)
            }

            Spacer(Modifier.weight(.24f))
            VoiceOrb(phase)
            Spacer(Modifier.height(26.dp))
            Text(
                label,
                color = Pearl,
                style = MaterialTheme.typography.headlineMedium,
                fontWeight = FontWeight.Normal,
                textAlign = TextAlign.Center,
                modifier = Modifier.semantics { liveRegion = LiveRegionMode.Polite },
            )
            Spacer(Modifier.height(10.dp))
            Text(
                helper,
                color = Pearl.copy(alpha = if (response.isNullOrBlank() && transcript.isBlank()) .78f else .88f),
                style = MaterialTheme.typography.bodyMedium,
                textAlign = TextAlign.Center,
                modifier = Modifier.fillMaxWidth().heightIn(max = 120.dp).padding(horizontal = 18.dp)
                    .verticalScroll(rememberScrollState()).semantics { liveRegion = LiveRegionMode.Polite },
                maxLines = 7,
            )
            Spacer(Modifier.height(14.dp))
            ActivityWave(phase)
            Spacer(Modifier.weight(1f))

            FilledIconButton(
                onClick = when (phase) {
                    VoicePhase.Thinking, VoicePhase.Speaking, VoicePhase.Stopped -> onStop
                    else -> onMic
                },
                enabled = true,
                modifier = Modifier.size(68.dp),
                shape = CircleShape,
                colors = IconButtonDefaults.filledIconButtonColors(
                    containerColor = if (phase == VoicePhase.Speaking) Color(0xFFD85656) else Pearl,
                    contentColor = Ink,
                ),
            ) {
                val icon = when (phase) {
                    VoicePhase.Listening, VoicePhase.Thinking, VoicePhase.Speaking -> Icons.Default.Stop
                    VoicePhase.Stopped -> Icons.Default.PlayArrow
                    else -> Icons.Default.Mic
                }
                val description = when (phase) {
                    VoicePhase.Listening -> "Stop listening"
                    VoicePhase.Thinking, VoicePhase.Speaking -> "Stop Hyusk"
                    VoicePhase.Stopped -> "Resume Hyusk"
                    else -> "Start listening"
                }
                Icon(icon, description, modifier = Modifier.size(29.dp))
            }
            Spacer(Modifier.height(12.dp))

            AnimatedVisibility(visible = true) {
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(26.dp))
                        .background(Color(0xFF111317)).padding(start = 10.dp, end = 6.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    OutlinedTextField(
                        value = command,
                        onValueChange = onCommandChange,
                        placeholder = { Text("Ask anything…", color = Muted) },
                        modifier = Modifier.weight(1f),
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Send),
                        keyboardActions = KeyboardActions(onSend = { onSubmit() }),
                        colors = OutlinedTextFieldDefaults.colors(
                            focusedTextColor = Pearl,
                            unfocusedTextColor = Pearl,
                            focusedBorderColor = Color.Transparent,
                            unfocusedBorderColor = Color.Transparent,
                            cursorColor = Pearl,
                        ),
                    )
                    FilledIconButton(
                        onClick = onSubmit,
                        enabled = command.isNotBlank() && phase != VoicePhase.Thinking && phase != VoicePhase.Stopped,
                        modifier = Modifier.size(46.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(containerColor = Pearl, contentColor = Ink),
                    ) { Icon(Icons.AutoMirrored.Filled.Send, "Send command") }
                }
            }
        }
    }
}

@Composable
private fun AmbientBackground(phase: VoicePhase) {
    val transition = rememberInfiniteTransition(label = "ambient")
    val drift by transition.animateFloat(.20f, .34f, infiniteRepeatable(tween(2400), RepeatMode.Reverse), label = "drift")
    Canvas(Modifier.fillMaxSize()) {
        val strength = if (phase == VoicePhase.Ready || phase == VoicePhase.Stopped) .12f else drift
        drawCircle(
            Brush.radialGradient(listOf(Color.White.copy(alpha = strength), Color.Transparent)),
            radius = size.minDimension * .55f,
            center = Offset(size.width / 2f, size.height * .37f),
        )
    }
}

@Composable
private fun VoiceOrb(phase: VoicePhase) {
    val transition = rememberInfiniteTransition(label = "voice-orb")
    val breath by transition.animateFloat(.96f, 1.04f, infiniteRepeatable(tween(1350, easing = FastOutSlowInEasing), RepeatMode.Reverse), label = "breath")
    val orbit by transition.animateFloat(0f, 360f, infiniteRepeatable(tween(5200)), label = "orbit")
    val active = phase == VoicePhase.Listening || phase == VoicePhase.Thinking || phase == VoicePhase.Speaking
    Box(
        Modifier.size(270.dp).semantics { contentDescription = "Hyusk butterfly, ${phase.name.lowercase()}" },
        contentAlignment = Alignment.Center,
    ) {
        Canvas(Modifier.fillMaxSize().scale(if (active) breath else 1f)) {
            val radius = size.minDimension * .38f
            drawCircle(Brush.radialGradient(listOf(Color.White.copy(alpha = .20f), Color.Transparent)), radius * 1.45f)
            drawCircle(Color.White.copy(alpha = .06f), radius)
            drawCircle(Color.White.copy(alpha = .44f), radius, style = Stroke(1.2.dp.toPx()))
            if (phase == VoicePhase.Listening || phase == VoicePhase.Speaking) {
                for (index in 0..2) {
                    drawCircle(Color.White.copy(alpha = .16f - index * .035f), radius * (1.08f + index * .10f), style = Stroke((2.2f - index * .4f).dp.toPx()))
                }
            }
            if (phase == VoicePhase.Thinking) {
                rotate(orbit) {
                    drawArc(Color.White.copy(alpha = .62f), 8f, 92f, false, style = Stroke(1.4.dp.toPx(), cap = StrokeCap.Round))
                    drawCircle(Pearl, 3.5.dp.toPx(), Offset(center.x + radius * 1.12f, center.y))
                }
                rotate(-orbit * .72f) {
                    drawArc(Color.White.copy(alpha = .24f), 172f, 118f, false, style = Stroke(.8.dp.toPx()))
                }
            }
        }
        Image(
            painterResource(R.drawable.hyusk_butterfly_mark),
            contentDescription = null,
            modifier = Modifier.size(150.dp).scale(if (active) breath else 1f)
                .alpha(if (phase == VoicePhase.Stopped) .38f else 1f),
        )
    }
}

@Composable
private fun ActivityWave(phase: VoicePhase) {
    val transition = rememberInfiniteTransition(label = "wave")
    val movement by transition.animateFloat(0f, 6.28f, infiniteRepeatable(tween(900)), label = "movement")
    Canvas(Modifier.width(148.dp).height(28.dp)) {
        val visible = phase == VoicePhase.Listening || phase == VoicePhase.Speaking
        if (!visible) {
            drawLine(Hairline, Offset(0f, center.y), Offset(size.width, center.y), 1.dp.toPx())
            return@Canvas
        }
        repeat(17) { index ->
            val x = size.width * index / 16f
            val amplitude = (4f + 8f * kotlin.math.abs(sin(movement + index * .72f))).dp.toPx()
            drawLine(Pearl.copy(alpha = .72f), Offset(x, center.y - amplitude / 2f), Offset(x, center.y + amplitude / 2f), 1.4.dp.toPx(), StrokeCap.Round)
        }
    }
}
