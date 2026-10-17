package org.xrtranslate.app.textactions;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.ActivityNotFoundException;
import android.content.ComponentName;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ActivityInfo;
import android.content.pm.PackageManager;
import android.provider.Settings;
import android.widget.Toast;
import org.xrtranslate.app.R;

/** Offers Samsung's selection-menu setup once, after system permission dialogs close. */
public final class TextSelectionSetup {
    private static final ComponentName MANAGER = new ComponentName("com.android.settings",
        "com.samsung.android.settings.display.SecProcessTextManageAppsFragment");
    private static final String OFFERED = "text_selection_setup_offered";
    private TextSelectionSetup() {}

    public static void offer(Activity activity) {
        if (activity.isFinishing() || activity.isDestroyed()) return;
        SharedPreferences preferences = activity.getSharedPreferences("platform_permissions", Activity.MODE_PRIVATE);
        if (preferences.getBoolean(OFFERED, false)) return;
        try {
            ActivityInfo manager = activity.getPackageManager().getActivityInfo(MANAGER, 0);
            if (!manager.enabled || !manager.exported || !manager.applicationInfo.enabled) return;
            // Samsung and Chromium use this list for PROCESS_TEXT visibility. Only Settings writes it.
            String enabled = Settings.Global.getString(activity.getContentResolver(), "process_text_manager_apps");
            if (enabled != null) {
                for (String entry : enabled.split("#")) {
                    if (entry.equals(activity.getPackageName()) || entry.equals(activity.getPackageName() + ".TextTranslation")) {
                        preferences.edit().putBoolean(OFFERED, true).apply();
                        return;
                    }
                }
            }
        } catch (PackageManager.NameNotFoundException | SecurityException unavailable) {
            return;
        }
        new AlertDialog.Builder(activity)
            .setTitle(R.string.text_selection_setup_title)
            .setMessage(R.string.text_selection_setup_message)
            .setPositiveButton(R.string.text_selection_setup_open, (dialog, which) -> {
                try {
                    activity.startActivity(new Intent().setComponent(MANAGER));
                } catch (ActivityNotFoundException | SecurityException unavailable) {
                    Toast.makeText(activity, R.string.text_selection_setup_unavailable, Toast.LENGTH_LONG).show();
                }
            })
            .setNegativeButton(android.R.string.cancel, null)
            .show();
        preferences.edit().putBoolean(OFFERED, true).apply();
    }
}
