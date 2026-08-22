from __future__ import annotations

import sys
import unittest
from pathlib import Path

import torch

EXPORT_ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EXPORT_ROOT))

from build import (  # noqa: E402
    DEFAULT_PHONE_DURATION_FRAMES,
    MAX_PHONE_DURATION_FRAMES,
    stable_phone_durations,
    validate_melo_cuda_profile,
)


class DurationContractTests(unittest.TestCase):
    def test_fp16_underflow_cannot_zero_a_valid_phone(self) -> None:
        log_duration = torch.tensor([[[-100.0, -20.0, 0.0, 4.0]]], dtype=torch.float16)
        text_mask = torch.tensor([[[1.0, 1.0, 1.0, 0.0]]], dtype=torch.float16)
        length_scale = torch.tensor(1.0, dtype=torch.float16)

        duration = stable_phone_durations(log_duration, text_mask, length_scale)

        self.assertEqual(duration.dtype, torch.float16)
        self.assertEqual(duration.tolist(), [[[1.0, 1.0, 1.0, 0.0]]])

    def test_length_scale_cannot_zero_a_valid_phone(self) -> None:
        log_duration = torch.full((1, 1, 3), -10.0, dtype=torch.float16)
        text_mask = torch.ones((1, 1, 3), dtype=torch.float16)
        length_scale = torch.tensor(0.0001, dtype=torch.float16)

        duration = stable_phone_durations(log_duration, text_mask, length_scale)

        self.assertEqual(duration.tolist(), [[[1.0, 1.0, 1.0]]])

    def test_non_finite_predictions_receive_a_finite_default(self) -> None:
        log_duration = torch.tensor(
            [[[float("nan"), float("inf"), float("-inf"), 0.0]]],
            dtype=torch.float32,
        )
        text_mask = torch.ones((1, 1, 4), dtype=torch.float16)
        length_scale = torch.tensor(1.0, dtype=torch.float16)

        duration = stable_phone_durations(log_duration, text_mask, length_scale)

        self.assertEqual(
            duration.tolist(),
            [[[DEFAULT_PHONE_DURATION_FRAMES] * 3 + [1.0]]],
        )

    def test_duration_is_bounded_before_dynamic_graph_allocation(self) -> None:
        log_duration = torch.tensor([[[100.0]]], dtype=torch.float32)
        text_mask = torch.ones((1, 1, 1), dtype=torch.float16)
        length_scale = torch.tensor(10.0, dtype=torch.float16)

        duration = stable_phone_durations(log_duration, text_mask, length_scale)

        self.assertEqual(duration.item(), MAX_PHONE_DURATION_FRAMES)

    def test_cuda_profile_rejects_cpu_acoustic_compute(self) -> None:
        profile = [
            {
                "cat": "Node",
                "name": "cuda-conv",
                "args": {"provider": "CUDAExecutionProvider", "op_name": "Conv"},
            },
            {
                "cat": "Node",
                "name": "cpu-matmul",
                "args": {
                    "provider": "CPUExecutionProvider",
                    "op_name": "MatMul",
                    "input_type_shape": [{"float16": [1, 192, 32]}],
                    "output_type_shape": [{"float16": [1, 192, 32]}],
                },
            },
        ]

        with self.assertRaisesRegex(RuntimeError, "CPU compute fallback.*MatMul"):
            validate_melo_cuda_profile(profile)

    def test_cuda_profile_allows_cpu_shape_control(self) -> None:
        profile = [
            {"cat": "Node", "args": {"provider": "CUDAExecutionProvider", "op_name": "Conv"}},
            {
                "cat": "Node",
                "args": {
                    "provider": "CPUExecutionProvider",
                    "op_name": "Shape",
                    "input_type_shape": [{"float16": [1, 192, 32]}],
                    "output_type_shape": [{"int64": [3]}],
                },
            },
            {
                "cat": "Node",
                "args": {
                    "provider": "CPUExecutionProvider",
                    "op_name": "Gather",
                    "input_type_shape": [{"int64": [3]}, {"int64": []}],
                    "output_type_shape": [{"int64": []}],
                },
            },
        ]

        summary = validate_melo_cuda_profile(profile)

        self.assertEqual(summary["cuda_convolution_nodes"], 1)
        self.assertEqual(summary["cpu_compute_fallback_nodes"], 0)
        self.assertEqual(summary["cpu_control_nodes"], 2)

    def test_cuda_profile_rejects_unclassified_cpu_work(self) -> None:
        profile = [
            {"cat": "Node", "args": {"provider": "CUDAExecutionProvider", "op_name": "Conv"}},
            {"cat": "Node", "name": "unknown", "args": {"provider": "CPUExecutionProvider", "op_name": "Add"}},
        ]

        with self.assertRaisesRegex(RuntimeError, "CPU compute fallback.*Add"):
            validate_melo_cuda_profile(profile)


if __name__ == "__main__":
    unittest.main()
