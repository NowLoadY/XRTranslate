//! Usage guidelines, legal disclaimer, and acceptable use policies for XRTranslate.
//!
//! This module maintains comprehensive, localized usage policies designed to clearly
//! define software boundaries, protect developers and code contributors from liability
//! regarding user misuse, enforce voice rights restrictions, and mandate compliance
//! with applicable laws.

use crate::i18n::UiLanguage;

/// Returns the complete, multi-section usage guidelines and legal disclaimer
/// formatted for dialog presentation.
pub fn full_guidelines_text(language: UiLanguage) -> &'static str {
    match language {
        UiLanguage::Chinese => GUIDELINES_ZH,
        UiLanguage::English => GUIDELINES_EN,
        UiLanguage::Japanese => GUIDELINES_JA,
        UiLanguage::Korean => GUIDELINES_KO,
        UiLanguage::Russian => GUIDELINES_RU,
    }
}

/// Returns concise summary bullet points suitable for inline UI cards / notices.
pub fn notice_summary_items(language: UiLanguage) -> &'static [&'static str] {
    match language {
        UiLanguage::Chinese => &NOTICE_ITEMS_ZH,
        UiLanguage::English => &NOTICE_ITEMS_EN,
        UiLanguage::Japanese => &NOTICE_ITEMS_JA,
        UiLanguage::Korean => &NOTICE_ITEMS_KO,
        UiLanguage::Russian => &NOTICE_ITEMS_RU,
    }
}

// ============================================================================
// Comprehensive Full-Length Guidelines
// ============================================================================

const GUIDELINES_ZH: &str = "\
【XRTranslate 开源使用规范与法律免责声明】

1. 技术中立与开源性质
XRTranslate 是一款开源的本地跨语言沟通与辅助工具，旨在为个人用户提供跨语言交流、语音识别与辅助翻译研究支持。本项目及开发者遵循技术中立原则，以“按现状（AS-IS）”形式提供软件，不对软件的特定用途适用性、稳定性或无缺陷性作任何明示或暗示的保证。

2. 声音克隆与声纹权专属限制
声音克隆功能仅限用于克隆与合成用户本人合法拥有并亲自授权的声音。严禁在未经授权的情况下录制、克隆、模仿、伪造或合成任何第三人（包括但不限于自然人、公众人物、演职人员及受版权保护的配音作品）的声音；严禁利用合成或克隆语音进行欺诈、冒用身份、深度伪造（Deepfake）、骚扰、诽谤或侵犯他人人格权、肖像权与声音权。

3. 守法合规与禁止行为
用户在使用本软件及相关衍生功能时，必须严格遵守所在国家或地区适用的法律法规，以及所连接第三方平台（包括但不限于 VRChat 服务条款及社区守则）的相关规范。严禁将本软件用于任何非法用途，包括但不限于电信网络诈骗、散布虚假信息、侵犯知识产权或商业秘密、侵犯个人隐私、实施网络暴力或开展任何违法犯罪活动。

4. AI 生成局限与免责声明
本软件集成的语音识别（ASR）、机器翻译（MT）与语音合成（TTS）等算法均基于概率统计模型运行，生成的内容可能存在识别错误、误译、语义遗漏、语气失真或意外不当言论。本软件及开发者不对 AI 输出内容的真实性、准确性、完整性与合规性承担任何责任。请勿将本软件输出直接用于医疗、法律、金融、合同签署、应急救援等对准确性有关键要求的场景。

5. 用户独立责任与开发者免责
XRTranslate 的核心推理与数据处理均在用户本地设备或用户自行配置的服务端进行，项目方不收集、不存储、无法控制用户的语音数据、翻译文本与传输行为。因用户个人使用、配置、传播或二次开发本软件所产生的一切民事、行政或刑事法律责任及任何直接、间接损失，均由用户自行全权承担。项目开发者与代码贡献者概不承担任何连带责任。";

const GUIDELINES_EN: &str = "\
[XRTranslate Open Source Usage Guidelines & Legal Disclaimer]

1. Technology Neutrality & Open Source License
XRTranslate is an open-source assistive communication tool designed for cross-language communication, accessibility, and AI research. This software is provided by the authors and code contributors on an \"AS IS\" basis without warranties of any kind, either express or implied, including but not limited to the warranties of merchantability, fitness for a particular purpose, and non-infringement.

