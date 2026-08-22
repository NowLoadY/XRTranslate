# 模型与运行时资源矩阵

本文是本地模型、用户设备能力和原生运行时之间的交付边界。模型清单以
`xrtranslate-assets` 为唯一事实源，运行时归档以 `config.json` 为唯一事实源；
界面、下载器和后端只消费这些声明，不再各自维护文件名或设备判断。

## 三类资源

| 类型 | 所有者 | 存储位置 | 交付方式 | 更新与校验 |
| --- | --- | --- | --- | --- |
| 模型包 | `xrtranslate-assets` | `models/<package>` | 欢迎页或设置页按 provider 下载，不进入 release | 每文件固定 revision、大小、SHA-256；支持 `models_directory` 自定义 |
| 推理引擎核心 | `xrtranslate-config` / release packager | `<runtime_root>/<engine>` | CPU ONNX 核心随原生程序；llama.cpp 与 CUDA ONNX provider 按需下载 | 固定版本、归档大小、SHA-256、必需文件集合；支持 `runtime_directory` 自定义 |
| 设备加速包 | `xrtranslate-config` | `<runtime_root>/cuda/<version>` 与 `<runtime_root>/cudnn/<major>` | 仅兼容 NVIDIA 设备按需下载 | CUDA ABI 与驱动能力匹配；llama.cpp 与 ONNX 复用 CUDA，ONNX 另消费匹配 major 的 cuDNN |

所有网络传输统一经过 `xrtranslate-download`，因此模型和运行时共用断点续传、
代理、重试、进度、大小与 SHA-256 校验。模型与 runtime installer 只传递中立的
`DownloadSource`；GitHub/Hugging Face 官方地址到镜像地址的转换由下载 crate 的
单一镜像路由负责。后端不下载资源；默认 release 不包含
TTS、ASR 或翻译大模型，也没有 Python 环境。兼容的离线打包选项只能显式加入
已校验的 ASR/翻译 GGUF；Audio8 与 OpenVoice TTS 都始终由用户按需下载。

## Provider 需求

| 功能 / provider | 模型资源 | 推理资源 | GPU 策略 | 缺失时行为 |
| --- | --- | --- | --- | --- |
| ASR `qwen3-gguf` | Qwen3-ASR Q4 GGUF + mmproj，合计 1,924,209,664 B | llama.cpp server | NVIDIA >= 8 GiB；匹配 CUDA 13.3、13.1 或 12.4，否则拒绝 | 欢迎页下载/修复；未就绪不启动本地 ASR |
| 翻译 `hunyuan` 普通 | Hy-MT2 1.8B Q4 GGUF，1,133,080,448 B | 与 ASR 共用 llama.cpp | 与 ASR 共用同一 server/runtime 选择 | 欢迎页下载/修复 |
| 翻译 `hunyuan` 大 | Hy-MT2 7B Q4 GGUF，4,624,648,896 B | 与 ASR 共用 llama.cpp | 同上 | 欢迎页下载/修复 |
| TTS `audio8` | Audio8 FP16 ONNX 完整包，2,171,728,005 B | ONNX Runtime 1.28 | NVIDIA >= 8 GiB；Auto/CUDA，失败即拒绝，不回退 CPU | TTS 是可选功能，可跳过；启用时下载/修复 |
| TTS `openvoice` | 互斥 English ONNX 变体：v3 EN-Newest（安装 255,830,497 B / 下载 207,772,473 B）或 v2 五口音（安装 255,966,606 B / 下载 207,838,325 B） | ONNX Runtime 1.28；BERT/Melo/converter/reference encoder 四 session | NVIDIA >= 8 GiB；Auto/CUDA，任一 session 失败则整组失败 | TTS 可跳过；当前验证语种为 English、22,050 Hz；v2 口音不产生重复下载；未安装/激活语种不生成任务 |
| 远程 OpenAI ASR/翻译 | 无本地模型 | HTTPS + API key | 不需要本地 GPU runtime | 缺 API key 时给出配置诊断，不触发模型下载 |
| Silero VAD | release 内固定 ONNX，2,327,524 B | CPU ONNX core | CPU，实时小模型不加载 CUDA provider | release 预检失败则拒绝打包 |
| ERes2NetV2 说话人识别 | release 内固定 ONNX，71,964,309 B | CPU ONNX core | CPU | 同上 |
| GTCRN 降噪 | release 内固定 ONNX，535,638 B | CPU ONNX core | CPU | 同上 |

## 设备到下载计划

| 用户设备 | ONNX 计划 | llama.cpp 计划 | 额外下载 |
| --- | --- | --- | --- |
| 无 NVIDIA GPU 或显存 < 8 GiB | release 内小型 ONNX 组件仍可使用 compact CPU core | 大型本地 ASR/翻译/TTS 选项禁用且运行时拒绝 | 不下载模型或 CUDA/provider/cuDNN，不存在大型模型 CPU fallback |
| NVIDIA + 驱动支持 CUDA 12 | ORT 1.28 CUDA12 同源核心/provider + cuDNN 9 CUDA12 | CUDA 12.4 server | CUDA 12.4 共享包只下载一次；另下载匹配的 cuDNN12 |
| NVIDIA + 驱动/GPU 支持 CUDA 13 | ORT 1.28 CUDA13 同源核心/provider + cuDNN 9 CUDA13 | 驱动支持时优先 CUDA 13.3，否则选择 CUDA 13.1 | 匹配版本的共享 CUDA 包只下载一次；另下载匹配的 cuDNN13 |
| NVIDIA Blackwell (50 系, CC 12.0+) | ORT 1.28 CUDA13 同源核心/provider + cuDNN 9 CUDA13 | 驱动 13.1/13.2 选择 b8913 CUDA 13.1；驱动 >= 13.3 优先 CUDA 13.3 | 最低 CUDA 12.8，绝不选 12.4；使用 13.1 时仍提示通过 NVIDIA App 升级驱动 |
| NVIDIA 存在但无完整兼容归档 | 小型内置 ONNX 仍使用 CPU core | 大型本地模型不可用 | 计划失败并显示明确的驱动/归档修复原因 |

