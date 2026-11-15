# NetEase playback events

Source: https://github.com/Coooookies/netease-cloudmusic-detector
Revision: f86eb7c83914ffcc045ca2279655fda2b87b279c
License: MIT (Copyright (c) 2025 ButterCookies); retained in LICENSE.

The Windows adapter adapts `ElogAnalysis.decode` and the playback event names
in `src/cloudmusic-detector/constant.ts`. It reads the local log incrementally
and reconstructs a bounded playback clock from native load, state and seek events.
SMTC supplies the selected track and playback state; events must match both and
belong to a live CloudMusic process. No Node.js helper or client modification is used.
