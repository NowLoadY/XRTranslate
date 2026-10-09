# 实时声纹识别与时间线流水线

## 方案

默认实现采用 [3D-Speaker ERes2NetV2](https://github.com/modelscope/3D-Speaker) 的 192 维说话人嵌入，并通过 ONNX Runtime 在 Rust 后台原生推理。3D-Speaker 公布的 VoxCeleb1-O EER 为 0.61%，官方 ONNX CPU 参考 RTF 为 0.142；它在识别质量、实时延迟、模型体积和原生部署之间适合本项目的 0.5–8 秒 VAD 片段。

流水线严格按以下顺序运行：

```text
PCM16/16 kHz
  -> Silero VAD（含预卷与硬切片重叠）
  -> 仅对 VAD 活跃语音：
  -> 80 维 Kaldi FBank + CMN
  -> ERes2NetV2 ONNX（L2 归一化声纹）
  -> 稳定候选确认 + 有界在线余弦聚类（speaker-01、speaker-02…）
  -> 说话人边界与绝对音频时间戳
  -> Qwen3-ASR
  -> 源文本分段与单调时间分配
  -> Hy-MT2 两路有序并发翻译
  -> WebSocket / UI / OSC
```

每个音频 generation 拥有独立的说话人聚类状态。切换音频源或重置路由后，旧作业由 generation gate 丢弃，声纹 ID 和时间线从新流重新开始。硬切片复制的音频只用于保护跨边界音素，第二个片段对外的时间起点会扣除 overlap，因此时间线不会重复倒退。

## ONNX 模型与 Release

模型来自 ModelScope `iic/speech_eres2netv2_sv_zh-cn_16k-common@v1.0.1`，使用 PyTorch 2.11 与 3D-Speaker 官方导出器生成。项目使用的单文件模型为：

```text
models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx
```

其输入为动态长度 `[batch, frames, 80]`，输出为 `[batch, 192]`。当前导出文件大小为 71,964,309 字节。

`build_release.ps1` 将 Silero VAD 与该声纹 ONNX 一并作为每个 Release 的必带原生资源；它不受 `-IncludeModels` 控制。打包器会把发布包中的 `speaker.enabled` 改为 `true`，并固定相对模型路径。Release 运行时不需要 Python、Conda、PyTorch 或 3D-Speaker 源码。

源码开发配置默认保持关闭，缺少本地模型时仍可开发其他功能。需要直接从源码启用时，在 `config.json` 设置：

```json
{
  "speaker": {
    "enabled": true,
    "model_path": "models/3D-Speaker-ERes2NetV2/speaker_embedding.onnx",
    "similarity_threshold": 0.50,
    "same_speaker_hysteresis": 0.12,
    "speaker_switch_margin": 0.04,
    "max_speakers": 8,
    "min_utterance_ms": 750,
    "intra_threads": 2
  }
}
```

如需重新生成模型，请使用 3D-Speaker 官方 `speakerlab/bin/export_speaker_embedding_onnx.py`。PyTorch 2.11 的新 ONNX 导出器可能生成 `.onnx` 与 `.onnx.data` 两个文件；发布前应使用 ONNX API 保存为不含 external data 的单文件，防止打包遗漏权重。

## 性能与调参

- `similarity_threshold` 越高，错合并越少，但同一人更容易被拆成多个 ID。远场、变声器或系统混音可在 `0.58–0.68` 小范围校准。
- `same_speaker_hysteresis` 只降低连续片段归属上一个人的门槛；数值越大越黏，越小则换人更灵敏。默认运行配置使用 `0.12`，仍保留多窗口确认以抑制噪声跳号。
- `speaker_switch_margin` 要求另一个已有声纹取得足够的相似度优势，避免两个相近声纹在临界点来回切换。
- `max_speakers` 是严格内存边界；达到上限后，低于门槛的片段只匹配最近的已有声纹，不污染聚类中心。
- `min_utterance_ms` 以下的片段标记为 `speaker-unknown`，避免用爆破音或极短噪声创建身份。
- ONNX 推理位于 Tokio blocking region，不会阻塞 WebSocket/VAD intake；Hy-MT2 使用每会话最多两路、保持输出顺序的并发请求，与托管服务的四个推理 slot 匹配。

活动语音中确认换人会产生 `SpeakerChange` 边界，立即结束上一句并为新说话人开启下一句。自然停顿已经由 VAD 切句时，新句会重新判断身份，不会仅因停顿较短就预先继承上一人的编号；尚未确认的新声纹证据可以跨短停顿保留，以兼顾短句响应和噪声抑制。

## 时间线语义

协议版本为 2。`source_segment_ready` 与 `translation_ready` 都带有：

- `speaker_id`
- `source_start_ms`
- `source_end_ms`（exclusive）

时间相对于当前 audio generation。ASR 目前不返回词级时间戳，因此一个 VAD 片段被拆成多句时，会按各句 Unicode 字符数比例分配连续区间；这保证顺序、边界和说话人元数据一致，但不是强制对齐结果。

Rust 客户端会在 Recognition History 与 Translation History 中显示紧凑的 `S1`、`S2` 标签和对应时间范围。声纹身份是识别基础设施提供的标准元数据，在服务端配置允许且模型可用时随识别工作流运行。OSC 工作台的“说话人编号”只控制 VRChat Chatbox 是否添加 `[S1]` 形式的前缀；关闭仅节省 OSC 字符，不会关闭声纹计算，也不会移除其他消费者收到的身份元数据。

## 能力边界

当前路径为低延迟“一个 VAD 片段一个主说话人”的声纹区分，适合 VRChat 麦克风或系统音频中轮流发言的场景。它不会把同一片段内同时说话的多人拆成多个活动轨。若需要重叠说话人检测，可在该接口前接入 [NVIDIA Streaming Sortformer](https://docs.nvidia.com/nemo/speech/nightly/asr/speaker_diarization/models.html)；该模型支持端到端在线 diarization，但计算量和块延迟更高，不能直接替代当前的低延迟默认路径。
