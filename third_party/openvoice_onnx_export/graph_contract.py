"""Validate exported ONNX graphs against XRTranslate's provider-private ABI."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import onnx
from onnx import ModelProto, TensorProto, ValueInfoProto


EXPECTED_OPSET = 16
BERT_HIDDEN_SIZE = 768


class GraphContractError(RuntimeError):
    """An exported graph does not match the runtime tensor contract."""


@dataclass(frozen=True)
class DynamicDimension:
    name: str


Dimension = int | DynamicDimension


@dataclass(frozen=True)
class TensorContract:
    name: str
    element_type: int
    shape: tuple[Dimension, ...]


def _dynamic(name: str) -> DynamicDimension:
    return DynamicDimension(name)


def _melo_inputs(token_width: int) -> tuple[TensorContract, ...]:
    return (
        TensorContract("x_tst", TensorProto.INT32, (1, token_width)),
        TensorContract("x_tst_lenghts", TensorProto.INT32, (1,)),
        TensorContract("speakers", TensorProto.INT32, (1,)),
        TensorContract("tones", TensorProto.INT32, (1, token_width)),
        TensorContract("lang_ids", TensorProto.INT32, (1, token_width)),
        TensorContract("ja_bert", TensorProto.FLOAT16, (1, BERT_HIDDEN_SIZE, token_width)),
        TensorContract("length_scale", TensorProto.FLOAT16, ()),
    )


MELO_OUTPUTS = (
    TensorContract("output", TensorProto.FLOAT16, (1, 1, _dynamic("output_size"))),
)

BERT_INPUTS = (
    TensorContract(
        "input_ids",
        TensorProto.INT32,
        (_dynamic("batch_size"), _dynamic("sequence_length")),
    ),
)

BERT_OUTPUTS = (
    TensorContract(
        "logits",
        TensorProto.FLOAT16,
        (_dynamic("sequence_length"), BERT_HIDDEN_SIZE),
    ),
)


def verify_melo_graph(path: Path, token_width: int) -> None:
    """Require the fixed-width Melo graph ABI consumed by the Rust runtime."""
    if token_width <= 0:
        raise ValueError("Melo token width must be positive")
    _verify_graph(path, _melo_inputs(token_width), MELO_OUTPUTS)


def verify_bert_graph(path: Path) -> None:
    """Require the FP16 768-wide BERT feature graph consumed by each frontend."""
    _verify_graph(path, BERT_INPUTS, BERT_OUTPUTS)


def _verify_graph(
    path: Path,
    expected_inputs: tuple[TensorContract, ...],
    expected_outputs: tuple[TensorContract, ...],
) -> None:
    model = onnx.load(path, load_external_data=False)
    onnx.checker.check_model(model)
    _verify_opset(model, path)
    _verify_values("input", model.graph.input, expected_inputs, path)
    _verify_values("output", model.graph.output, expected_outputs, path)


def _verify_opset(model: ModelProto, path: Path) -> None:
    standard_versions = [
        item.version for item in model.opset_import if item.domain in ("", "ai.onnx")
    ]
    if standard_versions != [EXPECTED_OPSET]:
        raise GraphContractError(
            f"{path}: expected one standard ONNX opset {EXPECTED_OPSET}, "
            f"found {standard_versions}"
        )


def _verify_values(
    kind: str,
    actual_values,
    expected_values: tuple[TensorContract, ...],
    path: Path,
) -> None:
    actual_names = [value.name for value in actual_values]
    expected_names = [value.name for value in expected_values]
    if actual_names != expected_names:
        raise GraphContractError(
            f"{path}: expected {kind} names {expected_names}, found {actual_names}"
        )
    for actual, expected in zip(actual_values, expected_values, strict=True):
        _verify_value(kind, actual, expected, path)


def _verify_value(
    kind: str,
    actual: ValueInfoProto,
    expected: TensorContract,
    path: Path,
) -> None:
    if not actual.type.HasField("tensor_type"):
        raise GraphContractError(f"{path}: {kind} {actual.name!r} is not a tensor")
    tensor_type = actual.type.tensor_type
    if tensor_type.elem_type != expected.element_type:
        raise GraphContractError(
            f"{path}: {kind} {actual.name!r} expected dtype "
            f"{_dtype_name(expected.element_type)}, found "
            f"{_dtype_name(tensor_type.elem_type)}"
        )
    actual_shape = tuple(_read_dimension(dimension) for dimension in tensor_type.shape.dim)
    if actual_shape != expected.shape:
        raise GraphContractError(
            f"{path}: {kind} {actual.name!r} expected shape "
            f"{_format_shape(expected.shape)}, found {_format_shape(actual_shape)}"
        )


def _read_dimension(dimension) -> Dimension | None:
    if dimension.HasField("dim_value"):
        return dimension.dim_value
    if dimension.HasField("dim_param") and dimension.dim_param:
        return DynamicDimension(dimension.dim_param)
    return None


def _format_shape(shape) -> str:
    values = []
    for dimension in shape:
        if isinstance(dimension, DynamicDimension):
            values.append(dimension.name)
        elif dimension is None:
            values.append("?")
        else:
            values.append(str(dimension))
    return "[" + ",".join(values) + "]"


def _dtype_name(element_type: int) -> str:
    try:
        return TensorProto.DataType.Name(element_type).lower()
    except ValueError:
        return f"unknown({element_type})"
