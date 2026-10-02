package org.xrtranslate.app.audio;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

/** Keeps an explicitly started microphone capture alive while the user multitasks. */
public final class MicrophoneService extends Service {
    static { System.loadLibrary("rust_client"); }
    private static final String CHANNEL = "microphone_translation";
    private static final int NOTIFICATION = 7105;
    private static final Handler MAIN = new Handler(Looper.getMainLooper());
    private PowerManager.WakeLock wakeLock;
    private long generation;
    private static native long captureGeneration();
    private static native void captureServiceChanged(long generation, boolean active);

    public static void setCaptureActive(Context context, boolean active, long generation) {
        Context application = context.getApplicationContext();
        MAIN.post(() -> {
            long current = captureGeneration();
            Intent service = new Intent(application, MicrophoneService.class);
            try {
                if (active && current == generation) application.startForegroundService(service.putExtra("generation", generation));
                else if (!active && current == 0) application.stopService(service);
            } catch (RuntimeException error) {
                android.util.Log.w("XRTranslate", "Cannot update microphone capture service", error);
                captureServiceChanged(generation, false);
            }
        });
    }

    @Override public void onCreate() {
        super.onCreate();
        getSystemService(NotificationManager.class).createNotificationChannel(new NotificationChannel(
            CHANNEL, getString(R.string.recording_title), NotificationManager.IMPORTANCE_LOW));
        wakeLock = getSystemService(PowerManager.class).newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK, "XRTranslate:Microphone");
        wakeLock.setReferenceCounted(false);
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        long requested = intent == null ? 0 : intent.getLongExtra("generation", 0);
        Intent open = new Intent(this, MainActivity.class).setAction("org.xrtranslate.app.OPEN_TRANSLATION")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent content = PendingIntent.getActivity(this, 0, open,
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        Notification notification = new Notification.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.text_translation_notification)
            .setContentTitle(getString(R.string.recording_title))
            .setContentText(getString(R.string.recording_message))
            .setContentIntent(content).setOngoing(true).setOnlyAlertOnce(true)
            .setCategory(Notification.CATEGORY_SERVICE).setVisibility(Notification.VISIBILITY_PRIVATE)
            .build();
        try {
            if (Build.VERSION.SDK_INT >= 30) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE);
            else startForeground(NOTIFICATION, notification);
            long current = captureGeneration();
            if (current == 0 || current != requested) {
                if (current == 0) stopSelfResult(startId);
                return START_NOT_STICKY;
            }
            generation = requested;
            wakeLock.acquire();
            captureServiceChanged(generation, true);
        } catch (RuntimeException error) {
            android.util.Log.w("XRTranslate", "Cannot keep microphone capture active", error);
            captureServiceChanged(requested, false);
            stopSelfResult(startId);
        }
        return START_NOT_STICKY;
    }

    @Override public IBinder onBind(Intent intent) { return null; }

    @Override public void onDestroy() {
        captureServiceChanged(generation, false);
        if (wakeLock != null && wakeLock.isHeld()) wakeLock.release();
        stopForeground(STOP_FOREGROUND_REMOVE);
        super.onDestroy();
    }
}
