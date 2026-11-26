package dev.cortex.workspace.notify

import android.Manifest
import android.annotation.SuppressLint
import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.provider.Settings
import android.webkit.WebView
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import app.tauri.PermissionState
import app.tauri.annotation.Command
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.security.SecureRandom

// The phone's own notifications. No push service is involved: while the owner keeps them on, a
// foreground service with one quiet ongoing notification keeps the app's process, and so its
// end-to-end encrypted connections to the paired computers, alive in the background. The Rust side
// follows each computer's owner moments and asks this plugin to raise or withdraw a notification; a
// tap opens the app at that task. The build script installs this file into the generated project.
object Notices {
  const val ONGOING_ID = 1
  const val MOMENT_ID = 2
  const val EXTRA_MACHINE = "dev.cortex.workspace.notify.MACHINE"
  const val EXTRA_TARGET = "dev.cortex.workspace.notify.TARGET"
  const val EXTRA_KEY = "dev.cortex.workspace.notify.KEY"
  val CHANNELS = setOf("answer", "done", "failure")
  private const val PREFS = "cortex-notify"

  // The texts Android shows, in the app's language: the interface hands them over each time notifications
  // are turned on (checked on the Rust side), and they are kept so the service shows them too. English stays
  // where none was given.
  fun saveWords(context: Context, words: JSObject) {
    context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString("words", words.toString()).apply()
  }

  private fun word(context: Context, path: List<String>, fallback: String): String {
    val saved = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString("words", null) ?: return fallback
    return try {
      var node: org.json.JSONObject = org.json.JSONObject(saved)
      for (key in path.dropLast(1)) node = node.optJSONObject(key) ?: return fallback
      node.optString(path.last(), "").take(200).ifBlank { fallback }
    } catch (e: org.json.JSONException) {
      fallback
    }
  }

  private fun channel(context: Context, id: String, name: String, note: String, importance: Int): NotificationChannel {
    val channel = NotificationChannel(id, word(context, listOf("channels", id, "name"), name), importance)
    channel.description = word(context, listOf("channels", id, "note"), note)
    return channel
  }

  fun channels(context: Context) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
    val manager = context.getSystemService(NotificationManager::class.java) ?: return
    val ongoing = channel(context, "ongoing", "Connection", "The quiet notification shown while the app keeps its connection to your computers.", NotificationManager.IMPORTANCE_MIN)
    ongoing.setShowBadge(false)
    val answer = channel(context, "answer", "Needs your answer", "A task asks a question, stopped for your decision or needs access.", NotificationManager.IMPORTANCE_HIGH)
    val done = channel(context, "done", "Results ready", "A task finished and its result waits for your review.", NotificationManager.IMPORTANCE_DEFAULT)
    val failure = channel(context, "failure", "Failures", "A task failed, a sprint paused or a task made no progress.", NotificationManager.IMPORTANCE_HIGH)
    manager.createNotificationChannels(listOf(ongoing, answer, done, failure))
  }

  // A random key kept in the app's private storage. Only the app's own notifications carry it, so an
  // intent another app sends to this activity with made-up extras leads nowhere.
  @Synchronized
  fun key(context: Context): String {
    val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    prefs.getString("key", null)?.let { return it }
    val bytes = ByteArray(16)
    SecureRandom().nextBytes(bytes)
    val key = bytes.joinToString("") { "%02x".format(it) }
    prefs.edit().putString("key", key).apply()
    return key
  }

  fun open(context: Context, machine: String?, target: String?, request: Int): PendingIntent? {
    val intent = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return null
    intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    if (machine != null && target != null) {
      intent.putExtra(EXTRA_MACHINE, machine).putExtra(EXTRA_TARGET, target).putExtra(EXTRA_KEY, key(context))
    }
    return PendingIntent.getActivity(context, request, intent,
      PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
  }

  fun icon(context: Context): Int {
    val id = context.resources.getIdentifier("ic_stat_cortex", "drawable", context.packageName)
    return if (id != 0) id else android.R.drawable.stat_notify_chat
  }

  fun ongoing(context: Context): Notification =
    NotificationCompat.Builder(context, "ongoing")
      .setSmallIcon(icon(context))
      .setContentTitle(word(context, listOf("ongoingTitle"), "Connected to your computers"))
      .setContentText(word(context, listOf("ongoingText"), "Following your computers for questions, results and failures."))
      .setOngoing(true)
      .setPriority(NotificationCompat.PRIORITY_MIN)
      .setCategory(NotificationCompat.CATEGORY_SERVICE)
      .setContentIntent(open(context, null, null, 0))
      .build()

  fun allowed(context: Context): Boolean =
    (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU ||
      ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED) &&
      NotificationManagerCompat.from(context).areNotificationsEnabled()

  fun status(context: Context): JSObject {
    val power = context.getSystemService(Context.POWER_SERVICE) as? PowerManager
    val out = JSObject()
    out.put("permission", if (allowed(context)) "granted" else "denied")
    out.put("batteryExempt", power?.isIgnoringBatteryOptimizations(context.packageName) == true)
    return out
  }
}

