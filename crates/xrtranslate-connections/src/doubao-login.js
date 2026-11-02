// Runs only in the dedicated official-site login view. No native IPC bridge.
(() => {
    if (location.origin !== "https://www.doubao.com" || window.top !== window) return;
    if (window.__xrAccountCapture) return;
    let confirmed = false;
    let displayName = "";
    let accountId = "";
    const observe = data => {
        const profile = data?.data?.profile_brief;
        if (data?.code === 0 && profile) {
            confirmed = true;
            const id = profile.user_id ?? profile.uid;
            accountId = (typeof id === "string" || typeof id === "number") ? String(id).slice(0, 160) : "";
            // Only use identity text already returned by the official login page.
            displayName = [profile.email, profile.phone_number, profile.mobile, profile.nickname, profile.name]
                .find(value => typeof value === "string" && value.trim())?.trim().slice(0, 160) || "";
        }
    };
    const isProfile = target => {
        try {
            const url = new URL(target, location.href);
            return url.origin === location.origin && url.pathname === "/alice/profile/self";
        } catch { return false; }
    };
    const originalFetch = window.fetch;
    window.fetch = function (...args) {
        const promise = originalFetch.apply(this, args);
        if (isProfile(typeof args[0] === "string" ? args[0] : args[0]?.url)) {
            promise.then(response => response.clone().json()).then(observe).catch(() => {});
        }
        return promise;
    };
    const open = XMLHttpRequest.prototype.open;
    const send = XMLHttpRequest.prototype.send;
    const requests = new WeakSet();
    XMLHttpRequest.prototype.open = function (...args) {
        requests.delete(this);
        if (isProfile(args[1])) requests.add(this);
        return open.apply(this, args);
    };
    XMLHttpRequest.prototype.send = function (...args) {
        if (requests.has(this)) this.addEventListener("load", () => {
            try { observe(JSON.parse(this.responseText)); } catch {}
        }, { once: true });
        return send.apply(this, args);
    };
    window.__xrAccountCapture = () => {
        try {
            const device = JSON.parse(localStorage.getItem("samantha_web_web_id") || "{}");
            const tea = JSON.parse(localStorage.getItem("__tea_cache_tokens_497858") || "{}");
            return { confirmed, attributes: { device_id: String(device.web_id || ""), web_id: String(tea.web_id || ""), display_name: displayName, account_id: accountId } };
        } catch { return null; }
    };
})();
