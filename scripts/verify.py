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
import urllib.parse
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
<ol id="ordered" start="3"><li>three</li><li value="7">seven</li><li>eight</li></ol>
<pre id="code">const answer = 42;</pre>
<button id="btn" type="button">change</button>
<input id="text" name="q" type="text" placeholder="query">
<input id="num" type="number" name="num">
<input id="file" type="file" name="upload">
<textarea id="secret" name="secret">verify-secret-value</textarea>
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

FIXTURE_BUGS = """<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>ema2 fixture bugs</title>
<style>#ov{position:fixed;inset:0;background:rgba(0,0,0,.55);z-index:99;display:none}</style>
</head><body>
<h1>Bug fixture</h1>
<input id="a" value="abc"><input id="b">
<input id="hidden-search" name="q" type="search" value="hidden-value" style="display:none">
<input id="named-search" name="q" type="search" placeholder="global search">
<input id="num" type="number" value="42">
<select id="sel"><option value="v1">one</option><option value="v2" selected>two</option></select>
<textarea id="area">secret</textarea>
<a id="abs" href="https://example.org/x?a=1#f">abs</a>
<a id="btnlink" role="button" href="/second.html">button-link</a>
<a id="hashlink" href="#">hash link</a>
<a id="under" href="/second.html">under overlay</a>
<button id="neg" type="button" style="position:absolute;left:-30px;top:20px;width:20px;height:20px">neg</button>
<button id="alert" type="button" onclick="alert('boom')">alert</button>
<button id="print" type="button" onclick="window.print()">print</button>
<button id="pop" type="button" onclick="window.open('/second.html')">popup</button>
<form id="reqform" action="/second.html" method="get">
  <input id="req" name="req" required>
  <button id="req-submit" type="submit">submit required</button>
</form>
<div id="ov"></div>
<script>
function ema2ReportGuard(stage) {
  const guard = window.__ema2_guard;
  const params = new URLSearchParams({
    stage: stage,
    guard: guard ? '1' : '0',
    armed: guard && guard.armed ? '1' : '0',
    native: guard && guard.original && guard.original.alert === window.alert ? '1' : '0',
    at: String(Date.now())
  });
  fetch('/guard-report?' + params.toString()).catch(function () {});
}
(function () {
  // disarm 由后台在命令释放时调用；事件驱动上报，避免永久轮询污染网络检查。
  const guard = window.__ema2_guard;
  if (!guard || typeof guard.disarm !== 'function') return;
  const disarm = guard.disarm;
  guard.disarm = function () {
    const result = disarm.apply(this, arguments);
    ema2ReportGuard('disarm');
    return result;
  };
})();
</script>
</body></html>
"""

FIXTURE_IDLE = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture idle</title></head>
<body><h1>Idle</h1>
<script>
window.addEventListener('load', function () {
  const guard = window.__ema2_guard;
  const params = new URLSearchParams({
    stage: 'idle',
    guard: guard ? '1' : '0',
    armed: guard && guard.armed ? '1' : '0',
    at: String(Date.now())
  });
  fetch('/guard-report?' + params.toString()).catch(function () {});
});
</script></body></html>
"""


FIXTURE_VISIBLE = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture visible</title></head>
<body>
<nav id="nav">nav-only-text <a href="/second.html">nav-link</a></nav>
<main id="visible-main">
<h1>Visible Main</h1>
<p>main paragraph one has enough text to be selected as the main container. It mentions visible rendering, raw text, and the difference between cleaned article extraction and the browser innerText result.</p>
<p>main paragraph two adds another sentence so the main container clearly dominates the page text.</p>
<input id="visible-input" value="visible-value">
<div style="display:none">hidden-text</div>
<button id="visible-btn" type="button">visible button</button>
</main>
</body></html>
"""


FIXTURE_CONTENTS = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture contents</title></head>
<body>
<h1>Contents Fixture</h1>
<div class="content" style="display:contents">
<p>contents paragraph alpha is wrapped by a display:contents container and must still be readable.</p>
<p>contents paragraph beta keeps extra filler so the block reaches real article length.</p>
</div>
</body></html>
"""

FIXTURE_LANDMARK = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture landmark</title></head>
<body>
<header role="banner"><div>banner strip text skipzone</div></header>
<article>
<header><h1>Landmark Article Title</h1><p>article header lead text stays inside the article shell</p></header>
<p>article body paragraph with enough words to dominate the page text budget.</p>
</article>
<footer role="contentinfo">contentinfo strip text skipzone</footer>
</body></html>
"""

FIXTURE_BR = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture br</title></head>
<body><h1>Br Fixture</h1><p id="brp">line alpha<br>line beta</p></body></html>
"""

FIXTURE_BADLINK = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture badlink</title></head>
<body><h1>Badlink Fixture</h1>
<p><a id="jslink" href="/second.html"><script>var leakme = 1;</script></a>tail text stays</p>
<p><a id="fraglink" href="#anchorname"></a><a id="imglink" href="/second.html"><img src="/big.bin" alt="Fix Logo"></a></p>
</body></html>
"""

