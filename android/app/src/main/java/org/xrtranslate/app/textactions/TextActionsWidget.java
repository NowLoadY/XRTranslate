package org.xrtranslate.app.textactions;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.view.View;
import android.widget.RemoteViews;
import java.util.List;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

/** Kept under its existing provider name so installed widgets update in place. */
public final class TextActionsWidget extends AppWidgetProvider {
    public static final String OPEN_TRANSLATION = "org.xrtranslate.app.OPEN_TRANSLATION";

    @Override public void onUpdate(Context context, AppWidgetManager manager, int[] ids) {
        update(context, manager, ids);
    }

    @Override public void onAppWidgetOptionsChanged(Context context, AppWidgetManager manager,
            int id, Bundle options) {
        update(context, manager, new int[]{id});
    }

    static void refresh(Context context) {
        AppWidgetManager manager = AppWidgetManager.getInstance(context);
        update(context, manager, manager.getAppWidgetIds(new ComponentName(context, TextActionsWidget.class)));
    }

    private static void update(Context context, AppWidgetManager manager, int[] ids) {
        if (ids.length == 0) return;
        List<TranslationResults.Result> results = TranslationResults.snapshot(context);
        for (int id : ids) {
            RemoteViews view = new RemoteViews(context.getPackageName(), R.layout.translation_results_widget);
            PendingIntent open = entry(context, false);
            view.setOnClickPendingIntent(R.id.widget_surface, open);
            view.setOnClickPendingIntent(R.id.widget_header, open);
            view.setOnClickPendingIntent(R.id.widget_empty, open);
            view.setEmptyView(R.id.widget_results, R.id.widget_empty);
            view.setPendingIntentTemplate(R.id.widget_results, entry(context, true));
            if (Build.VERSION.SDK_INT >= 31) {
                RemoteViews.RemoteCollectionItems.Builder items = new RemoteViews.RemoteCollectionItems.Builder()
                    .setHasStableIds(true).setViewTypeCount(1);
                for (TranslationResults.Result result : results) items.addItem(result.id, row(context, result));
                view.setRemoteAdapter(R.id.widget_results, items.build());
            } else {
                legacyAdapter(context, view, id);
            }
            manager.updateAppWidget(id, view);
        }
        if (Build.VERSION.SDK_INT < 31) refreshLegacy(manager, ids);
    }

    static RemoteViews row(Context context, TranslationResults.Result result) {
        RemoteViews view = new RemoteViews(context.getPackageName(), R.layout.translation_result_item);
        view.setTextViewText(R.id.widget_source, result.source);
        view.setViewVisibility(R.id.widget_source, result.source.isEmpty() ? View.GONE : View.VISIBLE);
        view.setTextViewText(R.id.widget_translation, result.translated);
        view.setOnClickFillInIntent(R.id.widget_result_item, new Intent());
        return view;
    }

    private static PendingIntent entry(Context context, boolean collection) {
        Intent intent = new Intent(context, MainActivity.class).setAction(OPEN_TRANSLATION)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        int flags = PendingIntent.FLAG_UPDATE_CURRENT;
        // Collection clicks need fill-in intents; their destination remains explicit.
        flags |= collection ? (Build.VERSION.SDK_INT >= 31 ? PendingIntent.FLAG_MUTABLE : 0)
                            : PendingIntent.FLAG_IMMUTABLE;
        return PendingIntent.getActivity(context, collection ? 102 : 101, intent, flags);
    }

    @SuppressWarnings("deprecation") // Collection service compatibility for Android 10 and 11 only.
    private static void legacyAdapter(Context context, RemoteViews view, int id) {
        Intent adapter = new Intent(context, TranslationResultsService.class)
            .putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id)
            .setData(Uri.parse("xrt-widget://results/" + id));
        view.setRemoteAdapter(R.id.widget_results, adapter);
    }

    @SuppressWarnings("deprecation") // Superseded by RemoteCollectionItems on Android 12+.
    private static void refreshLegacy(AppWidgetManager manager, int[] ids) {
        manager.notifyAppWidgetViewDataChanged(ids, R.id.widget_results);
    }
}
