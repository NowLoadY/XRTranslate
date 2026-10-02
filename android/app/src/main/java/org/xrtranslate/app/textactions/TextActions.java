package org.xrtranslate.app.textactions;

import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.os.Build;
import android.text.Editable;
import android.text.InputFilter;
import android.text.TextWatcher;
import android.view.View;
import android.widget.EditText;
import android.widget.TextView;
import android.widget.Toast;
import androidx.appcompat.app.AlertDialog;
import java.util.UUID;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

/** Platform entry points only: translation and request ownership live in Rust. */
public final class TextActions {
    public static final String WRITE = "org.xrtranslate.app.WRITE_TEXT";
    public static final String CLIPBOARD = "org.xrtranslate.app.TRANSLATE_CLIPBOARD";
    private static final int MAX_TEXT = 32768;
    private static native void submit(String id, String text, boolean configure);
    private static native void cancel();

    private final MainActivity activity;
    private final SharedPreferences draft;
    private AlertDialog dialog;
    private EditText editor;
    private TextView statusView;
    private String id = "", text = "", result = "";
    private int status = R.string.text_action_hint;
    private boolean external, requested, configuring, resumed, readClipboard;

    public TextActions(MainActivity activity) {
        this.activity = activity;
        draft = activity.getSharedPreferences("text_action_draft", Context.MODE_PRIVATE);
    }

    public void create(Intent intent, boolean restored) {
        if (!restored && accept(intent)) return;
        id = draft.getString("id", "");
        text = draft.getString("text", "");
        result = draft.getString("result", "");
        external = draft.getBoolean("external", false);
        requested = draft.getBoolean("requested", false);
        configuring = draft.getBoolean("configuring", false);
        if (!id.isEmpty() && requested && result.isEmpty() && !text.trim().isEmpty()) {
            status = configuring ? R.string.text_action_setup : R.string.text_action_translating;
            submit(id, text, configuring);
        }
    }

    public boolean accept(Intent intent) {
        if (intent == null) return false;
        String action = intent.getAction();
        boolean shared = Intent.ACTION_PROCESS_TEXT.equals(action) || Intent.ACTION_SEND.equals(action);
        boolean clipboard = CLIPBOARD.equals(action);
        if (!shared && !clipboard && !WRITE.equals(action)) return false;
        if (shared && !"text/plain".equals(intent.getType())) return false;
        CharSequence incoming = shared ? intent.getCharSequenceExtra(
            Intent.ACTION_PROCESS_TEXT.equals(action) ? Intent.EXTRA_PROCESS_TEXT : Intent.EXTRA_TEXT) : "";
        // Consume the payload once; recreation resumes the private pending draft.
        activity.setIntent(new Intent(activity, MainActivity.class));
        cancel();
        dismiss();
        id = UUID.randomUUID().toString();
        text = incoming == null ? "" : incoming.toString().trim();
        result = "";
        external = shared;
        requested = configuring = false;
        readClipboard = clipboard;
        status = R.string.text_action_hint;
        boolean tooLong = text.length() > MAX_TEXT;
        if (tooLong) {
            text = "";
            status = R.string.text_action_too_long;
        }
        save();
        if (shared && !text.isEmpty()) start(false);
        if (resumed) show();
        return true;
    }

    public void resume() {
        resumed = true;
        if (!id.isEmpty() && !configuring && !readClipboard) show();
        focused();
    }

    public void pause() { resumed = false; save(); }

    public void focused() {
        if (!hasFocus()) return;
        if (readClipboard) {
            readClipboard = false;
            ClipboardManager clipboard = activity.getSystemService(ClipboardManager.class);
            ClipData clip = clipboard.getPrimaryClip();
            CharSequence value = clip != null && clip.getItemCount() > 0 ? clip.getItemAt(0).getText() : null;
            text = value == null ? "" : value.toString().trim();
            if (text.length() > MAX_TEXT) {
                text = "";
                status = R.string.text_action_too_long;
            } else if (text.isEmpty()) status = R.string.text_action_empty_clipboard;
            else start(false);
            save();
            show();
        }
        copyCompleted();
    }

    private boolean hasFocus() {
        return resumed && (activity.hasWindowFocus() || (dialog != null && dialog.getWindow() != null
            && dialog.getWindow().getDecorView().hasWindowFocus()));
    }

