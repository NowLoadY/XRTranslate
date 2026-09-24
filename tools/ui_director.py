#!/usr/bin/env python3
"""
XRTranslate UI Director - Remote Automation Client & Screenplay Engine
Zero-dependency, pure Python standard library automation client.

Supports:
1. Python Scripting API (`UIDirector` class)
2. Command-line execution (`python tools/ui_director.py click "Settings"`)
3. Interactive Director REPL console (`python tools/ui_director.py repl`)
4. Script playback for automated demo recording (`python tools/ui_director.py run <script.txt>`)
"""

from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
from typing import Any, Dict, List, Optional, Union

DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 18920


class UIDirector:
    """Client for controlling the XRTranslate UI via the Remote Director protocol."""

    def __init__(self, host: str = DEFAULT_HOST, port: int = DEFAULT_PORT, timeout: float = 5.0):
        self.base_url = f"http://{host}:{port}"
        self.timeout = timeout

    def _send_command(self, cmd: str, args: Any = None, **kwargs) -> Dict[str, Any]:
        payload: Dict[str, Any] = {"cmd": cmd}
        if args is not None:
            payload["args"] = args
        payload.update(kwargs)

        data = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            self.base_url,
            data=data,
            headers={"Content-Type": "application/json"},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=self.timeout) as resp:
                result_bytes = resp.read()
                return json.loads(result_bytes.decode("utf-8"))
        except urllib.error.URLError as e:
            return {
                "success": False,
                "message": f"Connection to UI Director failed at {self.base_url}: {e}",
            }
        except Exception as e:
            return {"success": False, "message": f"Error: {e}"}

    def status(self) -> Dict[str, Any]:
        """Get application and UI director status."""
        return self._send_command("status")

    def get_page(self) -> str:
        """Get the name of the currently active UI page."""
        resp = self._send_command("get_page")
        if resp.get("success") and "data" in resp and "page" in resp["data"]:
            return resp["data"]["page"]
        return ""

    def page(self, page_name: str) -> Dict[str, Any]:
        """Navigate to a target page (e.g. 'Translation', 'Settings', 'AudioStudio', 'PromptStudio', 'onboarding:1')."""
        return self._send_command("page", page_name)

    def list_elements(self, filter_text: Optional[str] = None) -> List[Dict[str, Any]]:
        """List all discoverable UI components on the current page."""
        resp = self._send_command("list", filter=filter_text)
        if resp.get("success") and "data" in resp:
            return resp["data"]
        return []

    def inspect(self, target: str) -> Dict[str, Any]:
        """Inspect detailed information about a widget by label, index (#0), or hex ID."""
        return self._send_command("inspect", target)

    def click(self, target: str) -> Dict[str, Any]:
        """Simulate a click on a button, toggle, or interactive element."""
        return self._send_command("click", target)

    def set_value(self, target: str, value: Union[bool, float, int, str]) -> Dict[str, Any]:
        """Set the value of an input field, toggle, checkbox, slider, or combobox."""
        return self._send_command("set", target=target, value=value)

    def get_value(self, target: str) -> Dict[str, Any]:
        """Get the current value and properties of a UI element."""
        return self._send_command("get", target)

    def wait(self, seconds: float) -> None:
        """Pause execution for a given number of seconds."""
        time.sleep(seconds)

    def wait_element(self, target: str, timeout: float = 10.0, poll_interval: float = 0.2) -> bool:
        """Wait until a specific element appears on screen."""
        start = time.time()
        while time.time() - start < timeout:
            for elem in self.list_elements():
                label = elem.get("label", "")
                id_hex = elem.get("id_hex", "")
                idx_str = f"#{elem.get('index')}"
                if (
                    target.lower() in label.lower()
                    or target == idx_str
                    or target.lower() == id_hex.lower()
                ):
                    return True
            time.sleep(poll_interval)
        return False

    def wait_page(self, page_name: str, timeout: float = 10.0, poll_interval: float = 0.2) -> bool:
        """Wait until a specific page becomes active."""
        start = time.time()
        while time.time() - start < timeout:
            current = self.get_page()
            if page_name.lower() in current.lower():
                return True
            time.sleep(poll_interval)
        return False


