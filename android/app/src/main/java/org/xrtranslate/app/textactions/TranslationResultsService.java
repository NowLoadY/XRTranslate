package org.xrtranslate.app.textactions;

import android.content.Context;
import android.content.Intent;
import android.widget.RemoteViews;
import android.widget.RemoteViewsService;
import java.util.Collections;
import java.util.List;

/** Android 10/11 collection adapter; newer systems receive the snapshot directly. */
public final class TranslationResultsService extends RemoteViewsService {
    @Override public RemoteViewsFactory onGetViewFactory(Intent intent) {
        return new Factory(getApplicationContext());
    }

    private static final class Factory implements RemoteViewsFactory {
        private final Context context;
        private volatile List<TranslationResults.Result> results = Collections.emptyList();
        Factory(Context context) { this.context = context; }
        @Override public void onCreate() { onDataSetChanged(); }
        @Override public void onDataSetChanged() { results = TranslationResults.snapshot(context); }
        @Override public void onDestroy() { results = Collections.emptyList(); }
        @Override public int getCount() { return results.size(); }
        @Override public RemoteViews getViewAt(int position) {
            List<TranslationResults.Result> snapshot = results;
            return position >= 0 && position < snapshot.size() ? TextActionsWidget.row(context, snapshot.get(position)) : null;
        }
        @Override public RemoteViews getLoadingView() { return null; }
        @Override public int getViewTypeCount() { return 1; }
        @Override public long getItemId(int position) {
            List<TranslationResults.Result> snapshot = results;
            return position >= 0 && position < snapshot.size() ? snapshot.get(position).id : 0;
        }
        @Override public boolean hasStableIds() { return true; }
    }
}
