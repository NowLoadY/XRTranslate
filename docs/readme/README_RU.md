<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Скачать последний релиз</a>
</p>

<p align="center">
  <strong>Субтитры и голосовой перевод в реальном времени для ПК, медиа и VRChat</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <a href="README_FR.md">Français</a> •
  <a href="README_ES.md">Español</a> •
  <b>Русский</b> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#обзор-интерфейса">Обзор интерфейса</a> •
  <a href="#руководство-пользователя">Руководство пользователя</a> •
  <a href="#основные-пути-и-файлы">Основные пути и файлы</a> •
  <a href="#citation">Citation</a> •
  <a href="#участники--contributors">Участники</a> •
  <a href="#благодарности">Благодарности</a> •
  <a href="#лицензия">Лицензия</a>
</p>

<p align="center">
  Выберите язык и микрофон, затем запустите перевод в реальном времени в один клик.
</p>

---

## Обзор интерфейса

При первом запуске страница приветствия поможет выполнить первоначальную настройку. В дальнейшем её можно в любой момент открыть из боковой панели.

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate страница приветствия" width="760" />
</p>

Поддержка генерации субтитров к видео и их прямого высокопроизводительного воспроизведения.

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate генерация субтитров к видео" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate окно перевода" width="900" />
</p>

OSC-субтитры могут отображаться раздельно по сообщениям или объединяться в общий список.

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="OSC-субтитры отображаются раздельно" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="OSC-субтитры объединены" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Раздельное отображение</sub></td>
    <td align="center"><sub>Объединенное отображение</sub></td>
  </tr>
</table>

Поддержка быстрого набора текста и отправки перевода в реальном времени через OSC.

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="Ввод через OSC и перевод в реальном времени" width="760" />
</p>

Добавьте каждому переводу свой стиль.

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="Настройка стиля перевода" width="760" />
</p>

Audio Studio помогает держать микрофон, системный звук и маршруты перевода под рукой.

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## Руководство пользователя

Скачайте последнюю версию для Windows со страницы [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases), распакуйте архив и запустите XRTranslate. Мастер первого запуска автоматически настроит среду выполнения и модели; после завершения настройки можно сразу начинать перевод.

---

## Основные пути и файлы

| Элемент | Путь по умолчанию | Описание |
| :--- | :--- | :--- |
| **Файлы моделей** | `models/` | Хранение пакетов моделей распознавания речи и перевода |
| **Терминологические корпуса** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Независимо поддерживаемый сервис терминологии и контекста Markdown |
| **Журналы выполнения** | `runtime/logs/` | Логи фоновых служб и клиента |
| **Локальная конфигурация** | `config.json` | Номера портов, параметры моделей и конфигурация рендеринга |

### Использование ресурсов моделями по умолчанию

Приведенные ниже значения соответствуют настройкам по умолчанию при работе обеих моделей на GPU. Они могут незначительно варьироваться в зависимости от видеокарты, версии llama.cpp и настроек.

| Модель | Назначение | Размер файла | Примерное использование VRAM |
| :--- | :--- | :--- | :--- |
| **Модель распознавания речи** | Распознавание речи | Около 1,8 ГБ | Около 2,7 ГБ |
| **Модель перевода** | Перевод | Около 1,1 ГБ | Около 1,4 ГБ |

При одновременной работе обе модели используют около **4,1 ГБ** VRAM. Рекомендуется видеокарта с объемом памяти от 8 ГБ, чтобы оставить запас для Windows и других программ.

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

## Участники / Contributors

Искренняя благодарность всем авторам кода и бета-тестерам, принявшим участие в разработке и тестировании XRTranslate. Полный список участников, их контакты и вклад см. в [docs/contributors.md](../contributors.md).

---

## Благодарности

Особая благодарность оригинальному проекту [X-Translator](https://github.com/zhaoyx239/X-Translator) и команде его авторов за выдающийся вклад.

Также благодарим [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts) и [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Особая благодарность [Yakutan](https://github.com/febilly/Yakutan) и его автору [febilly](https://github.com/febilly) за вдохновение и ценный вклад.

## Лицензия

Этот репозиторий содержит код, распространяемый под различными лицензиями с открытым исходным кодом:

- Код, созданный в рамках оригинального проекта X-Translator, распространяется под [MIT License](../../LICENSE-MIT).
- Нативный клиент на Rust и вновь добавленный код распространяются под [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE).

Точные условия лицензирования см. в соответствующих файлах лицензий и исходном коде.
