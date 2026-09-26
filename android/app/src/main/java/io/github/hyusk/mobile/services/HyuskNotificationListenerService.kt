package io.github.hyusk.mobile.services

import android.app.Notification
import android.app.RemoteInput
import android.content.Intent
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification

data class NotificationAction(val packageName: String, val key: String, val actionIndex: Int, val replyKey: String)

class HyuskNotificationListenerService : NotificationListenerService() {
    override fun onNotificationPosted(sbn: StatusBarNotification) { /* indexed by the active workflow host */ }
    override fun onNotificationRemoved(sbn: StatusBarNotification) = Unit

    fun reply(action: NotificationAction, message: CharSequence): Boolean {
        if (!EmergencyStop.allowAction()) return false
        val notification = activeNotifications.firstOrNull { it.key == action.key && it.packageName == action.packageName }?.notification ?: return false
        val target = notification.actions?.getOrNull(action.actionIndex) ?: return false
        val remote = target.remoteInputs?.firstOrNull { it.resultKey == action.replyKey } ?: return false
        val intent = Intent().apply { RemoteInput.addResultsToIntent(arrayOf(remote), this, android.os.Bundle().apply { putCharSequence(remote.resultKey, message) }) }
        return runCatching { target.actionIntent.send(this, 0, intent) }.isSuccess
    }
}
