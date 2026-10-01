package org.xrtranslate.app;

import android.os.Bundle;
import android.view.WindowInsets;
import com.google.androidgamesdk.GameActivity;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;

public final class MainActivity extends GameActivity {
    static { System.loadLibrary("rust_client"); }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
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
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            getWindow().setDecorFitsSystemWindows(false);
            android.view.WindowInsetsController controller = getWindow().getInsetsController();
            if (controller != null) {
                controller.hide(WindowInsets.Type.statusBars() | WindowInsets.Type.navigationBars());
                controller.setSystemBarsBehavior(android.view.WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(
                android.view.View.SYSTEM_UI_FLAG_LAYOUT_STABLE
                | android.view.View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
                | android.view.View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION
                | android.view.View.SYSTEM_UI_FLAG_FULLSCREEN
                | android.view.View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                | android.view.View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
            );
        }
    }

    @Override public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus) {
            applyImmersiveFullscreen();
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

    public void prepareConfiguration() throws java.io.IOException { copyAssets("application/config.json", new File(getFilesDir(), "config.json")); }

    public void prepareResources() throws java.io.IOException { copyAssets("application", getFilesDir()); }

    private static native void fileDialogCompleted(long id, String path, String error);
    private final java.util.concurrent.ExecutorService documents = java.util.concurrent.Executors.newSingleThreadExecutor();
    private final java.util.Map<Long, SaveDocument> saves = new java.util.concurrent.ConcurrentHashMap<>();
    private long pickerId = 0;
    private boolean pickerSave;
    private String pickerName;
    private static final int DOCUMENT_REQUEST = 7101;
    private static final class SaveDocument {
        final android.net.Uri uri; final File file;
        SaveDocument(android.net.Uri uri, File file) { this.uri = uri; this.file = file; }
    }

    public void requestFileDialog(long id, boolean save, String name, String extensions) {
        runOnUiThread(() -> {
            if (pickerId != 0 || isFinishing() || isDestroyed()) { fileDialogCompleted(id, null, "Finish the current file selection first."); return; }
            pickerId = id; pickerSave = save; pickerName = name;
            android.content.Intent intent = new android.content.Intent(save ? android.content.Intent.ACTION_CREATE_DOCUMENT : android.content.Intent.ACTION_OPEN_DOCUMENT);
            intent.addCategory(android.content.Intent.CATEGORY_OPENABLE);
            intent.setType(extensions.equals("json") ? "application/json" : save ? "application/octet-stream" : "*/*");
            if (save) intent.putExtra(android.content.Intent.EXTRA_TITLE, name);
            try { startActivityForResult(intent, DOCUMENT_REQUEST); }
            catch (RuntimeException error) { pickerId = 0; fileDialogCompleted(id, null, "Cannot open the file picker."); }
        });
    }

    @Override protected void onActivityResult(int request, int result, android.content.Intent data) {
        super.onActivityResult(request, result, data);
        if (request != DOCUMENT_REQUEST || pickerId == 0) return;
        final long id = pickerId; final boolean save = pickerSave; final String fallback = pickerName;
        pickerId = 0;
        if (result != RESULT_OK || data == null || data.getData() == null) { fileDialogCompleted(id, null, null); return; }
        final android.net.Uri uri = data.getData();
        documents.execute(() -> {
            File destination = null;
            try {
                String name = fallback;
                try (android.database.Cursor cursor = getContentResolver().query(uri, new String[]{android.provider.OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                    if (cursor != null && cursor.moveToFirst()) name = cursor.getString(0);
                }
                if (name == null || name.isEmpty()) name = "document";
                name = new File(name).getName();
                if (name.equals(".") || name.equals("..")) throw new java.io.IOException("Invalid document name");
                File directory = save ? new File(getCacheDir(), "exports/" + id) : new File(getFilesDir(), "imports");
                if (!directory.isDirectory() && !directory.mkdirs()) throw new java.io.IOException("Cannot create document storage");
                destination = new File(directory, name);
                for (int suffix = 2; !destination.createNewFile(); suffix++) {
                    int dot = name.lastIndexOf('.');
                    destination = new File(directory, dot > 0 ? name.substring(0, dot) + " (" + suffix + ")" + name.substring(dot) : name + " (" + suffix + ")");
                }
                if (save) saves.put(id, new SaveDocument(uri, destination));
                else try (InputStream input = getContentResolver().openInputStream(uri); FileOutputStream output = new FileOutputStream(destination)) { copy(input, output); }
                fileDialogCompleted(id, destination.getAbsolutePath(), null);
            } catch (Exception error) {
                if (destination != null) destination.delete();
                fileDialogCompleted(id, null, "Cannot read or prepare the selected document.");
            }
        });
    }

    public void commitFileSave(long id) {
        SaveDocument document = saves.remove(id);
        if (document == null) return;
        documents.execute(() -> {
            try (InputStream input = new java.io.FileInputStream(document.file); java.io.OutputStream output = getContentResolver().openOutputStream(document.uri, "wt")) { copy(input, output); }
            catch (Exception error) { fileDialogCompleted(id, null, "Cannot save the selected document."); }
            finally { document.file.delete(); }
        });
    }

    private static void copy(InputStream input, java.io.OutputStream output) throws java.io.IOException {
        if (input == null || output == null) throw new java.io.IOException("Document stream unavailable");
        byte[] buffer = new byte[65536];
        for (int count; (count = input.read(buffer)) >= 0;) {
            if (Thread.currentThread().isInterrupted()) throw new java.io.IOException("Document operation cancelled");
            output.write(buffer, 0, count);
        }
    }

    @Override protected void onDestroy() {
        if (pickerId != 0) { fileDialogCompleted(pickerId, null, null); pickerId = 0; }
        documents.shutdownNow();
        super.onDestroy();
    }

    private void copyAssets(String source, File destination) throws java.io.IOException {
        String[] children = getAssets().list(source);
        if (children != null && children.length > 0) {
            if (!destination.isDirectory() && !destination.mkdirs()) throw new java.io.IOException("Cannot create application storage");
            for (String child : children) copyAssets(source + "/" + child, new File(destination, child));
        } else if (!destination.exists()) {
            File temporary = File.createTempFile("resource-", ".part", destination.getParentFile());
            try {
                try (InputStream input = getAssets().open(source); FileOutputStream output = new FileOutputStream(temporary)) { copy(input, output); output.getFD().sync(); }
                if (!temporary.renameTo(destination)) throw new java.io.IOException("Cannot prepare application resource");
            } finally { temporary.delete(); }
        }
    }
}
