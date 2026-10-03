package org.xrtranslate.app.updates;

import android.content.ClipData;
import android.content.Intent;
import android.content.pm.PackageInfo;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Bundle;
import android.provider.Settings;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.core.content.FileProvider;
import java.io.File;
import org.xrtranslate.app.MainActivity;

/** Only the platform installation handoff lives here; discovery and download are shared. */
public final class UpdateInstaller {
    public interface Completion { void complete(String error); }
    private final MainActivity activity;
    private final Completion completion;
    private final ActivityResultLauncher<Intent> permission;
    private final ActivityResultLauncher<Intent> installer;
    private String pending;

    public UpdateInstaller(MainActivity activity, Completion completion) {
        this.activity = activity;
        this.completion = completion;
        permission = activity.registerForActivityResult(new ActivityResultContracts.StartActivityForResult(), result -> {
            if (activity.getPackageManager().canRequestPackageInstalls()) launchInstaller();
            else finish(null); // A declined permission is retryable, just like cancelling installation.
        });
        installer = activity.registerForActivityResult(new ActivityResultContracts.StartActivityForResult(), result -> finish(null));
    }

    public void restore(Bundle state) {
        if (state != null) pending = state.getString("update_apk");
    }

    public void save(Bundle state) { state.putString("update_apk", pending); }

    public String validate(String path, String version) {
        try {
            File apk = new File(path).getCanonicalFile();
            File root = new File(activity.getFilesDir(), "runtime/updates/downloads").getCanonicalFile();
            if (!root.equals(apk.getParentFile()) || !apk.isFile()) return "The downloaded update is missing.";
            PackageManager manager = activity.getPackageManager();
            PackageInfo candidate = manager.getPackageArchiveInfo(apk.getPath(), PackageManager.GET_SIGNING_CERTIFICATES);
            PackageInfo installed = manager.getPackageInfo(activity.getPackageName(), PackageManager.GET_SIGNING_CERTIFICATES);
            if (candidate == null || !activity.getPackageName().equals(candidate.packageName)) return "The update is not an XRTranslate package.";
            if (!version.equals(candidate.versionName)) return "The update version does not match the selected release.";
            if (candidate.getLongVersionCode() <= installed.getLongVersionCode()) return "The update is not newer than the installed version.";
            if (candidate.signingInfo == null || installed.signingInfo == null) return "The update has no valid signature.";
            // Exact signer equality, or an authenticated rotation lineage containing the installed signer.
            android.content.pm.Signature[] current = installed.signingInfo.getApkContentsSigners();
            android.content.pm.Signature[] next = candidate.signingInfo.getApkContentsSigners();
            if (!new java.util.HashSet<>(java.util.Arrays.asList(current)).equals(new java.util.HashSet<>(java.util.Arrays.asList(next)))) {
                android.content.pm.Signature[] lineage = candidate.signingInfo.getSigningCertificateHistory();
                if (current.length != 1 || candidate.signingInfo.hasMultipleSigners() || lineage == null
                    || !java.util.Arrays.asList(lineage).contains(current[0])) return "The update signature does not match this installation.";
            }
            return null;
        } catch (Exception error) { return "Cannot validate the downloaded update."; }
    }

    public void request(String path) {
        activity.runOnUiThread(() -> {
            if (pending != null || activity.isFinishing() || activity.isDestroyed()) {
                completion.complete("Finish the current installation first.");
                return;
            }
            pending = path;
            try {
                if (activity.getPackageManager().canRequestPackageInstalls()) launchInstaller();
                else permission.launch(new Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                    Uri.parse("package:" + activity.getPackageName())));
            } catch (RuntimeException error) { finish("Cannot open Android installation settings."); }
        });
    }

    private void launchInstaller() {
        if (pending == null) { finish(null); return; }
        try {
            Uri uri = FileProvider.getUriForFile(activity, activity.getPackageName() + ".exports", new File(pending));
            Intent intent = new Intent(Intent.ACTION_VIEW)
                .setDataAndType(uri, "application/vnd.android.package-archive")
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
            intent.setClipData(ClipData.newRawUri("XRTranslate", uri));
            installer.launch(intent);
        } catch (RuntimeException error) { finish("Cannot open the Android package installer."); }
    }

    private void finish(String error) {
        pending = null;
        completion.complete(error);
    }
}
