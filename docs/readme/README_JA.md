<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">最新リリースをダウンロード</a>
</p>

<p align="center">
  <strong>デスクトップ・メディア・VRChat 向けリアルタイム字幕＆音声翻訳</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <b>日本語</b> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <a href="README_FR.md">Français</a> •
  <a href="README_ES.md">Español</a> •
  <a href="README_RU.md">Русский</a> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#インターフェースプレビュー">インターフェースプレビュー</a> •
  <a href="#利用ガイド">利用ガイド</a> •
  <a href="#主要なファイルとパス">主要なファイルとパス</a> •
  <a href="#citation">Citation</a> •
  <a href="#貢献者--contributors">貢献者</a> •
  <a href="#謝辞">謝辞</a> •
  <a href="#ライセンス">ライセンス</a>
</p>

<p align="center">
  言語とマイクを選択するだけで、ワンクリックでリアルタイム翻訳を開始できます。
</p>

---

## インターフェースプレビュー

初回起動時には、ウェルカムページが必要なセットアップを案内します。設定完了後もサイドバーからいつでも再度開くことができます。

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate ウェルカムページ" width="760" />
</p>

動画字幕の生成および高いパフォーマンスでの直接再生に対応しています。

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate 動画字幕生成" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate 翻訳画面" width="900" />
</p>

OSC 字幕は、メッセージを個別に表示することも、結合して並べることも可能です。

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="OSC 字幕個別表示" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="OSC 字幕結合表示" /></td>
  </tr>
  <tr>
    <td align="center"><sub>個別表示</sub></td>
    <td align="center"><sub>結合表示</sub></td>
  </tr>
</table>

OSC を介した高速タイピング入力とリアルタイム翻訳送信に対応しています。

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="OSC タイピング入力とリアルタイム翻訳" width="760" />
</p>

翻訳に自分らしいスタイルを加えられます。

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="翻訳スタイルをカスタマイズ" width="760" />
</p>

Audio Studio なら、マイク・システム音声・翻訳ルートをすっきり確認できます。

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## 利用ガイド

[GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases) から最新の Windows 版をダウンロードし、解凍して XRTranslate を起動してください。初回起動ウィザードがランタイム環境とモデルを自動的に準備します。セットアップ完了後、すぐに翻訳を開始できます。

---

## 主要なファイルとパス

| 項目 | デフォルトパス | 説明 |
| :--- | :--- | :--- |
| **モデルファイル** | `models/` | 音声認識モデルや翻訳モデルなどのモデルパッケージを格納 |
| **専門用語コーパス** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | 独立して管理される Markdown 用語辞書・コンテキストサービス |
| **実行ログ** | `runtime/logs/` | バックエンドサービスおよびクライアントの実行ログ |
| **ローカル設定** | `config.json` | ポート番号、モデル設定、レンダリングパラメータ |

### デフォルトモデルのリソース使用量

以下はデフォルト設定で、両方のモデルを GPU で実行した場合の参考値です。GPU の種類、llama.cpp のバージョン、各種設定により若干変動する場合があります。

| モデル | 用途 | ファイルサイズ | 推定 VRAM 使用量 |
| :--- | :--- | :--- | :--- |
| **音声認識モデル** | 音声認識 | 約 1.8 GB | 約 2.7 GB |
| **翻訳モデル** | 翻訳 | 約 1.1 GB | 約 1.4 GB |

両モデルを同時に実行する場合、約 **4.1 GB** の VRAM を使用します。OS や他のアプリケーション用の領域を確保するため、8 GB 以上の VRAM を搭載した GPU を推奨します。

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

## 貢献者 / Contributors

XRTranslate の開発へのコード貢献およびベータテストにご参加いただいた皆様に深く感謝いたします。完全な貢献者一覧、ソーシャルリンク、および貢献内容については [docs/contributors.md](../contributors.md) をご覧ください。

---

## 謝辞

オリジナルの [X-Translator](https://github.com/zhaoyx239/X-Translator) プロジェクトおよびその開発者チームの卓越した貢献に心より感謝申し上げます。

また、[XTalk](https://github.com/xcc-zach/xtalk)、[X-ASR](https://github.com/Gilgamesh-J/X-ASR)、[Paraformer](https://github.com/modelscope/FunASR)、[SenseVoice](https://github.com/FunAudioLLM/SenseVoice)、[NiuTrans LMT](https://github.com/NiuTrans/LMT)、[Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT)、[X-Voice](https://github.com/sunnyxrxrx/X-Voice)、[IndexTTS](https://github.com/index-tts/index-tts)、[OpenSTBench](https://github.com/sjtuayj/OpenSTBench) に感謝いたします。

インスピレーションと多大な貢献をいただいた [Yakutan](https://github.com/febilly/Yakutan) および作者の [febilly](https://github.com/febilly) 氏に深く感謝いたします。

## ライセンス

本リポジトリには、異なるオープンソースライセンスの下で公開されているコードが含まれています：

- オリジナルプロジェクト X-Translator に由来するコードは [MIT License](../../LICENSE-MIT) を継承します。
- Rust ネイティブクライアントおよび新規追加コードは [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE) の下で提供されます。

詳細なライセンス範囲については、リポジトリ内のライセンスファイルおよび対応するソースコードをご参照ください。
