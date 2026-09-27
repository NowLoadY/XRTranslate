# 文本语音回环诊断

从仓库根目录运行真实模型的 `文本 → 翻译 → TTS → ASR → 回译`：

```powershell
cargo run -p xrtranslate-backend --features managed-ort -- --manage-llama-servers roundtrip --text "你好，很高兴见到你。今天我们一起去公园散步。" --source zh --target en --output target/roundtrip/run-001
```

同一输出目录可重复使用，每次开始会自动清除该目录内的旧诊断产物。
只清理带诊断标记或有效旧报告的结果目录，不遍历删除其他测试目录；
若目录中有无关文件、输入文件、符号链接或目录联接，则拒绝清理，避免误删。
输入 JSON 和参考音频应放在输出目录外。成功退出码为 0，失败为非 0。
`report.json` 包含状态、失败阶段、错误、各阶段文本、耗时和提示词执行记录；
`speech.wav` 是实际送入 ASR 的 16 kHz 单声道 PCM16 音频。
失败报告保留此前已完成的阶段。agent 应先检查 `status` 和 `stage`，
再比较输入、正向译文、识别文本和回译，判断语义是否保留。
`passed` 只表示基本推理路径有有效输出，不代表翻译或发音质量达标。

命令读取软件的 `config.json` 及用户配置覆盖，复用 `NativeProviderPlan`、
`NativeInference`、`NativeTtsAdapter`、提示词默认图、文本切块和音频重采样。
源和目标必须是不同的单一语言，且所选模型支持正向翻译和反向识别/翻译。
默认使用软件保存的麦克风音色 `xrtranslate_microphone`；也可临时注册参考录音：

```powershell
cargo run -p xrtranslate-backend --features managed-ort -- --config config.json --manage-llama-servers roundtrip --text "你好，很高兴见到你。" --source zh --target en --reference-wav reference.wav --reference-text "参考录音的准确文本" --output target/roundtrip/run-002
```

参考录音应满足当前 TTS 提供方的音色注册要求。临时音色仅存在于此进程，
不会覆盖软件保存的音色。`--voice` 可指定已有音色或临时音色名称。
全局参数（如 `--config`、`--manage-llama-servers`）放在 `roundtrip` 前。
已有独立模型服务时省略 `--manage-llama-servers`；否则命令自动启动和回收
所需的 llama-server。`--timeout-seconds` 默认 600 秒，包含初始化和推理；
异步等待超时会生成失败报告，但底层已进入的阻塞模型计算可能仍需时间退出。

此入口直接检验软件推理层，不启动桌面 UI、WebSocket 服务或 XR Corpus，
不覆盖麦克风采集、VAD、声卡播放、OSC 或语料检索，也不加载桌面保存的自定义提示词图。
需要验证录音经 WebSocket/VAD 的路径时使用 [录音检查](speech-check.md)。
已有模型专项测试覆盖语言恢复、提供方协议及模型细节，与这个回环并不等价，故保留。
回环代码集中在 `apps/xrtranslate-backend/src/diagnostics/`，没有为回环另外增加回归测试。

## 已安装模型及语种查询

```powershell
cargo run -p xrtranslate-backend --features managed-ort -- models
```

输出 JSON 包含每个已安装包的 `id`、`capability`、`provider`、`languages`、
`hardware` 和 `directory`，包括当前未选中的包。目录和语种来自软件的统一模型目录，
并遵循用户的安装路径覆盖。文件缺失、不可读或大小不符的包不会进入列表；查询不下载模型，
不扫描无关目录，也不加载权重。在线 API 不属于已安装模型，不参与自动组合。
`runtime_verified: false` 表示这里只验证安装完整性，运行库、显存及实际推理结果由回环验证。

其他 Rust 调用方可直接使用以下上层接口（位于 `xrtranslate-config/src/models.rs`）：

- `AppConfig::installed_models(root)`：统一安装清单和语种/硬件元数据。
- `InstalledModel::supports_language(language)`：复用生产路径的语种匹配规则。
- `AppConfig::with_model_assets(ids)`：创建临时模型选择，每项能力选择一个包，保留提供方调参，不写配置文件。

单次回环可通过 `--asr-model`、`--translation-model`、`--tts-model` 指定清单中的 ID。
覆盖 ASR/翻译模型时必须带 `--manage-llama-servers`，确保实际启动的是所选权重。

## 自动组合回环

准备 UTF-8 JSON 文件 `inputs.json`，为希望覆盖的每种源语言提供相应语言的测试文本：

```json
{
  "zh": "你好，很高兴见到你。今天我们一起去公园散步。",
  "en": "Hello, nice to see you. Let us go for a walk in the park today."
}
```

先查看自动生成的组合列表：

```powershell
cargo run -p xrtranslate-backend --features managed-ort -- roundtrip-matrix --inputs inputs.json --output target/roundtrip/plan --plan-only
```

运行所有符合语种条件的已安装模型组合：

```powershell
cargo run -p xrtranslate-backend --features managed-ort -- roundtrip-matrix --inputs inputs.json --output target/roundtrip/matrix --timeout-seconds 120
```

`--target zh,en` 可限制目标语种；省略时枚举所有兼容目标语种。只有输入文件中提供了文本的
语言才作为源语言。组合要求翻译模型支持两个语种、TTS 和 ASR 支持中间语种，
并排除同一基础语言之间的转换。TTS 每次测试一个语言包，不重复测试行为等价的包集合。
可复用单次回环的 `--voice`、`--reference-wav`、`--reference-text`。

批量入口依次调用已有单次回环，每项启动独立进程并自动管理模型服务。
初始化或推理失败会记录下来并继续下一项。每项的报告、音频位于 `case-NNNN/`，
完整日志位于 `case-NNNN.log`，程序及参数数组位于 `case-NNNN.command.json`；
`matrix.json` 随每项完成更新通过/失败计数。
每次运行同时生成 `failures.md`（便于阅读）和 `failures.json`（供 agent 分析），
只收集失败组合，按失败阶段统计，包含模型、语言、输入、完整阶段报告、提示词记录、
复现参数和最多 16 KiB 的日志末尾。子进程崩溃或没有生成报告也会列入异常。
没有失败时也会生成空异常报告；计划及运行中状态与最终通过状态明确区分。
任何一项失败或没有可测试组合时返回非 0；计划模式不会把尚未执行的组合标为通过。
`--timeout-seconds` 是每项回环的异步期限，并非整个矩阵的总时限；阻塞模型调用的退出限制同上。

代码职责：资产层 `installed_assets()` 校验安装文件；配置层 `models.rs` 提供公共查询和选择；
后端 `diagnostics/matrix.rs` 只枚举、调度和汇总，推理统一调用 `roundtrip`，不复制实现。
输出清理与异常报告集中在 `diagnostics/results.rs`，单次与批量入口共用。

内置 Hy-MT 提示词将风格说明置于翻译指令之前，并显式标注 `Current input`。
这避免低量化模型将“翻译下列文本”之后的风格说明当作待翻译正文。
自定义提示词图仍按用户编排执行；诊断使用内置图。
