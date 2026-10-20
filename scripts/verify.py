#!/usr/bin/env python3
"""EMA2 真机验收（Python，标准库，不使用 Node）。

做两件事：

1. 静态契约检查：`extension/background.js` 里每个 `fail('code'` 都必须出现在
   `docs/TOOLS.md` 的错误码表里（文档漂移会让 AI 无法自我纠正）。
2. 真实 Edge 冒烟：起一个本机 fixture HTTP 服务，用真实 `ema2` CLI 跑
   open / snapshot / act / read / js / wait / observe / files / shot / batch，
   以及几个必须报错的负例。

**只在真实、前台可见的 Edge 里跑**：脚本会 `tabs activate` 把窗口提到前台，
并断言 `document.hasFocus()`；不启动无头/后台浏览器，也不使用隔离 profile。

用法：

    python scripts/verify.py            # 需要 Edge 已加载扩展并连接 7744
    python scripts/verify.py --port 7744 --keep-server

退出码：0 全过 / 1 有用例失败 / 2 环境不满足（daemon 或扩展没连）。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PORT = 7744

FIXTURE_HOME = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>ema2 fixture home</title></head>
<body>
<h1>Fixture Home</h1>
<p id="para">intro text with a <a id="link" href="/second.html">second page</a> link</p>
<table id="table">
  <thead><tr><th>key</th><th>value</th></tr></thead>
  <tbody><tr><td>alpha</td><td>1</td></tr><tr><td>beta</td><td>2</td></tr></tbody>
</table>
<ul id="list"><li>one</li><li>two</li></ul>
<pre id="code">const answer = 42;</pre>
<button id="btn" type="button">change</button>
<input id="text" name="q" type="text" placeholder="query">
<input id="file" type="file" name="upload">
<div id="out">idle</div>
<script>
  document.getElementById('btn').addEventListener('click', function () {
    document.getElementById('out').textContent = 'clicked';
    document.title = 'ema2 fixture clicked';
  });
</script>
</body></html>
"""

