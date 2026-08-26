# Audio8 TTS 音频失真修复

> 本文保留当时的故障复现和性能证据。当前运行策略已收紧为 NVIDIA GPU
> 至少 7 GiB 且完整 CUDA/cuDNN 闭包；托管 Audio8 模型不再回退 CPU。

## 现象

完成音色克隆并开启 TTS 后，音频能够写入选定的扬声器或虚拟麦克风，但内容是重复呻吟、啸叫或近似周期噪声。问题发生在合成阶段，而不是播放阶段：声码器输出已经失真，之后的 44.1 kHz PCM16 播放与设备重采样只会忠实播放错误波形。

INT4 路径生成的 codec token 大量固定重复。更换目标文本、参考文本和参考音色后，Slow AR 的 logits、hidden state 以及 48 组 KV cache 都不随提示内容变化，说明模型没有真正接收到文本和音色条件。

## 原因

Rust 实现曾被怀疑与官方 runtime 存在提示词、位置编码、attention mask、KV cache 或采样配置差异。逐项对照官方源码后，这些契约已完全一致：

- 提示词按官方片段分别分词，并使用相同的语义起止 token；
- Slow/Fast AR 的输入布局、位置、cache shape、cache 更新和 codec 转置一致；
- 使用官方默认的 `temperature = 0.7`、`top_p = 0.9`、`top_k = 50`、`seed = 42`；
- top-p 边界、重复感知采样和 NumPy PCG64 随机流一致；
- 注册编码器与声码器固定走模型清单指定的 CPU 路径。

探针进一步证明 Rust 的 cache 传输正常：手工改变 cache 会改变 logits，FP16 Slow AR 也会随文本改变全部 48 组 cache；只有 INT4 Slow AR 的量化 QKV 路径输出与提示内容无关。ONNX Runtime 1.24 与 1.28、不同图优化级别均产生相同的退化 token，因此根因不是 Runtime 版本过旧，而是该 INT4 自回归导出在当前执行路径上的数值/算子兼容性问题。

## 修复方法

1. Slow AR 和 Fast AR 改用与 Audio8 官方模型同权重、同 tensor/cache 契约的 FP16 ONNX 导出；注册编码器、声码器、tokenizer 和 manifest 继续使用官方固定版本。FP16 Slow/Fast 图与其余文件分别固定到不可变 revision，并为每个文件校验大小与 SHA-256。
2. 新模型作为 `audio8-tts-onnx-fp16` 接入现有模型清单、原子安装器和统一下载进度管理器，安装到 `models/Audio8-TTS-Preview-0.6B-ONNX-FP16`，不打包进 release。
3. Rust runtime 同时接受 FP16 logits 和官方图的完整序列输出，始终只采样最后一个时间步；保留每阶段的 NaN/Inf 和 shape 校验，异常结果不会进入播放设备。
4. `auto` 执行策略为 CUDA 优先、不可用时回退 CPU。界面与配置均不再提供 DirectML 或 Vulkan；DirectML 的旧配置会迁移为 `auto`。注册编码器和声码器仍遵循官方配置使用 CPU。
5. 克隆结果继续采用覆盖语义：每个音源只保留最新的参考文本和 codec codes，注册完成即释放原始录音，不会因克隆次数增加而堆积。

## 验证与耗时

在同一台 Windows 设备上，以相同参考 profile、文本“你好，这是 Audio8 TTS。”和最多 64 个新 token 做纯 Rust 端到端测试（包含会话加载、profile 导入和合成，不包含下载）：

| 执行路径 | 耗时 | 生成音频 | 结果 |
| --- | ---: | ---: | --- |
| CPU，FP16 Slow + FP16 Fast | 23.50 s | 51 帧 / 2.368 s | Qwen3-ASR 准确识别为“你好，这是Audio八TTS。” |
| DirectML GPU（已移除），FP16 Slow + FP16 Fast | 14.00 s | 64 帧 / 2.972 s | 产生重复语句，质量不合格，不进入任何生产配置 |
| CUDA 13，FP16 Slow + FP16 Fast 会话初始化 | 2.28 s | 不生成音频 | 两个 AR 会话均确认使用 CUDA EP，无 CPU 回落 |

CUDA 13 验证设备为 RTX 5070 Ti、驱动 610.47（CUDA 13.3）。测试使用同一官方 ORT 1.28 CUDA13 包内的核心与 provider，以及 llama.cpp CUDA 13.3 包中的共享 CUDA DLL。当前完整 CUDA 合成耗时仍需在可读取完整模型包的发布环境复测，因此这里只记录已真实测量的会话初始化时间，不以初始化时间冒充端到端合成时间。CPU 完整链路生成 49 帧 / 2.276 秒音频时，ASR 可稳定辨认为目标句。整个注册、推理、采样、解码和验证链路均不依赖 Python。
