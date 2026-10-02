package org.xrtranslate.app.textactions;

import android.app.Activity;
import android.content.Intent;
import android.os.Bundle;
import android.widget.Toast;
import org.xrtranslate.app.R;

/** Receives the explicit text selection, then immediately returns to its caller. */
public final class TextActions extends Activity {
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        Intent intent = getIntent();
        String action = intent.getAction();
        boolean selected = Intent.ACTION_PROCESS_TEXT.equals(action);
        if ((selected || Intent.ACTION_SEND.equals(action)) && "text/plain".equals(intent.getType())) {
            CharSequence value = intent.getCharSequenceExtra(selected ? Intent.EXTRA_PROCESS_TEXT : Intent.EXTRA_TEXT);
            String text = value == null ? "" : value.toString().trim();
            if (text.length() > 32768) {
                Toast.makeText(this, R.string.text_action_too_long, Toast.LENGTH_LONG).show();
            } else if (!text.isEmpty()) {
                try {
                    startForegroundService(new Intent(this, TextTranslationService.class)
                        .putExtra(Intent.EXTRA_TEXT, text).putExtra("conversation", getCallingPackage()));
                } catch (RuntimeException error) {
                    Toast.makeText(this, R.string.text_action_failed, Toast.LENGTH_LONG).show();
                }
            }
        }
        // Returning no replacement also preserves editable selections in the source app.
        setResult(RESULT_CANCELED);
        finish();
    }
}
