"""Language-private frontend resources for reproducible OpenVoice packages."""

from __future__ import annotations

import importlib.metadata
import json
import re
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from artifacts import extract_ngc_member
from languages import LanguageSpec


@dataclass(frozen=True)
class FrontendRecipe:
    """Completeness gate for one language-private text frontend.

    ``required_runtime_data`` describes data, not Python dependencies. A recipe
    remains blocked until every item can be packaged immutably with its own
    license/NOTICE and the runtime implementation has parity tests.
    """

    key: str
    language_key: str
    required_runtime_data: tuple[str, ...]
    source_paths: tuple[str, ...]
    blockers: tuple[str, ...]
    builder: Callable[..., list[Path]] | None

    @property
    def buildable(self) -> bool:
        return self.builder is not None and not self.blockers


def _canonical_pinyin(value: str) -> str | None:
    value = value.lower().replace("ü", "v").replace("u:", "v")
    return value if re.fullmatch(r"[a-zv]+[1-5]", value) else None


def _generate_chinese_lexicon(output: Path) -> None:
    from pypinyin import Style, lazy_pinyin  # pylint: disable=import-outside-toplevel
    from pypinyin.phrases_dict import phrases_dict  # pylint: disable=import-outside-toplevel

    characters: dict[str, str] = {}
    for codepoint in range(0x3400, 0xA000):
        character = chr(codepoint)
        values = lazy_pinyin(
            character,
            style=Style.TONE3,
            neutral_tone_with_five=True,
            errors=lambda text: list(text),
        )
        if len(values) == 1 and (value := _canonical_pinyin(values[0])):
            characters[character] = value

    phrases: dict[str, list[str]] = {}
    for phrase in sorted(phrases_dict):
        if len(phrase) < 2 or not all(character in characters for character in phrase):
            continue
        normalized = [
            _canonical_pinyin(value)
            for value in lazy_pinyin(
                phrase,
                style=Style.TONE3,
                neutral_tone_with_five=True,
                errors=lambda text: list(text),
            )
        ]
        if len(normalized) == len(phrase) and all(normalized):
            phrases[phrase] = [value for value in normalized if value is not None]

    output.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "source": {
                    "package": "pypinyin",
                    "version": importlib.metadata.version("pypinyin"),
                },
                "characters": characters,
                "phrases": phrases,
            },
            ensure_ascii=False,
            separators=(",", ":"),
        )
        + "\n",
        encoding="utf-8",
    )


def _build_chinese_mixed_english(
    repo_root: Path,
    bert_vocab_source: Path,
    ngc_archive: Path,
    cmudict_license_source: Path,
    frontend: Path,
    licenses: Path,
) -> list[Path]:
    vocab = frontend / "bert_vocab.txt"
    shutil.copyfile(bert_vocab_source, vocab)
    opencpop = frontend / "opencpop-strict.txt"
    shutil.copyfile(
        repo_root / "third_party" / "MeloTTS" / "melo" / "text" / "opencpop-strict.txt",
        opencpop,
    )
    lexicon = frontend / "chinese_lexicon.json"
    _generate_chinese_lexicon(lexicon)
    cmudict = frontend / "cmudict.json"
    extract_ngc_member(ngc_archive, "/cmudict.json", cmudict)

    bert_license = licenses / "apache-2.0.txt"
    cmudict_license = licenses / "cmudict.txt"
    extract_ngc_member(
        ngc_archive, "/licences/bert_base_uncased/LICENCE.txt", bert_license
    )
    shutil.copyfile(cmudict_license_source, cmudict_license)
    pypinyin_license = licenses / "pypinyin.txt"
    distribution = importlib.metadata.distribution("pypinyin")
    installed_license = Path(
        distribution.locate_file("pypinyin-0.50.0.dist-info/LICENSE.txt")
    )
    shutil.copyfile(installed_license, pypinyin_license)
    notice = licenses / "chinese-frontend-notice.txt"
    notice.write_text(
        "Chinese tone-sandhi behavior is derived from MeloTTS's "
        "melo/text/tone_sandhi.py, Copyright (c) 2021 PaddlePaddle Authors, "
        "licensed under Apache License 2.0. See apache-2.0.txt.\n",
        encoding="utf-8",
    )
    return [
        vocab,
        opencpop,
        lexicon,
        cmudict,
        bert_license,
        cmudict_license,
        pypinyin_license,
        notice,
    ]


