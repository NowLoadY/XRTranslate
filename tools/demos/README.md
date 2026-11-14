# UI Director tools

These tools record XRTranslate through [UI Director](../../crates/ui-director/README.md).

| Command | Purpose |
| --- | --- |
| `session.py` | Start an isolated client and record a JSON script |
| `record.py` | Record an existing client or its built-in guide |
| `live.py` | Capture a live application window |
| `finish.py` | Compose captured footage and audio |
| `assemble.py` | Assemble clips, titles, audio and subtitles |

Recording requires Windows, Python, Pillow and imageio-ffmpeg. Live capture
also requires windows-capture, NumPy and OpenCV.

```powershell
python tools/demos/session.py --help
python tools/demos/record.py --help
python tools/demos/live.py --help
python tools/demos/finish.py --help
python tools/demos/assemble.py --help
```

Use a fresh portable package for `session.py --bundle`. Recording uses the
client's Director port; `--port` selects it. `record.py --guide` records the
bundled guide, with optional `--page` and `--preview` filters.

See [video assembly](assembly/README.md) for the assembly input format.
