package org.xrtranslate.app;

import android.app.Application;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.net.ConnectivityManager;
import android.net.Proxy;
import android.net.ProxyInfo;
import android.os.Build;
import android.system.Os;
import android.util.AtomicFile;
import android.util.Log;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import org.json.JSONArray;
import org.json.JSONObject;

/** Bridge system network settings to the UI and native backend processes. */
public final class XRTranslateApplication extends Application {
    private AtomicFile proxyFile;

    @Override public void onCreate() {
        super.onCreate();
        File path = new File(getFilesDir(), "system-proxy.json");
        proxyFile = new AtomicFile(path);
        // Publish the path once, before any native library or worker starts.
        try { Os.setenv("XRT_ANDROID_PROXY_FILE", path.getAbsolutePath(), true); }
        catch (Exception error) { Log.w("XRTranslate", "Cannot initialize system proxy bridge", error); }
        BroadcastReceiver receiver = new BroadcastReceiver() {
            @Override public void onReceive(Context context, Intent intent) { updateProxy(); }
        };
        IntentFilter filter = new IntentFilter(Proxy.PROXY_CHANGE_ACTION);
        if (Build.VERSION.SDK_INT >= 33) registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED);
        else registerReceiver(receiver, filter);
        updateProxy();
    }

    private void updateProxy() {
        FileOutputStream output = null;
        try {
            ProxyInfo proxy = getSystemService(ConnectivityManager.class).getDefaultProxy();
            JSONObject settings = new JSONObject();
            String url = "";
            if (proxy != null && proxy.getHost() != null && proxy.getPort() > 0) {
                String host = proxy.getHost();
                if (host.contains(":")) host = "[" + host + "]";
                url = "http://" + host + ":" + proxy.getPort();
            }
            settings.put("url", url);
            settings.put("exclusions", new JSONArray(proxy == null ? new String[0] : proxy.getExclusionList()));
            output = proxyFile.startWrite();
            output.write(settings.toString().getBytes(StandardCharsets.UTF_8));
            proxyFile.finishWrite(output);
        } catch (Exception error) {
            if (output != null) proxyFile.failWrite(output);
            Log.w("XRTranslate", "Cannot refresh system proxy settings", error);
        }
    }
}
