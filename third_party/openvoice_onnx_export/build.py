"""Export an official MeloTTS language checkpoint to XRTranslate's ONNX contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import sys
import urllib.request
import zipfile
from pathlib import Path
from typing import Any

import numpy as np
import onnx
import onnxruntime as ort
import torch
from huggingface_hub import hf_hub_download
from transformers import AutoModelForMaskedLM

from languages import LANGUAGES, LanguageSpec, package_language_record
from frontend_recipes import (
    buildable_language_keys,
    build_frontend,
    require_buildable_recipe,
    sample_token_ids,
)
from graph_contract import verify_bert_graph, verify_melo_graph


EXPECTED_TORCH = "2.11.0+cu128"
MELOTTS_COMMIT = "209145371cff8fc3bd60d7be902ea69cbdb7965a"
OPENVOICE_COMMIT = "74a1d147b17a8c3092dd5430504bd83ef6c7eb23"
OPENVOICE_REPOSITORY = "myshell-ai/OpenVoiceV2"
OPENVOICE_REVISION = "f36e7edfe1684461a8343844af60babc2efbb727"
REFERENCE_ENCODER_REPOSITORY = "TigreGotico/voiceclonnx-openvoice-v2"
REFERENCE_ENCODER_REVISION = "34d010c192c97f763207f488f6057fd07fee42ad"
NGC_V2_ARCHIVE_URL = "https://api.ngc.nvidia.com/v2/models/nvidia/nvigisdk/openvoice/versions/OpenVoice%20v2/files/%7B09F5E010-5D94-413C-8852-ABC34464DDF8%7D.zip"
NGC_V2_ARCHIVE_SHA256 = "266dc4662965858e07a1c8cb086f17e1c30f0fdc3202e8934103dc7927314811"
NGC_V2_ARCHIVE_BYTES = 204_579_050
CMUDICT_LICENSE_URL = "https://raw.githubusercontent.com/cmusphinx/cmudict/74790861f652b15e4ac49015a90074ad62a27690/LICENSE"
CMUDICT_LICENSE_BYTES = 1_754
CMUDICT_LICENSE_SHA256 = "bd4ce8e44170a5f9f481310ca85c51de3c4f851a65e679b40e603b143bd3542a"
OPSET = 16
MELO_GRAPH_TOKEN_WIDTH = 512


class HParams:
    def __init__(self, **kwargs: Any) -> None:
        for key, value in kwargs.items():
            if isinstance(value, dict):
                value = HParams(**value)
            setattr(self, key, value)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def file_record(path: Path, root: Path) -> dict[str, Any]:
    return {
        "path": path.relative_to(root).as_posix(),
        "bytes": path.stat().st_size,
        "sha256": sha256(path),
    }


def verify_file(path: Path, expected_bytes: int, expected_sha256: str) -> None:
    if path.stat().st_size != expected_bytes or sha256(path) != expected_sha256:
        raise RuntimeError(f"Immutable source verification failed: {path}")


def download_ngc_archive(cache_root: Path) -> Path:
    cache_root.mkdir(parents=True, exist_ok=True)
    archive = cache_root / "openvoice-v2-ngc.zip"
    if archive.is_file():
        try:
            verify_file(archive, NGC_V2_ARCHIVE_BYTES, NGC_V2_ARCHIVE_SHA256)
            return archive
        except RuntimeError:
            archive.unlink()
    partial = archive.with_suffix(".zip.part")
    if partial.exists():
        partial.unlink()
    urllib.request.urlretrieve(NGC_V2_ARCHIVE_URL, partial)
    verify_file(partial, NGC_V2_ARCHIVE_BYTES, NGC_V2_ARCHIVE_SHA256)
    partial.replace(archive)
    return archive


def download_cmudict_license(cache_root: Path) -> Path:
    cache_root.mkdir(parents=True, exist_ok=True)
    target = cache_root / "cmudict-license.txt"
    if target.is_file():
        try:
            verify_file(target, CMUDICT_LICENSE_BYTES, CMUDICT_LICENSE_SHA256)
            return target
        except RuntimeError:
            target.unlink()
    partial = target.with_suffix(".txt.part")
    if partial.exists():
        partial.unlink()
    urllib.request.urlretrieve(CMUDICT_LICENSE_URL, partial)
    verify_file(partial, CMUDICT_LICENSE_BYTES, CMUDICT_LICENSE_SHA256)
    partial.replace(target)
    return target


def extract_ngc_member(archive: Path, suffix: str, output: Path) -> None:
    with zipfile.ZipFile(archive) as package:
        matches = [name for name in package.namelist() if name.endswith(suffix)]
        if len(matches) != 1:
            raise RuntimeError(f"Expected one NGC member ending in {suffix!r}, got {matches}")
        output.parent.mkdir(parents=True, exist_ok=True)
        with package.open(matches[0]) as source, output.open("wb") as target:
            shutil.copyfileobj(source, target)


def require_environment() -> None:
    if torch.__version__ != EXPECTED_TORCH:
        raise RuntimeError(
            f"Expected the unchanged torch211cu128 PyTorch build {EXPECTED_TORCH}, "
            f"found {torch.__version__}."
        )
    if not torch.cuda.is_available():
        raise RuntimeError("CUDA is required for the FP16 MeloTTS export.")


def load_melo_source(repo_root: Path):
    source_root = repo_root / "third_party" / "MeloTTS"
    if not (source_root / "melo" / "models.py").is_file():
        raise RuntimeError(f"Pinned MeloTTS source is missing from {source_root}")
    sys.path.insert(0, str(source_root))
    from melo import commons  # pylint: disable=import-outside-toplevel
    from melo.models import SynthesizerTrn  # pylint: disable=import-outside-toplevel

    # `generate_path` calls sequence_mask with an FP16 duration tensor. The
    # legacy exporter then emits an invalid FP16 ONNX Range. Keep this control
    # path in FP32 while leaving the acoustic network and its public ABI FP16.
    def onnx_sequence_mask(length: torch.Tensor, max_length=None):
        if max_length is None:
            max_length = length.max()
        positions = torch.arange(
            max_length, dtype=torch.float32, device=length.device
        )
        return positions.unsqueeze(0) < length.float().unsqueeze(1)

    commons.sequence_mask = onnx_sequence_mask

    # Upstream reshapes the generated duration path with the dynamic output
    # length as an explicit third dimension. The legacy exporter encodes that
    # symbolic value as ONNX Reshape's `0` sentinel at dimension 2, which is
    # invalid for the rank-2 input whenever real text predicts a duration
    # different from the trace sample. The package ABI is intentionally fixed
    # to batch=1/token_width=512, so infer only the duration axis with `-1`.
    def onnx_generate_path(duration: torch.Tensor, mask: torch.Tensor):
        cum_duration = torch.cumsum(duration, -1)
        path = onnx_sequence_mask(cum_duration.reshape(-1), mask.shape[2]).to(mask.dtype)
        path = path.reshape(1, MELO_GRAPH_TOKEN_WIDTH, -1)
        path = path - torch.nn.functional.pad(path, (0, 0, 1, 0, 0, 0))[:, :-1]
        return path.unsqueeze(1).transpose(2, 3) * mask

    commons.generate_path = onnx_generate_path

    return SynthesizerTrn, onnx_sequence_mask, onnx_generate_path


class MeloOnnxWrapper(torch.nn.Module):
    """Stable graph boundary shared by every MeloTTS language package."""

    def __init__(self, model: torch.nn.Module, sequence_mask, generate_path) -> None:
        super().__init__()
        self.model = model
        self.sequence_mask = sequence_mask
        self.generate_path = generate_path

    def forward(
        self,
        x_tst: torch.Tensor,
        x_tst_lenghts: torch.Tensor,
        speakers: torch.Tensor,
        tones: torch.Tensor,
        lang_ids: torch.Tensor,
        ja_bert: torch.Tensor,
        length_scale: torch.Tensor,
    ) -> torch.Tensor:
        bert = torch.zeros(
            (x_tst.shape[0], 1024, x_tst.shape[1]),
            dtype=ja_bert.dtype,
            device=ja_bert.device,
        )
        speakers = speakers.long()
        g = self.model.emb_g(speakers).unsqueeze(-1)
        encoded, means, _logs, text_mask = self.model.enc_p(
            x_tst.long(),
            x_tst_lenghts.long(),
            tones.long(),
            lang_ids.long(),
            bert,
            ja_bert,
            g=g,
        )
        # MeloTTS officially supports sdp_ratio=0. Use its deterministic
        # duration predictor and the latent distribution mean. Embedding an
        # unseeded ONNX RNG made identical text vary by process and could yield
        # a degenerate duration on CUDA before the application could recover.
        log_duration = self.model.dp(encoded, text_mask, g=g)
        duration = torch.exp(log_duration) * text_mask * length_scale
        duration = torch.ceil(duration)
        output_lengths = torch.clamp_min(torch.sum(duration, [1, 2]), 1).long()
        output_mask = torch.unsqueeze(self.sequence_mask(output_lengths, None), 1).to(
            text_mask.dtype
        )
        attention_mask = torch.unsqueeze(text_mask, 2) * torch.unsqueeze(output_mask, -1)
        attention = self.generate_path(duration, attention_mask)
        means = torch.matmul(attention.squeeze(1), means.transpose(1, 2)).transpose(1, 2)
        latent = self.model.flow(means, output_mask, g=g, reverse=True)
        audio = self.model.dec(latent * output_mask, g=g)
        return audio.reshape(1, 1, -1)


class BertFeatureWrapper(torch.nn.Module):
    """Expose the MeloTTS hidden-state feature without an MLM vocabulary head."""

    def __init__(self, model: torch.nn.Module) -> None:
        super().__init__()
        self.model = model

    def forward(self, input_ids: torch.Tensor) -> torch.Tensor:
        input_ids = input_ids.long()
        attention_mask = torch.ones_like(input_ids)
        token_type_ids = torch.zeros_like(input_ids)
        outputs = self.model(
            input_ids=input_ids,
            attention_mask=attention_mask,
            token_type_ids=token_type_ids,
            output_hidden_states=True,
            return_dict=True,
        )
        return outputs.hidden_states[-3][0]


def export_melo(
    repo_root: Path, spec: LanguageSpec, config_path: Path, checkpoint_path: Path, output: Path
) -> None:
    SynthesizerTrn, sequence_mask, generate_path = load_melo_source(repo_root)
    config_data = json.loads(config_path.read_text(encoding="utf-8"))
    hps = HParams(**config_data)
    model = SynthesizerTrn(
        len(hps.symbols),
        hps.data.filter_length // 2 + 1,
        hps.train.segment_size // hps.data.hop_length,
        n_speakers=hps.data.n_speakers,
        num_tones=hps.num_tones,
        num_languages=hps.num_languages,
        **vars(hps.model),
    )
    checkpoint = torch.load(checkpoint_path, map_location="cpu", weights_only=True)
    model.load_state_dict(checkpoint["model"], strict=True)
    device = torch.device("cuda:0")
    wrapper = MeloOnnxWrapper(
        model.eval().half().to(device), sequence_mask, generate_path
    ).eval()

    tokens = MELO_GRAPH_TOKEN_WIDTH
    x_tst = torch.zeros((1, tokens), dtype=torch.int32, device=device)
    x_tst_lenghts = torch.tensor([12], dtype=torch.int32, device=device)
    speakers = torch.tensor([spec.speaker_id], dtype=torch.int32, device=device)
    tones = torch.zeros((1, tokens), dtype=torch.int32, device=device)
    lang_ids = torch.full(
        (1, tokens), spec.melo_language_id, dtype=torch.int32, device=device
    )
    ja_bert = torch.zeros((1, 768, tokens), dtype=torch.float16, device=device)
    length_scale = torch.tensor(1.0, dtype=torch.float16, device=device)

    output.parent.mkdir(parents=True, exist_ok=True)
    with torch.inference_mode():
        torch.onnx.export(
            wrapper,
            (
                x_tst,
                x_tst_lenghts,
                speakers,
                tones,
                lang_ids,
                ja_bert,
                length_scale,
            ),
            output,
            input_names=[
                "x_tst",
                "x_tst_lenghts",
                "speakers",
                "tones",
                "lang_ids",
                "ja_bert",
                "length_scale",
            ],
            output_names=["output"],
            dynamic_axes={"output": {2: "output_size"}},
            opset_version=OPSET,
            do_constant_folding=True,
            dynamo=False,
        )
    graph = onnx.load(output)
    output_dims = graph.graph.output[0].type.tensor_type.shape.dim
    output_dims[0].ClearField("dim_param")
    output_dims[0].dim_value = 1
    output_dims[1].ClearField("dim_param")
    output_dims[1].dim_value = 1
    output_dims[2].ClearField("dim_value")
    output_dims[2].dim_param = "output_size"
    onnx.checker.check_model(graph)
    onnx.save(graph, output)
    verify_melo_graph(output, MELO_GRAPH_TOKEN_WIDTH)


def smoke_melo(path: Path, speaker_id: int, language_id: int) -> dict[str, Any]:
    providers = ["CUDAExecutionProvider", "CPUExecutionProvider"]
    session = ort.InferenceSession(str(path), providers=providers)
    tokens = MELO_GRAPH_TOKEN_WIDTH
    outputs = session.run(
        ["output"],
        {
            "x_tst": np.zeros((1, tokens), dtype=np.int32),
            "x_tst_lenghts": np.array([12], dtype=np.int32),
            "speakers": np.array([speaker_id], dtype=np.int32),
            "tones": np.zeros((1, tokens), dtype=np.int32),
            "lang_ids": np.full((1, tokens), language_id, dtype=np.int32),
            "ja_bert": np.zeros((1, 768, tokens), dtype=np.float16),
            "length_scale": np.array(1.0, dtype=np.float16),
        },
    )[0]
    if outputs.ndim != 3 or outputs.shape[0:2] != (1, 1):
        raise RuntimeError(f"Unexpected MeloTTS output shape: {outputs.shape}")
    if not np.isfinite(outputs).all() or float(np.max(np.abs(outputs))) < 1.0e-5:
        raise RuntimeError("MeloTTS ONNX smoke output is silent or non-finite")
    return {
        "providers": session.get_providers(),
        "output_shape": list(outputs.shape),
        "peak": float(np.max(np.abs(outputs))),
    }


def export_bert(model_directory: Path, output: Path, input_ids: list[int]) -> None:
    device = torch.device("cuda:0")
    model = AutoModelForMaskedLM.from_pretrained(
        model_directory,
        local_files_only=True,
        attn_implementation="eager",
    )
    wrapper = BertFeatureWrapper(model.eval().half().to(device)).eval()
    input_ids = torch.tensor([input_ids], dtype=torch.int32, device=device)
    with torch.inference_mode():
        torch.onnx.export(
            wrapper,
            (input_ids,),
            output,
            input_names=["input_ids"],
            output_names=["logits"],
            dynamic_axes={
                "input_ids": {0: "batch_size", 1: "sequence_length"},
                "logits": {0: "sequence_length"},
            },
            opset_version=OPSET,
            do_constant_folding=True,
            dynamo=False,
        )
    verify_bert_graph(output)


def smoke_bert(path: Path, input_ids: list[int], hidden_size: int) -> dict[str, Any]:
    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    output = session.run(
        ["logits"],
        {"input_ids": np.array([input_ids], dtype=np.int32)},
    )[0]
    if output.shape != (len(input_ids), hidden_size) or not np.isfinite(output).all():
        raise RuntimeError(f"Unexpected BERT feature output: {output.shape}")
    return {
        "providers": session.get_providers(),
        "output_shape": list(output.shape),
    }


def build(repo_root: Path, spec: LanguageSpec, output_root: Path) -> Path:
    # Discovery is not availability. Reject incomplete/unlicensed candidates
    # before deleting output, touching caches, or starting any network request.
    require_buildable_recipe(spec)
    package_root = output_root / "packages" / spec.key / "v1"
    if package_root.exists():
        shutil.rmtree(package_root)
    graphs = package_root / "models"
    voices = package_root / "voices"
    frontend = package_root / "frontend"
    licenses = package_root / "licenses"
    for directory in (graphs, voices, frontend, licenses):
        directory.mkdir(parents=True, exist_ok=True)

    config_source = Path(
        hf_hub_download(spec.melo_repository, "config.json", revision=spec.melo_revision)
    )
    checkpoint_source = Path(
        hf_hub_download(spec.melo_repository, "checkpoint.pth", revision=spec.melo_revision)
    )
    if sha256(config_source) != spec.config_sha256:
        raise RuntimeError("Pinned MeloTTS config SHA-256 does not match")
    if sha256(checkpoint_source) != spec.checkpoint_sha256:
        raise RuntimeError("Pinned MeloTTS checkpoint SHA-256 does not match")
    config_data = json.loads(config_source.read_text(encoding="utf-8"))
    if config_data["data"]["spk2id"].get(spec.speaker_key) != spec.speaker_id:
        raise RuntimeError("Pinned MeloTTS speaker key/id does not match the language spec")
    if config_data["data"]["sampling_rate"] != spec.expected_sample_rate_hz:
        raise RuntimeError("Pinned MeloTTS sample rate does not match the language spec")
    if config_data["num_tones"] != spec.expected_num_tones:
        raise RuntimeError("Pinned MeloTTS tone count does not match the language spec")
    if config_data["num_languages"] != spec.expected_num_languages:
        raise RuntimeError("Pinned MeloTTS language count does not match the language spec")
    embedding_source = Path(
        hf_hub_download(
            OPENVOICE_REPOSITORY,
            spec.openvoice_embedding_path,
            revision=OPENVOICE_REVISION,
        )
    )
    if sha256(embedding_source) != spec.openvoice_embedding_sha256:
        raise RuntimeError("Pinned OpenVoice source embedding SHA-256 does not match")
    bert_config_source = Path(
        hf_hub_download(
            spec.bert_repository, "config.json", revision=spec.bert_revision
        )
    )
    bert_weights_source = Path(
        hf_hub_download(
            spec.bert_repository,
            spec.bert_weights_filename,
            revision=spec.bert_revision,
        )
    )
    bert_vocab_source = Path(
        hf_hub_download(spec.bert_repository, "vocab.txt", revision=spec.bert_revision)
    )
    for source, expected, label in (
        (bert_config_source, spec.bert_config_sha256, "config"),
        (bert_weights_source, spec.bert_weights_sha256, "weights"),
        (bert_vocab_source, spec.bert_vocab_sha256, "vocabulary"),
    ):
        if sha256(source) != expected:
            raise RuntimeError(f"Pinned BERT {label} SHA-256 does not match")
    bert_config = json.loads(bert_config_source.read_text(encoding="utf-8"))
    if bert_config.get("hidden_size") != spec.bert_hidden_size:
        raise RuntimeError("Pinned BERT hidden size does not match the language spec")
    reference_encoder_source = Path(
        hf_hub_download(
            REFERENCE_ENCODER_REPOSITORY,
            "tone_ref_encoder.onnx",
            revision=REFERENCE_ENCODER_REVISION,
        )
    )
    ngc_archive = download_ngc_archive(output_root / ".cache")
    cmudict_license_source = download_cmudict_license(output_root / ".cache")

    config_target = package_root / "model_config.json"
    shutil.copyfile(config_source, config_target)
    embedding = torch.load(embedding_source, map_location="cpu", weights_only=True)
    embedding = embedding.detach().float().cpu().reshape(-1)
    if embedding.numel() != 256:
        raise RuntimeError(f"Unexpected source embedding shape: {tuple(embedding.shape)}")
    (voices / f"{spec.key}.bin").write_bytes(embedding.numpy().astype("<f4").tobytes())

    melo_target = graphs / "melo.onnx"
    export_melo(repo_root, spec, config_source, checkpoint_source, melo_target)
    melo_smoke = smoke_melo(melo_target, spec.speaker_id, spec.melo_language_id)
    bert_target = graphs / "bert.onnx"
    golden_input_ids = sample_token_ids(
        spec.frontend_recipe, spec.sample_text, bert_vocab_source
    )
    export_bert(bert_config_source.parent, bert_target, golden_input_ids)
    bert_smoke = smoke_bert(bert_target, golden_input_ids, spec.bert_hidden_size)
    frontend_files = build_frontend(
        spec.frontend_recipe,
        repo_root,
        bert_vocab_source,
        ngc_archive,
        cmudict_license_source,
        frontend,
        licenses,
    )

    converter_target = graphs / "converter.onnx"
    extract_ngc_member(
        ngc_archive,
        "/SynthesizerTrnConverter_onnx16_v2_float16.onnx",
        converter_target,
    )
    reference_encoder_target = graphs / "reference_encoder.onnx"
    shutil.copyfile(reference_encoder_source, reference_encoder_target)

    openvoice_license = licenses / "openvoice-melotts.txt"
    extract_ngc_member(ngc_archive, "/licences/openVoice_melloTTS/LICENCE.txt", openvoice_license)

    files = [
        config_target,
        melo_target,
        bert_target,
        converter_target,
        reference_encoder_target,
        voices / f"{spec.key}.bin",
        openvoice_license,
        *frontend_files,
    ]
    manifest = {
        "schema_version": 1,
        "package_id": f"openvoice-v2-{spec.key}-onnx-fp16",
        "language": package_language_record(spec),
        "upstream": {
            "melotts_source": {
                "repository": "myshell-ai/MeloTTS",
                "revision": MELOTTS_COMMIT,
            },
            "openvoice_source": {
                "repository": "myshell-ai/OpenVoice",
                "revision": OPENVOICE_COMMIT,
            },
            "melo_checkpoint": {
                "repository": spec.melo_repository,
                "revision": spec.melo_revision,
                "config_sha256": sha256(config_source),
                "checkpoint_sha256": sha256(checkpoint_source),
            },
            "source_embedding": {
                "repository": OPENVOICE_REPOSITORY,
                "revision": OPENVOICE_REVISION,
                "path": spec.openvoice_embedding_path,
                "sha256": sha256(embedding_source),
            },
            "openvoice_core": {
                "ngc_archive_url": NGC_V2_ARCHIVE_URL,
                "ngc_archive_bytes": NGC_V2_ARCHIVE_BYTES,
                "ngc_archive_sha256": NGC_V2_ARCHIVE_SHA256,
                "reference_encoder_repository": REFERENCE_ENCODER_REPOSITORY,
                "reference_encoder_revision": REFERENCE_ENCODER_REVISION,
                "reference_encoder_sha256": sha256(reference_encoder_source),
            },
            "cmudict_license": {
                "url": CMUDICT_LICENSE_URL,
                "bytes": CMUDICT_LICENSE_BYTES,
                "sha256": CMUDICT_LICENSE_SHA256,
            },
            "bert": {
                "repository": spec.bert_repository,
                "revision": spec.bert_revision,
                "config_sha256": sha256(bert_config_source),
                "weights_sha256": sha256(bert_weights_source),
                "vocab_sha256": sha256(bert_vocab_source),
            },
        },
        "conversion": {
            "python": sys.version,
            "torch": torch.__version__,
            "torch_cuda": torch.version.cuda,
            "onnx": onnx.__version__,
            "onnxruntime": ort.__version__,
            "opset": OPSET,
            "precision": "fp16",
        },
        "graph_contract": {
            "inputs": [
                f"x_tst:int32[1,{MELO_GRAPH_TOKEN_WIDTH}]",
                "x_tst_lenghts:int32[1]",
                "speakers:int32[1]",
                f"tones:int32[1,{MELO_GRAPH_TOKEN_WIDTH}]",
                f"lang_ids:int32[1,{MELO_GRAPH_TOKEN_WIDTH}]",
                f"ja_bert:float16[1,768,{MELO_GRAPH_TOKEN_WIDTH}]",
                "length_scale:float16[]",
            ],
            "output": "output:float16[batch,1,samples]",
            "sample_rate_hz": spec.expected_sample_rate_hz,
            "fixed_execution_tokens": MELO_GRAPH_TOKEN_WIDTH,
        },
        "validation": {"melo": melo_smoke, "bert": bert_smoke},
        "files": [file_record(path, package_root) for path in files],
    }
    manifest_path = package_root / "package-manifest.json"
    manifest_path.write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    return manifest_path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    buildable_languages = buildable_language_keys(LANGUAGES)
    parser.add_argument("--language", choices=buildable_languages, required=True)
    parser.add_argument("--output", type=Path, required=True)
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    require_environment()
    repo_root = Path(__file__).resolve().parents[2]
    output_root = args.output.resolve()
    manifest = build(repo_root, LANGUAGES[args.language], output_root)
    print(manifest)


if __name__ == "__main__":
    main()
