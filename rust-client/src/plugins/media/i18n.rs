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
