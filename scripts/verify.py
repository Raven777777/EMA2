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
<p id="para2">second paragraph stays separate</p>
<table id="table">
  <thead><tr><th>key</th><th>value</th></tr></thead>
  <tbody><tr><td>alpha</td><td>1</td></tr><tr><td>beta</td><td>2</td></tr></tbody>
</table>
<ul id="list"><li>one</li><li>two</li></ul>
<pre id="code">const answer = 42;</pre>
<button id="btn" type="button">change</button>
<input id="text" name="q" type="text" placeholder="query">
<input id="num" type="number" name="num">
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

FIXTURE_TALL = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture tall</title>
<style>body{margin:0;font:16px sans-serif}.spacer{height:3000px}</style></head>
<body><h1>Tall</h1><div class="spacer" id="top">top</div>
<button id="tall-btn" type="button">tall button</button>
<div class="spacer" id="bottom">bottom</div></body></html>
"""

FIXTURE_SHADOW = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture shadow</title></head>
<body><h1>Shadow</h1><div id="host"></div><p>light paragraph</p>
<script>
const root = document.getElementById('host').attachShadow({ mode: 'open' });
root.innerHTML = '<p id="shadow-text">shadow paragraph unique-text</p><button>shadow button</button>';
</script></body></html>
"""

