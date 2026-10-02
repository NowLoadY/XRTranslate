package org.xrtranslate.app.textactions;

import android.app.PendingIntent;
import android.appwidget.AppWidgetManager;
import android.appwidget.AppWidgetProvider;
import android.content.Context;
import android.content.Intent;
import android.widget.RemoteViews;
import org.xrtranslate.app.MainActivity;
import org.xrtranslate.app.R;

public final class TextActionsWidget extends AppWidgetProvider {
    @Override public void onUpdate(Context context, AppWidgetManager manager, int[] ids) {
        for (int id : ids) {
            RemoteViews view = new RemoteViews(context.getPackageName(), R.layout.text_actions_widget);
            view.setOnClickPendingIntent(R.id.widget_write, entry(context, TextActions.WRITE, 1));
            view.setOnClickPendingIntent(R.id.widget_clipboard, entry(context, TextActions.CLIPBOARD, 2));
            manager.updateAppWidget(id, view);
        }
    }

    private PendingIntent entry(Context context, String action, int code) {
        Intent intent = new Intent(context, MainActivity.class).setAction(action);
        return PendingIntent.getActivity(context, code, intent,
            PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }
}