FIXTURE_SECOND = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>ema2 fixture second</title></head>
<body><h1>Second Page</h1><p id="second-body">second body text</p>
<a id="back" href="/">back home</a></body></html>
"""


class FixtureHandler(BaseHTTPRequestHandler):
    def log_message(self, *_args):  # 静默：验收输出只留 PASS/FAIL
        return

    def _send(self, body: bytes, ctype: str = "text/html; charset=utf-8"):
        self.send_response(200)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):  # noqa: N802 - http.server 约定
        path = self.path.split("?")[0]
        if path in ("/", "/index.html"):
            self._send(FIXTURE_HOME.encode())
        elif path == "/second.html":
            self._send(FIXTURE_SECOND.encode())
        elif path == "/api":
            self._send(json.dumps({"ok": True, "path": path}).encode(), "application/json")
        else:
            self.send_error(404)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def find_binary() -> Path:
    env = os.environ.get("EMA2_BIN")
    if env:
        return Path(env)
    for candidate in (ROOT / "bin" / "ema2.exe", ROOT / "bin" / "ema2"):
        if candidate.exists():
            return candidate
    return Path("ema2")


class Runner:
    def __init__(self, binary: Path, port: int):
        self.binary = binary
        self.port = port
        self.failures: list[str] = []
        self.passes = 0

    def run(self, *args: str, timeout: float = 90.0) -> subprocess.CompletedProcess:
        command = [str(self.binary), "--port", str(self.port), *args]
        return subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )

    def check(self, name: str, condition: bool, detail: str = "") -> bool:
        if condition:
            self.passes += 1
            print(f"PASS  {name}")
            return True
        self.failures.append(f"{name}: {detail}" if detail else name)
        print(f"FAIL  {name}")
        if detail:
            print("      " + detail.replace("\n", "\n      "))
        return False

    def expect_ok(self, name: str, result: subprocess.CompletedProcess, *needles: str) -> str:
        output = (result.stdout or "") + (result.stderr or "")
        ok = result.returncode == 0
        missing = [needle for needle in needles if needle not in output]
        self.check(name, ok and not missing, output.strip()[:1200])
        return output

    def expect_error(
        self,
        name: str,
        result: subprocess.CompletedProcess,
        code: str,
        exit_code: int,
    ) -> str:
        output = (result.stdout or "") + (result.stderr or "")
        ok = result.returncode == exit_code and f"error={code}" in output
        self.check(name, ok, f"exit={result.returncode}\n{output.strip()[:1200]}")
        return output


def static_contract_check() -> bool:
    js = (ROOT / "extension" / "background.js").read_text(encoding="utf-8")
    docs = (ROOT / "docs" / "TOOLS.md").read_text(encoding="utf-8")
    codes = sorted(set(re.findall(r"fail\('([a-z_]+)'", js)))
    missing = [code for code in codes if f"`{code}`" not in docs]
    print(f"PASS  static: {len(codes)} error codes documented" if not missing else "FAIL  static: error codes")
    if missing:
        print("      缺少文档:", ", ".join(missing))
    return not missing


def wait_for_extension(runner: Runner, timeout: float = 40.0) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        result = runner.run("status", timeout=15)
        if "extension=connected" in (result.stdout or ""):
            return True
        time.sleep(1.5)
    return False


def main() -> int:
    parser = argparse.ArgumentParser(description="EMA2 real-Edge verification")
    parser.add_argument("--port", type=int, default=DEFAULT_PORT)
    parser.add_argument("--keep-server", action="store_true", help="结束后保留 daemon")
    parser.add_argument("--skip-real-edge", action="store_true", help="只跑静态契约检查")
    args = parser.parse_args()

    print(f"ema2 verify · root={ROOT} port={args.port}")
    print("real foreground Edge only: no headless / no background browser")
    if not static_contract_check():
        return 1
    if args.skip_real_edge:
        print("skipped real-Edge checks (--skip-real-edge)")
        return 0

    binary = find_binary()
    runner = Runner(binary, args.port)
    status_before = runner.run("status", timeout=15)
    already_running = "relay=running" in (status_before.stdout or "")
    if not already_running:
        started = runner.run("server", timeout=30)
        if started.returncode != 0:
            print("FAIL  cannot start daemon")
            print((started.stdout or "") + (started.stderr or ""))
            return 2
    if not wait_for_extension(runner):
        print("FAIL  extension is not connected")
        print("      打开 edge://extensions 重新加载 EMA2，确认 popup 端口 = %d" % args.port)
        if not already_running and not args.keep_server:
            runner.run("server", "stop", timeout=20)
        return 2

    fixture_port = free_port()
    server = ThreadingHTTPServer(("127.0.0.1", fixture_port), FixtureHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{fixture_port}"
    print(f"fixture: {base}")

    tmp = Path(tempfile.mkdtemp(prefix="ema2-verify-"))
    upload = tmp / "upload.txt"
    upload.write_text("verify upload payload", encoding="utf-8")
    shot_path = tmp / "shot.png"

    try:
        opened = runner.expect_ok("open fixture", runner.run("open", f"{base}/"), "status=200", "net_hook=on", "title=ema2 fixture home")
        tab_match = re.search(r"^id=(\d+)", opened, re.M)
        runner.check("open returns tab id", tab_match is not None, opened)

        # 前台语义：把窗口提到前台，并断言页面真的拿到了焦点。
        if tab_match:
            activated = runner.run("tabs", "activate", tab_match.group(1))
            runner.check(
                "window brought to foreground",
                activated.returncode == 0 and "focused=1" in (activated.stdout or ""),
                (activated.stdout or "") + (activated.stderr or ""),
            )
            focus = runner.run("js", "return document.hasFocus()")
            runner.check(
                "page is focused (foreground)",
                "true" in (focus.stdout or "").lower(),
                "浏览器必须在前台可见状态下运行；本测试禁止无头/后台浏览器 " + (focus.stdout or "") + (focus.stderr or ""),
            )

        snapshot = runner.expect_ok("snapshot refs", runner.run("snapshot"), "button", "textbox")
        runner.check("snapshot has refs", re.search(r"refs=[1-9]", snapshot) is not None, snapshot)

        click = runner.expect_ok("act click button", runner.run("act", "click", "--selector", "#btn"), "settle=done")
        acted = runner.run("js", "return document.getElementById('out').textContent")
        runner.check("click had effect", "clicked" in (acted.stdout or ""), (acted.stdout or "") + (acted.stderr or "")[:400])

        nav = runner.expect_ok("act click link navigates", runner.run("act", "click", "--selector", "#link"), "settle=done")
        runner.check("link changed url", "second.html" in nav, nav)
        runner.run("open", f"{base}/")

        md = runner.expect_ok("read md", runner.run("read", "--mode", "md", "--no-main"), "Fixture Home")
        runner.check("md keeps links", re.search(r"\]\(http[^)]+second\.html\)", md) is not None, md[:600])
        runner.check("md keeps table", "| key | value |" in md and "| alpha | 1 |" in md, md[:600])
        runner.check("md keeps list", "- one" in md and "- two" in md, md[:400])

        runner.expect_ok("js value", runner.run("js", "1+1"), "2")
        runner.expect_error("js throw is js_exception", runner.run("js", "throw new Error('boom')"), "js_exception", 4)
        runner.expect_ok("wait selector", runner.run("wait", "selector", "#btn"), "ok=selector=#btn")
        runner.expect_error("read missing selector", runner.run("read", "--selector", "#nope"), "selector_not_found", 4)

        runner.run("js", "console.error('verify-boom')")
        console = runner.run("observe", "console", "--level", "error")
        runner.check("observe console", "verify-boom" in (console.stdout or ""), (console.stdout or "") + (console.stderr or "")[:400])

        runner.run("js", "fetch('/api').then(r => r.detail)")
        time.sleep(0.8)
        network = runner.run("observe", "network", "--filter", "/api")
        runner.check("observe network", "200" in (network.stdout or "") and "/api" in (network.stdout or ""), (network.stdout or "")[:600])

        shot = runner.run("shot", "--out", str(shot_path))
        runner.check(
            "shot writes a file",
            shot.returncode == 0 and shot_path.exists() and shot_path.stat().st_size > 1000,
            (shot.stdout or "") + (shot.stderr or ""),
        )

        runner.run("open", f"{base}/")
        uploaded = runner.run("files", "upload", str(upload).replace("\\", "/"), "--selector", "#file")
        runner.expect_ok("files upload (DataTransfer)", uploaded, "uploaded=1")
        file_state = runner.run("js", "const f = document.getElementById('file').files[0]; return f ? f.name + ':' + f.size : 'none'")
        runner.check("upload reaches input", "upload.txt" in (file_state.stdout or ""), (file_state.stdout or "") + (file_state.stderr or "")[:300])

        batch = runner.run("batch", "read", "--urls", f"{base}/,{base}/second.html", "--concurrency", "2", timeout=120)
        runner.check(
            "batch reads both tabs",
            batch.returncode == 0 and batch.stdout.count("url=") >= 2 and "Second Page" in (batch.stdout or ""),
            (batch.stdout or "") + (batch.stderr or "")[-600:],
        )

        runner.expect_error("unknown act flag", runner.run("act", "click", "--at", "10,20"), "invalid_argument", 1)
        runner.expect_error("non-ref positional", runner.run("act", "click", "not-a-ref"), "invalid_argument", 1)
        runner.expect_error("stale ref", runner.run("act", "click", "e999@d999"), "stale_ref", 4)

        closed = runner.run("tabs", "close", "--session")
        runner.check("tabs close --session", closed.returncode == 0 and "closed=" in (closed.stdout or ""), (closed.stdout or "")[:300])
    finally:
        server.shutdown()
        if not already_running and not args.keep_server:
            runner.run("server", "stop", timeout=20)

    print()
    if runner.failures:
        print(f"FAILED {len(runner.failures)} / {runner.passes + len(runner.failures)}")
        for item in runner.failures:
            print("  -", item.splitlines()[0])
        return 1
    print(f"OK {runner.passes} checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