FIXTURE_SPA = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture spa</title></head>
<body><h1>SPA</h1><a id="spa" href="/second.html">spa link</a><div id="out">idle</div>
<script>
document.getElementById('spa').addEventListener('click', function (event) {
  event.preventDefault();
  document.getElementById('out').textContent = 'spa clicked';
});
</script></body></html>
"""

FIXTURE_WAIT = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture wait</title></head>
<body><h1>Wait</h1><button id="fetch" type="button">fetch</button>
<a id="go" href="/second.html">go</a><div id="out">idle</div>
<script>
document.getElementById('fetch').addEventListener('click', function () {
  setTimeout(function () {
    fetch('/api').then(function () { document.getElementById('out').textContent = 'fetched'; });
  }, 200);
});
</script></body></html>
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
        elif path == "/tall.html":
            self._send(FIXTURE_TALL.encode())
        elif path == "/shadow.html":
            self._send(FIXTURE_SHADOW.encode())
        elif path == "/spa.html":
            self._send(FIXTURE_SPA.encode())
        elif path == "/wait.html":
            self._send(FIXTURE_WAIT.encode())
        elif path == "/slow":
            time.sleep(3)
            self._send(FIXTURE_HOME.encode())
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


def markdown_link_check() -> bool:
    """本地相对链接必须存在；行内代码里的 `[text](url)` 不算链接。"""
    broken: list[str] = []
    checked = 0
    skip = {".git", "bin", "target", "node_modules"}
    for path in ROOT.rglob("*.md"):
        if skip & set(path.parts):
            continue
        text = re.sub(r"`[^`\n]*`", "", path.read_text(encoding="utf-8", errors="replace"))
        for match in re.finditer(r"\[[^\]]*\]\(([^)\s]+)\)", text):
            target = match.group(1)
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            resolved = (path.parent / target.split("#")[0]).resolve()
            checked += 1
            if not resolved.exists():
                broken.append(f"{path.relative_to(ROOT)} -> {target}")
    print(f"PASS  static: {checked} local markdown links" if not broken else f"FAIL  static: {len(broken)} broken links")
    for item in broken:
        print("      ", item)
    return not broken


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
    if not markdown_link_check():
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

        status = runner.run("status")
        runner.check(
            "status tabs count stays numeric",
            re.search(r"^tabs=\d+ active_tab=", status.stdout or "", re.M) is not None,
            (status.stdout or "") + (status.stderr or ""),
        )

        tab_page = runner.run("tabs", "list", "--budget", "200")
        tab_page_out = (tab_page.stdout or "") + (tab_page.stderr or "")
        runner.check(
            "tabs list supports budget paging",
            tab_page.returncode == 0 and "total=" in tab_page_out and "next=" in tab_page_out,
            tab_page_out[:800],
        )

        runner.expect_error(
            "fill input[type=number] reports normalization",
            runner.run("act", "fill", "--selector", "#num", "abc"),
            "value_rejected",
            4,
        )
        runner.expect_ok(
            "fill input[type=number] writes valid value",
            runner.run("act", "fill", "--selector", "#num", "42"),
            "ok=fill",
        )
        num_value = runner.run("js", "return document.getElementById('num').value")
        runner.check("number field value is 42", "42" in (num_value.stdout or ""), (num_value.stdout or "") + (num_value.stderr or ""))

        # 运行时 SyntaxError 不能被当成编译错误重跑；副作用只能发生一次。
        runner.expect_error(
            "js runtime SyntaxError is not re-executed",
            runner.run("js", "window.__ema2Side = (window.__ema2Side || 0) + 1; JSON.parse('{');"),
            "js_exception",
            4,
        )
        side = runner.run("js", "return window.__ema2Side || 0")
        runner.check("js side effect ran once", "1" in (side.stdout or ""), (side.stdout or "") + (side.stderr or ""))

        wait_fail = runner.run(
            "act", "click", "--selector", "#btn",
            "--wait-for", "url=does-not-exist status=200 timeout=1s",
        )
        wait_output = (wait_fail.stdout or "") + (wait_fail.stderr or "")
        runner.check(
            "waitFor failure keeps completed steps",
            wait_fail.returncode == 4 and "error=response_timeout" in wait_output
            and "step1 ok=click" in wait_output,
            wait_output[:1200],
        )

        md = runner.expect_ok("read md", runner.run("read", "--mode", "md", "--no-main"), "Fixture Home")
        runner.check("md keeps links", re.search(r"\]\(http[^)]+second\.html\)", md) is not None, md[:600])
        runner.check("md keeps table", "| key | value |" in md and "| alpha | 1 |" in md, md[:600])
        runner.check("md keeps list", "- one" in md and "- two" in md, md[:400])
        runner.check(
            "md keeps paragraph breaks",
            re.search("intro text[^\n]*\n\n[^\n]*second paragraph", md) is not None,
            md[:600],
        )

        runner.expect_ok("js value", runner.run("js", "1+1"), "2")
        runner.expect_error("js throw is js_exception", runner.run("js", "throw new Error('boom')"), "js_exception", 4)
        runner.expect_ok("wait selector", runner.run("wait", "selector", "#btn"), "ok=selector=#btn")
        runner.expect_error("read missing selector", runner.run("read", "--selector", "#nope"), "selector_not_found", 4)

        # shadow DOM 正文必须能被 read 看到。
        runner.run("open", f"{base}/shadow.html")
        shadow_read = runner.expect_ok(
            "read shadow DOM text",
            runner.run("read", "--no-main", "--mode", "text"),
            "shadow paragraph unique-text",
        )
        shadow_snapshot = runner.expect_ok(
            "snapshot shadow DOM controls",
            runner.run("snapshot"),
            "shadow button",
        )
        runner.check("shadow read keeps light DOM", "light paragraph" in shadow_read, shadow_read[:600])
        runner.check("shadow snapshot has refs", "refs=" in shadow_snapshot, shadow_snapshot[:600])

        # SPA 链接只改 DOM 不导航：应 settle=done + nav=none，而不是 pending。
        runner.run("open", f"{base}/spa.html")
        spa = runner.run("act", "click", "--selector", "#spa")
        spa_out = (spa.stdout or "") + (spa.stderr or "")
        runner.check(
            "SPA anchor settles without false pending",
            spa.returncode == 0 and "settle=done" in spa_out and "nav=none" in spa_out and "settle=pending" not in spa_out,
            spa_out[:1200],
        )
        spa_text = runner.run("js", "return document.getElementById('out').textContent")
        runner.check("SPA handler ran", "spa clicked" in (spa_text.stdout or ""), (spa_text.stdout or "") + (spa_text.stderr or ""))

        # 多步脚本里的 waitFor 必须在它所在行执行，而不是等所有步骤结束。
        wait_script = tmp / "wait.ema2"
        wait_script.write_text(
            'click selector="#fetch"\nwaitFor url=api status=200 timeout=3s\nclick selector="#go"\n',
            encoding="utf-8",
        )
        runner.run("open", f"{base}/wait.html")
        wait_act = runner.run("act", "--script", str(wait_script))
        wait_act_out = (wait_act.stdout or "") + (wait_act.stderr or "")
        runner.check(
            "multi-step waitFor runs in place",
            wait_act.returncode == 0 and "response=200" in wait_act_out and "url=" in wait_act_out and "second.html" in wait_act_out,
            wait_act_out[:1600],
        )

        # scroll down/up 必须遵守方向和屏数。
        runner.run("open", f"{base}/tall.html")
        runner.run("js", "scrollTo(0, 0)")
        runner.expect_ok("scroll down", runner.run("act", "scroll", "down", "2"), "scroll=")
        down_state = runner.run("js", "return JSON.stringify({y: scrollY, h: innerHeight})")
        try:
            down = json.loads((down_state.stdout or "").strip().splitlines()[-1])
        except (ValueError, IndexError):
            down = {"y": 0, "h": 1}
        runner.check(
            "scroll down moves about two screens",
            down["y"] > down["h"] * 1.2,
            (down_state.stdout or "") + (down_state.stderr or ""),
        )
        runner.expect_ok("scroll up", runner.run("act", "scroll", "up", "1"), "scroll=")
        up_state = runner.run("js", "return JSON.stringify({y: scrollY, h: innerHeight})")
        try:
            up = json.loads((up_state.stdout or "").strip().splitlines()[-1])
        except (ValueError, IndexError):
            up = {"y": down["y"], "h": down["h"]}
        runner.check(
            "scroll up decreases scrollY",
            up["y"] < down["y"],
            (up_state.stdout or "") + (up_state.stderr or ""),
        )

        # open --timeout 必须按 waitTimeout 生效，并明确报告 pending。
        slow_start = time.time()
        slow = runner.run("open", f"{base}/slow", "--timeout", "1", timeout=30)
        slow_elapsed = time.time() - slow_start
        slow_out = (slow.stdout or "") + (slow.stderr or "")
        runner.check(
            "open timeout reports load=pending",
            slow.returncode == 0 and "load=pending" in slow_out and slow_elapsed < 12,
            f"elapsed={slow_elapsed:.1f}s\n{slow_out[:1200]}",
        )

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

        big = tmp / "big.bin"
        big.write_bytes(b"x" * (5 * 1024 * 1024))
        runner.run("open", f"{base}/")
        big_upload = runner.run("files", "upload", str(big).replace("\\", "/"), "--selector", "#file", timeout=120)
        runner.expect_ok("files upload at 5 MiB", big_upload, "uploaded=1")
        big_state = runner.run("js", "const f = document.getElementById('file').files[0]; return f ? f.size : -1")
        runner.check("large upload reaches input", str(5 * 1024 * 1024) in (big_state.stdout or ""), (big_state.stdout or "") + (big_state.stderr or "")[:300])

        batch = runner.run("batch", "read", "--urls", f"{base}/,{base}/second.html", "--concurrency", "2", timeout=120)
        runner.check(
            "batch reads both tabs",
            batch.returncode == 0 and batch.stdout.count("url=") >= 2 and "Second Page" in (batch.stdout or ""),
            (batch.stdout or "") + (batch.stderr or "")[-600:],
        )

        runner.expect_error("unknown act flag", runner.run("act", "click", "--at", "10,20"), "invalid_argument", 1)
        runner.expect_error("non-ref positional", runner.run("act", "click", "not-a-ref"), "invalid_argument", 1)
        runner.expect_error("stale ref", runner.run("act", "click", "e999@d999"), "stale_ref", 4)
        runner.expect_error(
            "act rejects unquoted extra text",
            runner.run("act", "type", "e1", "hello", "world"),
            "invalid_argument",
            1,
        )
        runner.expect_error(
            "batch urls rejects replace",
            runner.run("batch", "open", "--urls", f"{base}/", "--replace"),
            "invalid_argument",
            1,
        )

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