2. Strict Voice Rights & Cloning Restrictions
Voice cloning functionality is strictly and exclusively restricted to the user's own voice with lawful personal authorization. You are strictly prohibited from recording, cloning, mimicking, forging, or synthesizing the voice of any third party (including but not limited to individuals, public figures, voice actors, or copyrighted audio works) without explicit prior written authorization. Using synthesized voices for fraud, deceptive impersonation, deepfakes, harassment, defamation, or infringing on personality, privacy, or voice rights is strictly forbidden.

3. Lawful Compliance & Prohibited Conduct
You must comply with all applicable local, national, and international laws and regulations, as well as the terms and community rules of any platforms used in conjunction with this software (including VRChat Terms of Service and Community Guidelines). You agree not to use XRTranslate for any unlawful or malicious activities, including but not limited to fraud, spreading misinformation, intellectual property infringement, trade secret theft, privacy invasion, or cyber harassment.

4. AI Limitations & Output Disclaimer
Speech recognition (ASR), machine translation (MT), and speech synthesis (TTS) models are probabilistic neural networks and may produce hallucinations, mistranslations, omissions, or unintended statements. The developers make no representations or warranties regarding the accuracy, completeness, or reliability of AI-generated content. Do not rely on output from this software for critical scenarios such as medical, legal, financial, contractual, or emergency communications.

5. Independent User Liability & Developer Indemnification
XRTranslate operates locally on your device and connects to user-configured APIs and models. The project developers do not monitor, store, or control your audio, text, or communications. You bear sole and exclusive legal, civil, and criminal liability for all inputs, outputs, transmissions, and consequences arising from your use or misuse of this software. You agree to hold harmless and indemnify the developers and code contributors against any claims, losses, or legal liabilities arising from your use.";

const GUIDELINES_JA: &str = "\
【XRTranslate オープンソース利用規約および免責事項】

1. 技術的中立性とオープンソースの性質
XRTranslateは、言語間コミュニケーション、アクセシビリティ支援、およびAI研究を目的として提供されているオープンソースソフトウェアです。本ソフトウェアは「現状有姿（AS-IS）」で提供され、開発者およびコード貢献者は、商品性や特定目的への適合性を含め、明示・黙示を問わず一切の保証を行いません。

2. 音声クローンおよび声紋権の制限
音声クローン機能は、ユーザー本人の合法的かつ明示的な承諾に基づく自己の音声にのみ使用が限定されます。事前の正当な許諾なく、第三者（一般個人、公人、声優、著作権で保護された音声等）の音声を録音、複製、模倣、偽造、または合成する行為を固く禁じます。なりすまし、詐欺、ディープフェイクの作成、名誉毀損、嫌がらせ、肖像権・声紋権・人格権を侵害する用途での使用は一切禁止されています。

3. 法令順守および禁止事項
利用者は、居住する国・地域の法令および利用するプラットフォーム（VRChat利用規約およびコミュニティガイドライン等）を厳格に順守しなければなりません。詐欺、虚偽情報の流布、知的財産権の侵害、プライバシー侵害、迷惑行為など、あらゆる違法または不正な目的に本ソフトウェアを利用することを固く禁じます。

4. AI出力の限界および非保証
音声認識（ASR）、機械翻訳（MT）、音声合成（TTS）は統計的ニューラルネットワークに基づいているため、誤認識、誤訳、欠落、または不適切な表現を出力する可能性があります。開発者は、AI出力の正確性、信頼性、完全性について責任を負いません。医療、法務、金融、契約締結、緊急連絡などの極めて高い正確性を要する場面には使用しないでください。

5. 利用者の自己責任および免責
XRTranslateの処理はユーザーのローカル環境およびユーザーが設定したAPI上で行われ、開発者がユーザーの音声、テキスト、通信内容を収集・管理することはありません。本ソフトウェアの使用、設定、改変、または第三者への提供によって生じるすべての民事・刑事上の責任および損害は、利用者自身が単独で負うものとし、開発者およびコード貢献者は一切の連帯責任を負いません。";

const GUIDELINES_KO: &str = "\
[XRTranslate 오픈소스 이용 수칙 및 법적 면책 조항]

1. 기술적 중립성 및 오픈소스 성격
XRTranslate는 언어 간 소통, 접근성 지원 및 AI 연구를 목적으로 개발된 오픈소스 도구입니다. 본 소프트웨어는 '있는 그대로(AS-IS)' 제공되며, 개발자 및 코드 기여자는 상품성 및 특정 목적에 대한 적합성을 포함하여 어떠한 명시적 또는 묵시적 보증도 하지 않습니다.

