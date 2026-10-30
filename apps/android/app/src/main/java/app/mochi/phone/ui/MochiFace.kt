package app.mochi.phone.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.scale
import androidx.compose.ui.graphics.drawscope.translate
import app.mochi.phone.link.MochiState
import kotlin.math.sin

private val Body = Color(0xFFFBF4EC)
private val BodyEdge = Color(0xFFE6D8CA)
private val Ink = Color(0xFF2B2420)
private val Blush = Color(0xFFF7A8B4)
private val Mouth = Color(0xFF9B7B6A)
private val Tear = Color(0xFF8FD3FF)

/**
 * The same Mochi as the desktop SVG, drawn on a Canvas and driven by [state]. Replace
 * with RiveAnimationView once mochi.riv exists (same state inputs as the desktop).
 * [gazeX]/[gazeY] in -1..1 move the pupils; on the phone they drift toward the mic button.
 */
@Composable
fun MochiFace(state: MochiState, modifier: Modifier = Modifier, gazeX: Float = 0f, gazeY: Float = 0f) {
    val t = rememberInfiniteTransition(label = "idle")
    val breathe by t.animateFloat(0f, 1f, infiniteRepeatable(tween(3200, easing = LinearEasing), RepeatMode.Restart), label = "breathe")
    val wobble by t.animateFloat(0f, 1f, infiniteRepeatable(tween(700, easing = LinearEasing), RepeatMode.Restart), label = "wobble")
    val blush by animateFloatAsState(if (state == MochiState.SUCCESS || state == MochiState.CONFIRM) 0.95f else if (state == MochiState.LOST) 0.35f else 0.55f, label = "blush")
    val shrink by animateFloatAsState(if (state == MochiState.LOST) 0.9f else 1f, tween(600), label = "shrink")

    Canvas(modifier) {
        val w = size.width
        val u = w / 120f // the SVG's 120-unit viewBox
        val breath = 1f + 0.015f * sin(breathe * 2f * Math.PI).toFloat()
        val wob = if (state == MochiState.LOST) 4f * u * sin(wobble * 2f * Math.PI).toFloat() else 0f

        // shadow
        drawOval(Color.Black.copy(alpha = 0.12f), topLeft = Offset(26 * u, 99 * u), size = Size(68 * u, 10 * u))

        translate(left = wob) {
            scale(scaleX = shrink * breath, scaleY = shrink / breath, pivot = Offset(60 * u, 100 * u)) {
                // body
                val body = Path().apply {
                    moveTo(60 * u, 8 * u)
                    cubicTo(96 * u, 8 * u, 112 * u, 34 * u, 112 * u, 60 * u)
                    cubicTo(112 * u, 90 * u, 90 * u, 102 * u, 60 * u, 102 * u)
                    cubicTo(30 * u, 102 * u, 8 * u, 90 * u, 8 * u, 60 * u)
                    cubicTo(8 * u, 34 * u, 24 * u, 8 * u, 60 * u, 8 * u)
                    close()
                }
                drawPath(body, Brush.radialGradient(listOf(Color(0xFFFFFDF9), Body, Color(0xFFECDFD2)), center = Offset(50 * u, 38 * u), radius = 84 * u))
                drawPath(body, BodyEdge, style = Stroke(1f * u))
                drawOval(Brush.radialGradient(listOf(Color.White.copy(alpha = 0.9f), Color.Transparent), center = Offset(44 * u, 30 * u), radius = 22 * u), topLeft = Offset(22 * u, 18 * u), size = Size(44 * u, 24 * u))

                // blush
                drawOval(Blush.copy(alpha = blush), topLeft = Offset(20 * u, 66 * u), size = Size(16 * u, 8 * u))
                drawOval(Blush.copy(alpha = blush), topLeft = Offset(84 * u, 66 * u), size = Size(16 * u, 8 * u))

                // eyes
                for (cx in listOf(42f, 78f)) {
                    val ex = cx * u + gazeX * 3.2f * u
                    val ey = 56 * u + gazeY * 3f * u + if (state == MochiState.CONFIRM) -3f * u else 0f
                    if (state == MochiState.SUCCESS) {
                        val arc = Path().apply { moveTo(ex - 7 * u, ey + 1 * u); quadraticTo(ex, ey - 8 * u, ex + 7 * u, ey + 1 * u) }
                        drawPath(arc, Ink, style = Stroke(3f * u, cap = StrokeCap.Round))
                    } else {
                        val ry = if (state == MochiState.LISTENING) 9.2f else 8.2f
                        drawOval(Ink, topLeft = Offset(ex - 6.2f * u, ey - ry * u), size = Size(12.4f * u, 2 * ry * u))
                        drawCircle(Color.White, 2f * u, Offset(ex - 2.2f * u, ey - 3.2f * u))
                        drawCircle(Color.White.copy(alpha = 0.7f), 0.9f * u, Offset(ex + 2f * u, ey + 3.5f * u))
                        if (state == MochiState.LOST) {
                            drawOval(Tear.copy(alpha = 0.75f), topLeft = Offset(ex - 6.4f * u, ey + 1.4f * u), size = Size(12.8f * u, 7.2f * u))
                            val lid = Path().apply { moveTo(ex - 9 * u, ey + 2 * u); quadraticTo(ex, ey + 7 * u, ex + 9 * u, ey + 2 * u); lineTo(ex + 9 * u, ey + 12 * u); lineTo(ex - 9 * u, ey + 12 * u); close() }
                            drawPath(lid, Body)
                            val drop = 10f * u * wobble
                            drawCircle(Tear.copy(alpha = 1f - wobble), 2.2f * u, Offset(ex + 5 * u, ey + 8 * u + drop))
                        }
                    }
                }

                // mouth
                val mx = 60 * u; val my = 76 * u
                when (state) {
                    MochiState.SUCCESS -> {
                        val p = Path().apply { moveTo(mx - 7 * u, my - 1 * u); quadraticTo(mx, my + 9 * u, mx + 7 * u, my - 1 * u); close() }
                        drawPath(p, Color(0xFFC0695F))
                    }
                    MochiState.LOST -> {
                        val p = Path().apply {
                            moveTo(mx - 7 * u, my + 1 * u)
                            quadraticTo(mx - 5 * u, my - 2 * u, mx - 3 * u, my + 1 * u)
                            quadraticTo(mx - 1 * u, my + 4 * u, mx + 1 * u, my + 1 * u)
                            quadraticTo(mx + 3 * u, my - 2 * u, mx + 5 * u, my + 1 * u)
                        }
                        drawPath(p, Mouth, style = Stroke(1.6f * u, cap = StrokeCap.Round))
                    }
                    MochiState.CONFIRM -> drawCircle(Color(0xFFB97B6F), 2.4f * u, Offset(mx, my))
                    MochiState.WORKING, MochiState.THINKING, MochiState.LEARNING -> {
                        val p = Path().apply { moveTo(mx - 6 * u, my - 1 * u); quadraticTo(mx, my + 6 * u, mx + 6 * u, my - 1 * u) }
                        drawPath(p, Mouth, style = Stroke(1.8f * u, cap = StrokeCap.Round))
                    }
                    else -> {
                        val p = Path().apply { moveTo(mx - 3 * u, my); quadraticTo(mx, my + 2 * u, mx + 3 * u, my) }
                        drawPath(p, Mouth, style = Stroke(1.6f * u, cap = StrokeCap.Round))
                    }
                }
            }
        }
    }
}