def run_screenplay_script(director: UIDirector, script_path: str) -> None:
    """Execute a director script line-by-line."""
    with open(script_path, "r", encoding="utf-8") as f:
        lines = f.readlines()

    for line_no, raw_line in enumerate(lines, 1):
        line = raw_line.strip()
        if not line or line.startswith("#") or line.startswith("//"):
            continue

        print(f"[{line_no}] > {line}")
        parts = line.split(maxsplit=2)
        cmd = parts[0].lower()

        if cmd == "page" and len(parts) >= 2:
            resp = director.page(parts[1])
            print(f"  -> {resp.get('message')}")
        elif cmd == "click" and len(parts) >= 2:
            target = parts[1] if len(parts) == 2 else f"{parts[1]} {parts[2]}"
            # Strip quotes if wrapped
            if (target.startswith('"') and target.endswith('"')) or (
                target.startswith("'") and target.endswith("'")
            ):
                target = target[1:-1]
            resp = director.click(target)
            print(f"  -> {resp.get('message')}")
        elif cmd == "set" and len(parts) >= 3:
            target = parts[1]
            if (target.startswith('"') and target.endswith('"')) or (
                target.startswith("'") and target.endswith("'")
            ):
                target = target[1:-1]
            val_str = parts[2]
            if val_str.lower() == "true":
                val = True
            elif val_str.lower() == "false":
                val = False
            else:
                try:
                    val = float(val_str)
                except ValueError:
                    val = val_str
            resp = director.set_value(target, val)
            print(f"  -> {resp.get('message')}")
        elif cmd in ("wait", "sleep") and len(parts) >= 2:
            try:
                sec = float(parts[1])
                print(f"  -> Waiting {sec}s...")
                time.sleep(sec)
            except ValueError:
                print(f"  -> Invalid wait duration: {parts[1]}")
        elif cmd == "wait_element" and len(parts) >= 2:
            target = parts[1]
            found = director.wait_element(target, timeout=10.0)
            print(f"  -> Element found: {found}")
        elif cmd == "list":
            elems = director.list_elements()
            print(f"  -> {len(elems)} elements found on page:")
            for e in elems:
                print(
                    f"     #{e['index']} [{e['kind']}] \"{e['label']}\" value={e.get('value')} enabled={e.get('enabled')}"
                )
        else:
            print(f"  -> Unknown command: {line}")


def repl(director: UIDirector) -> None:
    """Run an interactive director shell."""
    print("==================================================")
    print(" XRTranslate UI Director - Interactive Console")
    print(" Type 'help' for available commands, 'exit' to quit.")
    print("==================================================")

    while True:
        try:
            line = input("director> ").strip()
        except (EOFError, KeyboardInterrupt):
            print()
            break

        if not line:
            continue
        if line.lower() in ("exit", "quit"):
            break
        if line.lower() == "help":
            print("Commands:")
            print("  status                  - Check UI server status")
            print("  page <name>             - Switch page (Translation, Settings, AudioStudio, PromptStudio, onboarding:1)")
            print("  get_page                - Print current active page")
            print("  list [filter]           - List widgets on current page")
            print("  inspect <target>        - Show widget details by label or #index")
            print("  click <target>          - Click button/widget by label or #index")
            print("  set <target> <value>    - Set value (bool, number, or string)")
            print("  get <target>            - Read value of widget")
            print("  wait <seconds>          - Sleep for given seconds")
            print("  run <script.txt>        - Execute a director script file")
            continue

        if line.lower().startswith("run "):
            path = line[4:].strip()
            run_screenplay_script(director, path)
            continue

        parts = line.split(maxsplit=2)
        cmd = parts[0].lower()
        if cmd == "status":
            print(json.dumps(director.status(), indent=2, ensure_ascii=False))
        elif cmd == "get_page":
            print(f"Current page: {director.get_page()}")
        elif cmd == "page" and len(parts) >= 2:
            print(json.dumps(director.page(parts[1]), indent=2, ensure_ascii=False))
        elif cmd == "list":
            filter_text = parts[1] if len(parts) > 1 else None
            elems = director.list_elements(filter_text)
            print(f"Page '{director.get_page()}' ({len(elems)} elements):")
            for e in elems:
                val = e.get("value")
                val_repr = f" = {val}" if val not in (None, "None", {}) else ""
                print(
                    f"  #{e['index']:<3} [{e['kind']:<10}] \"{e['label']}\"{val_repr} (id: {e['id_hex'][:8]}...)"
                )
        elif cmd == "inspect" and len(parts) >= 2:
            print(json.dumps(director.inspect(parts[1]), indent=2, ensure_ascii=False))
        elif cmd == "click" and len(parts) >= 2:
            target = line[len("click ") :].strip()
            if (target.startswith('"') and target.endswith('"')) or (
                target.startswith("'") and target.endswith("'")
            ):
                target = target[1:-1]
            print(json.dumps(director.click(target), indent=2, ensure_ascii=False))
        elif cmd == "get" and len(parts) >= 2:
            target = line[len("get ") :].strip()
            print(json.dumps(director.get_value(target), indent=2, ensure_ascii=False))
        elif cmd == "set" and len(parts) >= 3:
            target = parts[1]
            if (target.startswith('"') and target.endswith('"')) or (
                target.startswith("'") and target.endswith("'")
            ):
                target = target[1:-1]
            val_str = parts[2]
            if val_str.lower() == "true":
                val = True
            elif val_str.lower() == "false":
                val = False
            else:
                try:
                    val = float(val_str)
                except ValueError:
                    val = val_str
            print(json.dumps(director.set_value(target, val), indent=2, ensure_ascii=False))
        elif cmd == "wait" and len(parts) >= 2:
            time.sleep(float(parts[1]))
            print(f"Waited {parts[1]}s")
        else:
            print(f"Unknown command: {line}. Type 'help' for usage.")


