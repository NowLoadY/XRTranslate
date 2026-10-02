"""Prepare a private demo copy of a clean portable bundle and run a screenplay.

Only model/runtime library locations are reused from --installed; user state,
recordings, cloned voices and debug.md are never copied. All spawned processes
belong to this invocation and are stopped when recording finishes.
"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import time
import urllib.request

from record import record


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2), encoding="utf-8")


def run(args):
    script = json.loads(args.script.read_text(encoding="utf-8"))
    args.session = args.session.resolve()
    stage = args.session / "app"
    if args.session.exists():
        raise FileExistsError("Choose a fresh session directory; existing data is never removed")
    # Read entrypoints from the existing packager, excluding all mutable runtime state.
    manifest = json.loads((args.bundle / "release-manifest.json").read_text(encoding="utf-8"))
    shutil.copytree(args.bundle, stage, ignore=shutil.ignore_patterns("runtime", "debug.md"))
    executable = stage / manifest["entrypoints"]["client"]
    shutil.copyfile(args.client, executable)
    config = json.loads((stage / "config.json").read_text(encoding="utf-8"))
    config["model_manager"]["models_directory"] = str(args.installed.resolve() / "models")
    config["server"] = {"host": "127.0.0.1", "port": args.port + 1}
    config["tts"]["provider"] = script["session"]["tts_provider"]
    inference = script["session"].get("inference", False)
    if inference:
        config["model_manager"]["llama_server_path"] = str(args.installed.resolve() / "runtime/llama.cpp/llama-server.exe")
        for offset, service in ((5, "asr"), (6, "translation")):
            provider = config[service]["providers"][config[service]["provider"]]
            provider["url"] = f"http://127.0.0.1:{args.port + offset}/v1/chat/completions"
    config["integrations"]["vrcx"]["enabled"] = False
    config["osc"].update(enabled=False, send_port=args.port + 2, listen_port=args.port + 3)
    write_json(stage / "config.json", config)

    runtime = stage / "runtime"
    runtime.mkdir(exist_ok=True)
    settings = script["session"]["settings"]
    settings["server_url"] = f"ws://127.0.0.1:{args.port + 1}/ws"
    settings["osc_settings"].update(send_port=args.port + 2, listen_port=args.port + 3)
    for name in ("rust-client-settings.json", "app_state.json"):
        write_json(runtime / name, settings)
    # The installed runtime's own manifest remains the single source of library paths.
    marker = args.installed / "runtime/native-runtime.json"
    if marker.exists():
        selection = json.loads(marker.read_text(encoding="utf-8"))
        def absolute(value):
            return str((args.installed / value).resolve()) if value else value
        for key in ("provider_dir", "onnx_core_library", "cuda_bin_dir", "cudnn_bin_dir"):
            if key in selection:
                selection[key] = absolute(selection[key])
        selection["preload_libraries"] = [absolute(p) for p in selection.get("preload_libraries", [])]
        write_json(runtime / marker.name, selection)
    else:
        shutil.copytree(args.bundle / "runtime/onnxruntime", runtime / "onnxruntime")
    for relative in ("resources/bin/mpv-2.dll", "rust-client/resources/bin/mpv-2.dll"):
        source = args.installed / relative
        if source.exists():
            destination = stage / "resources/bin/mpv-2.dll"
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
            break

    processes, logs = [], []
    env = dict(os.environ, XRTRANSLATE_WINDOW_BACKDROP="none", RUST_LOG="info", PYTHONUTF8="1")
    startup = subprocess.STARTUPINFO()
    startup.dwFlags |= subprocess.STARTF_USESHOWWINDOW
    startup.wShowWindow = 0
    def start(name, command):
        log = (args.session / f"{name}.log").open("w", encoding="utf-8")
        logs.append(log)
        process = subprocess.Popen(command, cwd=stage, env=env, stdout=log, stderr=log,
                                   startupinfo=startup, creationflags=subprocess.CREATE_NO_WINDOW)
        processes.append(process)
        return process
    try:
        corpus_url = f"http://127.0.0.1:{args.port + 4}"
        config_args = ["--config", str(stage / "config.json")]
        start("corpus", [str(stage / manifest["entrypoints"]["corpus"]), *config_args, "--listen", f"127.0.0.1:{args.port + 4}"])
        backend = start("backend", [str(stage / manifest["entrypoints"]["backend"]), *config_args, "--corpus-url", corpus_url,
                                    *(["--manage-llama-servers"] if inference else [])])
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + 180
        while True:
            if backend.poll() is not None:
                raise RuntimeError(f"Demo backend exited; see {args.session / 'backend.log'}")
            try:
                with opener.open(f"http://127.0.0.1:{args.port + 1}/healthz", timeout=1) as response:
                    if response.status == 200:
                        break
            except OSError:
                pass
            if time.monotonic() >= deadline:
                raise RuntimeError("Demo backend did not become ready")
            time.sleep(0.25)
        start("app", [str(executable), "--director-port", str(args.port)])
        write_json(args.session / "processes.json", [p.pid for p in processes])
        # The recording client separately waits for the UI's first usable frame.
        record(argparse.Namespace(script=args.script, output=args.output, port=args.port, preview=args.preview))
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True,
                               creationflags=subprocess.CREATE_NO_WINDOW)
                process.wait(timeout=15)
        for log in logs:
            log.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("script", type=Path)
    parser.add_argument("--bundle", type=Path, required=True, help="Clean, unpacked portable release")
    parser.add_argument("--client", type=Path, required=True, help="Client built with the Director capture API")
    parser.add_argument("--installed", type=Path, required=True, help="Installation/repo containing models and native runtime")
    parser.add_argument("--session", type=Path, required=True, help="New scratch directory")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--port", type=int, default=19821)
    parser.add_argument("--preview", type=int)
    run(parser.parse_args())
