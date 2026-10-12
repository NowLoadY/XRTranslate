package org.xrtranslate.app.textactions;

import android.app.Activity;
import android.content.ClipboardManager;
import android.content.ClipData;
import android.content.Intent;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.ResultReceiver;
import android.view.View;
import android.widget.TextView;
import android.widget.Toast;
import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import java.lang.ref.WeakReference;
import org.xrtranslate.app.R;

/** Focused clipboard preview; the shared service keeps translating if the user leaves. */
public final class ClipboardAction extends Activity {
    private boolean handled;
    private TextView source;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        // A recreated window must not translate the already replaced clipboard again.
        if (state != null) { finish(); return; }
        setContentView(R.layout.clipboard_translation);
        source = findViewById(R.id.clipboard_source);
        View page = findViewById(R.id.clipboard_page);
        View content = findViewById(R.id.clipboard_content);
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        WindowCompat.getInsetsController(getWindow(), page).setAppearanceLightStatusBars(true);
        WindowCompat.getInsetsController(getWindow(), page).setAppearanceLightNavigationBars(true);
        getWindow().setNavigationBarContrastEnforced(false);
        int padding = dp(28);
        ViewCompat.setOnApplyWindowInsetsListener(page, (view, insets) -> {
            androidx.core.graphics.Insets bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() | WindowInsetsCompat.Type.displayCutout());
            view.setPadding(bars.left + padding, bars.top + padding, bars.right + padding, bars.bottom + padding);
            return insets;
        });
        page.addOnLayoutChangeListener((view, left, top, right, bottom, oldLeft, oldTop, oldRight, oldBottom) -> {
            int width = Math.min(dp(640), view.getWidth() - view.getPaddingLeft() - view.getPaddingRight());
            if (content.getLayoutParams().width != width) {
                content.getLayoutParams().width = width;
                content.requestLayout();
            }
            int height = view.getHeight() - view.getPaddingTop() - view.getPaddingBottom();
            int lines = Math.max(2, Math.min(10, (height - dp(100)) / Math.max(1, source.getLineHeight())));
            if (source.getMaxLines() != lines) source.setMaxLines(lines);
        });
    }

    @Override public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (!focused || handled || source == null) return;
        handled = true;
        try {
            ClipData clip = getSystemService(ClipboardManager.class).getPrimaryClip();
            CharSequence value = clip == null || clip.getItemCount() == 0 ? null : clip.getItemAt(0).getText();
            String text = value == null ? "" : value.toString().trim();
            if (text.isEmpty()) { fail(R.string.quick_clipboard_empty); return; }
            if (text.length() > 32768) { fail(R.string.text_action_too_long); return; }
            source.setText(text);
            startForegroundService(new Intent(this, TextTranslationService.class)
                .putExtra(Intent.EXTRA_TEXT, text).putExtra("conversation", "clipboard")
                .putExtra(TextTranslationService.COMPLETION, new Completion(this)));
        } catch (RuntimeException unavailable) {
            fail(R.string.text_action_failed);
        }
    }

    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private void fail(int message) {
        Toast.makeText(this, message, Toast.LENGTH_LONG).show();
        finish();
    }

    private static final class Completion extends ResultReceiver {
        private final WeakReference<ClipboardAction> activity;
        Completion(ClipboardAction activity) {
            super(new Handler(Looper.getMainLooper()));
            this.activity = new WeakReference<>(activity);
        }
        @Override protected void onReceiveResult(int state, Bundle data) {
            ClipboardAction current = activity.get();
            if (current != null && !current.isFinishing() && !current.isDestroyed()) current.finish();
        }
    }
}
