package org.xrtranslate.app.documents;

import android.content.ClipData;
import android.content.Intent;
import android.net.Uri;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.FileProvider;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.xrtranslate.app.MainActivity;

public final class DocumentActions {
    public interface Completion { void complete(long id, String path, String error); }
    private final MainActivity activity;
    private final Completion completion;
    private final ExecutorService worker = Executors.newSingleThreadExecutor();
    private final Map<Long, File> exports = new ConcurrentHashMap<>();
    private final ActivityResultLauncher<Intent> picker;
    private long pickerId;

    public DocumentActions(MainActivity activity, Completion completion) {
        this.activity = activity;
        this.completion = completion;
        picker = activity.registerForActivityResult(new ActivityResultContracts.StartActivityForResult(),
            result -> onPicked(result.getResultCode(), result.getData()));
    }

    public void request(long id, boolean export, String name, String extensions) {
        activity.runOnUiThread(() -> {
            if (activity.isFinishing() || activity.isDestroyed() || pickerId != 0) {
                completion.complete(id, null, "Finish the current file selection first.");
                return;
            }
            if (export) {
                worker.execute(() -> prepareExport(id, name));
                return;
            }
            pickerId = id;
            Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType(extensions.equals("json") ? "application/json" : "*/*");
            try { picker.launch(intent); }
            catch (RuntimeException error) {
                pickerId = 0;
                completion.complete(id, null, "Cannot open the file picker.");
            }
        });
    }

    private void prepareExport(long id, String name) {
        try {
            File root = new File(activity.getCacheDir(), "exports");
            if (!root.isDirectory() && !root.mkdirs()) throw new java.io.IOException("Storage unavailable");
            File[] old = root.listFiles();
            if (old != null) for (File directory : old) {
                if (directory.lastModified() < System.currentTimeMillis() - 86_400_000L) {
                    File[] files = directory.listFiles();
                    if (files != null) for (File file : files) file.delete();
                    directory.delete();
                }
            }
            String filename = new File(name).getName();
            if (filename.isEmpty() || filename.equals(".") || filename.equals("..")) filename = "result.txt";
            File directory = java.nio.file.Files.createTempDirectory(root.toPath(), "share-").toFile();
            File file = new File(directory, filename);
            if (!file.createNewFile()) throw new java.io.IOException("Cannot create export");
            exports.put(id, file);
            completion.complete(id, file.getAbsolutePath(), null);
        } catch (Exception error) {
            completion.complete(id, null, "Cannot prepare the exported file.");
        }
    }

    public void share(long id) {
        File file = exports.remove(id);
        if (file == null) return;
        activity.runOnUiThread(() -> {
            if (activity.isFinishing() || activity.isDestroyed()) return;
            try {
                Uri uri = FileProvider.getUriForFile(activity, activity.getPackageName() + ".exports", file);
                String extension = file.getName().substring(file.getName().lastIndexOf('.') + 1).toLowerCase(java.util.Locale.ROOT);
                String mime = android.webkit.MimeTypeMap.getSingleton().getMimeTypeFromExtension(extension);
                Intent intent = new Intent(Intent.ACTION_SEND)
                    .setType(mime == null ? "application/octet-stream" : mime)
                    .putExtra(Intent.EXTRA_STREAM, uri)
                    .putExtra(Intent.EXTRA_TITLE, file.getName())
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                intent.setClipData(ClipData.newRawUri(file.getName(), uri));
                activity.startActivity(Intent.createChooser(intent, null));
            } catch (RuntimeException error) {
                completion.complete(id, null, "Cannot share the exported file.");
            }
        });
    }

    private void onPicked(int result, Intent data) {
        if (pickerId == 0) return;
        long id = pickerId;
        pickerId = 0;
        if (result != MainActivity.RESULT_OK || data == null || data.getData() == null) {
            completion.complete(id, null, null);
            return;
        }
        Uri uri = data.getData();
        worker.execute(() -> {
            File destination = null;
            try {
                String name = "document";
                try (android.database.Cursor cursor = activity.getContentResolver().query(uri,
                    new String[]{android.provider.OpenableColumns.DISPLAY_NAME}, null, null, null)) {
                    if (cursor != null && cursor.moveToFirst() && cursor.getString(0) != null) name = cursor.getString(0);
                }
                name = new File(name).getName();
                if (name.isEmpty() || name.equals(".") || name.equals("..")) throw new java.io.IOException("Invalid document name");
                File directory = new File(activity.getFilesDir(), "imports");
                if (!directory.isDirectory() && !directory.mkdirs()) throw new java.io.IOException("Storage unavailable");
                destination = new File(directory, name);
                for (int suffix = 2; !destination.createNewFile(); suffix++) {
                    int dot = name.lastIndexOf('.');
                    destination = new File(directory, dot > 0 ? name.substring(0, dot) + " (" + suffix + ")" + name.substring(dot) : name + " (" + suffix + ")");
                }
                try (InputStream input = activity.getContentResolver().openInputStream(uri);
                     FileOutputStream output = new FileOutputStream(destination)) {
                    if (input == null) throw new java.io.IOException("Document stream unavailable");
                    byte[] buffer = new byte[65536];
                    for (int count; (count = input.read(buffer)) >= 0;) {
                        if (Thread.currentThread().isInterrupted()) throw new java.io.IOException("Import cancelled");
                        output.write(buffer, 0, count);
                    }
                }
                completion.complete(id, destination.getAbsolutePath(), null);
            } catch (Exception error) {
                if (destination != null) destination.delete();
                completion.complete(id, null, "Cannot read the selected document.");
            }
        });
    }

    public void close() {
        if (pickerId != 0) { completion.complete(pickerId, null, null); pickerId = 0; }
        worker.shutdownNow();
    }
}