2. 음성 복제 및 음성권에 관한 엄격한 제한
음성 복제 기능은 사용자가 합법적으로 소유하고 동의한 본인의 음성에 한해서만 사용해야 합니다. 명시적인 사전 허가 없이 제3자(일반인, 공인, 성우, 저작권 보호 음성 등)의 음성을 무단 녹음, 복제, 모방, 위조 또는 합성하는 행위는 엄격히 금지됩니다. 사기, 신칭 모용(사칭), 딥페이크 제작, 명예훼손, 괴롭힘 또는 인격권·음성권을 침해하는 모든 행위는 금지됩니다.

3. 법률 준수 및 금지 행위
사용자는 관련 국가 및 지역의 법률과 규정, 그리고 연동되는 플랫폼(VRChat 이용약관 및 커뮤니티 가이드라인 등)을 엄격히 준수해야 합니다. 본 소프트웨어를 사기, 허위사실 유포, 지식재산권 침해, 개인정보 유출, 사이버 괴롭힘 등 어떠한 불법적 용도로도 사용해서는 안 됩니다.

4. AI 생성의 한계 및 면책 조항
본 소프트웨어에 포함된 음성 인식(ASR), 기계 번역(MT), 음성 합성(TTS)은 확률 모델에 기반하며 인식 오류, 오역, 누락 또는 부적절한 표현이 발생할 수 있습니다. 개발자는 AI 출력물의 정확성 및 완전성에 대해 책임을 지지 않으며, 의료·법률·금융·계약·긴급 상황 등 높은 정확도가 요구되는 분야에 본 결과물을 무비판적으로 신뢰하여 사용해서는 안 됩니다.

5. 사용자의 전적인 책임 및 개발자 면책
XRTranslate의 핵심 연산은 사용자의 로컬 기기 또는 사용자가 설정한 API에서 실행되며, 개발자는 사용자의 음성, 텍스트, 전송 데이터를 수집하거나 통제하지 않습니다. 본 소프트웨어의 사용, 설정 및 파생 결과로 인해 발생하는 모든 민·형사상 법적 책임과 손해는 전적으로 사용자가 부담하며, 개발자 및 코드 기여자는 이에 대해 일체의 책임을 지지 않습니다.";

const GUIDELINES_RU: &str = "\
[Правила использования и отказ от ответственности XRTranslate]

1. Технологический нейтралитет и открытый исходный код
XRTranslate — это программное обеспечение с открытым исходным кодом, созданное для преодоления языковых барьеров, улучшения доступности и исследований в области ИИ. ПО предоставляется авторами и авторами кода на условиях «КАК ЕСТЬ» (AS-IS), без каких-либо явных или подразумеваемых гарантий пригодности для конкретных целей, коммерческой ценности или ненарушения прав.

2. Ограничения клонирования голоса и прав на голос
Функция клонирования голоса предназначена исключительно для использования собственного голоса пользователя с его личного согласия. Категорически запрещается без предварительного письменного разрешения записывать, клонировать, имитировать, подделывать или синтезировать голос третьих лиц (включая физических лиц, публичных персон, актеров озвучивания и аудиоматериалы, защищенные авторским правом). Запрещено использовать синтез голоса для мошенничества, выдачи себя за другое лицо, создания дипфейков, клеветы, преследования или нарушения личных неимущественных прав.

3. Соблюдение законодательства и запрещенные действия
Пользователь обязан строго соблюдать применимое национальное и международное законодательство, а также правила сторонних платформ (включая Условия обслуживания и правила сообщества VRChat). Запрещается использовать ПО в любых противоправных целях, включая мошенничество, распространение дезинформации, нарушение прав интеллектуальной собственности, коммерческой тайны, конфиденциальности или кибербуллинг.

4. Ограничения ИИ и отказ от ответственности за результат
Модели распознавания речи (ASR), машинного перевода (MT) и синтеза речи (TTS) основаны на вероятностных нейросетях и могут содержать ошибки, неточности перевода, пропуски или некорректные формулировки. Разработчики не гарантируют точность, полноту и надежность сгенерированных данных. Не используйте вывод программы в критически важных сферах (медицина, юриспруденция, финансы, подписание договоров, экстренные службы).

5. Исключительная ответственность пользователя и освобождение разработчиков
Обработка данных в XRTranslate выполняется локально на устройстве пользователя либо через настроенные им серверы. Разработчики не собирают, не хранят и не контролируют аудиоданные, тексты и коммуникации пользователя. Всю полноту юридической, гражданской и уголовной ответственности за использование ПО и его последствия несет исключительно пользователь. Разработчики и авторы кода не несут ответственности за действия пользователя.";

