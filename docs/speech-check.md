# Recorded speech check

`check-speech` replays a saved voice recording through the same backend route as
the desktop client's speak mode: 16 kHz microphone PCM, voice activity
detection, recognition, translation, voice registration, and synthesized audio.
It sends 100 ms audio frames at recording speed and waits for the backend's
ordered completion event. The command prints recognized and translated text,
stage timings, and the path to the synthesized WAV.

Prepare one clear 3–8 second recording with a known transcript. Leave a short
pause at the end. The same recording can be reused without speaking again.
Convert an existing recording or media excerpt with FFmpeg if necessary:

```sh
ffmpeg -i recording.mp3 -ar 16000 -ac 1 -c:a pcm_s16le speech.wav
```

Select installed recognition, translation, and TTS providers in XRTranslate.
The TTS language pack must support the target language. Start a desktop
translation session so its backend is running, then from the repository root:

```sh
cargo run -p rust-client --example check-speech -- speech.wav zh en
```

The third and fourth arguments are the spoken and target language codes. An
optional fifth argument selects the output WAV; the default is
`target/check-speech.wav`. Listen to that file and compare the printed text to
the known transcript and intended meaning. The command fails when recognition,
translation, voice registration, or TTS audio is missing. Its time stamps expose
slow stages; audio quality still requires listening. With TTS provider `none`,
the command still reports recognition and translation, then exits with a clear
message that synthesis remains unchecked.
