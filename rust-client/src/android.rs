pub use xrtranslate_client_core::android::*;
mod navigation;

#[unsafe(no_mangle)]
fn android_main(activity: android_activity::AndroidApp) {
    xrtranslate_client_core::android::launch(activity, || {
        crate::run().map_err(|error| error.to_string())
    });
}