// Holds the process in the foreground while notifications are on. It is started from the open app
// only, and not restarted by the system on its own: without the app's process there is nothing it
// could follow.
class NotifyService : Service() {
  override fun onBind(intent: Intent?): IBinder? = null

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    Notices.channels(this)
    val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE)
      ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE else 0
    ServiceCompat.startForeground(this, Notices.ONGOING_ID, Notices.ongoing(this), type)
    return START_NOT_STICKY
  }
}

@TauriPlugin(permissions = [Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "post")])
class NotifyPlugin(private val activity: Activity) : Plugin(activity) {
  companion object {
    private var pending: JSObject? = null
    private var waiting: Invoke? = null

    // A tap on one of the app's notifications, from the intent that started or reached the activity.
    // The intent's extras are removed, so a recreated activity does not open it again.
    @Synchronized
    fun offer(context: Context, intent: Intent?) {
      if (intent == null) return
      val machine = intent.getStringExtra(Notices.EXTRA_MACHINE)
      val target = intent.getStringExtra(Notices.EXTRA_TARGET)
      val key = intent.getStringExtra(Notices.EXTRA_KEY)
      intent.removeExtra(Notices.EXTRA_MACHINE)
      intent.removeExtra(Notices.EXTRA_TARGET)
      intent.removeExtra(Notices.EXTRA_KEY)
      if (machine == null || target == null || machine.length > 256 || target.length > 2048) return
      if (key != Notices.key(context)) return
      val tap = JSObject()
      tap.put("machineId", machine)
      tap.put("target", target)
      val wait = waiting
      if (wait != null) {
        waiting = null
        wait.resolve(tap)
      } else {
        pending = tap
      }
    }

    @Synchronized
    fun next(invoke: Invoke) {
      val tap = pending
      if (tap != null) {
        pending = null
        invoke.resolve(tap)
      } else {
        // One reader at a time; an older one is simply never answered.
        waiting = invoke
      }
    }
  }

  private fun text(args: JSObject, key: String): String = args.getString(key, "") ?: ""

  override fun load(webView: WebView) {
    offer(activity.applicationContext, activity.intent)
  }

  override fun onNewIntent(intent: Intent) {
    offer(activity.applicationContext, intent)
  }

  @Command
  fun start(invoke: Invoke) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU && getPermissionState("post") != PermissionState.GRANTED) {
      requestPermissionForAlias("post", invoke, "asked")
    } else {
      begin(invoke)
    }
  }

  @PermissionCallback
  fun asked(invoke: Invoke) {
    begin(invoke)
  }

  private fun begin(invoke: Invoke) {
    val context = activity.applicationContext
    Notices.saveWords(context, invoke.getArgs())
    Notices.channels(context)
    try {
      ContextCompat.startForegroundService(context, Intent(context, NotifyService::class.java))
    } catch (e: Exception) {
      invoke.reject("notifications_unavailable")
      return
    }
    invoke.resolve(Notices.status(context))
  }

  @Command
  fun stop(invoke: Invoke) {
    val context = activity.applicationContext
    context.stopService(Intent(context, NotifyService::class.java))
    invoke.resolve(Notices.status(context))
  }

  @SuppressLint("MissingPermission")
  @Command
  fun show(invoke: Invoke) {
    val context = activity.applicationContext
    val args = invoke.getArgs()
    val tag = text(args, "tag")
    val channel = text(args, "channel")
    val machine = text(args, "machineId")
    val target = text(args, "target")
    if (tag.isEmpty() || channel !in Notices.CHANNELS || machine.isEmpty() || target.isEmpty()) {
      invoke.reject("invalid_notification")
      return
    }
    val body = text(args, "text").take(400)
    Notices.channels(context)
    val notification = NotificationCompat.Builder(context, channel)
      .setSmallIcon(Notices.icon(context))
      .setContentTitle(text(args, "title").take(120))
      .setContentText(body)
      .setStyle(NotificationCompat.BigTextStyle().bigText(body))
      .setAutoCancel(true)
      .setCategory(if (channel == "answer") NotificationCompat.CATEGORY_MESSAGE else NotificationCompat.CATEGORY_STATUS)
      .setContentIntent(Notices.open(context, machine, target, tag.hashCode()))
      .build()
    if (Notices.allowed(context)) {
      NotificationManagerCompat.from(context).notify(tag, Notices.MOMENT_ID, notification)
    }
    invoke.resolve()
  }

  @Command
  fun withdraw(invoke: Invoke) {
    val tag = text(invoke.getArgs(), "tag")
    if (tag.isNotEmpty()) NotificationManagerCompat.from(activity.applicationContext).cancel(tag, Notices.MOMENT_ID)
    invoke.resolve()
  }

  @Command
  fun next(invoke: Invoke) {
    Companion.next(invoke)
  }

  // Android's list of apps under battery optimisation: there the owner can let this app keep its
  // connection while the phone sleeps. The app does not ask for the exemption itself.
  @Command
  fun battery(invoke: Invoke) {
    try {
      activity.startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
    } catch (e: ActivityNotFoundException) {
      try {
        activity.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + activity.packageName)))
      } catch (e: ActivityNotFoundException) {
        invoke.reject("settings_unavailable")
        return
      }
    }
    invoke.resolve()
  }
}
