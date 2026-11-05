# Media speech tracks

With TTS configured, a media task with complete translated subtitles offers
**Generate speech track**. It uses the task's target language and the voice
selected in TTS Center. Generation has progress and cancellation; the completed
track is selected for playback and can be exported as WAV. The host's live
speech-playback switch does not control file generation.

`tts_session::speech` exposes `POST /tts/synthesize`: a `SpeechRequest` containing
text, language and an optional voice selection returns mono PCM16 WAV. It reuses
the backend's prepared speech resources, language checks, voice library and
model input limits. It neither translates the text nor plays the result. The
request does not alter the globally selected voice. This endpoint is independent
of the media plugin.

`media::speech` owns the cancellable file job. It snapshots the translated cues
and voice, requests one cue at a time, resamples to 48 kHz and writes progressively
to a temporary WAV. Each cue starts at its subtitle timestamp. Speech can use
the following gap; if it would overlap the next cue, the existing MPV runtime
accelerates it without changing pitch. The final cue fits within the media's
remaining duration. No Python or separate FFmpeg executable is used. Builds
without MPV can generate naturally fitting speech, but report when timing
adjustment requires the playback runtime.

Missing translations, mismatched target languages and invalid timing fail before
inference. A failed or cancelled job removes its temporary output. Changed
subtitle input is detected before publishing. Only a completed track replaces
the saved track; saving failure preserves the previous one. Errors include the
cue number and time interval in client logs, while backend logs retain synthesis
failures. Audio content and credentials are not logged by the file job.

`MediaTask::source_audio` remains the recognition input. `playback_audio` may
instead select the generated track; this avoids feeding synthesized translation
back into recognition or changing the source channel settings. Completed files
belong to `runtime/media-speech`, with references stored in the existing media
task document. Regeneration and task deletion clean up owned files.

The UI uses existing shared buttons and menus, so CLI discovery and operation
need no additional action protocol. The existing task query includes the saved
speech track and playback selection.

The ignored integration check `translated_speech_is_aligned_without_changing_recognition_input`
uses a local HTTP fixture and the installed MPV runtime. It verifies translated
request text, voice selection, cue timing, silence, pitch preservation and
recognition-source isolation without loading AI models.
