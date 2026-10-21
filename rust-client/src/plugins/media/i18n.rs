use crate::i18n::UiLanguage;

pub fn tr(language: UiLanguage, english: &'static str) -> &'static str {
    DICTIONARY
        .iter()
        .find_map(|(key, zh, ja, ko, ru)| {
            (*key == english).then_some(match language {
                UiLanguage::English => english,
                UiLanguage::Chinese => *zh,
                UiLanguage::Japanese => *ja,
                UiLanguage::Korean => *ko,
                UiLanguage::Russian => *ru,
            })
        })
        .unwrap_or_else(|| crate::i18n::tr(language, english))
}

const DICTIONARY: &[(&str, &str, &str, &str, &str)] = &[
    (
        "Channel routing",
        "声道选择",
        "チャンネル選択",
        "채널 선택",
        "Выбор каналов",
    ),
    (
        "Preparing vocal model",
        "正在准备人声模型",
        "ボーカルモデルを準備中",
        "보컬 모델 준비 중",
        "Подготовка модели вокала",
    ),
    (
        "Vocal model queued",
        "人声模型等待下载",
        "ボーカルモデルのダウンロード待機中",
        "보컬 모델 다운로드 대기 중",
        "Модель вокала в очереди",
    ),
    (
        "Downloading vocal model",
        "正在下载人声模型",
        "ボーカルモデルをダウンロード中",
        "보컬 모델 다운로드 중",
        "Загрузка модели вокала",
    ),
    (
        "Could not prepare vocal model",
        "人声模型准备失败",
        "ボーカルモデルの準備に失敗",
        "보컬 모델 준비 실패",
        "Не удалось подготовить модель вокала",
    ),
    (
        "Download vocal model",
        "下载人声模型",
        "ボーカルモデルをダウンロード",
        "보컬 모델 다운로드",
        "Загрузить модель вокала",
    ),
    (
        "Vocal model is not ready",
        "人声模型尚未就绪",
        "ボーカルモデルの準備ができていません",
        "보컬 모델이 아직 준비되지 않았습니다",
        "Модель вокала ещё не готова",
    ),
    (
        "Delete subtitle",
        "删除字幕",
        "字幕を削除",
        "자막 삭제",
        "Удалить субтитр",
    ),
    (
        "Delete this subtitle?",
        "删除这条字幕？",
        "この字幕を削除しますか？",
        "이 자막을 삭제할까요?",
        "Удалить этот субтитр?",
    ),
    (
        "Retranslate original",
        "重新翻译原文",
        "原文を再翻訳",
        "원문 다시 번역",
        "Перевести исходный текст заново",
    ),
    (
        "Double-click to edit.",
        "双击编辑。",
        "ダブルクリックして編集。",
        "두 번 클릭하여 편집합니다.",
        "Дважды щёлкните для редактирования.",
    ),
    (
        "Save or cancel this edit first.",
        "请先保存或取消当前编辑。",
        "編集中の内容を保存またはキャンセルしてください。",
        "먼저 현재 편집을 저장하거나 취소하세요.",
        "Сначала сохраните или отмените правки.",
    ),
    ("Start time", "开始", "開始", "시작", "Начало"),
    ("End time", "结束", "終了", "종료", "Конец"),
    (
        "Edit subtitle",
        "编辑字幕",
        "字幕を編集",
        "자막 편집",
        "Редактировать субтитр",
    ),
    (
        "Pause translation to edit.",
        "暂停翻译后即可编辑。",
        "翻訳を一時停止すると編集できます。",
        "번역을 일시 정지하면 편집할 수 있습니다.",
        "Приостановите перевод для редактирования.",
    ),
    (
        "Import media",
        "导入媒体",
        "メディアを読み込む",
        "미디어 가져오기",
        "Импорт медиа",
    ),
    (
        "Open media",
        "打开媒体",
        "メディアを開く",
        "미디어 열기",
        "Открыть медиа",
    ),
    (
        "Choose or drop audio, video or subtitles, or paste a stream URL.",
        "选择或拖入音频、视频、字幕，也可粘贴媒体流地址。",
        "音声・動画・字幕を選択またはドロップするか、ストリームURLを貼り付けてください。",
        "오디오, 동영상, 자막을 선택하거나 끌어 놓거나 스트림 URL을 붙여넣으세요.",
        "Выберите или перетащите аудио, видео, субтитры либо вставьте адрес потока.",
    ),
    (
        "Choose file",
        "选择文件",
        "ファイルを選択",
        "파일 선택",
        "Выбрать файл",
    ),
    (
        "Double-click video or press Esc to leave fullscreen. Space to play/pause, arrows to seek.",
        "双击画面或按 Esc 退出全屏；空格播放/暂停，左右键快退/快进。",
        "ダブルクリックまたはEscで全画面を終了。スペースで再生/一時停止、矢印でシーク。",
        "더블 클릭 또는 Esc로 전체 화면 종료. 스페이스로 재생/일시정지, 화살표로 탐색.",
        "Двойной щелчок или Esc — выход; пробел — пауза, стрелки — перемотка.",
    ),
];
