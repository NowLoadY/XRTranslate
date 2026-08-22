"""Immutable upstream specifications for OpenVoice V2 base voices.

Presence in :data:`LANGUAGES` means that the upstream artifacts have been
identified and pinned. It does *not* mean that XRTranslate can build or ship
the language. Frontend completeness and redistribution approval are separate
gates owned by ``frontend_recipes``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass


_SHA256 = re.compile(r"[0-9a-f]{64}")
_REVISION = re.compile(r"[0-9a-f]{40}")


@dataclass(frozen=True)
class LicenseReview:
    """Recorded redistribution evidence for one upstream model artifact."""

    spdx_id: str | None
    evidence_url: str
    redistribution_approved: bool
    note: str


@dataclass(frozen=True)
class LanguageSpec:
    key: str
    label: str
    language_tag: str
    melo_repository: str
    melo_revision: str
    config_sha256: str
    checkpoint_sha256: str
    speaker_key: str
    speaker_id: int
    openvoice_embedding_path: str
    openvoice_embedding_sha256: str
    frontend_language: str
    frontend_recipe: str
    melo_language_id: int
    melo_tone_start: int
    expected_sample_rate_hz: int
    expected_num_tones: int
    expected_num_languages: int
    bert_repository: str
    bert_revision: str
    bert_config_sha256: str
    bert_weights_filename: str
    bert_weights_sha256: str
    bert_vocab_sha256: str
    bert_hidden_size: int
    bert_license: LicenseReview
    sample_text: str


def _model_card(repository: str, revision: str) -> str:
    return f"https://huggingface.co/{repository}/blob/{revision}/README.md"


LANGUAGES = {
    "zh": LanguageSpec(
        key="zh",
        label="Chinese (mixed English)",
        language_tag="zh",
        melo_repository="myshell-ai/MeloTTS-Chinese",
        melo_revision="af5d207a364ea4208c6f589c89f57f88414bdd16",
        config_sha256="d58b5acdab89ad2bbd65325affab309ae3cb964834b02f9a60587474e81c8bb9",
        checkpoint_sha256="a74e9eadffff065c75eb6dfa040efa72cad23e72cfea70d39190bc174fb97093",
        speaker_key="ZH",
        speaker_id=1,
        openvoice_embedding_path="base_speakers/ses/zh.pth",
        openvoice_embedding_sha256=(
            "2b353de562700c13faacf096ecfc0adcafd26e6704a9feef572be1279714e031"
        ),
        frontend_language="ZH_MIX_EN",
        frontend_recipe="chinese_mixed_english",
        melo_language_id=3,
        melo_tone_start=0,
        expected_sample_rate_hz=44_100,
        expected_num_tones=11,
        expected_num_languages=4,
        bert_repository="bert-base-multilingual-uncased",
        bert_revision="7cbf9a625e29989f6b9c6c2fa68234c304f7e38f",
        bert_config_sha256="fba5d4b0a351a43f6ccb7a6587301fd9f6876ca36aae62af762af67c8f18db1c",
        bert_weights_filename="model.safetensors",
        bert_weights_sha256="b33adb2b700b7029a64a4a14ddec6bda8555d2ca879e80a75789fd9542a6290e",
        bert_vocab_sha256="87b44292b452f6c05afa49b2e488e7eedf79ea4f4c39db6f2f4b37764228ef3f",
        bert_hidden_size=768,
        bert_license=LicenseReview(
            spdx_id="Apache-2.0",
            evidence_url=_model_card(
                "bert-base-multilingual-uncased",
                "7cbf9a625e29989f6b9c6c2fa68234c304f7e38f",
            ),
            redistribution_approved=True,
            note="Hugging Face model metadata declares Apache-2.0.",
        ),
        sample_text="你好，欢迎使用语音翻译。",
    ),
    "es": LanguageSpec(
        key="es",
        label="Spanish",
        language_tag="es",
        melo_repository="myshell-ai/MeloTTS-Spanish",
        melo_revision="dbb5496df39d11a66c1d5f5a9ca357c3c9fb95fb",
        config_sha256="54488d922a2983f4d6a7f57158cc5f2714ea117994e1b22147a8579791554221",
        checkpoint_sha256="9077a7e7e5fd8e42f3f922641c401f1936971c08465a3e7ccb19d57a659e72ae",
        speaker_key="ES",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/es.pth",
        openvoice_embedding_sha256=(
            "b8cece8853fb75b9f5217a1f5cda9807bac92a3e4c4547fc651e404d05deff63"
        ),
        frontend_language="ES",
        frontend_recipe="spanish_gruut",
        melo_language_id=5,
        melo_tone_start=12,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="dccuchile/bert-base-spanish-wwm-uncased",
        bert_revision="d1c9c4565c9d6731e57ed7f027b802697bad861e",
        bert_config_sha256="ee7d29a157d70dd6736e8dfadff7e32544566c701e95565a8849c1e65218e86f",
        bert_weights_filename="pytorch_model.bin",
        bert_weights_sha256="5480283d2ac26ac36df538fa5c12412b89ff176db693d00e71735200d9e0e99b",
        bert_vocab_sha256="a7e3f713aafb7d9dbd789a0f0bc30a457e622d963294e40b391b4d83ca4508b5",
        bert_hidden_size=768,
        bert_license=LicenseReview(
            spdx_id=None,
            evidence_url=_model_card(
                "dccuchile/bert-base-spanish-wwm-uncased",
                "d1c9c4565c9d6731e57ed7f027b802697bad861e",
            ),
            redistribution_approved=False,
            note="The pinned model card/repository declares no license.",
        ),
        sample_text="Hola, bienvenido a la traduccion por voz.",
    ),
    "fr": LanguageSpec(
        key="fr",
        label="French",
        language_tag="fr",
        melo_repository="myshell-ai/MeloTTS-French",
        melo_revision="1e9bf590262392d8bffb679b0a3b0c16b0f9fdaf",
        config_sha256="361e84109451acb2f0331c2c9f3c5437e9a502380bbf8741b02545e41b062139",
        checkpoint_sha256="fdf967d514f91582e451c482cab655e5d736821c3ba87ede8bb0625709642b29",
        speaker_key="FR",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/fr.pth",
        openvoice_embedding_sha256=(
            "8a01f6d30a73efa368c288a542a522a2bcdd4e2ec5589d8646b307cf8e2ad9ae"
        ),
        frontend_language="FR",
        frontend_recipe="french_gruut",
        melo_language_id=6,
        melo_tone_start=13,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="dbmdz/bert-base-french-europeana-cased",
        bert_revision="b895c3cf291f7bf4c15639078a6bee0b3e272c5b",
        bert_config_sha256="0da603d6d30507b56d7ff92010bf68b95b9c6330be707c578c18bffc9baaced0",
        bert_weights_filename="pytorch_model.bin",
        bert_weights_sha256="ff5df6abf065e94b474d58176a72f89a977320ddd06448a258797e74a4257a89",
        bert_vocab_sha256="2012518b30b8c3534aaa98b1c39f988158b925633b0039d7f273ca96017b77ad",
        bert_hidden_size=768,
        bert_license=LicenseReview(
            spdx_id="MIT",
            evidence_url=_model_card(
                "dbmdz/bert-base-french-europeana-cased",
                "b895c3cf291f7bf4c15639078a6bee0b3e272c5b",
            ),
            redistribution_approved=True,
            note="Hugging Face model metadata declares MIT.",
        ),
        sample_text="Bonjour, bienvenue dans la traduction vocale.",
    ),
    "jp": LanguageSpec(
        key="jp",
        label="Japanese",
        language_tag="ja",
        melo_repository="myshell-ai/MeloTTS-Japanese",
        melo_revision="367f8795464b531b4e97c1515bddfc1243e60891",
        config_sha256="207def0d31bf7623e20f4a5e690f217747661bf495319c0139303122b6debcc3",
        checkpoint_sha256="96ae783e6ec0177aa810e2a645aec5d136a6f4992fdea26ee92b7b04d9688ad0",
        speaker_key="JP",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/jp.pth",
        openvoice_embedding_sha256=(
            "7b645ff428de4a57a22122318968f1e6127ac81fda2e2aa66062deccd3864416"
        ),
        frontend_language="JP",
        frontend_recipe="japanese_mecab",
        melo_language_id=1,
        melo_tone_start=6,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="tohoku-nlp/bert-base-japanese-v3",
        bert_revision="65243d6e5629b969c77309f217bd7b1a79d43c7e",
        bert_config_sha256="2a86800f7f45c980d14cbb0f22a71e4f42642fad8c4b2b658fb98b65dfa9e527",
        bert_weights_filename="pytorch_model.bin",
        bert_weights_sha256="e172862e0674054d65e0ba40d67df2a4687982f589db44aa27091c386e5450a4",
        bert_vocab_sha256="5e9a696b0191b833cfdf8eefada01f41f23ccbd7e7746946864260b1cdd0a784",
        bert_hidden_size=768,
        bert_license=LicenseReview(
            spdx_id="Apache-2.0",
            evidence_url=_model_card(
                "tohoku-nlp/bert-base-japanese-v3",
                "65243d6e5629b969c77309f217bd7b1a79d43c7e",
            ),
            redistribution_approved=True,
            note="Hugging Face model metadata declares Apache-2.0.",
        ),
        sample_text="こんにちは、音声翻訳へようこそ。",
    ),
    "kr": LanguageSpec(
        key="kr",
        label="Korean",
        language_tag="ko",
        melo_repository="myshell-ai/MeloTTS-Korean",
        melo_revision="0207e5adfc90129a51b6b03d89be6d84360ed323",
        config_sha256="74543376976dfadde45ba34336fa79c7e95509f43a7c2e701b22c0f71fd7695c",
        checkpoint_sha256="48e3ff3fd0b5348e095f0468e60ae727507564100f58142ef3a922ead6e0a4d0",
        speaker_key="KR",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/kr.pth",
        openvoice_embedding_sha256=(
            "f501479d6072741a396725bec79144653e9f4a5381b85901e29683aa169795df"
        ),
        frontend_language="KR",
        frontend_recipe="korean_g2pkk",
        melo_language_id=4,
        melo_tone_start=11,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="kykim/bert-kor-base",
        bert_revision="1779cc0982ada0216dd6de0dd4e86fb78201926d",
        bert_config_sha256="c37ae92c39e3600e7c9645148e406286d44c9863cb8a237196ac6ad35bc47749",
        bert_weights_filename="pytorch_model.bin",
        bert_weights_sha256="ae43a392e533ccb9fd38e5c65130aeee50381b87e81e795f9d90469accd78236",
        bert_vocab_sha256="25a329a892130c73f7dffd6aafa0382d713f93633ef320e49ea54a449096a089",
        bert_hidden_size=768,
        bert_license=LicenseReview(
            spdx_id=None,
            evidence_url=_model_card(
                "kykim/bert-kor-base",
                "1779cc0982ada0216dd6de0dd4e86fb78201926d",
            ),
            redistribution_approved=False,
            note="The pinned model card/repository declares no license.",
        ),
        sample_text="안녕하세요, 음성 번역에 오신 것을 환영합니다.",
    ),
}


def validate_language_spec(spec: LanguageSpec) -> None:
    """Reject mutable or structurally incomplete upstream specifications."""

    if spec.key not in {"zh", "es", "fr", "jp", "kr"}:
        raise ValueError(f"Unsupported OpenVoice V2 language key: {spec.key!r}")
    for label, value in (
        ("MeloTTS revision", spec.melo_revision),
        ("BERT revision", spec.bert_revision),
    ):
        if not _REVISION.fullmatch(value):
            raise ValueError(f"{spec.key}: {label} must be an immutable 40-hex commit")
    for label, value in (
        ("MeloTTS config", spec.config_sha256),
        ("MeloTTS checkpoint", spec.checkpoint_sha256),
        ("OpenVoice embedding", spec.openvoice_embedding_sha256),
        ("BERT config", spec.bert_config_sha256),
        ("BERT weights", spec.bert_weights_sha256),
        ("BERT vocabulary", spec.bert_vocab_sha256),
    ):
        if not _SHA256.fullmatch(value):
            raise ValueError(f"{spec.key}: {label} must have a pinned SHA-256")
    if spec.openvoice_embedding_path != f"base_speakers/ses/{spec.key}.pth":
        raise ValueError(f"{spec.key}: source embedding path does not match language key")
    if spec.bert_weights_filename not in {"model.safetensors", "pytorch_model.bin"}:
        raise ValueError(f"{spec.key}: unsupported pinned BERT weights filename")
    if spec.melo_language_id < 0 or spec.melo_language_id >= spec.expected_num_languages:
        raise ValueError(f"{spec.key}: MeloTTS language id is outside the model range")
    if spec.melo_tone_start < 0 or spec.melo_tone_start >= spec.expected_num_tones:
        raise ValueError(f"{spec.key}: MeloTTS tone offset is outside the model range")
    if not spec.bert_license.evidence_url.startswith("https://huggingface.co/"):
        raise ValueError(f"{spec.key}: BERT license evidence must use the pinned model card")
    if spec.bert_license.redistribution_approved and not spec.bert_license.spdx_id:
        raise ValueError(f"{spec.key}: approved BERT license requires an SPDX id")


def validate_language_specs() -> None:
    if set(LANGUAGES) != {"zh", "es", "fr", "jp", "kr"}:
        raise ValueError("OpenVoice V2 language matrix is incomplete")
    for key, spec in LANGUAGES.items():
        if key != spec.key:
            raise ValueError(f"Language registry key {key!r} does not match {spec.key!r}")
        validate_language_spec(spec)


validate_language_specs()


_PACKAGE_LANGUAGE_FIELDS = (
    "key",
    "label",
    "language_tag",
    "melo_repository",
    "melo_revision",
    "config_sha256",
    "checkpoint_sha256",
    "speaker_key",
    "speaker_id",
    "openvoice_embedding_path",
    "openvoice_embedding_sha256",
    "frontend_language",
    "frontend_recipe",
    "melo_language_id",
    "expected_sample_rate_hz",
    "expected_num_tones",
    "expected_num_languages",
    "bert_repository",
    "bert_revision",
    "bert_config_sha256",
    "bert_weights_sha256",
    "bert_vocab_sha256",
    "bert_hidden_size",
    "sample_text",
)


def package_language_record(spec: LanguageSpec) -> dict[str, object]:
    """Keep the v1 package manifest stable while workflow metadata evolves."""

    return {field: getattr(spec, field) for field in _PACKAGE_LANGUAGE_FIELDS}
