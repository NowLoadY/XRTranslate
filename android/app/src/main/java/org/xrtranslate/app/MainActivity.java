package org.xrtranslate.app;

import android.os.Bundle;
import android.view.WindowInsets;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;
import com.google.androidgamesdk.GameActivity;
import org.xrtranslate.app.textactions.TranslationResults;

public final class MainActivity extends GameActivity {
    static { System.loadLibrary("rust_client"); }
    private static native void openTranslation();

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        acceptNavigation(getIntent());
        requestTranslationNotifications();
        getSharedPreferences("text_action_draft", MODE_PRIVATE).edit().clear().apply();
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.P) {
            android.view.WindowManager.LayoutParams lp = getWindow().getAttributes();
            lp.layoutInDisplayCutoutMode = android.view.WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            getWindow().setAttributes(lp);
        }
        applyImmersiveFullscreen();
        getWindow().getDecorView().setOnApplyWindowInsetsListener((view, insets) -> {
            applyImmersiveFullscreen();
            if (android.os.Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets keyboard = insets.getInsets(WindowInsets.Type.ime());
                view.setPadding(0, 0, 0, keyboard.bottom);
            } else {
                view.setPadding(0, 0, 0, 0);
            }
            return insets;
        });
    }

    private void applyImmersiveFullscreen() {
        WindowCompat.setDecorFitsSystemWindows(getWindow(), false);
        WindowInsetsControllerCompat controller = WindowCompat.getInsetsController(getWindow(), getWindow().getDecorView());
        controller.hide(WindowInsetsCompat.Type.systemBars());
        controller.setSystemBarsBehavior(WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            applyImmersiveFullscreen();
        }
    }

    @Override protected void onNewIntent(android.content.Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        acceptNavigation(intent);
    }

    private void acceptNavigation(android.content.Intent intent) {
        if (intent != null && "org.xrtranslate.app.OPEN_TRANSLATION".equals(intent.getAction())) {
            openTranslation();
            setIntent(new android.content.Intent(this, MainActivity.class));
        }
    }

    public void publishTranslationResult(String source, String translated) {
        TranslationResults.publish(this, source, translated);
    }

    public void copyText(String text) {
        getSystemService(android.content.ClipboardManager.class).setPrimaryClip(
            android.content.ClipData.newPlainText("XRTranslate", text));
    }

    private void requestTranslationNotifications() {
        if (android.os.Build.VERSION.SDK_INT < 33) return;
        android.content.SharedPreferences preferences = getSharedPreferences("platform_permissions", MODE_PRIVATE);
        if (!preferences.getBoolean("notifications_requested", false)) {
            preferences.edit().putBoolean("notifications_requested", true).apply();
            requestPermissions(new String[]{android.Manifest.permission.POST_NOTIFICATIONS}, 7104);
        }
    }

    public void showStartupError(String message) {
        runOnUiThread(() -> {
            if (isFinishing() || isDestroyed()) return;
            new androidx.appcompat.app.AlertDialog.Builder(this)
                .setTitle("XRTranslate")
                .setMessage(message)
                .setPositiveButton(android.R.string.ok, (dialog, which) -> finish())
                .setOnCancelListener(dialog -> finish())
                .show();
        });
    }

    private static native void microphonePermissionResult(boolean granted);
    private static native void foregroundChanged(boolean foreground);
    private static final int MICROPHONE_REQUEST = 7102;
    private boolean microphoneRequested;

    public boolean hasMicrophonePermission() {
        return checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED;
    }

    public void setMicrophoneCaptureActive(boolean active, long generation) {
        org.xrtranslate.app.audio.MicrophoneService.setCaptureActive(this, active, generation);
    }

    public void requestMicrophonePermission() {
        runOnUiThread(() -> {
            if (hasMicrophonePermission()) { microphonePermissionResult(true); return; }
            if (!microphoneRequested && !isFinishing() && !isDestroyed()) {
                microphoneRequested = true;
                requestPermissions(new String[]{android.Manifest.permission.RECORD_AUDIO}, MICROPHONE_REQUEST);
            }
        });
    }

    @Override public void onRequestPermissionsResult(int request, String[] permissions, int[] results) {
        super.onRequestPermissionsResult(request, permissions, results);
        if (request == MICROPHONE_REQUEST) {
            microphoneRequested = false;
            microphonePermissionResult(results.length > 0 && results[0] == android.content.pm.PackageManager.PERMISSION_GRANTED);
        }
    }

    @Override protected void onResume() { super.onResume(); foregroundChanged(true); }
    @Override protected void onPause() { foregroundChanged(false); super.onPause(); }

    public void prepareConfiguration() throws java.io.IOException { ApplicationResources.prepare(this, false); }

    public void prepareResources() throws java.io.IOException { ApplicationResources.prepare(this, true); }

    private static native void fileDialogCompleted(long id, String path, String error);
    private final org.xrtranslate.app.documents.DocumentActions documents =
        new org.xrtranslate.app.documents.DocumentActions(this, MainActivity::fileDialogCompleted);

    public void requestFileDialog(long id, boolean save, String name, String extensions) {
        documents.request(id, save, name, extensions);
    }

    public void commitFileSave(long id) { documents.share(id); }

    @Override protected void onDestroy() {
        stopService(new android.content.Intent(this, org.xrtranslate.app.audio.MicrophoneService.class));
        documents.close();
        super.onDestroy();
    }

}
