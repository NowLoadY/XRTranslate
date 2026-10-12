package org.xrtranslate.app.textactions;

import android.content.Context;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import org.json.JSONArray;
import org.json.JSONException;
import org.json.JSONObject;

/** A small completed-result snapshot shared by the app, background entry and widget. */
public final class TranslationResults {
    private static final int MAX_RESULTS = 20, MAX_TEXT = 2048;
    private static final String STORAGE = "translation_results", KEY = "recent";
    private static final ExecutorService WRITER = Executors.newSingleThreadExecutor(
        runnable -> new Thread(runnable, "translation-widget"));
    private static List<Result> results;
    private static long generation, nextId;
    private static boolean updating;

    static final class Result {
        final long id;
        final String source, translated, sourceLang, targetLang;
        final boolean canSave;
        volatile boolean saved;
        Result(long id, JSONObject value) {
            this.id = id;
            source = bounded(value.optString("source"));
            translated = bounded(value.optString("translated"));
            sourceLang = value.optString("source_lang");
            targetLang = value.optString("target_lang");
            canSave = value.optBoolean("can_save") && !source.isEmpty()
                && !sourceLang.isEmpty() && !targetLang.isEmpty();
            saved = value.optBoolean("saved");
        }
        boolean matches(Result other) {
            return source.equals(other.source) && translated.equals(other.translated)
                && sourceLang.equals(other.sourceLang) && targetLang.equals(other.targetLang);
        }
        JSONObject json() {
            JSONObject value = new JSONObject();
            try {
                value.put("id", id).put("source", source).put("translated", translated)
                    .put("source_lang", sourceLang).put("target_lang", targetLang)
                    .put("can_save", canSave).put("saved", saved);
            } catch (JSONException impossible) { throw new IllegalStateException(impossible); }
            return value;
        }
    }

    private TranslationResults() {}

    public static void publish(Context context, String payload) {
        try { publish(context, new JSONObject(payload)); }
        catch (JSONException invalid) { android.util.Log.w("XRTranslate", "Invalid result snapshot"); }
    }

    public static synchronized void publish(Context context, JSONObject payload) {
        Context application = context.getApplicationContext();
        load(application);
        Result next = new Result(++nextId, payload);
        if (next.translated.isEmpty()) return;
        if (!results.isEmpty() && results.get(0).matches(next)) return;
        for (int i = results.size() - 1; i >= 0; i--) {
            if (results.get(i).matches(next)) {
                next.saved = results.get(i).saved;
                results.remove(i);
            }
        }
        results.add(0, next);
        if (results.size() > MAX_RESULTS) results.remove(results.size() - 1);
        changed(application);
    }

    static synchronized void markSaved(Context context, long id) {
        for (Result result : snapshot(context)) if (result.id == id) {
            result.saved = true;
            changed(context.getApplicationContext());
            break;
        }
    }

    private static void changed(Context context) {
        generation++;
        if (!updating) {
            updating = true;
            WRITER.execute(() -> persistAndRefresh(context));
        }
    }

    static synchronized List<Result> snapshot(Context context) {
        load(context);
        return new ArrayList<>(results);
    }

    private static void load(Context context) {
        if (results != null) return;
        results = new ArrayList<>();
        try {
            JSONArray saved = new JSONArray(context.getSharedPreferences(STORAGE, Context.MODE_PRIVATE)
                .getString(KEY, "[]"));
            for (int i = 0; i < Math.min(saved.length(), MAX_RESULTS); i++) {
                JSONObject item = saved.getJSONObject(i);
                String translated = bounded(item.optString("translated"));
                if (translated.isEmpty()) continue;
                long id = item.getLong("id");
                nextId = Math.max(nextId, id);
                results.add(new Result(id, item));
            }
        } catch (JSONException ignored) {
            results.clear();
        }
    }

    private static void persistAndRefresh(Context context) {
        // Only this single runnable is queued; publications replace the bounded
        // snapshot while a disk write or launcher update is in progress.
        while (true) {
            List<Result> snapshot;
            long revision;
            synchronized (TranslationResults.class) {
                snapshot = new ArrayList<>(results);
                revision = generation;
            }
            JSONArray saved = new JSONArray();
            for (Result result : snapshot) saved.put(result.json());
            try {
                context.getSharedPreferences(STORAGE, Context.MODE_PRIVATE).edit().putString(KEY, saved.toString()).commit();
                TextActionsWidget.refresh(context);
                QuickActions.refresh(context);
            } catch (RuntimeException unavailable) {
                // A temporarily unavailable launcher must not block future results.
            }
            synchronized (TranslationResults.class) {
                if (revision == generation) {
                    updating = false;
                    return;
                }
            }
        }
    }

    private static String bounded(String text) {
        if (text == null) return "";
        text = text.trim();
        if (text.length() <= MAX_TEXT) return text;
        int end = Character.isHighSurrogate(text.charAt(MAX_TEXT - 1)) ? MAX_TEXT - 1 : MAX_TEXT;
        return text.substring(0, end) + "…";
    }
}
