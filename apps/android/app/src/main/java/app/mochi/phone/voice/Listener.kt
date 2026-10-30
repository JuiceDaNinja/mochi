package app.mochi.phone.voice

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionListener
import android.speech.RecognizerIntent
import android.speech.SpeechRecognizer

/**
 * Tap-to-talk speech input on top of the platform recognizer. Emits partial results as the
 * user speaks (the first one doubles as "user is speaking" so the desktop goes quiet) and
 * one final result. Wake word ("Hey Mochi") is a drop-in later: Picovoice Porcupine needs
 * an access key, so v1 ships with the big microphone button.
 */
class Listener(context: Context, private val onPartial: (String) -> Unit, private val onFinal: (String) -> Unit, private val onState: (Boolean) -> Unit) {
    private val recognizer: SpeechRecognizer? = if (SpeechRecognizer.isRecognitionAvailable(context)) SpeechRecognizer.createSpeechRecognizer(context) else null
    private var listening = false

    init {
        recognizer?.setRecognitionListener(object : RecognitionListener {
            override fun onReadyForSpeech(params: Bundle?) { listening = true; onState(true) }
            override fun onBeginningOfSpeech() {}
            override fun onRmsChanged(rmsdB: Float) {}
            override fun onBufferReceived(buffer: ByteArray?) {}
            override fun onEndOfSpeech() { listening = false; onState(false) }
            override fun onError(error: Int) { listening = false; onState(false) }
            override fun onPartialResults(partialResults: Bundle?) {
                partialResults?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.let { if (it.isNotBlank()) onPartial(it) }
            }
            override fun onResults(results: Bundle?) {
                listening = false
                onState(false)
                results?.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION)?.firstOrNull()?.let { if (it.isNotBlank()) onFinal(it) }
            }
            override fun onEvent(eventType: Int, params: Bundle?) {}
        })
    }

    val available get() = recognizer != null

    fun start() {
        val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH).apply {
            putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
            putExtra(RecognizerIntent.EXTRA_LANGUAGE, "en-US")
            putExtra(RecognizerIntent.EXTRA_PARTIAL_RESULTS, true)
            putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
        }
        recognizer?.startListening(intent)
    }

    fun stop() { recognizer?.stopListening() }
    fun destroy() { recognizer?.destroy() }
}