RECIPES = {
    "chinese_mixed_english": FrontendRecipe(
        key="chinese_mixed_english",
        language_key="zh",
        required_runtime_data=(
            "pinned BERT WordPiece vocabulary",
            "OpenCPOP pinyin-to-phone mapping",
            "pypinyin character and phrase lexicon",
            "CMU English pronunciation dictionary",
            "Apache-2.0, pypinyin and CMUdict license/NOTICE files",
        ),
        source_paths=(
            "third_party/MeloTTS/melo/text/opencpop-strict.txt",
            "third_party/MeloTTS/melo/text/tone_sandhi.py",
        ),
        blockers=(),
        builder=_build_chinese_mixed_english,
    ),
    "spanish_gruut": FrontendRecipe(
        key="spanish_gruut",
        language_key="es",
        required_runtime_data=(
            "pinned BERT WordPiece vocabulary",
            "MeloTTS Spanish normalization rules",
            "gruut Spanish lexicon, phonology and tokenizer data",
            "licenses/NOTICE for BERT, MeloTTS and the complete gruut closure",
        ),
        source_paths=(
            "third_party/MeloTTS/melo/text/spanish.py",
            "third_party/MeloTTS/melo/text/es_phonemizer/es_to_ipa.py",
        ),
        blockers=(
            "the pinned Spanish BERT repository declares no redistribution license",
            "the gruut-es runtime data closure and its licenses are not pinned",
            "the provider runtime has no parity-tested Spanish frontend",
        ),
        builder=None,
    ),
    "french_gruut": FrontendRecipe(
        key="french_gruut",
        language_key="fr",
        required_runtime_data=(
            "pinned cased BERT WordPiece vocabulary",
            "MeloTTS French normalization and abbreviation rules",
            "gruut French lexicon, phonology and tokenizer data",
            "MIT and complete gruut-closure license/NOTICE files",
        ),
        source_paths=(
            "third_party/MeloTTS/melo/text/french.py",
            "third_party/MeloTTS/melo/text/fr_phonemizer/fr_to_ipa.py",
        ),
        blockers=(
            "the gruut-fr runtime data closure and its licenses are not pinned",
            "the provider runtime has no parity-tested French frontend",
        ),
        builder=None,
    ),
    "japanese_mecab": FrontendRecipe(
        key="japanese_mecab",
        language_key="jp",
        required_runtime_data=(
            "pinned Japanese BERT vocabulary and tokenizer configuration",
            "MeloTTS Japanese number, kana and phone rules",
            "MeCab-compatible UniDic dictionary data",
            "Apache-2.0 and complete MeCab/UniDic/pykakasi license/NOTICE files",
        ),
        source_paths=(
            "third_party/MeloTTS/melo/text/japanese.py",
        ),
        blockers=(
            "the MeCab/UniDic/pykakasi runtime data closure and licenses are not pinned",
            "the provider runtime has no parity-tested Japanese frontend",
        ),
        builder=None,
    ),
    "korean_g2pkk": FrontendRecipe(
        key="korean_g2pkk",
        language_key="kr",
        required_runtime_data=(
            "pinned Korean BERT WordPiece vocabulary",
            "MeloTTS Korean normalization dictionaries",
            "g2pK/MeCab-ko pronunciation and dictionary data",
            "licenses/NOTICE for BERT, MeloTTS and the complete Korean G2P closure",
        ),
        source_paths=(
            "third_party/MeloTTS/melo/text/korean.py",
            "third_party/MeloTTS/melo/text/ko_dictionary.py",
        ),
        blockers=(
            "the pinned Korean BERT repository declares no redistribution license",
            "the g2pK/MeCab-ko runtime data closure and its licenses are not pinned",
            "the provider runtime has no parity-tested Korean frontend",
        ),
        builder=None,
    ),
}


def recipe_for(recipe: str) -> FrontendRecipe:
    try:
        return RECIPES[recipe]
    except KeyError as error:
        raise RuntimeError(
            f"No OpenVoice frontend recipe is registered for {recipe!r}"
        ) from error


def require_buildable_recipe(spec: LanguageSpec) -> FrontendRecipe:
    """Fail before downloads when a candidate is not safe and complete to ship."""

    recipe = recipe_for(spec.frontend_recipe)
    if recipe.language_key != spec.key:
        raise RuntimeError(
            f"Frontend recipe {recipe.key!r} belongs to {recipe.language_key!r}, "
            f"not {spec.key!r}"
        )
    blockers = list(recipe.blockers)
    if not spec.bert_license.redistribution_approved:
        blockers.insert(
            0,
            f"BERT redistribution is not approved: {spec.bert_license.note} "
            f"Evidence: {spec.bert_license.evidence_url}",
        )
    if recipe.builder is None and not blockers:
        blockers.append("the frontend package builder is not implemented")
    if blockers:
        raise RuntimeError(
            f"OpenVoice language {spec.key!r} is pinned but not buildable:\n- "
            + "\n- ".join(blockers)
        )
    return recipe


def buildable_language_keys(language_specs: dict[str, LanguageSpec]) -> tuple[str, ...]:
    """Return only fully implemented and redistribution-approved recipes."""

    return tuple(
        sorted(
            key
            for key, spec in language_specs.items()
            if spec.bert_license.redistribution_approved
            and recipe_for(spec.frontend_recipe).buildable
        )
    )


def check_sources(recipe: FrontendRecipe, repo_root: Path) -> None:
    for relative_path in recipe.source_paths:
        path = repo_root / relative_path
        if not path.is_file() or path.stat().st_size == 0:
            raise RuntimeError(f"Frontend source is missing or empty: {path}")


def sample_token_ids(recipe: str, sample_text: str, vocab_path: Path) -> list[int]:
    """Encode the recipe's golden sample without relying on mutable tokenizer files."""
    candidate = recipe_for(recipe)
    if not candidate.buildable or recipe != "chinese_mixed_english":
        raise RuntimeError(f"No sample tokenizer is registered for {recipe!r}")
    vocabulary = {
        token: index
        for index, token in enumerate(vocab_path.read_text(encoding="utf-8").splitlines())
    }
    cls = vocabulary["[CLS]"]
    sep = vocabulary["[SEP]"]
    unknown = vocabulary["[UNK]"]
    pieces = [
        vocabulary.get(character, unknown)
        for character in sample_text
        if not character.isspace()
    ]
    return [cls, *pieces, sep]


def build_frontend(
    recipe: str,
    repo_root: Path,
    bert_vocab_source: Path,
    ngc_archive: Path,
    cmudict_license_source: Path,
    frontend: Path,
    licenses: Path,
) -> list[Path]:
    """Build exactly one language frontend through an explicit recipe seam."""
    candidate = recipe_for(recipe)
    if not candidate.buildable or candidate.builder is None:
        raise RuntimeError(f"OpenVoice frontend recipe {recipe!r} is not buildable")
    check_sources(candidate, repo_root)
    return candidate.builder(
        repo_root,
        bert_vocab_source,
        ngc_archive,
        cmudict_license_source,
        frontend,
        licenses,
    )
