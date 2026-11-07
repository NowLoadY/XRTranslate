# Media speech tracks

After translating the subtitles, choose **Generate speech track** in the media
task. The speech uses the task's target language and the voice selected in
TTS Center. You can follow its progress, cancel it, or continue unfinished work.
The live speech-playback switch does not need to be enabled.

For online voices, choose a service under **Voice Playback** on the Welcome
page and enter its API key. MiniMax, SiliconFlow and Alibaba Cloud Bailian are
available alongside Google and Groq. Select the voice in **TTS Center** before
generating the track. For a long video, check the provider's available credit
and usage limits first.

The generated speech follows the subtitle timeline. You can listen to it from
the player's audio-track menu and adjust the speech clips on the timeline.
If you change the translated text or target language, finish generating the
updated speech before exporting a dubbed video.

## Export a dubbed video

Open **Export → Export video**, then select **Speech track**:

- **Keep background audio** combines the speech with the separated background.
  Enable vocal separation when recognizing the media to prepare this background
  track. If it is unavailable, the export settings tell you how to prepare it;
  you can also export speech alone.
- **Burn subtitles into video** is optional. It uses the subtitle content, font,
  size and color configured in the player.

Dubbed exports replace the original audio with the selected speech and optional
background. They do not mix the original audio back in. The quality of background
separation determines whether faint traces of the original voice remain.
Choose **Original audio** instead if you want a subtitled video with its original
sound.

The task remembers the export choices. Video export shows progress and supports
cancellation. The original video is preserved. Local video export requires video
playback support; install it from the media page if it is not available yet.

To save speech without a video, choose **Export speech track** for a WAV file.
Subtitles can still be exported separately as SRT, VTT or LRC.
