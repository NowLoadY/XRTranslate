# UI Director 演示录制

本目录提供通用录制工具，复用程序的 UI Director 导航、控件操作、文件音频输入和渲染画面捕获。剧本和输入素材集中保存在 `local/`，由 Git 忽略。

- `local/`：本地剧本与对应素材；JSON、音频和配置不提交到代码库。
- `record.py`：复用 `tools/ui_director.py`，捕获程序渲染画面并合成为白底视频。
- `pointer.py`：与实际操作共享坐标和时间的圆角鼠标、移动与点击动画。
- `session.py`：创建独立的演示副本，运行录制，结束后关闭本次启动的进程。

## 一次录制

需要 Windows、Python、Pillow、imageio-ffmpeg，以及已经安装模型和运行库的 XRTranslate。准备一个**从发布包全新解压、未运行过**的 portable 目录；不要把日常使用的安装目录作为 `--bundle`。

```powershell
python -m pip install Pillow imageio-ffmpeg
cargo build -p rust-client --release --locked --features mpv
python tools/demos/session.py tools/demos/local/vrchat.zh-CN.json `
  --bundle dist/v0.2.11-ui-refresh/XRTranslate-v0.2.11-win-x64 `
  --client target/release/rust-client.exe `
  --installed . `
  --session target/demos/vrchat-take-01 `
  --output dist/videos/vrchat-take-01
```

每次选一个新的 session/output 目录。加 `--preview 4` 可先录包含真实推理的前四章。`--port` 默认 19821，相关演示服务和模型进程使用其后的六个端口，运行前应确保这些端口空闲。

`--installed` 只提供模型目录与原生运行库位置。个人配置、录音、克隆文件和 `debug.md` 不会复制；演示状态留在独立 session 中。启用 `session.inference` 时复用后端的模型进程管理，在独立端口启动并回收本次需要的本地模型。剧本通过 Director 的文件输入接口，将示例音频送进正式识别会话；点击“开始翻译”时不会采集实际麦克风。它不连接 SteamVR；OSC 仅发送到独立的本机演示端口，不向 VRChat 发送消息。应用窗口无需置顶；画面来自程序自身，不包含桌面其他窗口、系统通知或麦克风声音。

输出包括 1920×1080 / 60 fps 的无音轨 MP4（内嵌章节）、章节时间表、字幕 SRT、各章预览图与实际操作记录。视频时长和章节由所选剧本决定。操作记录包括等待到的真实预览文本，便于核查模型结果。

录制按目标帧率逐帧推进程序动画，等待实际渲染完成，再合成对应的视频帧。电脑较慢时只延长录制时间，保存的视频仍按预定节奏播放。采集与合成之间只缓冲少量帧。后台窗口也会继续播放 UI 动画，无需抢占桌面焦点；断开采集后程序恢复正常计时和焦点判断。

推理等待按真实状态完成，视频会压缩等待时间，相关章节有说明。OSC 结果出现后可明确留帧供阅读，避免正常的消息过期计时在慢录时使内容过早消失。留帧复用刚采集的真实画面，鼠标和镜头仍以 60 fps 合成；`recording.json` 分别记录实际采集帧数和留帧数。

## 修改剧本

章节的 `prepare` 在进入该章前调用同一组操作接口，用于准备状态（例如演示新建会议前停止普通翻译）；这些操作也记录在 `recording.json` 中。

每章定义 `page`、`duration`、`title`、`body`；可选 `note` 和开场示意对话。`camera` 中 `at` 是镜头开始移动的章内秒数，`zoom` 是放大倍数，`center` 是原始界面中归一化的中心点。镜头快速起步、平滑减速，到达后停稳；全局 `camera_move_seconds` 默认 0.65 秒，单个镜头可用 `move` 覆盖。

`actions` 按 `at` 升序排列，优先使用 `page`、`click`、`set` 和已注册控件的标签。`tap` / `hover` 可使用标签或逻辑坐标，`scroll` 使用逻辑坐标；鼠标自动提前移动到实际操作位置，点击时放大回弹。`text` 向已聚焦的输入框写入文字，`type` 聚焦目标后在 `duration` 秒内逐字输入。`viewport` 指定物理尺寸和 UI 缩放，确保坐标可复现。

插件页面沿用程序的插件 ID，例如 `plugin:osc`、`plugin:ocr`。先用 `list` 或 `inspect` 查看当前界面的控件；共享文本输入框使用 `host_text_composer` / `osc_text_composer`，按钮沿用当前界面语言的标签。程序界面更新后，应按实际控件调整本地剧本。

`audio_file.path` 相对剧本目录解析，复用软件的媒体解码和识别输入通道，不替换识别或翻译结果。文件输入绑定一个活动任务通道；启动前配置文件，启动后再次调用即可重放。`wait` 检查目标的实际值，可用 `match` 正则、`stable` 稳定秒数、`timeout` 最大等待秒数；超时会终止录制。`hold: true` 在结果出现后留帧，下一次控件操作或新章节恢复采集。`snapshot_at` 可调整章节预览图的选取时间。

如果已经有独立的演示实例，可直接使用：

```powershell
python tools/demos/record.py tools/demos/local/vrchat.zh-CN.json --port 19821 --output dist/videos/take-02
```

直接录制不会重置已有表单、弹窗或折叠状态；完整可复现录制请使用 `session.py` 创建新实例。
