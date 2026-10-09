"""Immutable upstream specifications for OpenVoice V2 base voices.

Presence in :data:`LANGUAGES` means that the upstream artifacts have been
identified and pinned. It does *not* mean that XRTranslate can build or ship
the language. Frontend completeness and redistribution approval are separate
gates owned by ``frontend_recipes``.
"""

from __future__ import annotations

import re
from dataclasses import dataclass


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
    speaker_key: str
    speaker_id: int
    openvoice_embedding_path: str
    frontend_language: str
    frontend_recipe: str
    melo_language_id: int
    melo_tone_start: int
    expected_sample_rate_hz: int
    expected_num_tones: int
    expected_num_languages: int
    bert_repository: str
    bert_revision: str
    bert_weights_filename: str
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
        speaker_key="ZH",
        speaker_id=1,
        openvoice_embedding_path="base_speakers/ses/zh.pth",
        frontend_language="ZH_MIX_EN",
        frontend_recipe="chinese_mixed_english",
        melo_language_id=3,
        melo_tone_start=0,
        expected_sample_rate_hz=44_100,
        expected_num_tones=11,
        expected_num_languages=4,
        bert_repository="bert-base-multilingual-uncased",
        bert_revision="7cbf9a625e29989f6b9c6c2fa68234c304f7e38f",
        bert_weights_filename="model.safetensors",
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
        speaker_key="ES",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/es.pth",
        frontend_language="ES",
        frontend_recipe="spanish_gruut",
        melo_language_id=5,
        melo_tone_start=12,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="dccuchile/bert-base-spanish-wwm-uncased",
        bert_revision="d1c9c4565c9d6731e57ed7f027b802697bad861e",
        bert_weights_filename="pytorch_model.bin",
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
        speaker_key="FR",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/fr.pth",
        frontend_language="FR",
        frontend_recipe="french_gruut",
        melo_language_id=6,
        melo_tone_start=13,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="dbmdz/bert-base-french-europeana-cased",
        bert_revision="b895c3cf291f7bf4c15639078a6bee0b3e272c5b",
        bert_weights_filename="pytorch_model.bin",
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
        speaker_key="JP",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/jp.pth",
        frontend_language="JP",
        frontend_recipe="japanese_mecab",
        melo_language_id=1,
        melo_tone_start=6,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="tohoku-nlp/bert-base-japanese-v3",
        bert_revision="65243d6e5629b969c77309f217bd7b1a79d43c7e",
        bert_weights_filename="pytorch_model.bin",
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
        speaker_key="KR",
        speaker_id=0,
        openvoice_embedding_path="base_speakers/ses/kr.pth",
        frontend_language="KR",
        frontend_recipe="korean_g2pkk",
        melo_language_id=4,
        melo_tone_start=11,
        expected_sample_rate_hz=44_100,
        expected_num_tones=16,
        expected_num_languages=10,
        bert_repository="kykim/bert-kor-base",
        bert_revision="1779cc0982ada0216dd6de0dd4e86fb78201926d",
        bert_weights_filename="pytorch_model.bin",
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
    "speaker_key",
    "speaker_id",
    "openvoice_embedding_path",
    "frontend_language",
    "frontend_recipe",
    "melo_language_id",
    "expected_sample_rate_hz",
    "expected_num_tones",
    "expected_num_languages",
    "bert_repository",
    "bert_revision",
    "bert_hidden_size",
    "sample_text",
)


def package_language_record(spec: LanguageSpec) -> dict[str, object]:
    """Record the language contract without build-only readiness metadata."""

    return {field: getattr(spec, field) for field in _PACKAGE_LANGUAGE_FIELDS}
