<p align="center">
  <img src="../../rust-client/resources/branding/xrtranslate-logo.png" alt="XRTranslate" width="120" />
</p>

<h1 align="center">XRTranslate</h1>

<p align="center">
  <a href="https://github.com/NowLoadY/XRTranslate/releases/latest">Descargar la última versión</a>
</p>

<p align="center">
  <strong>Subtítulos y traducción de voz en tiempo real para escritorio, multimedia y VRChat</strong>
</p>

<p align="center">
  <a href="../../README.md">English</a> •
  <a href="README_CN.md">简体中文</a> •
  <a href="README_JA.md">日本語</a> •
  <a href="README_KO.md">한국어</a> •
  <a href="README_DE.md">Deutsch</a> •
  <a href="README_FR.md">Français</a> •
  <b>Español</b> •
  <a href="README_RU.md">Русский</a> •
  <a href="README_SV.md">Svenska</a>
</p>

<p align="center">
  <a href="#vista-previa-de-la-interfaz">Vista previa de la interfaz</a> •
  <a href="#guía-de-usuario">Guía de usuario</a> •
  <a href="#ubicaciones-comunes">Ubicaciones comunes</a> •
  <a href="#citation">Citation</a> •
  <a href="#colaboradores--contributors">Colaboradores</a> •
  <a href="#agradecimientos">Agradecimientos</a> •
  <a href="#licencia">Licencia</a>
</p>

<p align="center">
  Seleccione el idioma y el micrófono, y comience a traducir en tiempo real con un solo clic.
</p>

---

## Vista previa de la interfaz

En el primer inicio, la página de bienvenida le guiará a través de la configuración necesaria. Posteriormente podrá volver a abrirla en cualquier momento desde la barra lateral.

<p align="center">
  <img src="../../assets/preview-Welcome-1.png" alt="XRTranslate página de bienvenida" width="760" />
</p>

Admite la generación de subtítulos de video y su reproducción directa de alto rendimiento.

<p align="center">
  <img src="../../assets/preview-GenerateVideoSubtitle.png" alt="XRTranslate generación de subtítulos de video" width="760" />
</p>

<p align="center">
  <img src="../../assets/preview-Translation.png" alt="XRTranslate pantalla de traducción" width="900" />
</p>

Los subtítulos OSC se pueden mostrar por separado o combinados en una sola vista.

<table>
  <tr>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Separate.png" alt="Subtítulos OSC mostrados por separado" /></td>
    <td width="50%" align="center"><img src="../../assets/preview-OSC-Bilingual-Merge.png" alt="Subtítulos OSC combinados" /></td>
  </tr>
  <tr>
    <td align="center"><sub>Visualización individual</sub></td>
    <td align="center"><sub>Visualización combinada</sub></td>
  </tr>
</table>

Admite escritura rápida y transmisión de traducción en tiempo real a través de OSC.

<p align="center">
  <img src="../../assets/preview-OSC-type.png" alt="Escritura OSC y traducción en tiempo real" width="760" />
</p>

Haz que cada traducción tenga tu propio estilo.

<p align="center">
  <img src="../../assets/preview-PromptStudio.png" alt="Personaliza tu estilo de traducción" width="760" />
</p>

Audio Studio le ayuda a tener a mano el micrófono, el audio del sistema y las rutas de traducción.

<p align="center">
  <img src="../../assets/preview-AudioStudio.png" alt="XRTranslate Audio Studio" width="900" />
</p>

---

## Guía de usuario

Descargue la versión más reciente para Windows desde [GitHub Releases](https://github.com/NowLoadY/XRTranslate/releases), descomprima el archivo y ejecute XRTranslate. El asistente de inicio preparará el entorno de ejecución y los modelos automáticamente; una vez finalizado, podrá comenzar a traducir inmediatamente.

---

## Ubicaciones comunes

| Elemento | Ruta predeterminada | Descripción |
| :--- | :--- | :--- |
| **Archivos de modelos** | `models/` | Almacena los paquetes de modelos de reconocimiento de voz y traducción |
| **Corpus terminológicos** | [XR Corpus](https://github.com/NowLoadY/XR-Corpus) | Servicio independiente de terminología y contexto en Markdown |
| **Registros de ejecución** | `runtime/logs/` | Registros del servicio en segundo plano y del cliente |
| **Configuración local** | `config.json` | Números de puerto, configuración de modelos y parámetros de renderizado |

### Uso de recursos de los modelos predeterminados

Los siguientes valores son valores de referencia con la configuración predeterminada y ambos modelos ejecutándose en GPU. Pueden variar ligeramente según la tarjeta gráfica, la versión de llama.cpp y la configuración utilizada.

| Modelo | Propósito | Tamaño del archivo | Uso estimado de VRAM |
| :--- | :--- | :--- | :--- |
| **Modelo de reconocimiento de voz** | Reconocimiento de voz | Aprox. 1,8 GB | Aprox. 2,7 GB |
| **Modelo de traducción** | Traducción | Aprox. 1,1 GB | Aprox. 1,4 GB |

La ejecución conjunta de ambos modelos utiliza aproximadamente **4,1 GB** de VRAM. Se recomienda una tarjeta gráfica con 8 GB o más de VRAM para dejar espacio suficiente para Windows y otras aplicaciones.

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

## Colaboradores / Contributors

Agradecemos sinceramente a todos los desarrolladores que han contribuido con código y a los evaluadores beta que han participado en las pruebas de XRTranslate. Puede consultar la lista completa de colaboradores, enlaces sociales y detalles de sus contribuciones en [docs/contributors.md](../contributors.md).

---

## Agradecimientos

Agradecimiento especial al proyecto original [X-Translator](https://github.com/zhaoyx239/X-Translator) y a su equipo de autores por sus destacadas contribuciones.

Asimismo, agradecemos a [XTalk](https://github.com/xcc-zach/xtalk), [X-ASR](https://github.com/Gilgamesh-J/X-ASR), [Paraformer](https://github.com/modelscope/FunASR), [SenseVoice](https://github.com/FunAudioLLM/SenseVoice), [NiuTrans LMT](https://github.com/NiuTrans/LMT), [Hunyuan-MT](https://github.com/Tencent-Hunyuan/Hunyuan-MT), [X-Voice](https://github.com/sunnyxrxrx/X-Voice), [IndexTTS](https://github.com/index-tts/index-tts) y [OpenSTBench](https://github.com/sjtuayj/OpenSTBench).

Agradecimiento especial a [Yakutan](https://github.com/febilly/Yakutan) y a su autor [febilly](https://github.com/febilly) por su inspiración y valiosas contribuciones.

## Licencia

Este repositorio contiene código publicado bajo diferentes licencias de código abierto:

- El código procedente del proyecto original X-Translator permanece bajo [MIT License](../../LICENSE-MIT).
- El cliente nativo de Rust y el nuevo código añadido se distribuyen bajo [GNU Affero General Public License v3.0 (AGPL-3.0)](../../LICENSE).

Consulte los archivos de licencia correspondientes y el código fuente para conocer los términos aplicables.
