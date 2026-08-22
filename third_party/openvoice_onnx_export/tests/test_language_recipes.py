from __future__ import annotations

import sys
import tempfile
import unittest
from dataclasses import replace
from pathlib import Path


EXPORT_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = EXPORT_ROOT.parents[1]
sys.path.insert(0, str(EXPORT_ROOT))

from frontend_recipes import (  # noqa: E402
    RECIPES,
    buildable_language_keys,
    recipe_for,
    require_buildable_recipe,
    sample_token_ids,
    verify_pinned_sources,
)
from build import build  # noqa: E402
from languages import (  # noqa: E402
    LANGUAGES,
    package_language_record,
    validate_language_spec,
    validate_language_specs,
)


class LanguageSpecificationTests(unittest.TestCase):
    def test_official_v2_candidates_are_all_explicit(self) -> None:
        self.assertEqual(set(LANGUAGES), {"zh", "es", "fr", "jp", "kr"})
        self.assertEqual(
            {spec.frontend_language for spec in LANGUAGES.values()},
            {"ZH_MIX_EN", "ES", "FR", "JP", "KR"},
        )
        self.assertEqual(
            {key: spec.melo_tone_start for key, spec in LANGUAGES.items()},
            {"zh": 0, "es": 12, "fr": 13, "jp": 6, "kr": 11},
        )
        validate_language_specs()

    def test_every_candidate_has_a_private_frontend_recipe(self) -> None:
        self.assertEqual(
            {spec.frontend_recipe for spec in LANGUAGES.values()}, set(RECIPES)
        )
        self.assertEqual(
            {recipe.language_key for recipe in RECIPES.values()}, set(LANGUAGES)
        )

    def test_pinned_frontend_sources_match_vendored_melotts(self) -> None:
        for recipe in RECIPES.values():
            with self.subTest(recipe=recipe.key):
                verify_pinned_sources(recipe, REPO_ROOT)

    def test_revision_and_digest_validation_is_fail_closed(self) -> None:
        with self.assertRaisesRegex(ValueError, "40-hex"):
            validate_language_spec(replace(LANGUAGES["fr"], bert_revision="main"))
        with self.assertRaisesRegex(ValueError, "SHA-256"):
            validate_language_spec(replace(LANGUAGES["fr"], bert_vocab_sha256="unknown"))

    def test_source_embedding_is_pinned_per_language(self) -> None:
        for key, spec in LANGUAGES.items():
            with self.subTest(language=key):
                self.assertEqual(
                    spec.openvoice_embedding_path,
                    f"base_speakers/ses/{key}.pth",
                )
        self.assertEqual(
            len({spec.openvoice_embedding_sha256 for spec in LANGUAGES.values()}),
            len(LANGUAGES),
        )

    def test_new_workflow_metadata_does_not_change_v1_manifest_shape(self) -> None:
        record = package_language_record(LANGUAGES["zh"])
        self.assertNotIn("bert_weights_filename", record)
        self.assertNotIn("bert_license", record)
        self.assertNotIn("melo_tone_start", record)
        self.assertEqual(len(record), 24)


class FrontendReadinessTests(unittest.TestCase):
    def test_only_chinese_is_a_build_choice(self) -> None:
        self.assertEqual(buildable_language_keys(LANGUAGES), ("zh",))
        self.assertTrue(require_buildable_recipe(LANGUAGES["zh"]).buildable)

    def test_unlicensed_bert_candidates_fail_closed(self) -> None:
        for key in ("es", "kr"):
            with self.subTest(language=key):
                with self.assertRaisesRegex(RuntimeError, "redistribution is not approved"):
                    require_buildable_recipe(LANGUAGES[key])
                self.assertIsNone(LANGUAGES[key].bert_license.spdx_id)
                self.assertFalse(
                    LANGUAGES[key].bert_license.redistribution_approved
                )

    def test_incomplete_licensed_frontends_are_not_buildable(self) -> None:
        for key in ("fr", "jp"):
            with self.subTest(language=key):
                self.assertTrue(LANGUAGES[key].bert_license.redistribution_approved)
                with self.assertRaisesRegex(RuntimeError, "no parity-tested"):
                    require_buildable_recipe(LANGUAGES[key])

    def test_blocked_recipes_cannot_tokenize_a_golden_sample(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        vocab = Path(directory.name) / "vocab.txt"
        vocab.write_text("[UNK]\n[CLS]\n[SEP]\n", encoding="utf-8")
        for key in ("es", "fr", "jp", "kr"):
            recipe = LANGUAGES[key].frontend_recipe
            with self.subTest(recipe=recipe):
                with self.assertRaisesRegex(RuntimeError, "No sample tokenizer"):
                    sample_token_ids(recipe, LANGUAGES[key].sample_text, vocab)

    def test_blocked_build_fails_before_touching_existing_output(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        output_root = Path(directory.name)
        marker = output_root / "packages" / "fr" / "v1" / "keep.txt"
        marker.parent.mkdir(parents=True)
        marker.write_text("unchanged", encoding="utf-8")
        with self.assertRaisesRegex(RuntimeError, "no parity-tested"):
            build(REPO_ROOT, LANGUAGES["fr"], output_root)
        self.assertEqual(marker.read_text(encoding="utf-8"), "unchanged")

    def test_unknown_recipe_is_rejected(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "No OpenVoice frontend recipe"):
            recipe_for("generic_multilingual")


if __name__ == "__main__":
    unittest.main()
