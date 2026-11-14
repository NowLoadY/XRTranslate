# Video assembly

`tools/demos/assemble.py` combines video clips, title transitions, audio,
subtitles and chapters. It requires Python, NumPy, Pillow and FFmpeg;
`--ffmpeg` selects the executable, otherwise imageio-ffmpeg supplies it.

```powershell
python tools/demos/assemble.py --help
```

The JSON input contains:

- `assets`: video, audio and optional subtitle or chapter files.
- `timeline`: ordered `video` and `title` items with unique IDs.
- `audio`: tracks with timing, gain and optional fades.
- `versions`: language, font and title variables.

File paths are relative to the JSON file. `--only` selects a version and
`--preview` renders a preview. The script's `work` and `output` fields select
the intermediate and final output paths.
