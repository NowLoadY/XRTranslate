"""Language-private frontend resources for reproducible OpenVoice packages."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import re
import shutil
import zipfile
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

from languages import LanguageSpec


@dataclass(frozen=True)
class PinnedFrontendSource:
    """A source file whose behavior must be reproduced by a runtime frontend."""

    relative_path: str
    sha256: str


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
    pinned_sources: tuple[PinnedFrontendSource, ...]
    blockers: tuple[str, ...]
    builder: Callable[..., list[Path]] | None

    @property
    def buildable(self) -> bool:
        return self.builder is not None and not self.blockers


def _extract_ngc_member(archive: Path, suffix: str, output: Path) -> None:
    with zipfile.ZipFile(archive) as package:
        matches = [name for name in package.namelist() if name.endswith(suffix)]
        if len(matches) != 1:
            raise RuntimeError(f"Expected one NGC member ending in {suffix!r}, got {matches}")
        output.parent.mkdir(parents=True, exist_ok=True)
        with package.open(matches[0]) as source, output.open("wb") as target:
            shutil.copyfileobj(source, target)


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
    _extract_ngc_member(ngc_archive, "/cmudict.json", cmudict)

    bert_license = licenses / "apache-2.0.txt"
    cmudict_license = licenses / "cmudict.txt"
    _extract_ngc_member(
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
        pinned_sources=(
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/opencpop-strict.txt",
                "86c4b30928e3a4305c9148058c9e2e56b04ce741363fedff382421f4a1e3709d",
            ),
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/tone_sandhi.py",
                "5ff7f5a973466db8f2049db193fc80d679466b0f6f3124d80fc465a26a29af70",
            ),
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
        pinned_sources=(
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/spanish.py",
                "ece5fd1f3fc8b8a815f368da76583beec5fe0d95e1520e5e4dc9eec472c1898d",
            ),
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/es_phonemizer/es_to_ipa.py",
                "bfce9f017060e4e61915cd335fdbabd51098833b61d5101f2cea98554285fe8e",
            ),
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
        pinned_sources=(
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/french.py",
                "79874eb5bee2c67a3834eec47785c80f8ac368ba27841538437ab9847138357c",
            ),
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/fr_phonemizer/fr_to_ipa.py",
                "bba0d4a5d07c7726cfdd7b01bad06f4cee828df89050c78a8fc857ac4a39ffef",
            ),
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
        pinned_sources=(
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/japanese.py",
                "453e84008a8911225af3d551b008835c8fe0484556488c1191db520bb8185c7b",
            ),
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
        pinned_sources=(
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/korean.py",
                "aaa3762c6d976951a35e821f152340d26ec22194bc5b5ad9fac7a065ea68259d",
            ),
            PinnedFrontendSource(
                "third_party/MeloTTS/melo/text/ko_dictionary.py",
                "e51d544e074de5314df9f409c948e39648891c2d6097da17f2f542ec1de0fe3e",
            ),
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


def verify_pinned_sources(recipe: FrontendRecipe, repo_root: Path) -> None:
    """Verify that a recipe still describes the vendored immutable source."""

    for source in recipe.pinned_sources:
        path = repo_root / Path(source.relative_path)
        if not path.is_file():
            raise RuntimeError(f"Pinned frontend source is missing: {path}")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if digest != source.sha256:
            raise RuntimeError(
                f"Pinned frontend source SHA-256 does not match: {source.relative_path}"
            )


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
    verify_pinned_sources(candidate, repo_root)
    return candidate.builder(
        repo_root,
        bert_vocab_source,
        ngc_archive,
        cmudict_license_source,
        frontend,
        licenses,
    )
