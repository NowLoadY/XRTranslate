"""Generate non-runtime MeloTTS Spanish/French frontend golden fixtures.

This experiment intentionally stays outside the exporter/runtime contract.  It
records the exact upstream Python frontend output that a future Rust frontend
must reproduce.  It does not export models and does not install dependencies.

Example (PowerShell, from the repository root)::

    $env:PYTHONPATH = "runtime/.temp/gruut-223-env"
    C:/Users/22256/miniconda3/envs/torch211cu128/python.exe `
      third_party/openvoice_onnx_export/experiments/romance_frontend_golden.py `
      --hf-cache runtime/.temp/gruut-223-audit/hf `
      --output runtime/.temp/gruut-223-audit/romance_frontend_golden.json

The Hugging Face snapshots are pinned below.  ``snapshot_download`` may fill
only the caller-provided cache; no package or Python environment is modified.
"""

from __future__ import annotations

import argparse
import json
import os
import sqlite3
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
MELO_ROOT = REPOSITORY_ROOT / "third_party" / "MeloTTS"
if str(MELO_ROOT) not in sys.path:
    sys.path.insert(0, str(MELO_ROOT))


@dataclass(frozen=True)
class Recipe:
    language: str
    model_id: str
    revision: str
    gruut_package: str
    cleaner: Callable[[str], str]
    phonemizer: Callable[[str], str]
    language_id: int
    tone_offset: int


def distribute_phone(phone_count: int, token_count: int) -> list[int]:
    """Match MeloTTS's stable round-robin phone allocation."""
    result = [0] * token_count
    for _ in range(phone_count):
        least = min(result)
        result[result.index(least)] += 1
    return result


def intersperse(values: list[int], item: int = 0) -> list[int]:
    result = [item] * (len(values) * 2 + 1)
    result[1::2] = values
    return result


def lexicon_contains(package_name: str, word: str) -> bool:
    from gruut.utils import remove_non_word_chars

    package = __import__(package_name)
    database = Path(package.get_lang_dir()) / "espeak" / "lexicon.db"
    candidates = [
        word,
        word.lower(),
        remove_non_word_chars(word),
        remove_non_word_chars(word.lower()),
    ]
    with sqlite3.connect(database) as connection:
        return any(
            connection.execute(
                "SELECT 1 FROM word_phonemes WHERE word = ? LIMIT 1", (candidate,)
            ).fetchone()
            is not None
            for candidate in candidates
        )