FIXTURE_BUTTONS = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture buttons</title></head>
<body><h1>Buttons Fixture</h1>
<span class="sr" style="position:absolute;width:1px;height:1px;padding:0;margin:-1px;border:0;clip:rect(0 0 0 0);overflow:hidden">sr only label uniqbeta</span>
<button id="save" type="button">Save Now uniqbeta</button>
<div id="rolev" role="button">Share Prev uniqbeta</div>
<p>ordinary body paragraph uniqbeta</p>
</body></html>
"""

FIXTURE_EMPTY = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture empty</title></head>
<body></body></html>
"""

FIXTURE_LOGIN = """<!doctype html>
<html><head><meta charset="utf-8"><title>Sign in to ExampleCorp</title></head>
<body>
<h1>Sign in</h1>
<form id="login">
<input id="user" name="user" type="text" autocomplete="username">
<input id="pass" name="pass" type="password" autocomplete="current-password">
<button type="submit">Sign in</button>
</form>
</body></html>
"""

FIXTURE_CHALLENGE = """<!doctype html>
<html><head><meta charset="utf-8"><title>Just a moment...</title></head>
<body>
<div id="challenge-form">Confirm you are human to continue</div>
</body></html>
"""

FIXTURE_BLOCKS = """<!doctype html>
<html><head><meta charset="utf-8"><title>ema2 fixture blocks</title></head>
<body><h1>Blocks Fixture</h1>
<h5>H5 Heading Random Five</h5>
<h6>H6 Heading Random Six</h6>
<center>centered text Random Mode</center>
<dl><dt>Term Alpha</dt><dd>Definition Alpha carries the meaning</dd><dt>Term Beta</dt><dd>Definition Beta adds detail</dd></dl>
<details open><summary>Details Summary Label Opened</summary><p>collapsed details body stays</p></details>
<details><summary>Details Summary Label Closed</summary><p>hidden details body is excluded</p></details>
<my-block><p>custom element body stays visible</p></my-block>
<table id="outer"><tbody><tr><td>outer cell one</td></tr><tr><td>outer cell two<table><tbody><tr><td>inner cell alone</td></tr></tbody></table></td></tr></tbody></table>
</body></html>
"""