    private void show() {
        if (!resumed || configuring || id.isEmpty() || dialog != null || activity.isFinishing() || activity.isDestroyed()) return;
        View content = activity.getLayoutInflater().inflate(R.layout.text_action_dialog, null);
        editor = content.findViewById(R.id.text_action_input);
        statusView = content.findViewById(R.id.text_action_status);
        editor.setFilters(new InputFilter[]{new InputFilter.LengthFilter(MAX_TEXT)});
        editor.setText(text);
        statusView.setText(status);
        editor.addTextChangedListener(new TextWatcher() {
            public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
            public void onTextChanged(CharSequence s, int start, int before, int count) {
                if (requested || !result.isEmpty()) {
                    cancel();
                    id = UUID.randomUUID().toString();
                    requested = false;
                    result = "";
                }
                text = s.toString();
                status = R.string.text_action_hint;
                statusView.setText(status);
                save();
            }
            public void afterTextChanged(Editable s) {}
        });
        dialog = new AlertDialog.Builder(activity)
            .setTitle(R.string.text_action_title)
            .setView(content)
            .setPositiveButton(R.string.text_action_translate, null)
            .setNeutralButton(R.string.text_action_settings, null)
            .setNegativeButton(android.R.string.cancel, (which, button) -> close())
            .setOnCancelListener(which -> close())
            .create();
        dialog.setOnShowListener(which -> {
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener(view -> start(false));
            dialog.getButton(AlertDialog.BUTTON_NEUTRAL).setOnClickListener(view -> start(true));
            dialog.getWindow().getDecorView().getViewTreeObserver().addOnWindowFocusChangeListener(focus -> {
                if (focus) focused();
            });
            focused();
        });
        dialog.show();
    }

    private void start(boolean configure) {
        if (text.trim().isEmpty()) {
            status = R.string.text_action_hint;
            if (statusView != null) statusView.setText(status);
            return;
        }
        id = UUID.randomUUID().toString();
        result = "";
        requested = true;
        configuring = configure;
        status = configure ? R.string.text_action_setup : R.string.text_action_translating;
        save();
        if (configure) dismiss();
        else if (statusView != null) statusView.setText(status);
        submit(id, text, configure);
    }

    public void update(String request, int state, String translated) {
        if (!request.equals(id) || !requested || activity.isDestroyed()) return;
        if (state == 3 && translated != null && !translated.trim().isEmpty()) {
            result = translated;
            save();
            copyCompleted();
            return;
        }
        if (state == 2) {
            configuring = true;
            status = R.string.text_action_setup;
            save();
            dismiss();
            if (resumed) Toast.makeText(activity, status, Toast.LENGTH_LONG).show();
            return;
        }
        status = state == 1 ? R.string.text_action_translating
            : R.string.text_action_failed;
        if (state == 4) requested = false;
        configuring = false;
        save();
        if (statusView != null) statusView.setText(status);
        show();
    }

    private void copyCompleted() {
        if (result.isEmpty() || !hasFocus()) return;
        activity.getSystemService(ClipboardManager.class).setPrimaryClip(ClipData.newPlainText(
            activity.getString(R.string.text_action_title), result));
        if (Build.VERSION.SDK_INT < 33) Toast.makeText(activity, R.string.text_action_copied, Toast.LENGTH_SHORT).show();
        boolean returnToCaller = external;
        id = text = result = "";
        requested = configuring = false;
        draft.edit().clear().apply();
        dismiss();
        // Keep an existing XRT task and its UI state available for the next visit.
        if (returnToCaller) activity.moveTaskToBack(true);
    }

    private void save() {
        if (id.isEmpty()) return;
        draft.edit().putString("id", id).putString("text", text)
            .putString("result", result)
            .putBoolean("external", external).putBoolean("requested", requested)
            .putBoolean("configuring", configuring).apply();
    }

    private void close() {
        cancel();
        boolean returnToCaller = external;
        id = text = result = "";
        requested = configuring = readClipboard = false;
        draft.edit().clear().apply();
        dismiss();
        if (returnToCaller) activity.moveTaskToBack(true);
    }

    private void dismiss() {
        if (dialog != null) dialog.dismiss();
        dialog = null;
        editor = null;
        statusView = null;
    }

    public void destroy() {
        save();
        dismiss();
    }
}