def frontend_case(recipe: Recipe, tokenizer, text: str) -> dict[str, object]:
    from melo.text.symbols import symbols

    normalized = recipe.cleaner(text)
    tokenized = tokenizer.tokenize(normalized)
    groups: list[list[str]] = []
    for token in tokenized:
        if not token.startswith("##"):
            groups.append([token])
        else:
            groups[-1].append(token[2:])

    phones: list[str] = []
    tones: list[int] = []
    word2ph: list[int] = []
    group_records: list[dict[str, object]] = []
    for group in groups:
        word = "".join(group)
        if word == "[UNK]":
            group_phones = ["UNK"]
            source_hint = "unknown-token"
        else:
            group_phones = [phone for phone in recipe.phonemizer(word) if phone != " "]
            if not any(character.isalnum() for character in word):
                source_hint = "break"
            elif any(character.isdigit() for character in word):
                source_hint = "gruut-normalization"
            else:
                source_hint = (
                    "lexicon" if lexicon_contains(recipe.gruut_package, word) else "crf"
                )

        allocation = distribute_phone(len(group_phones), len(group))
        phones.extend(group_phones)
        tones.extend([0] * len(group_phones))
        word2ph.extend(allocation)
        group_records.append(
            {
                "pieces": group,
                "word": word,
                "source_hint": source_hint,
                "phones": group_phones,
                "word2ph": allocation,
            }
        )

    phones = ["_", *phones, "_"]
    tones = [0, *tones, 0]
    word2ph = [1, *word2ph, 1]
    symbol_to_id = {symbol: index for index, symbol in enumerate(symbols)}
    missing = sorted(set(phones).difference(symbol_to_id))
    if missing:
        raise RuntimeError(f"Phones absent from MeloTTS symbol table: {missing}")

    encoded = tokenizer(normalized, add_special_tokens=True)
    if len(encoded["input_ids"]) != len(word2ph):
        raise RuntimeError(
            f"BERT token/word2ph mismatch: {len(encoded['input_ids'])} != {len(word2ph)}"
        )

    bert_word2ph = [value * 2 for value in word2ph]
    bert_word2ph[0] += 1
    phone_ids = [symbol_to_id[phone] for phone in phones]
    return {
        "input": text,
        "normalized": normalized,
        "bert_tokens": tokenizer.convert_ids_to_tokens(encoded["input_ids"]),
        "bert_input_ids": encoded["input_ids"],
        "groups": group_records,
        "phones": phones,
        "raw_word2ph": word2ph,
        "bert_word2ph_after_add_blank": bert_word2ph,
        "phone_ids_after_add_blank": intersperse(phone_ids),
        "tone_ids_after_add_blank": intersperse(
            [tone + recipe.tone_offset for tone in tones]
        ),
        "language_ids_after_add_blank": intersperse(
            [recipe.language_id] * len(phones)
        ),
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hf-cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    args.hf_cache.mkdir(parents=True, exist_ok=True)
    os.environ["HF_HOME"] = str(args.hf_cache.resolve())

    from huggingface_hub import snapshot_download
    from transformers import AutoTokenizer
    from melo.text.es_phonemizer.cleaner import spanish_cleaners
    from melo.text.es_phonemizer.es_to_ipa import es2ipa
    from melo.text.fr_phonemizer.cleaner import french_cleaners
    from melo.text.fr_phonemizer.fr_to_ipa import fr2ipa

    recipes = [
        Recipe(
            language="es",
            model_id="dccuchile/bert-base-spanish-wwm-uncased",
            revision="d1c9c4565c9d6731e57ed7f027b802697bad861e",
            gruut_package="gruut_lang_es",
            cleaner=spanish_cleaners,
            phonemizer=es2ipa,
            language_id=5,
            tone_offset=12,
        ),
        Recipe(
            language="fr",
            model_id="dbmdz/bert-base-french-europeana-cased",
            revision="b895c3cf291f7bf4c15639078a6bee0b3e272c5b",
            gruut_package="gruut_lang_fr",
            cleaner=french_cleaners,
            phonemizer=fr2ipa,
            language_id=6,
            tone_offset=13,
        ),
    ]
    samples = {
        "es": [
            "Hola, XRTranslate cuesta 12,50 euros.",
            "¿Qué tal la síntesis multilingüe?",
        ],
        "fr": [
            "Bonjour, XRTranslate coûte 12,50 euros.",
            "Mme Dupont utilise une synthèse multilingue.",
        ],
    }

    result: dict[str, object] = {
        "schema_version": 1,
        "purpose": "non-runtime golden fixture for a future Rust frontend",
        "gruut": "2.2.3",
        "languages": {},
    }
    languages = result["languages"]
    assert isinstance(languages, dict)
    for recipe in recipes:
        snapshot = snapshot_download(
            recipe.model_id,
            revision=recipe.revision,
            cache_dir=args.hf_cache / "hub",
            allow_patterns=[
                "vocab.txt",
                "tokenizer.json",
                "tokenizer_config.json",
                "special_tokens_map.json",
                "config.json",
            ],
        )
        tokenizer = AutoTokenizer.from_pretrained(snapshot, local_files_only=True)
        languages[recipe.language] = {
            "model_id": recipe.model_id,
            "revision": recipe.revision,
            "tokenizer_class": tokenizer.__class__.__name__,
            "cases": [
                frontend_case(recipe, tokenizer, sample)
                for sample in samples[recipe.language]
            ],
        }

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(args.output.resolve())


if __name__ == "__main__":
    main()