CPU ONNX 核心为 16,277,856 B，取自官方
`onnxruntime-win-x64-gpu_cuda13-1.28.0.zip` 的 `onnxruntime.dll`；该核心本身可独立
执行 CPU session，随 release 提供并由 packager 校验 SHA-256。
CUDA12 provider 归档为 455,344,532 B，CUDA13 为 365,825,268 B；cuDNN
9.20.0.48 CUDA12 归档为 634,960,681 B，CUDA13 为 349,802,474 B。它们只在
TTS 请求 CUDA 时下载。共享 CUDA 12.4/13.1/13.3 归档按所选版本使用资源并集去重，
不会因同时启用 llama.cpp 与 TTS 下载两次；cuDNN 是 ONNX 独立依赖，不放入共享
CUDA 目录。

## ONNX 与 Runtime 目录隔离与兼容性迁移

Windows 下 CUDA ONNX 运行时按版本独立子目录存放，避免多版本或 CPU/GPU 混用导致 ABI 冲突：

```text
runtime/onnxruntime/
├─ cpu/
│  └─ onnxruntime.dll
├─ cuda-12/
│  ├─ onnxruntime.dll
│  ├─ onnxruntime_providers_shared.dll
│  └─ onnxruntime_providers_cuda.dll
└─ cuda-13/
   ├─ onnxruntime.dll
   ├─ onnxruntime_providers_shared.dll
   └─ onnxruntime_providers_cuda.dll

runtime/cuda/
├─ 12.4/
│  ├─ cudart64_12.dll
│  ├─ cublasLt64_12.dll
│  └─ cublas64_12.dll
├─ 13.1/
│  ├─ cudart64_13.dll
│  ├─ cublasLt64_13.dll
│  └─ cublas64_13.dll
└─ 13.3/
   ├─ cudart64_13.dll
   ├─ cublasLt64_13.dll
   └─ cublas64_13.dll

runtime/cudnn/
├─ 12/
│  ├─ cudnn64_9.dll
│  ├─ cudnn_graph64_9.dll
│  └─ ...其余声明的 cuDNN 9 DLL
└─ 13/
   ├─ cudnn64_9.dll
   ├─ cudnn_graph64_9.dll
   └─ ...其余声明的 cuDNN 9 DLL
```

### 存量运行时自动迁移
客户端启动时会自动执行 `migrate_legacy_runtime_layout`：
- 检测旧版平铺在 `runtime/llama.cpp/` 下的 DLL 并依据文件名后缀（`_13` 或 `_12`）迁移至 `runtime/cuda/13.3/` 或 `runtime/cuda/12.4/`；
- 检测旧版平铺在 `runtime/onnxruntime/` 根目录下的 DLL 并安全归档迁移至 `runtime/onnxruntime/cuda-13/`，无需用户重新下载。

## 启动资源校验与 4 步向导系统

1. **启动自检机制**：程序每次启动时调用 `onboarding::resolve_startup_onboarding_state`。
   - 若检测到缺失任一核心前置资源（API Key、本地 ASR/翻译模型包、已启用的 TTS provider 模型、llama.cpp 运行时或 ONNX 加速库），自动将 `first_run` 置为 `true` 并停留在向导页（Step 1: Welcome）；
   - 若所有前置条件均已就绪，则直接进入主会话翻译界面（`first_run = false`）。
2. **向导 4 步骤流程**：
   - **Step 1: Welcome**：欢迎页与核心特性介绍（三列卡片：Audio Input, Recognition & Translation, VRChat OSC）；
   - **Step 2: Install models**：本地/在线 ASR 与翻译模型配置、级别选择与完整性校验下载；
   - **Step 3: Optional TTS**：选择已配置的语音克隆 provider 及模型，或跳过；
   - **Step 4: Inference Runtime**：llama.cpp 与 ONNX 统一运行时环境配置（Option A 自动下载检测，Option B 自定义已有目录）。

## 生命周期与诊断

1. 配置层把当前 ASR、翻译、TTS provider 转成 `RuntimeRequirements`。
2. 客户端探测主 NVIDIA 设备、显存、驱动与 compute capability；显存至少
   8 GiB 后才计算资源并集和缺失字节。
3. 用户触发统一下载任务；下载完成后校验、只提取声明文件并原子替换目录。
4. `runtime/native-runtime.json` 发布 llama 与 ONNX 的独立 backend、CUDA ABI、
   ONNX core、provider 目录、CUDA/cuDNN 目录和预加载顺序。
5. 后端在任何 ONNX API 之前消费该清单。每个 TTS provider 的 session group 必须
   同时成功使用 CUDA，否则完整 group 失败；会话就绪事件报告实际 backend，
   而不是 UI 偏好。
6. 设置变更采用 last-write-wins：当前不可变下载可完成，随后重新规划并复用已校验资源。

UI 只显示 `计划 CUDA 13/12`、下载/修复进度、以及后端确认后的
`实际 CUDA 13/12`。无合格 NVIDIA、CUDA 闭包不完整和损坏文件都必须显示明确原因。
