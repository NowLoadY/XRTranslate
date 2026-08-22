from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

import onnx
from onnx import TensorProto, helper

EXPORT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EXPORT_ROOT))

from graph_contract import (  # noqa: E402
    BERT_HIDDEN_SIZE,
    EXPECTED_OPSET,
    GraphContractError,
    verify_bert_graph,
    verify_melo_graph,
)


TOKEN_WIDTH = 512


def _melo_values():
    inputs = [
        helper.make_tensor_value_info("x_tst", TensorProto.INT32, [1, TOKEN_WIDTH]),
        helper.make_tensor_value_info("x_tst_lenghts", TensorProto.INT32, [1]),
        helper.make_tensor_value_info("speakers", TensorProto.INT32, [1]),
        helper.make_tensor_value_info("tones", TensorProto.INT32, [1, TOKEN_WIDTH]),
        helper.make_tensor_value_info("lang_ids", TensorProto.INT32, [1, TOKEN_WIDTH]),
        helper.make_tensor_value_info(
            "ja_bert", TensorProto.FLOAT16, [1, BERT_HIDDEN_SIZE, TOKEN_WIDTH]
        ),
        helper.make_tensor_value_info("length_scale", TensorProto.FLOAT16, []),
    ]
    outputs = [
        helper.make_tensor_value_info(
            "output", TensorProto.FLOAT16, [1, 1, "output_size"]
        )
    ]
    return inputs, outputs


def _bert_values():
    inputs = [
        helper.make_tensor_value_info(
            "input_ids", TensorProto.INT32, ["batch_size", "sequence_length"]
        )
    ]
    outputs = [
        helper.make_tensor_value_info(
            "logits", TensorProto.FLOAT16, ["sequence_length", BERT_HIDDEN_SIZE]
        )
    ]
    return inputs, outputs


class GraphContractTests(unittest.TestCase):
    def _save(self, inputs, outputs, *, opset: int = EXPECTED_OPSET) -> Path:
        if outputs[0].name == "output":
            nodes = [helper.make_node("Identity", [inputs[-2].name], [outputs[0].name])]
        else:
            nodes = [
                helper.make_node(
                    "Cast",
                    [inputs[0].name],
                    [outputs[0].name],
                    to=TensorProto.FLOAT16,
                )
            ]
        graph = helper.make_graph(nodes, "contract-fixture", inputs, outputs)
        model = helper.make_model(
            graph,
            producer_name="xrtranslate-contract-test",
            opset_imports=[helper.make_opsetid("", opset)],
        )
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        path = Path(directory.name) / "fixture.onnx"
        onnx.save(model, path)
        return path

    def test_accepts_melo_contract(self) -> None:
        inputs, outputs = _melo_values()
        verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_accepts_bert_contract_with_768_hidden_features(self) -> None:
        inputs, outputs = _bert_values()
        verify_bert_graph(self._save(inputs, outputs))

    def test_rejects_wrong_opset(self) -> None:
        inputs, outputs = _melo_values()
        with self.assertRaisesRegex(GraphContractError, "opset 16"):
            verify_melo_graph(self._save(inputs, outputs, opset=17), TOKEN_WIDTH)

    def test_rejects_wrong_input_name(self) -> None:
        inputs, outputs = _melo_values()
        inputs[0].name = "tokens"
        with self.assertRaisesRegex(GraphContractError, "input names"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_wrong_output_name(self) -> None:
        inputs, outputs = _melo_values()
        outputs[0].name = "audio"
        with self.assertRaisesRegex(GraphContractError, "output names"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_wrong_dtype(self) -> None:
        inputs, outputs = _melo_values()
        inputs[4].type.tensor_type.elem_type = TensorProto.INT64
        with self.assertRaisesRegex(GraphContractError, "expected dtype int32"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_wrong_output_dtype(self) -> None:
        inputs, outputs = _melo_values()
        outputs[0].type.tensor_type.elem_type = TensorProto.FLOAT
        with self.assertRaisesRegex(GraphContractError, "expected dtype float16"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_dynamic_dimension_where_fixed_is_required(self) -> None:
        inputs, outputs = _melo_values()
        dimension = inputs[0].type.tensor_type.shape.dim[1]
        dimension.ClearField("dim_value")
        dimension.dim_param = "n_tokens"
        with self.assertRaisesRegex(GraphContractError, "expected shape"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_fixed_dimension_where_dynamic_is_required(self) -> None:
        inputs, outputs = _bert_values()
        dimension = inputs[0].type.tensor_type.shape.dim[1]
        dimension.ClearField("dim_param")
        dimension.dim_value = 4
        with self.assertRaisesRegex(GraphContractError, "expected shape"):
            verify_bert_graph(self._save(inputs, outputs))

    def test_rejects_unexpected_dynamic_dimension_name(self) -> None:
        inputs, outputs = _melo_values()
        outputs[0].type.tensor_type.shape.dim[2].dim_param = "samples"
        with self.assertRaisesRegex(GraphContractError, "output_size"):
            verify_melo_graph(self._save(inputs, outputs), TOKEN_WIDTH)

    def test_rejects_non_768_bert_hidden_size(self) -> None:
        inputs, outputs = _bert_values()
        outputs[0].type.tensor_type.shape.dim[1].dim_value = 1024
        with self.assertRaisesRegex(GraphContractError, "768"):
            verify_bert_graph(self._save(inputs, outputs))


if __name__ == "__main__":
    unittest.main()
