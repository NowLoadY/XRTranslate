package org.xrtranslate.app;

import android.app.Activity;
import android.app.Dialog;
import android.os.Handler;
import android.os.Looper;
import android.webkit.CookieManager;
import android.webkit.WebResourceRequest;
import android.webkit.WebSettings;
import android.webkit.WebStorage;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import androidx.webkit.WebViewCompat;
import androidx.webkit.WebViewFeature;
import org.json.JSONArray;
import org.json.JSONObject;
import java.net.URI;
import java.util.Collections;

/** Dedicated login view. Remote pages have no JavaScript-to-native interface. */
final class AccountLogin {
    private final Activity activity;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private Dialog dialog;
    private WebView view;
    private volatile String result;
    private boolean finishing;

    AccountLogin(Activity activity) { this.activity = activity; }

    void start(String specification) {
        activity.runOnUiThread(() -> {
            if (dialog != null) return;
            result = null;
            finishing = false;
            try {
                JSONObject spec = new JSONObject(specification);
                String url = spec.getString("url");
                String origin = spec.getString("origin");
                JSONArray hosts = spec.getJSONArray("hosts");
                view = new WebView(activity);
                WebSettings settings = view.getSettings();
                settings.setJavaScriptEnabled(true);
                settings.setDomStorageEnabled(true);
                settings.setAllowFileAccess(false);
                settings.setAllowContentAccess(false);
                settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
                settings.setSaveFormData(false);
                settings.setCacheMode(WebSettings.LOAD_NO_CACHE);
                // The connector follows the desktop site's speech/session profile.
                settings.setUserAgentString(spec.getString("user_agent"));
                CookieManager.getInstance().setAcceptThirdPartyCookies(view, false);
                if (!WebViewFeature.isFeatureSupported(WebViewFeature.DOCUMENT_START_SCRIPT)) {
                    finish("{\"error\":\"webview_update_required\"}");
                    return;
                }
                WebViewCompat.addDocumentStartJavaScript(view, spec.getString("initialization_script"), Collections.singleton(origin));
                view.setWebViewClient(new WebViewClient() {
                    @Override public boolean shouldOverrideUrlLoading(WebView v, WebResourceRequest request) {
                        return !allows(request.getUrl().toString(), hosts);
                    }
                });
                dialog = new Dialog(activity);
                dialog.setTitle("XRTranslate · " + spec.getString("label") + " · " + origin);
                dialog.setContentView(view);
                dialog.setOnCancelListener(ignored -> cancel());
                dialog.show();
                if (dialog.getWindow() != null) dialog.getWindow().setLayout(-1, -1);
                // This app uses its WebView only for temporary account login.
                // Other browsers' profiles and cookies are never accessed.
                WebStorage.getInstance().deleteAllData();
                CookieManager.getInstance().removeAllCookies(ignored -> {
                    if (view != null && !finishing) view.loadUrl(url);
                });
                Runnable poll = new Runnable() {
                    @Override public void run() {
                        if (view == null || finishing) return;
                        if (sameOrigin(view.getUrl(), origin)) {
                            view.evaluateJavascript(spec.optString("capture_script"), capture -> {
                                if (view == null || finishing || !sameOrigin(view.getUrl(), origin)) return;
                                try {
                                    JSONObject data = new JSONObject(capture);
                                    if (!data.optBoolean("confirmed")) return;
                                    JSONObject attributes = data.getJSONObject("attributes");
                                    JSONArray required = spec.getJSONArray("required_attributes");
                                    for (int i = 0; i < required.length(); i++) {
                                        if (attributes.optString(required.getString(i)).isEmpty()) return;
                                    }
                                    String cookies = CookieManager.getInstance().getCookie(url);
                                    if (cookies == null || cookies.isEmpty()) return;
                                    JSONObject value = new JSONObject();
                                    value.put("provider", spec.getString("provider"));
                                    value.put("url", view.getUrl());
                                    value.put("capture", data);
                                    value.put("cookies", cookies);
                                    finish(value.toString());
                                } catch (Exception ignored) { /* Wait for a complete login. */ }
                            });
                        }
                        handler.postDelayed(this, 1000);
                    }
                };
                handler.postDelayed(poll, 1000);
            } catch (Exception ignored) {
                finish("{\"error\":\"login_failed\"}");
            }
        });
    }

    private static boolean sameOrigin(String value, String origin) {
        if (value == null) return false;
        try {
            URI uri = URI.create(value), expected = URI.create(origin);
            return "https".equals(uri.getScheme()) && expected.getHost().equals(uri.getHost())
                && (uri.getPort() == -1 || uri.getPort() == 443) && uri.getUserInfo() == null;
        } catch (Exception ignored) { return false; }
    }

    private static boolean allows(String value, JSONArray hosts) {
        try {
            URI uri = URI.create(value);
            if (!"https".equals(uri.getScheme()) || uri.getUserInfo() != null || (uri.getPort() != -1 && uri.getPort() != 443)) return false;
            String host = uri.getHost();
            if (host == null) return false;
            for (int i = 0; i < hosts.length(); i++) {
                String allowed = hosts.getString(i);
                if (host.equals(allowed) || host.endsWith("." + allowed)) return true;
            }
        } catch (Exception ignored) {}
        return false;
    }

    void cancel() { activity.runOnUiThread(() -> finish("{\"cancelled\":true}")); }

    private void finish(String value) {
        if (finishing) return;
        finishing = true;
        handler.removeCallbacksAndMessages(null);
        if (dialog != null) { dialog.dismiss(); dialog = null; }
        if (view != null) { view.stopLoading(); view.clearHistory(); view.clearCache(true); view.destroy(); view = null; }
        WebStorage.getInstance().deleteAllData();
        CookieManager.getInstance().removeAllCookies(ignored -> {
            CookieManager.getInstance().flush();
            result = value;
        });
    }

    synchronized String takeResult() { String value = result; result = null; return value; }
}
