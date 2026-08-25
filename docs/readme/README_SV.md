<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Ladda ner den senaste versionen</a>
</p>

<p align="center">
  <strong>Realtidsundertexter och röstöversättning för skrivbord, media och VRChat</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <a href="README_FR.md">Français</a> •
  <a href="README_ES.md">Español</a> •
  <a href="README_RU.md">Русский</a> •
  <b>Svenska</b>
</p>

<p align="center">
  <a href="#gränssnittsöversikt">Gränssnittsöversikt</a> •
  <a href="#användarguide">Användarguide</a> •
  <a href="#vanliga-platser">Vanliga platser</a> •
  <a href="#citation">Citation</a> •
  <a href="#bidragsgivare--contributors">Bidragsgivare</a> •
  <a href="#erkännanden">Erkännanden</a> •
  <a href="#licens">Licens</a>
</p>

<p align="center">
  Välj språk och mikrofon, och starta realtidsöversättning med ett enda klick.
</p>

---

## Gränssnittsöversikt

Vid första start guidar välkomstsidan dig genom nödvändiga förberedelser. Den kan även när som helst öppnas igen från sidofältet.

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate välkomstsida" width="760" />
</p>

Stödjer generering av videoundertexter och direkt uppspelning med hög prestanda.

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate generering av videoundertexter" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate översättningsgränssnitt" width="900" />
</p>

OSC-undertexter kan visa meddelanden separat eller sammanfogade.

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="OSC-undertexter visas separat" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="OSC-undertexter sammanfogade" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Separat visning</sub></td>
    <td align="center"><sub>Sammanfogad visning</sub></td>
  </tr>
</table>

Stödjer snabb textinmatning och realtidsöversättning via OSC.

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="OSC-textinmatning och realtidsöversättning" width="760" />
</p>

Ge varje översättning din egen stil.

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="Anpassa din översättningsstil" width="760" />
</p>

Med Audio Studio är det enkelt att hålla koll på mikrofon, systemljud och översättningsvägar.

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## Användarguide

Ladda ner den senaste Windows-versionen från [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases), packa upp arkivet och starta XRTranslate. Förstagångsguiden konfigurerar körmiljön och modellerna automatiskt; när den är klar kan du börja översätta omedelbart.

---

## Vanliga platser

| Objekt | Standardkatalog | Beskrivning |
| :--- | :--- | :--- |
| **Modellfiler** | `models/` | Lagrar paket för taligenkännings- och översättningsmodeller |
| **Terminologikorpus** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Självständigt underhållen Markdown-terminologi och kontexttjänst |
| **Körningsloggar** | `runtime/logs/` | Loggar för backend-tjänster och klient |
| **Lokal konfiguration** | `config.json` | Portnummer, modellinställningar och renderingsparametrar |

### Standardmodellernas resursanvändning

Siffrorna nedan gäller standardinställningar med båda modellerna körandes på GPU. De kan variera något beroende på grafikkort, llama.cpp-version och inställningar.

| Modell | Syfte | Filstorlek | Uppskattad VRAM-användning |
| :--- | :--- | :--- | :--- |
| **Taligenkänningsmodell** | Taligenkänning | Ca 1,8 GB | Ca 2,7 GB |
| **Översättningsmodell** | Översättning | Ca 1,1 GB | Ca 1,4 GB |

När båda modellerna körs samtidigt används cirka **4,1 GB** VRAM. Ett grafikkort med 8 GB eller mer VRAM rekommenderas för att lämna tillräckligt med utrymme för Windows och övriga applikationer.

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

## Bidragsgivare / Contributors

Ett varmt tack till alla som bidragit med kod och deltagit i betatesterna av XRTranslate. Se [docs/contributors.md](../contributors.md) för en fullständig lista över deltagare, sociala länkar och bidragsdetaljer.

---

## Erkännanden

Ett särskilt tack till det ursprungliga projektet [X-Translator](https://github.com/zhaoyx239/X-Translator) och dess författarteam för deras enastående bidrag.

Vi tackar även [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts) och [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Ett särskilt tack till [Yakutan](https://github.com/febilly/Yakutan) och dess författare [febilly](https://github.com/febilly) för inspiration och värdefulla bidrag.

## Licens

Detta arkiv innehåller kod som släppts under olika öppen källkods-licenser:

- Kod från det ursprungliga X-Translator-projektet förblir under [MIT License](../../LICENSE-MIT).
- Den nativa Rust-klienten och nyligen tillagd kod publiceras under [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE).

Se respektive licensfiler och källkodsfiler för tillämpliga villkor.