def fixture_pdf() -> bytes:
    """最小合法 PDF（fixture 用）；offset 按 PDF 规范精确计算。"""
    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 300] >>",
    ]
    out = bytearray(b"%PDF-1.4\n")
    offsets = []
    for index, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{index} 0 obj\n".encode() + body + b"\nendobj\n"
    xref_at = len(out)
    out += b"xref\n0 4\n"
    out += b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode()
    out += b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n" + str(xref_at).encode() + b"\n%%EOF\n"
    return bytes(out)


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
        parsed = urllib.parse.urlsplit(self.path)
        path = parsed.path
        if path == "/guard-report":
            query = urllib.parse.parse_qs(parsed.query)
            reports = getattr(self.server, "guard_reports", None)
            if reports is not None:
                reports.append({key: values[-1] for key, values in query.items()})
            self._send(b"ok", "text/plain; charset=utf-8")
            return
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
        elif path == "/bugs.html":
            self._send(FIXTURE_BUGS.encode())
        elif path == "/visible.html":
            self._send(FIXTURE_VISIBLE.encode())
        elif path == "/contents.html":
            self._send(FIXTURE_CONTENTS.encode())
        elif path == "/landmark.html":
            self._send(FIXTURE_LANDMARK.encode())
        elif path == "/br.html":
            self._send(FIXTURE_BR.encode())
        elif path == "/badlink.html":
            self._send(FIXTURE_BADLINK.encode())
        elif path == "/buttons.html":
            self._send(FIXTURE_BUTTONS.encode())
        elif path == "/empty.html":
            self._send(FIXTURE_EMPTY.encode())
        elif path == "/login.html":
            self._send(FIXTURE_LOGIN.encode())
        elif path == "/challenge.html":
            self._send(FIXTURE_CHALLENGE.encode())
        elif path == "/blocks.html":
            self._send(FIXTURE_BLOCKS.encode())
        elif path == "/report.pdf":
            self._send(fixture_pdf(), "application/pdf")
        elif path == "/idle.html":
            self._send(FIXTURE_IDLE.encode())
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
    codes = sorted(set(re.findall(r"fail\('([a-z_]+)'", js)) | set(re.findall(r"code: '([a-z_]+)'", js)))
    missing = [code for code in codes if f"`{code}`" not in docs]
    print(f"PASS  static: {len(codes)} error codes documented" if not missing else "FAIL  static: error codes")
    if missing:
        print("      缺少文档:", ", ".join(missing))

    agents = (ROOT / "rust" / "assets" / "AGENTS.md").read_text(encoding="utf-8")
    agents_ok = "docs/TOOLS.md" not in agents and "ema2 act --help" in agents
    print("PASS  static: AGENTS.md is self-contained" if agents_ok else "FAIL  static: AGENTS.md points outside the package")
    if not agents_ok:
        print("      AGENTS.md must point to `ema2 act --help` and must not mention docs/TOOLS.md")
    manifest = json.loads((ROOT / "extension" / "manifest.json").read_text(encoding="utf-8"))
    idle_ok = "content_scripts" not in manifest and (ROOT / "extension" / "page_guard.js").exists()
    print("PASS  static: page guard is command-scoped" if idle_ok
          else "FAIL  static: manifest still injects page_guard globally")

    paths_rs = (ROOT / "rust" / "src" / "paths.rs").read_text(encoding="utf-8")
    version_match = re.search(r'PRODUCT_VERSION: &str = "([^"]+)"', paths_rs)
    product_version = version_match.group(1) if version_match else ""
    version_ok = bool(product_version) and manifest.get("version") == product_version
    print(
        f"PASS  static: extension version matches PRODUCT_VERSION ({product_version})" if version_ok
        else f"FAIL  static: extension version {manifest.get('version')} != PRODUCT_VERSION {product_version or '<missing>'}"
    )

    min_chrome = str(manifest.get("minimum_chrome_version", ""))
    chrome_ok = min_chrome.isdigit() and int(min_chrome) >= 111
    print("PASS  static: minimum_chrome_version >= 111" if chrome_ok
          else "FAIL  static: minimum_chrome_version must be >= 111 for registerContentScripts world=MAIN")
    return not missing and agents_ok and idle_ok and version_ok and chrome_ok


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
    server.guard_reports = []
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{fixture_port}"
    print(f"fixture: {base}")

    tmp = Path(tempfile.mkdtemp(prefix="ema2-verify-"))
    upload = tmp / "upload.txt"
    upload.write_text("verify upload payload", encoding="utf-8")
    shot_path = tmp / "shot.png"
    annotate_path = tmp / "annotate.png"

    try:
        version = runner.run("--version")
        runner.expect_ok("binary exposes product version", version, "2.4.2.0")

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
        runner.check(
            "snapshot does not leak textarea value",
            "verify-secret-value" not in snapshot,
            snapshot[:1200],
        )

        click = runner.expect_ok("act click button", runner.run("act", "click", "--selector", "#btn"), "settle=done")
        acted = runner.run("js", "return document.getElementById('out').textContent")
        runner.check("click had effect", "clicked" in (acted.stdout or ""), (acted.stdout or "") + (acted.stderr or "")[:400])

        nav = runner.expect_ok("act click link navigates", runner.run("act", "click", "--selector", "#link"), "settle=done")
        runner.check("link changed url", "second.html" in nav, nav)
        runner.run("open", f"{base}/")

        # 同 URL 的 --replace 必须等 reload 后的新文档，不能被旧文档的 complete 骗过。
        origin_before = runner.run("js", "return String(performance.timeOrigin)")
        replaced = runner.run("open", f"{base}/", "--replace")
        origin_after = runner.run("js", "return String(performance.timeOrigin)")
        runner.check(
            "open same URL --replace reloads the document",
            replaced.returncode == 0 and "status=200" in (replaced.stdout or "")
            and origin_before.stdout.strip() != origin_after.stdout.strip(),
            (replaced.stdout or "") + (replaced.stderr or "")
            + f" origin_before={origin_before.stdout.strip()} origin_after={origin_after.stdout.strip()}",
        )

        status = runner.run("status")
        runner.check(
            "status tabs count stays numeric",
            re.search(r"^tabs=\d+ active_tab=", status.stdout or "", re.M) is not None,
            (status.stdout or "") + (status.stderr or ""),
        )

        # 普通浏览不注入 guard：tabs new 不是会等待页面的工具命令。
        server.guard_reports.clear()
        idle_tab = runner.run("tabs", "new", f"{base}/idle.html")
        idle_match = re.search(r"^id=(\d+)", (idle_tab.stdout or ""), re.M)
        idle_deadline = time.time() + 5
        while time.time() < idle_deadline and not server.guard_reports:
            time.sleep(0.1)
        idle_report = server.guard_reports[-1] if server.guard_reports else {}
        runner.check(
            "idle browsing has no EMA2 page guard",
            idle_tab.returncode == 0 and idle_report.get("guard") == "0",
            f"tabs={idle_tab.stdout or ''}{idle_tab.stderr or ''} report={idle_report}",
        )
        if idle_match:
            runner.run("tabs", "close", idle_match.group(1))
        # 前面的链接点击已把最初标签导航到 second.html；显式恢复 fixture home。
        runner.run("open", f"{base}/", "--replace")

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

        neg = runner.run("act", "type", "--selector", "#text", "--", "-5")
        runner.check(
            "act type accepts hyphen text after --",
            neg.returncode == 0 and "ok=type" in (neg.stdout or "") + (neg.stderr or ""),
            (neg.stdout or "") + (neg.stderr or ""),
        )
        neg_value = runner.run("js", "return document.getElementById('text').value")
        runner.check("hyphen text reached input", "-5" in (neg_value.stdout or ""), (neg_value.stdout or "") + (neg_value.stderr or ""))

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
        runner.check(
            "read header carries http and content_type",
            "http=200" in md and "content_type=text/html" in md,
            md[:800],
        )
        runner.check("md keeps links", re.search(r"\]\(http[^)]+second\.html\)", md) is not None, md[:600])
        runner.check("md keeps table", "| key | value |" in md and "| alpha | 1 |" in md, md[:600])
        runner.check("md keeps list", "- one" in md and "- two" in md, md[:400])
        runner.check(
            "md keeps ordered-list numbering",
            "3. three" in md and "7. seven" in md and "8. eight" in md,
            md[:800],
        )
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

        help_act = runner.expect_ok("act --help is the script manual", runner.run("act", "--help"), "脚本规则", "ema2 act --help")
        runner.check(
            "act --help names unsupported CLI actions",
            "不能写 open/read/snapshot/wait/js/tabs" in help_act and "waitFor time 1s" in help_act,
            help_act[:1600],
        )
        runner.check(
            "act --help documents explicit tab targeting",
            "ema2 act click e12 --tab 42" in help_act
            and "ema2 act --script flow.ema2 --tab 42" in help_act,
            help_act[:1800],
        )
        runner.expect_ok("top-level help points to act help", runner.run("--help"), "ema2 act --help")
        runner.expect_ok(
            "read --help lists visible mode and budget defaults",
            runner.run("read", "--help"), "visible", "默认 4000", "200..200000",
        )
        runner.expect_ok(
            "observe --help exposes console default",
            runner.run("observe", "--help"), "默认 error",
        )
        runner.expect_ok(
            "batch --help exposes reuse session",
            runner.run("batch", "--help"), "--reuse session",
        )

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
        console_out = (console.stdout or "") + (console.stderr or "")
        runner.check(
            "observe console",
            "verify-boom" in console_out and "level=error" in console_out,
            console_out[:400],
        )
        console_next = re.search(r"^total=\d+ shown=\d+ next=(\d+|none)", console_out, re.M)
        runner.check("console observe reports next", console_next is not None, console_out[:400])

        runner.run("js", "fetch('/api').then(r => r.detail)")
        time.sleep(0.8)
        runner.run("observe", "network", "--filter", "/api", "--since", "0")
        if console_next:
            console_again = runner.run("observe", "console", "--level", "error", "--since", console_next.group(1))
            console_again_out = (console_again.stdout or "") + (console_again.stderr or "")
            runner.check(
                "console since is not reset by a network observe",
                console_again.returncode == 0 and "reset=1" not in console_again_out,
                console_again_out[:600],
            )
        time.sleep(0.8)
        network = runner.run("observe", "network", "--filter", "/api")
        runner.check("observe network", "200" in (network.stdout or "") and "/api" in (network.stdout or ""), (network.stdout or "")[:600])

        shot = runner.run("shot", "--out", str(shot_path))
        runner.check(
            "shot writes a file",
            shot.returncode == 0 and shot_path.exists() and shot_path.stat().st_size > 1000,
            (shot.stdout or "") + (shot.stderr or ""),
        )
        normalized_path = tmp / "nested" / ".." / "normalized.png"
        normalized_shot = runner.run("shot", "--out", str(normalized_path))
        normalized_out = (normalized_shot.stdout or "") + (normalized_shot.stderr or "")
        runner.check(
            "shot --out normalizes . and .. without resolving symlinks",
            normalized_shot.returncode == 0 and (tmp / "normalized.png").exists()
            and "..\\" not in normalized_out and "../" not in normalized_out,
            normalized_out,
        )

        time.sleep(1.2)
        annotate = runner.run("shot", "--annotate", "--out", str(annotate_path))
        annotate_out = (annotate.stdout or "") + (annotate.stderr or "")
        mark_refs = re.findall(r"^\[(e\d+@d\d+)\]", annotate_out, re.M)
        mark_coords = re.findall(r"@(-?\d+),(-?\d+)", annotate_out)
        runner.check(
            "shot --annotate keeps refs unique and coords non-negative",
            annotate.returncode == 0
            and annotate_path.exists()
            and len(mark_refs) == len(set(mark_refs))
            and all(int(x) >= 0 and int(y) >= 0 for x, y in mark_coords),
            annotate_out[:1600],
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
        reused_batch = runner.run(
            "batch", "read", "--urls", f"{base}/",
            "--reuse", "session", "--concurrency", "1", timeout=120,
        )
        reused_out = (reused_batch.stdout or "") + (reused_batch.stderr or "")
        runner.check(
            "batch --reuse session reuses a session tab",
            reused_batch.returncode == 0 and "reused=1" in reused_out
            and "Fixture Home" in reused_out,
            reused_out[-1000:],
        )

        # ---- read --mode visible ----
        runner.run("open", f"{base}/visible.html")
        visible_main = runner.expect_ok(
            "read visible follows the default main root",
            runner.run("read", "--mode", "visible"),
            "Visible Main",
            "main paragraph one",
        )
        runner.check(
            "visible main excludes nav",
            "nav-only-text" not in visible_main,
            visible_main[:800],
        )
        visible_body = runner.expect_ok(
            "read visible --no-main reads the whole rendered page",
            runner.run("read", "--mode", "visible", "--no-main"),
            "nav-only-text",
            "Visible Main",
            "visible button",
        )
        runner.check(
            "visible body excludes display:none text",
            "hidden-text" not in visible_body,
            visible_body[:1200],
        )
        visible_nav = runner.expect_ok(
            "read visible --selector scopes to the subtree",
            runner.run("read", "--mode", "visible", "--selector", "#nav"),
            "nav-only-text",
        )
        runner.check(
            "visible selector excludes main",
            "Visible Main" not in visible_nav,
            visible_nav[:600],
        )
        visible_input = runner.expect_ok(
            "read visible returns control value",
            runner.run("read", "--mode", "visible", "--selector", "#visible-input"),
            "visible-value",
        )
        clean_text = runner.run("read", "--mode", "text")
        clean_out = (clean_text.stdout or "") + (clean_text.stderr or "")
        runner.check(
            "clean text still excludes nav",
            clean_text.returncode == 0 and "Visible Main" in clean_out and "nav-only-text" not in clean_out,
            clean_out[:800],
        )

        # ---- P0 read：可见性 / 地标 / 块模型 / 链接 / 噪声 / 页面状态 fixtures ----
        runner.run("open", f"{base}/contents.html")
        contents_read = runner.run("read", "--no-main", "--mode", "text")
        contents_out = (contents_read.stdout or "") + (contents_read.stderr or "")
        runner.check(
            "read keeps display:contents subtrees (--no-main)",
            contents_read.returncode == 0
            and "contents paragraph alpha" in contents_out
            and "contents paragraph beta" in contents_out,
            contents_out[:1200],
        )
        contents_main = runner.run("read", "--mode", "md")
        runner.check(
            "read default main also keeps display:contents",
            contents_main.returncode == 0 and "contents paragraph alpha" in (contents_main.stdout or ""),
            (contents_main.stdout or "")[:1200],
        )

        runner.run("open", f"{base}/landmark.html")
        landmark_md = runner.run("read", "--mode", "md")
        landmark_out = (landmark_md.stdout or "") + (landmark_md.stderr or "")
        runner.check(
            "read keeps article header and skips page banner/contentinfo",
            landmark_md.returncode == 0
            and "Landmark Article Title" in landmark_out
            and "article header lead text" in landmark_out
            and "skipzone" not in landmark_out,
            landmark_out[:1200],
        )

        runner.run("open", f"{base}/br.html")
        br_md = runner.run("read", "--mode", "md", "--no-main")
        runner.check(
            "md renders <p>a<br>b</p> as two lines",
            re.search(r"line alpha\nline beta", br_md.stdout or "") is not None,
            ((br_md.stdout or "") + (br_md.stderr or ""))[:600],
        )

        runner.run("open", f"{base}/badlink.html")
        badlink_md = runner.run("read", "--mode", "md", "--no-main")
        badlink_out = (badlink_md.stdout or "") + (badlink_md.stderr or "")
        runner.check(
            "read never leaks inline script text from links",
            "leakme" not in badlink_out and badlink_out.count("var ") == 0,
            badlink_out[:1000],
        )
        runner.check(
            "empty-text links are dropped without bare urls",
            "#anchorname" not in badlink_out,
            badlink_out[:1000],
        )
        runner.check(
            "image links use the img alt as their label",
            "[Fix Logo](http" in badlink_out and "tail text stays" in badlink_out,
            badlink_out[:1000],
        )

        runner.run("open", f"{base}/buttons.html")
        buttons_md = runner.run("read", "--mode", "md", "--no-main")
        buttons_out = (buttons_md.stdout or "") + (buttons_md.stderr or "")
        runner.check(
            "read skips sr-only 1x1 spans and buttons",
            "sr only label uniqbeta" not in buttons_out
            and "Save Now uniqbeta" not in buttons_out
            and "Share Prev uniqbeta" not in buttons_out,
            buttons_out[:1200],
        )
        runner.check(
            "read keeps the ordinary paragraph",
            "ordinary body paragraph uniqbeta" in (buttons_md.stdout or ""),
            buttons_out[:1200],
        )
        buttons_snapshot = runner.run("snapshot")
        runner.check(
            "snapshot still exposes buttons that read skips",
            "Save Now uniqbeta" in (buttons_snapshot.stdout or ""),
            (buttons_snapshot.stdout or "")[:1200],
        )

        runner.run("open", f"{base}/blocks.html")
        blocks_md = runner.run("read", "--mode", "md", "--no-main")
        blocks_out = (blocks_md.stdout or "") + (blocks_md.stderr or "")
        runner.check(
            "read keeps h5/h6 headings and center blocks",
            "##### H5 Heading Random Five" in blocks_out
            and "###### H6 Heading Random Six" in blocks_out
            and "centered text Random Mode" in blocks_out,
            blocks_out[:1600],
        )
        runner.check(
            "read splits dt/dd and details blocks",
            "Term Alpha" in blocks_out
            and "Definition Alpha carries the meaning" in blocks_out
            and "Details Summary Label Opened" in blocks_out
            and "collapsed details body stays" in blocks_out
            and "custom element body stays visible" in blocks_out
            and "hidden details body is excluded" not in blocks_out,
            blocks_out[:1600],
        )
        runner.check(
            "read outer table ignores nested table rows",
            "outer cell one" in blocks_out
            and "outer cell two" in blocks_out
            and blocks_out.count("inner cell") == 1,
            blocks_out[:1600],
        )

        runner.run("open", f"{base}/empty.html")
        runner.expect_error(
            "read empty page reports empty_page",
            runner.run("read", "--mode", "md"), "empty_page", 4,
        )
        visible_empty = runner.run("read", "--mode", "visible")
        runner.check(
            "read visible empty keeps its hint",
            visible_empty.returncode == 0 and "visible text is empty" in (visible_empty.stdout or ""),
            ((visible_empty.stdout or "") + (visible_empty.stderr or ""))[:800],
        )

        runner.run("open", f"{base}/login.html")
        login_read = runner.run("read", "--mode", "md")
        login_out = (login_read.stdout or "") + (login_read.stderr or "")
        runner.check(
            "read reports login walls as state, not error",
            login_read.returncode == 0
            and "state=login_required" in login_out
            and "http=200" in login_out
            and "snapshot" in login_out,
            login_out[:1000],
        )

        runner.run("open", f"{base}/challenge.html")
        challenge_read = runner.run("read", "--mode", "md")
        challenge_out = (challenge_read.stdout or "") + (challenge_read.stderr or "")
        runner.check(
            "read reports challenge pages as state, not error",
            challenge_read.returncode == 0
            and "state=challenge" in challenge_out
            and "re-run read" in challenge_out,
            challenge_out[:1000],
        )

        notfound_open = runner.run("open", f"{base}/missing-page-404.html")
        notfound_out = (notfound_open.stdout or "") + (notfound_open.stderr or "")
        runner.check(
            "open 404 still succeeds and reports status=404",
            notfound_open.returncode == 0 and "status=404" in notfound_out,
            notfound_out[:800],
        )
        runner.expect_error(
            "read 404 page reports page_error",
            runner.run("read", "--mode", "md"), "page_error", 4,
        )

        pdf_open = runner.run("open", f"{base}/report.pdf")
        pdf_match = re.search(r"^id=(\d+)", (pdf_open.stdout or "") + (pdf_open.stderr or ""), re.M)
        runner.check(
            "open pdf reaches the viewer",
            pdf_open.returncode == 0 and pdf_match is not None,
            ((pdf_open.stdout or "") + (pdf_open.stderr or ""))[:800],
        )
        pdf_read = runner.run("read", "--mode", "md", *( ["--tab", pdf_match.group(1)] if pdf_match else [] ))
        pdf_out = (pdf_read.stdout or "") + (pdf_read.stderr or "")
        runner.check(
            "read on pdf reports pdf_page",
            pdf_read.returncode == 4 and "error=pdf_page" in pdf_out,
            pdf_out[:1200],
        )

        # ---- bin/BUGREPORT.md regressions ----
        bug_open = runner.run("open", f"{base}/bugs.html", "--replace", "--timeout", "5")
        bug_out = (bug_open.stdout or "") + (bug_open.stderr or "")
        bug_match = re.search(r"^id=(\d+)", bug_out, re.M)
        runner.check(
            "bug fixture opens",
            bug_open.returncode == 0 and bug_match is not None,
            bug_out[:1200],
        )
        bug_tab = bug_match.group(1) if bug_match else None

        net = runner.run("observe", "network", "--filter", "bugs.html")
        net_out = (net.stdout or "") + (net.stderr or "")
        runner.check(
            "page-load request has real status",
            net.returncode == 0 and "bugs.html" in net_out and " 200 " in net_out and "-1" not in net_out,
            net_out[:800],
        )

        links = runner.run("snapshot", "--filter", "links")
        links_out = (links.stdout or "") + (links.stderr or "")
        runner.check(
            "snapshot href keeps scheme/query/fragment",
            links.returncode == 0 and "href=https://example.org/x?a=1#f" in links_out
            and "href=http://127.0.0.1:" in links_out,
            links_out[:900],
        )
        runner.check(
            "snapshot --filter links never labels anchors as buttons",
            "link \"button-link\"" in links_out
            and "button \"button-link\"" not in links_out
            and "hash link" not in links_out,
            links_out[:900],
        )

        form = runner.run("snapshot", "--filter", "form")
        form_out = (form.stdout or "") + (form.stderr or "")
        runner.check(
            "snapshot --filter form exposes values (not password)",
            form.returncode == 0 and "value=secret" in form_out and "value=v2" in form_out,
            form_out[:1200],
        )

        runner.expect_ok("read textarea value", runner.run("read", "--selector", "#area", "--mode", "text"), "secret")

        named_search = runner.run("act", "fill", "--target", "searchbox:q", "named-search-value")
        named_search_out = (named_search.stdout or "") + (named_search.stderr or "")
        named_search_state = runner.run(
            "js",
            "return [document.getElementById('named-search').value,"
            " document.getElementById('hidden-search').value].join('|')",
        )
        runner.check(
            "role target falls back to visible HTML name",
            named_search.returncode == 0 and "ok=fill" in named_search_out
            and "named-search-value|hidden-value" in (named_search_state.stdout or ""),
            named_search_out + (named_search_state.stdout or "") + (named_search_state.stderr or ""),
        )

        server.guard_reports.clear()
        guard_cutoff = int(time.time() * 1000)
        alert = runner.run("act", "click", "--selector", "#alert")
        alert_out = (alert.stdout or "") + (alert.stderr or "")
        runner.check(
            "alert is recorded and dismissed",
            alert.returncode == 0 and "dialog_opened=alert" in alert_out
            and alert_out.count("dialog_opened=") == 1,
            alert_out[:1200],
        )
        # 事件驱动上报可能晚于 act 返回一拍；只采信 act 开始后采样的报告。
        guard_deadline = time.time() + 2
        fresh_reports = []
        while time.time() < guard_deadline:
            fresh_reports = [
                report for report in server.guard_reports
                if int(report.get("at", "0")) >= guard_cutoff
            ]
            if any(report.get("stage") == "disarm" for report in fresh_reports):
                break
            time.sleep(0.05)
        disarm_report = next(
            (report for report in reversed(fresh_reports) if report.get("stage") == "disarm"),
            {},
        )
        runner.check(
            "guard is disarmed when act returns",
            disarm_report.get("guard") == "1" and disarm_report.get("armed") == "0"
            and disarm_report.get("native") == "1",
            f"report={disarm_report}",
        )
        printing = runner.run("act", "click", "--selector", "#print")
        print_out = (printing.stdout or "") + (printing.stderr or "")
        runner.check(
            "window.print is blocked",
            printing.returncode == 0 and "dialog_opened=print" in print_out,
            print_out[:900],
        )
        popup = runner.run("act", "click", "--selector", "#pop")
        popup_out = (popup.stdout or "") + (popup.stderr or "")
        runner.check(
            "blocked window.open is reported",
            popup.returncode == 0 and "popup_blocked=1" in popup_out,
            popup_out[:900],
        )
        required = runner.run("act", "click", "--selector", "#req-submit")
        required_out = (required.stdout or "") + (required.stderr or "")
        runner.check(
            "HTML5 validation is reported",
            required.returncode == 0 and "validation=failed" in required_out,
            required_out[:900],
        )

        runner.run("js", "document.getElementById('ov').style.display='block'")
        obscured = runner.run("act", "click", "--selector", "#under")
        runner.expect_error("overlay makes click obscured", obscured, "obscured", 4)
        runner.run("js", "document.getElementById('ov').style.display='none'")

        runner.expect_ok("fill caret setup", runner.run("act", "fill", "--selector", "#a", "abc"), "ok=fill")
        runner.expect_ok(
            "press Control+A is trusted",
            runner.run("act", "press", "--selector", "#a", "Control+A"),
            "trusted=1",
        )
        runner.expect_ok(
            "type replaces selection at caret",
            runner.run("act", "type", "--selector", "#a", "X"),
            "trusted=1",
        )
        caret = runner.run("js", "return JSON.stringify(document.getElementById('a').value)")
        runner.check("caret insertion replaced selection", '"X"' in (caret.stdout or ""), (caret.stdout or "") + (caret.stderr or ""))
        runner.run("act", "press", "--selector", "#a", "Backspace")
        erased = runner.run("js", "return JSON.stringify(document.getElementById('a').value)")
        runner.check("trusted Backspace deletes", '""' in (erased.stdout or ""), (erased.stdout or "") + (erased.stderr or ""))
        runner.run("act", "type", "--selector", "#a", "hello")
        runner.run("act", "press", "--selector", "#a", "Tab")
        focused = runner.run("js", "return document.activeElement.id")
        runner.check("trusted Tab moves focus", "b" in (focused.stdout or "").strip(), (focused.stdout or "") + (focused.stderr or ""))

        runner.expect_error(
            "select failure restores selection",
            runner.run("act", "select", "--selector", "#sel", "nope"),
            "value_not_available",
            4,
        )
        sel_kept = runner.run("js", "return document.getElementById('sel').value")
        runner.check("select value survived failure", "v2" in (sel_kept.stdout or ""), (sel_kept.stdout or "") + (sel_kept.stderr or ""))
        runner.expect_error(
            "fill failure restores value",
            runner.run("act", "fill", "--selector", "#num", "abc"),
            "value_rejected",
            4,
        )
        num_kept = runner.run("js", "return document.getElementById('num').value")
        runner.check("number value survived failure", "42" in (num_kept.stdout or ""), (num_kept.stdout or "") + (num_kept.stderr or ""))

        timed = runner.run("js", "--timeout", "1", "while(true){}")
        runner.expect_error("js hard timeout", timed, "script_timeout", 4)
        runner.expect_ok("js recovers after timeout", runner.run("js", "1+1"), "2")

        new_tab = runner.run("tabs", "new", f"{base}/second.html")
        new_out = (new_tab.stdout or "") + (new_tab.stderr or "")
        new_match = re.search(r"^id=(\d+)", new_out, re.M)
        current_tab = runner.run("tabs", "current")
        runner.check(
            "tabs new activates by default",
            new_tab.returncode == 0 and new_match is not None
            and f"id={new_match.group(1)}" in (current_tab.stdout or ""),
            new_out + (current_tab.stdout or "") + (current_tab.stderr or ""),
        )
        if new_match:
            runner.run("tabs", "close", new_match.group(1))

        if bug_tab:
            # act --tab 必须操作非活动标签，而不是当前活动标签。
            other_tab = runner.run("tabs", "new", f"{base}/second.html")
            other_match = re.search(r"^id=(\d+)", other_tab.stdout or "", re.M)
            current_before = runner.run("tabs", "current")
            tab_act = runner.run(
                "act", "fill", "--tab", bug_tab, "--selector", "#a", "background-tab-value"
            )
            current_after = runner.run("tabs", "current")
            background_value = runner.run(
                "js", "--tab", bug_tab, "return document.getElementById('a').value"
            )
            tab_act_out = (tab_act.stdout or "") + (tab_act.stderr or "")
            active_before = re.search(r"^id=(\d+)", current_before.stdout or "", re.M)
            active_after = re.search(r"^id=(\d+)", current_after.stdout or "", re.M)
            runner.check(
                "act --tab changes an inactive tab without activating it",
                tab_act.returncode == 0 and "ok=fill" in tab_act_out
                and "background-tab-value" in (background_value.stdout or "")
                and active_before is not None and active_after is not None
                and active_before.group(1) == active_after.group(1)
                and active_after.group(1) != bug_tab,
                tab_act_out + (background_value.stdout or "")
                + (current_before.stdout or "") + (current_after.stdout or ""),
            )
            tab_script = tmp / "tab-target.ema2"
            tab_script.write_text('fill selector="#b" "script-tab-value"\n', encoding="utf-8")
            script_tab_act = runner.run(
                "act", "--script", str(tab_script), "--tab", bug_tab
            )
            script_tab_value = runner.run(
                "js", "--tab", bug_tab, "return document.getElementById('b').value"
            )
            runner.check(
                "act --script accepts --tab",
                script_tab_act.returncode == 0 and "ok=1/1" in (script_tab_act.stdout or "")
                and "script-tab-value" in (script_tab_value.stdout or ""),
                (script_tab_act.stdout or "") + (script_tab_act.stderr or "")
                + (script_tab_value.stdout or "") + (script_tab_value.stderr or ""),
            )
            if other_match:
                runner.run("tabs", "close", other_match.group(1))
            runner.run("tabs", "activate", bug_tab)

            # 先切到一个新标签，确保 bug_tab 确实不是活动标签。
            runner.run("open", f"{base}/second.html")
            inactive_shot = tmp / "inactive-tab.png"
            shot_tab = runner.run("shot", "--tab", bug_tab, "--out", str(inactive_shot))
            shot_tab_out = (shot_tab.stdout or "") + (shot_tab.stderr or "")
            runner.check(
                "shot --tab activates an inactive tab",
                shot_tab.returncode == 0 and "activated=1" in shot_tab_out and inactive_shot.exists(),
                shot_tab_out[:900],
            )

        bad_open = tmp / "bad-open.ema2"
        bad_open.write_text("open https://example.com\n", encoding="utf-8")
        bad_wait = tmp / "bad-wait.ema2"
        bad_wait.write_text("waitFor time 1s\n", encoding="utf-8")
        open_error = runner.expect_error(
            "script rejects open with a CLI alternative",
            runner.run("act", "--script", str(bad_open)),
            "invalid_argument",
            1,
        )
        runner.check("script error names ema2 open", "ema2 open" in open_error, open_error[:800])
        wait_error = runner.expect_error(
            "script rejects waitFor time with await hint",
            runner.run("act", "--script", str(bad_wait)),
            "invalid_argument",
            1,
        )
        runner.check("script error names await 1s", "await 1s" in wait_error, wait_error[:800])

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
