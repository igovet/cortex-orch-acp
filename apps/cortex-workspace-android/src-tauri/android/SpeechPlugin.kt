package dev.cortex.workspace.speech

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.speech.RecognizerIntent
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

// Dictation through the phone's own speech recognition: the system's recognizer screen listens and
// returns text. The app records nothing itself and asks for no microphone permission. The build
// script installs this file into the generated Android project.
@TauriPlugin
class SpeechPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun listen(invoke: Invoke) {
    val intent = Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
      .putExtra(RecognizerIntent.EXTRA_LANGUAGE_MODEL, RecognizerIntent.LANGUAGE_MODEL_FREE_FORM)
      .putExtra(RecognizerIntent.EXTRA_MAX_RESULTS, 1)
    try {
      startActivityForResult(invoke, intent, "heard")
    } catch (e: ActivityNotFoundException) {
      invoke.reject("speech_unavailable")
    }
  }

  @ActivityCallback
  fun heard(invoke: Invoke, result: ActivityResult) {
    val text = if (result.resultCode == Activity.RESULT_OK)
      result.data?.getStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS)?.firstOrNull() ?: ""
    else ""
    val out = JSObject()
    out.put("text", text)
    invoke.resolve(out)
  }
}
