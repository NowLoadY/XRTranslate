<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">下载最新版本</a>
</p>

<p align="center">
  <strong>适用于桌面、多媒体与 VRChat 的实时字幕与双向语音翻译</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <b>简体中文</b> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <a href="README_FR.md">Français</a> •
  <a href="README_ES.md">Español</a> •
  <a href="README_RU.md">Русский</a> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#界面预览">界面预览</a> •
  <a href="#使用指南">使用指南</a> •
  <a href="#常用位置">常用位置</a> •
  <a href="#citation">Citation</a> •
  <a href="#contributors--参与者">参与者</a> •
  <a href="#acknowledgements">致谢</a> •
  <a href="#license">许可证</a>
</p>

<p align="center">
  选择语言和麦克风，一键开启实时翻译。
</p>

---

## 界面预览

首次打开时，欢迎页会带你完成必要准备。之后也可以随时从侧边栏重新打开。

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate 欢迎页" width="760" />
</p>

支持生成视频字幕并直接高性能播放。

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate 生成视频字幕" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate 翻译界面" width="900" />
</p>

OSC 字幕可以逐条显示，也可以合并排列。

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="OSC 字幕逐条显示" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="OSC 字幕合并显示" /></td>
  </tr>
  <tr>
    <td align="center"><sub>逐条显示</sub></td>
    <td align="center"><sub>合并显示</sub></td>
  </tr>
</table>

支持通过 OSC 快速打字输入与实时翻译发送。

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="OSC 打字输入与实时翻译" width="760" />
</p>

让每次翻译都更像你的风格。

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="自定义你的翻译风格" width="760" />
</p>

Audio Studio 让麦克风、系统音频和翻译路由一目了然。

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## 使用指南

前往 [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases) 下载最新的 Windows 版本，解压后打开 XRTranslate。首次启动引导会自动准备运行环境和模型；完成引导后即可开始翻译。

---

## 常用位置

| 项目 | 默认路径 | 说明 |
| :--- | :--- | :--- |
| **模型文件** | `models/` | 放置语音识别模型与翻译模型等模型包 |
| **专业语料库** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | 独立维护的 Markdown 术语与上下文服务 |
| **运行日志** | `runtime/logs/` | 查看后台服务与客户端日志 |
| **本地服务设置** | `config.json` | 端口、模型及渲染参数配置 |

### 默认模型的资源占用

以下为默认设置、两个模型均使用显卡运行时的参考值；不同显卡、llama.cpp 版本和设置会有少量差异。

| 模型 | 用途 | 文件大小 | 预计显存占用 |
| :--- | :--- | :--- | :--- |
| **语音识别模型** | 语音识别 | 约 1.8 GB | 约 2.7 GB |
| **翻译模型** | 翻译 | 约 1.1 GB | 约 1.4 GB |

两个模型同时运行时，预计占用约 **4.1 GB** 显存。建议使用 8 GB 或以上显存的显卡，以留出系统和其他程序所需空间。

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

## Contributors / 参与者

衷心感谢为 XRTranslate 贡献代码与参与测试的全体成员。完整参与者名单、社交主页及贡献详情请参见 [docs/contributors.md](../contributors.md)。

---

## Acknowledgements

特别感谢原始项目 [X-Translator](https://github.com/zhaoyx239/X-Translator) 及其作者团队的卓越贡献。

同时感谢 [XTalk](https://github.com/xcc-zach/xtalk)、[X-ASR](https://github.com/Gilgamesh-J/X-ASR)、[Paraformer](https://github.com/modelscope/FunASR)、[SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT)、[Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT)、[X-Voice](https://github.com/sunnyxrxrx/X-Voice)、[IndexTTS](https://github.com/index-tts/index-tts) 与 [OpenSTBench](https://github.com/sjtuayj/OpenSTBench)。

特别感谢 [Yakutan](https://github.com/febilly/Yakutan) 及其作者 [febilly](https://github.com/febilly) 带来的启发与贡献。

## License

本项目包含采用不同开源许可证发布的代码：

- 原项目 X-Translator 相关代码沿用 [MIT License](../../LICENSE-MIT)。
- Rust 原生客户端及新增代码采用 [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE)。

具体许可范围以仓库中的许可证文件及对应源码为准。

---

## 使用声明

- 声音克隆功能仅限克隆您本人的声音。禁止克隆、模仿或冒充他人的声音。
- 禁止将 XRTranslate 用于任何非法用途。请遵守所在国家或地区适用的法律法规，并尊重隐私、人格权与知识产权。
- 语音识别、翻译及合成语音可能存在错误；重要内容请在使用前核对。
