<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Neueste Version herunterladen</a>
</p>

<p align="center">
  <strong>Echtzeit-Untertitel & Sprachübersetzung für Desktop, Medien und VRChat</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <b>Deutsch</b> •
  <a href="README_FR.md">Français</a> •
  <a href="README_ES.md">Español</a> •
  <a href="README_RU.md">Русский</a> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#interface-vorschau">Interface-Vorschau</a> •
  <a href="#benutzerhandbuch">Benutzerhandbuch</a> •
  <a href="#wichtige-pfade">Wichtige Pfade</a> •
  <a href="#citation">Citation</a> •
  <a href="#mitwirkende--contributors">Mitwirkende</a> •
  <a href="#danksagung">Danksagung</a> •
  <a href="#lizenz">Lizenz</a>
</p>

<p align="center">
  Wählen Sie Sprachen und Mikrofon aus und starten Sie die Echtzeit-Übersetzung mit einem Klick.
</p>

---

## Interface-Vorschau

Beim ersten Start führt die Willkommensseite durch die notwendige Einrichtung. Sie kann jederzeit über die Seitenleiste wieder geöffnet werden.

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate Willkommensseite" width="760" />
</p>

Unterstützt das Generieren von Video-Untertiteln und deren direkte Wiedergabe mit hoher Performance.

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate Video-Untertitelgenerierung" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate Übersetzungsansicht" width="900" />
</p>

OSC-Untertitel können Nachrichten separat anzeigen oder zusammengefasst anordnen.

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="OSC-Untertitel separat dargestellt" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="OSC-Untertitel zusammengefasst dargestellt" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Separate Anzeige</sub></td>
    <td align="center"><sub>Zusammengefasste Anzeige</sub></td>
  </tr>
</table>

Unterstützt schnelle Texteingabe und Echtzeit-Übersetzung über OSC.

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="OSC-Texteingabe und Echtzeit-Übersetzung" width="760" />
</p>

Lassen Sie jede Übersetzung zu Ihrem Stil passen.

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="Übersetzungsstil anpassen" width="760" />
</p>

Mit Audio Studio behalten Sie Mikrofon, Systemaudio und Übersetzungsrouten bequem im Blick.

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## Benutzerhandbuch

Laden Sie die neueste Windows-Version von [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases) herunter, entpacken Sie das Archiv und starten Sie XRTranslate. Der Ersteinrichtungs-Assistent bereitet die Laufzeitumgebung und Modelle automatisch vor. Nach Abschluss können Sie sofort mit der Übersetzung beginnen.

---

## Wichtige Pfade

| Element | Standardpfad | Beschreibung |
| :--- | :--- | :--- |
| **Modelldateien** | `models/` | Speicherort für Spracherkennungs- und Übersetzungsmodell-Pakete |
| **Terminologie-Korpora** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Unabhängig gepflegter Markdown-Terminologie- und Kontextdienst |
| **Ausführungsprotokolle** | `runtime/logs/` | Protokolle für Backend-Dienste und Client |
| **Lokale Konfiguration** | `config.json` | Portnummern, Modellkonfigurationen und Rendering-Parameter |

### Ressourcennutzung der Standardmodelle

Die folgenden Werte gelten für die Standardeinstellungen, wenn beide Modelle auf der GPU ausgeführt werden. Je nach Grafikkarte, llama.cpp-Version und Einstellungen können sie leicht abweichen.

| Modell | Zweck | Dateigröße | Geschätzte VRAM-Nutzung |
| :--- | :--- | :--- | :--- |
| **Spracherkennungsmodell** | Spracherkennung | Ca. 1,8 GB | Ca. 2,7 GB |
| **Übersetzungsmodell** | Übersetzung | Ca. 1,1 GB | Ca. 1,4 GB |

Wenn beide Modelle gleichzeitig ausgeführt werden, belegen sie etwa **4,1 GB** VRAM. Eine Grafikkarte mit mindestens 8 GB VRAM wird empfohlen, um genügend Speicher für Windows und andere Anwendungen zu belassen.

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

## Mitwirkende / Contributors

Ein herzlicher Dank geht an alle Code-Mitwirkenden und Beta-Tester, die an der Entwicklung und dem Testen von XRTranslate mitgewirkt haben. Die vollständige Liste der Mitwirkenden, Social-Links und Beitragsdetails finden Sie in [docs/contributors.md](../contributors.md).

---

## Danksagung

Ein besonderer Dank gilt dem ursprünglichen Projekt [X-Translator](https://github.com/zhaoyx239/X-Translator) und seinem Entwicklerteam für deren herausragende Beiträge.

Ebenfalls danken wir [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts) und [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Herzlichen Dank an [Yakutan](https://github.com/febilly/Yakutan) und dessen Autor [febilly](https://github.com/febilly) für die Inspiration und wertvollen Beiträge.

## Lizenz

Dieses Repository enthält Code unter verschiedenen Open-Source-Lizenzen:

- Vom ursprünglichen X-Translator-Projekt stammender Code verbleibt unter der [MIT License](../../LICENSE-MIT).
- Der native Rust-Client und neu hinzugefügter Code unterliegen der [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE).

Genaue Lizenzbedingungen entnehmen Sie bitte den Lizenzdateien im Repository und dem entsprechenden Quellcode.
