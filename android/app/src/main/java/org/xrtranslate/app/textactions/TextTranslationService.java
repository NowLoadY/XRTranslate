package org.xrtranslate.app.textactions;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.widget.Toast;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.xrtranslate.app.ApplicationResources;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

/** One bounded, user-requested translation; it never opens an application window. */
public final class TextTranslationService extends Service {
    private static final String PROGRESS = "text_translation_progress", RESULTS = "text_translation_results";
    private static final int NOTIFICATION = 7103;
    private static final long TIMEOUT_MS = 165000;
    private final Handler main = new Handler(Looper.getMainLooper());
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private static final java.util.concurrent.atomic.AtomicInteger NEXT_REQUEST = new java.util.concurrent.atomic.AtomicInteger();
    private volatile int request;
    private int serviceStartId;
    private String source = "";
    private volatile boolean running, loaded;
    private PowerManager.WakeLock wakeLock;
    private final Runnable timeout = () -> complete(request, 4, "");
    private native void translate(int id, String text, String directory, String libraries, String conversation);
    private static native void cancel(int id);

    @Override public void onCreate() {
        super.onCreate();
        wakeLock = getSystemService(PowerManager.class).newWakeLock(
            PowerManager.PARTIAL_WAKE_LOCK, "XRTranslate:SelectedText");
        wakeLock.setReferenceCounted(false);
        NotificationManager manager = getSystemService(NotificationManager.class);
        manager.createNotificationChannel(new NotificationChannel(PROGRESS,
            getString(R.string.text_action_translating), NotificationManager.IMPORTANCE_LOW));
        manager.createNotificationChannel(new NotificationChannel(RESULTS,
            getString(R.string.text_action_title), NotificationManager.IMPORTANCE_HIGH));
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        String text = intent == null ? null : intent.getStringExtra(Intent.EXTRA_TEXT);
        if (text == null || text.trim().isEmpty() || text.length() > 32768) {
            if (!running) stopSelfResult(startId);
            return START_NOT_STICKY;
        }
        if (loaded && running) cancel(request);
        serviceStartId = startId;
        source = text;
        int id = NEXT_REQUEST.incrementAndGet();
        request = id;
        String caller = intent.getStringExtra("conversation");
        String conversation = caller == null ? "request:" + id : "selection:" + caller;
        running = true;
        Notification notification = notification(PROGRESS, R.string.text_action_translating).setOngoing(true).build();
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTIFICATION, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE);
        else startForeground(NOTIFICATION, notification);
        releaseWakeLock();
        wakeLock.acquire(TIMEOUT_MS);
        main.removeCallbacks(timeout);
        main.postDelayed(timeout, TIMEOUT_MS);
        worker.execute(() -> {
            try {
                System.loadLibrary("rust_client");
                loaded = true;
                ApplicationResources.prepare(this, true);
                if (request == id && running) translate(id, text, getFilesDir().getAbsolutePath(), getApplicationInfo().nativeLibraryDir, conversation);
            } catch (Exception | LinkageError error) {
                complete(id, 4, "");
            }
        });
        return START_NOT_STICKY;
    }

    // JNI results are delivered on the worker; UI and clipboard operations stay serialized.
    public void complete(int id, int state, String result) {
        main.post(() -> {
            if (!running || request != id) return;
            int message = state == 2 ? R.string.text_action_setup : R.string.text_action_failed;
            if (state == 3 && result != null && !result.trim().isEmpty()) {
                try {
                    getSystemService(ClipboardManager.class).setPrimaryClip(
                        ClipData.newPlainText(getString(R.string.text_action_title), result));
                    TranslationResults.publish(this, source, result);
                    message = 0;
                } catch (RuntimeException error) {
                    message = R.string.text_action_failed;
                }
            }
            running = false;
            main.removeCallbacks(timeout);
            if (loaded) cancel(id);
            releaseWakeLock();
            stopForeground(STOP_FOREGROUND_REMOVE);
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (message != 0) {
                if (manager.areNotificationsEnabled()) manager.notify(NOTIFICATION, notification(RESULTS, message).build());
                else Toast.makeText(this, message, Toast.LENGTH_LONG).show();
            }
            stopSelfResult(serviceStartId);
        });
    }

    private Notification.Builder notification(String channel, int message) {
        Intent open = new Intent(this, MainActivity.class).setAction("org.xrtranslate.app.OPEN_TRANSLATION")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent pending = PendingIntent.getActivity(this, 0, open, PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
        return new Notification.Builder(this, channel).setSmallIcon(R.drawable.text_translation_notification)
            .setContentTitle(getString(R.string.text_action_title)).setContentText(getString(message))
            .setContentIntent(pending).setAutoCancel(true).setVisibility(Notification.VISIBILITY_PRIVATE)
            .setCategory(Notification.CATEGORY_STATUS);
    }

    @Override public void onTimeout(int startId) {
        if (startId == serviceStartId) complete(request, 4, "");
    }
    @Override public void onTimeout(int startId, int type) { onTimeout(startId); }
    @Override public IBinder onBind(Intent intent) { return null; }
    private void releaseWakeLock() {
        if (wakeLock != null && wakeLock.isHeld()) wakeLock.release();
    }
    @Override public void onDestroy() {
        running = false;
        main.removeCallbacks(timeout);
        if (loaded) cancel(request);
        releaseWakeLock();
        worker.shutdownNow();
        super.onDestroy();
    }
}
