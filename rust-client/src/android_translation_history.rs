//! Final result adapter shared by Android's notification and home-screen widget.
use jni::objects::JValue;
use std::{collections::BTreeMap, sync::Mutex};

use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationOutcome, TranslationSegment,
    TranslationSessionOwner,
};

#[derive(Default)]
pub(crate) struct ResultSubscriber {
    pending: Mutex<BTreeMap<u64, Vec<TranslationSegment>>>,
}

impl SessionEventSubscriber for ResultSubscriber {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_host()
    }

    fn on_translation_event(&self, _: &TranslationSessionOwner, event: &TranslationEvent) {
        match event {
            TranslationEvent::Segment(segment) => self.accept(std::slice::from_ref(segment)),
            TranslationEvent::ReplaceSegments(segments) => self.accept(segments),
            TranslationEvent::StreamEnded { stream_id }
            | TranslationEvent::Finished {
                stream_id,
                outcome: TranslationOutcome::Completed,
            } => {
                if let Some(segments) = self.pending.lock().unwrap().remove(stream_id) {
                    for segment in &segments {
                        publish(segment);
                    }
                }
            }
            TranslationEvent::Finished { stream_id, .. } => {
                self.pending.lock().unwrap().remove(stream_id);
            }
            TranslationEvent::Preview(_) => {}
        }
    }
}

impl ResultSubscriber {
    fn accept(&self, segments: &[TranslationSegment]) {
        let Some(first) = segments.first() else {
            return;
        };
        let mut pending = self.pending.lock().unwrap();
        pending.remove(&first.stream_id);
        if segments.iter().any(|segment| segment.revisable) {
            pending.insert(first.stream_id, segments.to_vec());
        } else {
            drop(pending);
            for segment in segments {
                publish(segment);
            }
        }
    }
}

pub(crate) fn result_json(
    source: &str,
    translated: &str,
    source_lang: &str,
    target_lang: &str,
) -> String {
    use crate::history::archive::vocabulary;
    let can_save = vocabulary::can_save(source)
        && !translated.trim().is_empty()
        && translated.encode_utf16().count() <= 2048
        && vocabulary::Entry {
            source: source.into(),
            translated: translated.into(),
            source_lang: source_lang.into(),
            target_lang: target_lang.into(),
        }
        .canonical()
        .is_ok();
    serde_json::json!({"source": source, "translated": translated,
        "source_lang": source_lang, "target_lang": target_lang, "can_save": can_save})
    .to_string()
}

fn publish(segment: &TranslationSegment) {
    let Some(translated) = segment
        .translated
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    else {
        return;
    };
    let payload = result_json(
        &segment.source,
        translated,
        &segment.source_lang,
        &segment.target_lang,
    );
    let _ = crate::android::with_activity(|env, activity| {
        env.with_local_frame(1, |env| {
            let payload = env.new_string(payload)?;
            env.call_method(
                activity,
                "publishTranslationResult",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&payload)],
            )?;
            Ok::<_, jni::errors::Error>(())
        })
    });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_textactions_BookmarkAction_save(
    mut env: jni::JNIEnv<'_>,
    _class: jni::objects::JClass<'_>,
    directory: jni::objects::JString<'_>,
    payload: jni::objects::JString<'_>,
) -> jni::sys::jboolean {
    let save = (|| -> Result<(), String> {
        let directory: String = env
            .get_string(&directory)
            .map_err(|e| e.to_string())?
            .into();
        let payload: String = env.get_string(&payload).map_err(|e| e.to_string())?.into();
        #[derive(serde::Deserialize)]
        struct Bookmark {
            source: String,
            translated: String,
            source_lang: String,
            target_lang: String,
        }
        let value: Bookmark = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
        crate::history::archive::save_default_word(
            std::path::Path::new(&directory),
            crate::history::archive::vocabulary::Entry {
                source: value.source,
                translated: value.translated,
                source_lang: value.source_lang,
                target_lang: value.target_lang,
            },
        )
    })();
    save.is_ok().into()
}