// ============================================================================
// Summary Bullet Points for Inline Cards / Settings Notice
// ============================================================================

const NOTICE_ITEMS_ZH: [&str; 4] = [
    "声音克隆功能仅限克隆您本人的声音。严禁未经授权录制、克隆、模仿或伪造他人声音，严禁用于冒名欺诈、深度伪造（Deepfake）或骚扰侵害。",
    "请依法合规使用本软件，严格遵守所在国家或地区法律法规及第三方平台（如 VRChat）守则，严禁用于任何违法犯罪或侵权用途。",
    "语音识别、机器翻译与语音合成基于概率统计模型，生成结果可能存在误译、遗漏或错误，请勿用于医疗、法律等高风险关键场景。",
    "XRTranslate 为本地运行的开源工具，不收集也不控制用户数据，用户对自身使用行为及生成传播的所有内容独立承担全部法律责任。",
];

const NOTICE_ITEMS_EN: [&str; 4] = [
    "Voice cloning is strictly for your own voice only. Do not record, clone, imitate, or synthesize another person's voice without explicit authorization, or use it for fraud or deception.",
    "Comply with all applicable laws, regulations, and third-party platform rules (e.g., VRChat Community Guidelines). Do not use XRTranslate for unlawful or infringing purposes.",
    "Speech recognition, translation, and synthesized speech are AI-generated and may contain errors. Verify important content and do not rely on it for critical scenarios.",
    "XRTranslate is an open-source tool running locally on your device. You bear sole and exclusive legal liability for all content generated and actions performed using this software.",
];

const NOTICE_ITEMS_JA: [&str; 4] = [
    "音声クローン機能はご自身の声にのみ使用できます。事前の正当な許諾なく他人の声をクローン、模倣、偽造、またはなりすましに利用することを固く禁じます。",
    "お住まいの国・地域の法令およびプラットフォーム（VRChat等の利用規約）を遵守し、違法・不正・権利侵害の目的に利用しないでください。",
    "音声認識、機械翻訳、音声合成はAIによる生成であり、誤りが含まれる可能性があります。重要事項や高リスクな場面での利用にはご注意ください。",
    "XRTranslateはローカル環境で動作するオープンソースツールです。利用に伴うすべての行為および生成内容に関する法的責任は利用者が単独で負うものとします。",
];

const NOTICE_ITEMS_KO: [&str; 4] = [
    "음성 복제 기능은 본인의 목소리에만 사용할 수 있습니다. 허가 없이 타인의 음성을 무단 복제, 모방, 위조하거나 사칭 및 사기 목적으로 사용하는 것은 금지됩니다.",
    "거주 국가 및 지역의 법률과 연동 플랫폼(VRChat 이용약관 등)을 준수하고, 불법적이거나 타인의 권리를 침해하는 용도로 사용하지 마세요.",
    "음성 인식, 번역 및 합성 음성은 AI 모델 기반이므로 오류나 오역이 발생할 수 있습니다. 중요하거나 위험한 상황에서는 사전에 내용을 확인하세요.",
    "XRTranslate는 로컬에서 구동되는 오픈소스 소프트웨어입니다. 본 프로그램을 통한 모든 사용 행위와 파생 결과에 대한 법적 책임은 전적으로 사용자가 부담합니다.",
];

const NOTICE_ITEMS_RU: [&str; 4] = [
    "Клонирование голоса разрешено только для вашего собственного голоса. Категорически запрещено без разрешения клонировать, имитировать или синтезировать чужой голос.",
    "Соблюдайте законы вашей страны и правила платформ (включая правила VRChat). Запрещено использовать программу в незаконных или мошеннических целях.",
    "Распознавание речи, перевод и синтез речи создаются моделями ИИ и могут содержать ошибки. Всегда проверяйте важную информацию перед использованием.",
    "XRTranslate — это локальное ПО с открытым исходным кодом. Всю полноту юридической ответственности за использование программы и созданный контент несет пользователь.",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_guidelines_contain_critical_clauses_in_all_languages() {
        for lang in UiLanguage::ALL {
            let text = full_guidelines_text(lang);
            assert!(!text.is_empty());
            assert!(text.contains("1."));
            assert!(text.contains("5."));
        }
    }

    #[test]
    fn notice_summary_has_four_items_in_all_languages() {
        for lang in UiLanguage::ALL {
            let items = notice_summary_items(lang);
            assert_eq!(items.len(), 4);
            for item in items {
                assert!(!item.is_empty());
            }
        }
    }
}
