package org.xrtranslate.app.textactions;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Handler;
import android.os.Looper;
import android.view.View;
import android.widget.RemoteViews;
import java.util.List;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

/** One reusable control strip for idle, text-translation and microphone notifications. */
public final class QuickActions {
    private static final String CHANNEL = "translation_quick_actions";
    private static final int IDLE = 7106, RECORDING = 7105, TRANSLATING = 7103;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private static boolean recording, translating;
    private QuickActions() {}

    public static void recording(Context context, boolean active) { recording = active; refresh(context); }
    public static void translating(Context context, boolean active) { translating = active; refresh(context); }

    public static void refresh(Context context) {
        Context application = context.getApplicationContext();
        MAIN.post(() -> {
            NotificationManager manager = application.getSystemService(NotificationManager.class);
            if (!manager.areNotificationsEnabled()) return;
            manager.createNotificationChannel(new NotificationChannel(CHANNEL,
                application.getString(R.string.quick_title), NotificationManager.IMPORTANCE_LOW));
            if (recording || translating) manager.cancel(IDLE);
            else manager.notify(IDLE, build(application, CHANNEL, R.string.quick_ready));
            if (recording) manager.notify(RECORDING, build(application, "microphone_translation", R.string.recording_title));
            if (translating) manager.notify(TRANSLATING, build(application, "text_translation_progress", R.string.text_action_translating));
        });
    }

    public static Notification build(Context context, String channel, int status) {
        List<TranslationResults.Result> results = TranslationResults.snapshot(context);
        TranslationResults.Result latest = results.isEmpty() ? null : results.get(0);
        Intent open = new Intent(context, MainActivity.class).setAction("org.xrtranslate.app.OPEN_TRANSLATION")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent content = PendingIntent.getActivity(context, 0, open, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Notification.Builder builder = new Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.text_translation_notification).setContentTitle(context.getString(R.string.quick_title))
            .setContentText(context.getString(status)).setContentIntent(content)
            .setStyle(new Notification.DecoratedCustomViewStyle())
            .setCustomContentView(controls(context, latest, status, false))
            .setCustomBigContentView(controls(context, latest, status, true))
            .setOngoing(true).setOnlyAlertOnce(true).setShowWhen(false)
            .setCategory(Notification.CATEGORY_SERVICE).setVisibility(Notification.VISIBILITY_PRIVATE);
        builder.setPublicVersion(new Notification.Builder(context, channel)
            .setSmallIcon(R.drawable.text_translation_notification).setContentTitle(context.getString(R.string.quick_title))
            .setContentText(context.getString(R.string.quick_unlock)).build());
        return builder.build();
    }

    private static RemoteViews controls(Context context, TranslationResults.Result result, int status, boolean expanded) {
        RemoteViews views = new RemoteViews(context.getPackageName(), R.layout.translation_controls);
        views.setTextViewText(R.id.quick_status, context.getString(status));
        Intent clipboard = new Intent(context, ClipboardAction.class)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS);
        views.setOnClickPendingIntent(R.id.quick_translate, PendingIntent.getActivity(context, 1, clipboard,
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE));
        views.setBoolean(R.id.quick_translate, "setEnabled", !translating);
        boolean canSave = result != null && result.canSave;
        views.setViewVisibility(R.id.quick_status, canSave ? View.GONE : View.VISIBLE);
        views.setViewVisibility(R.id.quick_bookmark, canSave ? View.VISIBLE : View.GONE);
        if (canSave) {
            // Data, not mutable extras, identifies the displayed snapshot across subsequent updates.
            Intent save = new Intent(context, BookmarkAction.class).setData(Uri.parse("xrt:bookmark/" + result.id))
                .putExtra("result", result.id).putExtra("entry", result.json().toString());
            views.setTextViewText(R.id.quick_bookmark, result.source);
            views.setTextViewCompoundDrawablesRelative(R.id.quick_bookmark, 0, 0,
                result.saved ? R.drawable.quick_star_filled : R.drawable.quick_star, 0);
            views.setContentDescription(R.id.quick_bookmark, result.source + ", "
                + context.getString(result.saved ? R.string.quick_saved : R.string.quick_save));
            views.setOnClickPendingIntent(R.id.quick_bookmark, PendingIntent.getBroadcast(context, 2, save,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE));
        }
        views.setViewVisibility(R.id.quick_result, expanded ? View.VISIBLE : View.GONE);
        if (expanded) {
            views.setTextViewText(R.id.quick_result, result == null ? context.getString(R.string.quick_hint)
                : result.source + "\n" + result.translated);
        }
        return views;
    }
}
