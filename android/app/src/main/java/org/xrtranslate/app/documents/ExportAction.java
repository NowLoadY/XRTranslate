package org.xrtranslate.app.documents;

import android.app.Activity;
import android.content.ClipData;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Toast;
import java.io.InputStream;
import java.io.OutputStream;
import org.xrtranslate.app.R;

/** Optional Open / Save destinations in the system share sheet. */
public final class ExportAction extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        if (state != null) return; // The document picker restores its pending result.
        Intent request = getIntent();
        if (request.getData() == null) { finish(); return; }
        try {
            if (Intent.ACTION_CREATE_DOCUMENT.equals(request.getAction())) {
                startActivityForResult(new Intent(Intent.ACTION_CREATE_DOCUMENT)
                    .addCategory(Intent.CATEGORY_OPENABLE).setType(request.getType())
                    .putExtra(Intent.EXTRA_TITLE, request.getStringExtra(Intent.EXTRA_TITLE)), 1);
            } else {
                Intent open = new Intent(Intent.ACTION_VIEW).setDataAndType(request.getData(), request.getType())
                    .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
                open.setClipData(ClipData.newRawUri("Export", request.getData()));
                startActivity(Intent.createChooser(open, getString(R.string.export_open)));
                finish();
            }
        } catch (RuntimeException unavailable) {
            failed();
            finish();
        }
    }

    @Override protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != 1 || result != RESULT_OK || data == null || data.getData() == null) { finish(); return; }
        new Thread(() -> {
            boolean saved = false;
            try (InputStream input = getContentResolver().openInputStream(getIntent().getData());
                 OutputStream output = getContentResolver().openOutputStream(data.getData(), "wt")) {
                if (input == null || output == null) throw new java.io.IOException("Document unavailable");
                byte[] buffer = new byte[65536];
                for (int count; (count = input.read(buffer)) != -1;) output.write(buffer, 0, count);
                saved = true;
            } catch (Exception unavailable) {
                // Report failures without exposing a provider's private paths.
            }
            boolean complete = saved;
            runOnUiThread(() -> {
                if (complete) Toast.makeText(this, R.string.export_saved, Toast.LENGTH_SHORT).show();
                else failed();
                finish();
            });
        }, "document-save").start();
    }

    private void failed() { Toast.makeText(this, R.string.export_failed, Toast.LENGTH_LONG).show(); }
}