def main() -> None:
    parser = argparse.ArgumentParser(description="XRTranslate UI Director Automation Client")
    parser.add_argument("--host", default=DEFAULT_HOST, help="Director server host")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT, help="Director server port")

    subparsers = parser.add_subparsers(dest="action")

    # repl
    subparsers.add_parser("repl", help="Start interactive Director shell")

    # status
    subparsers.add_parser("status", help="Get UI status")

    # get_page
    subparsers.add_parser("get_page", help="Get active UI page")

    # page
    page_p = subparsers.add_parser("page", help="Navigate to page")
    page_p.add_argument("name", help="Page name (e.g. Translation, Settings, AudioStudio)")

    # list
    list_p = subparsers.add_parser("list", help="List UI components")
    list_p.add_argument("--filter", "-f", help="Filter by label or kind")

    # inspect
    inspect_p = subparsers.add_parser("inspect", help="Inspect element")
    inspect_p.add_argument("target", help="Label, index (#0), or hex ID")

    # click
    click_p = subparsers.add_parser("click", help="Click element")
    click_p.add_argument("target", help="Label, index (#0), or hex ID")

    # set
    set_p = subparsers.add_parser("set", help="Set element value")
    set_p.add_argument("target", help="Target element")
    set_p.add_argument("value", help="Value to set")

    # get
    get_p = subparsers.add_parser("get", help="Get element value")
    get_p.add_argument("target", help="Target element")

    # run script
    run_p = subparsers.add_parser("run", help="Run a screenplay script file")
    run_p.add_argument("script_file", help="Path to script file")

    args = parser.parse_args()

    director = UIDirector(host=args.host, port=args.port)

    if args.action == "repl" or args.action is None:
        repl(director)
    elif args.action == "status":
        print(json.dumps(director.status(), indent=2, ensure_ascii=False))
    elif args.action == "get_page":
        print(f"Page: {director.get_page()}")
    elif args.action == "page":
        print(json.dumps(director.page(args.name), indent=2, ensure_ascii=False))
    elif args.action == "list":
        elems = director.list_elements(args.filter)
        print(f"Page '{director.get_page()}' ({len(elems)} elements):")
        for e in elems:
            val = e.get("value")
            val_repr = f" = {val}" if val not in (None, "None", {}) else ""
            print(
                f"  #{e['index']:<3} [{e['kind']:<10}] \"{e['label']}\"{val_repr} (id: {e['id_hex'][:8]}...)"
            )
    elif args.action == "inspect":
        print(json.dumps(director.inspect(args.target), indent=2, ensure_ascii=False))
    elif args.action == "click":
        print(json.dumps(director.click(args.target), indent=2, ensure_ascii=False))
    elif args.action == "set":
        val_str = args.value
        if val_str.lower() == "true":
            val = True
        elif val_str.lower() == "false":
            val = False
        else:
            try:
                val = float(val_str)
            except ValueError:
                val = val_str
        print(json.dumps(director.set_value(args.target, val), indent=2, ensure_ascii=False))
    elif args.action == "get":
        print(json.dumps(director.get_value(args.target), indent=2, ensure_ascii=False))
    elif args.action == "run":
        run_screenplay_script(director, args.script_file)


if __name__ == "__main__":
    main()
