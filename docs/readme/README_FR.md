<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Télécharger la dernière version</a>
</p>

<p align="center">
  <strong>Sous-titres et traduction vocale en temps réel pour bureau, médias et VRChat</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <b>Français</b> •
  <a href="README_ES.md">Español</a> •
  <a href="README_RU.md">Русский</a> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#aperçu-de-linterface">Aperçu de l'interface</a> •
  <a href="#guide-dutilisation">Guide d'utilisation</a> •
  <a href="#emplacements-courants">Emplacements courants</a> •
  <a href="#citation">Citation</a> •
  <a href="#contributeurs--contributors">Contributeurs</a> •
  <a href="#remerciements">Remerciements</a> •
  <a href="#licence">Licence</a>
</p>

<p align="center">
  Sélectionnez vos langues et votre microphone, puis démarrez la traduction en temps réel d'un simple clic.
</p>

---

## Aperçu de l'interface

Lors du premier lancement, la page d'accueil vous guide à travers la configuration nécessaire. Vous pouvez également la rouvrir à tout moment depuis la barre latérale.

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate page d'accueil" width="760" />
</p>

Prise en charge de la génération de sous-titres vidéo et de leur lecture directe haute performance.

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate génération de sous-titres vidéo" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate écran de traduction" width="900" />
</p>

Les sous-titres OSC peuvent être affichés de manière séparée ou regroupée.

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="Sous-titres OSC affichés séparément" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="Sous-titres OSC regroupés" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Affichage séparé</sub></td>
    <td align="center"><sub>Affichage regroupé</sub></td>
  </tr>
</table>

Prise en charge de la saisie rapide au clavier et de la transmission de traductions en temps réel via OSC.

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="Saisie OSC et traduction en temps réel" width="760" />
</p>

Donnez à chaque traduction votre propre style.

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="Personnalisez votre style de traduction" width="760" />
</p>

Audio Studio vous aide à garder un œil sur le micro, l’audio système et les routes de traduction.

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## Guide d'utilisation

Téléchargez la dernière version Windows depuis [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases), décompressez l'archive et lancez XRTranslate. L'assistant de premier démarrage prépare automatiquement l'environnement d'exécution et les modèles ; une fois cette étape terminée, vous pouvez commencer la traduction immédiatement.

---

## Emplacements courants

| Élément | Chemin par défaut | Description |
| :--- | :--- | :--- |
| **Fichiers de modèles** | `models/` | Stocke les paquets de modèles de reconnaissance vocale et de traduction |
| **Corpus terminologiques** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Service de terminologie et de contexte Markdown géré indépendamment |
| **Journaux d'exécution** | `runtime/logs/` | Journaux d'exécution des services d'arrière-plan et du client |
| **Configuration locale** | `config.json` | Numéros de port, configurations des modèles et paramètres de rendu |

### Utilisation des ressources par les modèles par défaut

Les valeurs ci-dessous correspondent aux paramètres par défaut avec les deux modèles s'exécutant sur le GPU. Elles peuvent varier légèrement selon votre carte graphique, la version de llama.cpp et vos paramètres.

| Modèle | Utilisation | Taille du fichier | VRAM estimée |
| :--- | :--- | :--- | :--- |
| **Modèle de reconnaissance vocale** | Reconnaissance vocale | Env. 1,8 Go | Env. 2,7 Go |
| **Modèle de traduction** | Traduction | Env. 1,1 Go | Env. 1,4 Go |

L'exécution simultanée des deux modèles utilise environ **4,1 Go** de VRAM. Une carte graphique de 8 Go ou plus est recommandée afin de conserver de la mémoire pour Windows et les autres applications.

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

## Contributeurs / Contributors

Nous remercions chaleureusement tous les contributeurs de code et testeurs bêta qui ont participé au développement et aux tests de XRTranslate. Retrouvez la liste complète des participants, leurs liens sociaux et le détail de leurs contributions dans [docs/contributors.md](../contributors.md).

---

## Remerciements

Nous remercions tout particulièrement le projet original [X-Translator](https://github.com/zhaoyx239/X-Translator) ainsi que son équipe d'auteurs pour leurs contributions remarquables.

Nous remercions également [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts) et [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Un grand merci à [Yakutan](https://github.com/febilly/Yakutan) et à son auteur [febilly](https://github.com/febilly) pour l'inspiration et leurs précieuses contributions.

## Licence

Ce dépôt contient du code publié sous différentes licences open source :

- Le code issu du projet original X-Translator demeure sous [MIT License](../../LICENSE-MIT).
- Le client natif Rust ainsi que le nouveau code ajouté sont publiés sous [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE).

Veuillez vous référer aux fichiers de licence et au code source correspondant pour connaître les conditions précises applicables.
