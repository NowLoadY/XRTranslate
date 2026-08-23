<p align="center">
  <img src="rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Download the latest release</a>
</p>

<p align="center">
  <strong>Real-Time Subtitles & Voice Translation for Desktop, Media, and VRChat</strong>
</p>

<p align="center">
  <b>English</b> •
  <a href="docs/readme/README_CN.md">简体中文</a> •
  <a href="docs/readme/README_JA.md">日本語</a> •
  <a href="docs/readme/README_KO.md">한국어</a> •
  <a href="docs/readme/README_DE.md">Deutsch</a> •
  <a href="docs/readme/README_FR.md">Français</a> •
  <a href="docs/readme/README_ES.md">Español</a> •
  <a href="docs/readme/README_RU.md">Русский</a> •
  <a href="docs/readme/README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#interface-preview">Interface Preview</a> •
  <a href="#user-guide">User Guide</a> •
  <a href="#common-locations">Common Locations</a> •
  <a href="#citation">Citation</a> •
  <a href="#contributors">Contributors</a> •
  <a href="#acknowledgements">Acknowledgements</a> •
  <a href="#license">License</a>
</p>

<p align="center">
  Select your languages and microphone, then start real-time translation with one click.
</p>

---

## Interface Preview

The welcome page walks through the initial setup and remains available from the sidebar.

<p align="center">
  <img src="assets/preview-Welcome-1.png" alt="XRTranslate welcome page" width="760" />
</p>

Supports generating video subtitles and playing them directly at high performance.

<p align="center">
  <img src="assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate video subtitle generation" width="760" />
</p>

<p align="center">
  <img src="assets/preview-Translation.png" alt="XRTranslate translation screen" width="900" />
</p>

OSC subtitles can keep messages separate or arrange them together.

<table>
  <tr>
    <td width="50%" align="center"><img src="assets/preview-OSC-Bilingual-Separate.png" alt="OSC subtitles shown separately" /></td>
    <td width="50%" align="center"><img src="assets/preview-OSC-Bilingual-Merge.png" alt="OSC subtitles merged" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Separate</sub></td>
    <td align="center"><sub>Merge</sub></td>
  </tr>
</table>

Supports fast typing and real-time translation through OSC.

<p align="center">
  <img src="assets/preview-OSC-type.png" alt="OSC Typing and Real-Time Translation" width="760" />
</p>

Make every translation sound like you!

<p align="center">
  <img src="assets/preview-PromptStudio.png" alt="Customize your translation style!" width="760" />
</p>

---

## User Guide

Download the latest Windows version from [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases), extract it, and open XRTranslate. The first-run guide prepares the runtime and models automatically; once it finishes, you can start translating.

Linux build instructions are available in [docs/linux-build.md](docs/linux-build.md); the platform boundary is described in [docs/platform-architecture.md](docs/platform-architecture.md).

---

## Common Locations

| Item | Default Path | Description |
| :--- | :--- | :--- |
| **Model Assets** | `models/` | Stores speech recognition and translation model packages |
| **Terminology Corpora** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Independently maintained Markdown terminology and context service |
| **Execution Logs** | `runtime/logs/` | Service & client execution logs |
| **Local Config** | `config.json` | Port numbers, model configurations, and rendering parameters |

### Default Model Resource Use

The figures below are for the default settings with both models running on the GPU. They can vary slightly with your GPU, llama.cpp version, and settings.

| Model | Purpose | File Size | Estimated VRAM Use |
| :--- | :--- | :--- | :--- |
| **Speech recognition model** | Speech recognition | About 1.8 GB | About 2.7 GB |
| **Translation model** | Translation | About 1.1 GB | About 1.4 GB |

Running both models together uses about **4.1 GB** of VRAM. An 8 GB or larger GPU is recommended to leave room for Windows and other applications.

---

## Citation

```bibtex
@misc{zhao2026xtranslatorrealtimemultilingualspeakeraware,
      title={X-Translator: A Real-Time Multilingual Speaker-Aware Speech-to-Speech Translation System},
      author={Yuxiang Zhao and Yichi Zhang and Yanjie An and Yanqiao Zhu and Zhanxun Liu and Yushen Chen and Qixi Zheng and Haina Zhu and Yunchong Xiao and Keqi Deng and Shuai Fan and Kai Yu and Xie Chen},
      year={2026},
      eprint={2607.17544},
      archivePrefix={arXiv},
      primaryClass={eess.AS},
      url={https://arxiv.org/abs/2607.17544},
}
```

---

## Contributors

Special thanks to all code contributors and beta testers who helped build and test XRTranslate. See [docs/contributors.md](docs/contributors.md) for the full list of participants, their social links, and contribution details.

---

## Acknowledgements

Special thanks to the original [X-Translator](https://github.com/zhaoyx239/X-Translator) project and its authors for their contributions.

This project also uses or draws inspiration from [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts), and [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Special thanks to [Yakutan](https://github.com/febilly/Yakutan) and its author [febilly](https://github.com/febilly) for their inspiration and contributions.

## License

This repository contains code released under different open-source licenses:

- Code originating from the original X-Translator project remains under the [MIT License](LICENSE-MIT).
- The native Rust client and newly added code are released under the [GNU Affero General Public License v3.0 (AGPL-3.0)](LICENSE).

Please refer to the corresponding license files and source files for the applicable licensing terms.

---

## Usage Notice

- Voice cloning may only be used with your own voice. Cloning, imitating, or impersonating another person's voice is prohibited.
- Do not use XRTranslate for unlawful purposes. Comply with the laws and regulations applicable in your country or region, and respect privacy, personality rights, and intellectual property rights.
- Speech recognition, translation, and synthesized speech may contain errors. Verify important content before use.
