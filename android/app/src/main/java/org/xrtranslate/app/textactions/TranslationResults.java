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
        final String source, translated;
        Result(long id, String source, String translated) {
            this.id = id; this.source = source; this.translated = translated;
        }
        boolean matches(String input, String output) {
            return source.equals(input) && translated.equals(output);
        }
    }

    private TranslationResults() {}

    public static synchronized void publish(Context context, String source, String translated) {
        source = bounded(source); translated = bounded(translated);
        if (translated.isEmpty()) return;
        Context application = context.getApplicationContext();
        load(application);
        if (!results.isEmpty() && results.get(0).matches(source, translated)) return;
        for (int i = results.size() - 1; i >= 0; i--) {
            if (results.get(i).matches(source, translated)) results.remove(i);
        }
        results.add(0, new Result(++nextId, source, translated));
        if (results.size() > MAX_RESULTS) results.remove(results.size() - 1);
        generation++;
        if (!updating) {
            updating = true;
            WRITER.execute(() -> persistAndRefresh(application));
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
                results.add(new Result(id, bounded(item.optString("source")), translated));
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
            for (Result result : snapshot) {
                JSONObject item = new JSONObject();
                try {
                    item.put("id", result.id).put("source", result.source).put("translated", result.translated);
                } catch (JSONException impossible) {
                    throw new IllegalStateException(impossible);
                }
                saved.put(item);
            }
            try {
                context.getSharedPreferences(STORAGE, Context.MODE_PRIVATE).edit().putString(KEY, saved.toString()).commit();
                TextActionsWidget.refresh(context);
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
