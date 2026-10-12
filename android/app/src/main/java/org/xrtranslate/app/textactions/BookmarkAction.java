package org.xrtranslate.app.textactions;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;
import android.widget.Toast;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.xrtranslate.app.R;

/** Saves the exact result shown when the star was tapped, through the shared Rust wordbook. */
public final class BookmarkAction extends BroadcastReceiver {
    private static final ExecutorService WORKER = Executors.newSingleThreadExecutor();
    private static native boolean save(String directory, String payload);

    @Override public void onReceive(Context context, Intent intent) {
        long id = intent.getLongExtra("result", -1);
        TranslationResults.Result result;
        try {
            String payload = intent.getStringExtra("entry");
            if (payload == null || id < 0) return;
            result = new TranslationResults.Result(id, new org.json.JSONObject(payload));
            if (!result.canSave) return;
        } catch (org.json.JSONException invalid) { return; }
        PendingResult pending = goAsync();
        WORKER.execute(() -> {
            boolean saved = false;
            try {
                System.loadLibrary("rust_client");
                saved = save(context.getFilesDir().getAbsolutePath(), result.json().toString());
                if (saved) TranslationResults.markSaved(context, id);
            } catch (RuntimeException | LinkageError unavailable) {
                android.util.Log.w("XRTranslate", "Cannot save notification word");
            } finally {
                int message = saved ? R.string.quick_saved : R.string.quick_save_failed;
                new Handler(Looper.getMainLooper()).post(() -> Toast.makeText(context, message, Toast.LENGTH_SHORT).show());
                pending.finish();
            }
        });
    }
}
