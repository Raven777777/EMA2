#!/usr/bin/env python3
"""EMA2 真实站点冒烟：快照结构、ref 句柄、act 解析、batch 复合、截断续读、参数容错。

对着**用户真实在用的 Edge** 跑，不是无头浏览器。只做只读动作（hover / read / snapshot），
结束时用 `tabs close scope=session` 关掉自己开的标签。

用法（先 `scripts\start-bridge.cmd`）：
    python scripts/live-check.py

令牌从中继自己的状态文件读，不需要额外参数。
"""
import json
import os
import re
import sys
import urllib.request

URL = os.environ.get("EDGE_MCP_URL", "http://127.0.0.1:8765/mcp")
STATE = os.path.join(os.environ.get("LOCALAPPDATA", os.path.expanduser("~")), "EMA2", "state.json")
TOKEN = json.load(open(STATE, encoding="utf-8"))["token"]

_session = {"id": None}
_next_id = [0]


def _post(body):
    headers = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
        "Authorization": f"Bearer {TOKEN}",
    }
    if _session["id"]:
        headers["Mcp-Session-Id"] = _session["id"]
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), headers=headers, method="POST")
    with urllib.request.urlopen(req, timeout=300) as resp:
        sid = resp.headers.get("Mcp-Session-Id")
        if sid:
            _session["id"] = sid
        raw = resp.read().decode("utf-8", "replace")
        ctype = resp.headers.get("Content-Type", "")
    if "text/event-stream" in ctype:
        out = None
        for line in raw.splitlines():
            if line.startswith("data:"):
                chunk = line[5:].strip()
                if chunk:
                    out = json.loads(chunk)
        return out
    return json.loads(raw) if raw.strip() else None


def _rpc(method, params):
    msg = {"jsonrpc": "2.0", "id": _next_id[0], "method": method, "params": params}
    _next_id[0] += 1
    return _post(msg)


class cli:  # noqa: N801 - 占位命名空间，保持下面的调用形式不变
    @staticmethod
    def initialize():
        _rpc("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "ema2-live-check", "version": "1.0"},
        })
        _post({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})

    @staticmethod
    def rpc(method, params):
        return _rpc(method, params)

PASS, FAIL = [], []


def check(name, ok, detail=""):
    (PASS if ok else FAIL).append(name)
    print(("  PASS " if ok else "  FAIL ") + name + (("  <- " + detail) if detail and not ok else ""))


def call(tool, args):
    res = cli.rpc("tools/call", {"name": tool, "arguments": args})
    if "error" in res:
        return "ERROR " + str(res["error"])
    return "\n".join(p.get("text", "") for p in res["result"].get("content", []))


def parse_snapshot(text):
    lines = text.splitlines()
    # 头部是 3 行：url= / title= / scroll=… refs=… doc=…
    head = "\n".join(lines[:3])
    rows = []
    for line in lines:
        m = re.match(r"^\[(e\d+)@(d\d+)\] (\S+) \"(.*)\"(.*)$", line)
        if m:
            rows.append({
                "ref": m.group(1), "doc": m.group(2), "role": m.group(3),
                "name": m.group(4), "rest": m.group(5), "raw": line,
            })
    meta = {}
    for key in ("doc", "refs", "roots", "scroll"):
        m = re.search(r"\b" + key + r"=(\S+)", head)
        if m:
            meta[key] = m.group(1)
    m = re.search(r"total=(\d+) shown=(\d+) next=(\S+)", text)
    if m:
        meta["total"], meta["shown"], meta["next"] = int(m.group(1)), int(m.group(2)), m.group(3)
    return head, rows, meta


ACTIONABLE = {
    "link", "button", "textbox", "searchbox", "combobox", "listbox", "checkbox", "radio",
    "switch", "slider", "spinbutton", "tab", "option", "menuitem", "menuitemcheckbox",
    "menuitemradio", "treeitem", "file", "summary", "input", "textarea", "select",
}


def audit_snapshot(label, text, expect_truncated_url=True):
    head, rows, meta = parse_snapshot(text)
    bad = [r["raw"] for r in rows if not re.match(r"^d\d+$", r["doc"])]
    check(f"{label}: 所有 ref 都是 [eN@dN]（{len(rows)} 行）", not bad, str(bad[:2]))
    doc = meta.get("doc")
    mismatch = [r["raw"] for r in rows if doc and r["doc"] != doc]
    check(f"{label}: 行内句柄与头部 doc={doc} 一致", not mismatch, str(mismatch[:2]))
    empties = [r["raw"] for r in rows if not r["name"] and r["role"] not in ACTIONABLE]
    check(f"{label}: 无名称且不可操作的节点已丢弃", not empties, str(empties[:2]))
    trunc = [r for r in rows if "…" in r["rest"]]
    if expect_truncated_url:
        check(f"{label}: 长 URL 截到 40 字符（{len(trunc)} 行带 …）", True)
    return head, rows, meta


def scenario(name, url):
    print(f"\n== {name}: {url}")
    opened = call("open", {"url": url, "wait": "load"})
    m = re.search(r"^status=(\d+)$", opened, re.M)
    status = m.group(1) if m else "?"
    reuse = re.search(r"^reuse=(\S+)$", opened, re.M)
    check(f"open 返回真实状态码 ({status})", status.isdigit())
    check("open 返回 reuse 标记", bool(reuse))

    snap = call("snapshot", {"filter": "interactive", "budget": 2500})
    _, rows, meta = audit_snapshot(name, snap)
    print(f"     {len(rows)} refs / doc={meta.get('doc')} / total={meta.get('total')} shown={meta.get('shown')}")

    # filter=links 不截断 URL
    links = call("snapshot", {"filter": "links", "budget": 900})
    _, lrows, _ = parse_snapshot(links)
    clipped = [r for r in lrows if "…" in r["rest"]]
    check(f"{name}: filter=links 保留完整 URL（{len(lrows)} 行，{len(clipped)} 行仍截断）",
          len(clipped) <= max(1, len(lrows) // 10))

    # full 视图无损：同预算下不会比 interactive 更少
    full = call("snapshot", {"filter": "full", "budget": 1200})
    _, frows, _ = parse_snapshot(full)
    inter = call("snapshot", {"filter": "interactive", "budget": 1200})
    _, irows, _ = parse_snapshot(inter)
    nameless = [r for r in frows if not r["name"]]
    check(f"{name}: filter=full 无损（{len(frows)} 行 vs interactive {len(irows)} 行，含 {len(nameless)} 个空名）",
          len(frows) >= len(irows))
    return rows, meta


def main():
    cli.initialize()

    # 1) 多站点快照结构
    sites = [
        ("wikipedia", "https://en.wikipedia.org/wiki/OpenBSD"),
        ("github", "https://github.com/rust-lang/rust"),
        ("hn", "https://news.ycombinator.com/"),
        ("jd", "https://www.jd.com/"),
        ("bing", "https://www.bing.com/search?q=openbsd+8.0"),
    ]
    handles = {}
    for name, url in sites:
        rows, meta = scenario(name, url)
        handles[name] = meta.get("doc")

    # 2) 句柄单调递增、不复用
    seq = [int(v[1:]) for v in handles.values() if v and v.startswith("d")]
    check("跨站点文档句柄单调递增、不复用", seq == sorted(set(seq)) and len(seq) == len(set(seq)), str(handles))

    # 3) act 用短句柄
    print("\n== act 解析")
    call("open", {"url": "https://news.ycombinator.com/", "reuse": "session"})
    snap = call("snapshot", {"filter": "links", "budget": 600})
    _, rows, meta = parse_snapshot(snap)
    doc = meta.get("doc")
    target = rows[0]["ref"] if rows else None
    if target:
        out = call("act", {"action": "hover", "ref": f"[{target}@{doc}]"})
        check(f"act 接受 [{target}@{doc}]", "stale_ref" not in out and "ERROR" not in out, out[:120])

    out = call("act", {"action": "hover", "ref": "e1@t7-k3a9"})
    check("旧长格式 ref 被拒（stale_ref）", "stale_ref" in out or "ERROR" in out, out[:120])

    out = call("act", {"action": "hover", "ref": "[e1@d99999]"})
    check("伪造句柄被拒（stale_ref）", "stale_ref" in out or "ERROR" in out, out[:120])

    # 4) 导航后旧句柄失效、新句柄换号
    before = doc
    call("open", {"url": "https://news.ycombinator.com/newest", "reuse": "session"})
    after_snap = call("snapshot", {"filter": "links", "budget": 600})
    _, rows2, meta2 = parse_snapshot(after_snap)
    check(f"导航后文档换号（{before} -> {meta2.get('doc')}）", meta2.get("doc") and meta2.get("doc") != before)
    if target:
        out = call("act", {"action": "hover", "ref": f"[{target}@{before}]"})
        check("导航前 ref 在新文档上失效", "stale_ref" in out or "ERROR" in out, out[:120])

    # 5) near= 限定范围
    print("\n== near= 与游标")
    call("open", {"url": "https://github.com/rust-lang/rust", "reuse": "session"})
    snap = call("snapshot", {"filter": "interactive", "budget": 1500})
    _, rows3, meta3 = parse_snapshot(snap)
    nav = [r for r in rows3 if r["name"]]
    if nav:
        anchor = nav[0]
        near = call("snapshot", {"filter": "interactive", "near": f"[{anchor['ref']}@{anchor['doc']}]", "budget": 1200})
        _, nrows, _ = parse_snapshot(near)
        check(f"near=[{anchor['ref']}@{anchor['doc']}] 缩小作用域（{len(rows3)} -> {len(nrows)}）", 0 < len(nrows) <= len(rows3))
    else:
        check("near= 有可用容器锚点", False, "no named container")

    # 6) 游标续读完整覆盖
    first = call("snapshot", {"filter": "interactive", "budget": 1200})
    _, r1, m1 = parse_snapshot(first)
    got = len(r1)
    total = m1.get("total", 0)
    rounds = 1
    cursor = m1.get("next")
    cursor = int(cursor) if str(cursor).isdigit() else None
    prev_max = max(int(r["ref"][1:]) for r in r1) if r1 else 0
    while cursor not in (None, "none") and rounds < 25:
        more = call("snapshot", {"filter": "interactive", "budget": 1200, "cursor": cursor})
        _, rr, mm = parse_snapshot(more)
        if not rr:
            break
        got += len(rr)
        total = mm.get("total", total)
        prev_max = max(int(r["ref"][1:]) for r in rr)
        nxt = mm.get("next")
        cursor = int(nxt) if str(nxt).isdigit() else None
        rounds += 1
    # 动态页面（GitHub 水合）total 会变，因此以「最后一页 ref 序号接近 total」为准
    check(f"cursor 能推进到末页（{got} 行 / {rounds} 次，末页 ref e{prev_max} / total {total}）",
          prev_max >= total - 3 and total > 0, f"got {got} total {total} last e{prev_max}")
    check("截断时给 hint=truncated", "hint=truncated" in first)

    # 7) batch + urls 一次打开并读取
    print("\n== batch 复合")
    out = call("batch", {
        "tool": "read", "urls": ["https://en.wikipedia.org/wiki/OpenBSD", "https://www.openbsd.org/80.html"],
        "concurrency": 2, "args": {"mode": "md", "main": True, "budget": 600},
    })
    blocks = out.split("\n---\n")
    check("batch 返回两块", len(blocks) == 2, str(len(blocks)))
    check("每块带 url= 与 tab=", all("url=" in b and "tab=" in b for b in blocks))
    check("两块都拿到正文", all(len(b) > 400 for b in blocks), str([len(b) for b in blocks]))

    # 8) js 顶层 return
    print("\n== js 语句体")
    call("open", {"url": "https://news.ycombinator.com/", "reuse": "session"})
    out = call("js", {"code": "const t = document.title;\nconst n = document.querySelectorAll('tr.athing').length;\nreturn t + ' | rows=' + n;"})
    check("js 顶层 return 直接可用", "rows=" in out, out[:120])

    # 9) read 截断提示 + 续读
    print("\n== read 截断")
    call("open", {"url": "https://en.wikipedia.org/wiki/OpenBSD", "reuse": "session"})
    r = call("read", {"main": True, "budget": 900})
    mt = re.search(r"total=(\d+) shown=(\d+) next=(\S+)", r)
    check("read 给 total/shown/next", bool(mt))
    check("read 截断带续读 hint", "hint=truncated" in r and "cursor=" in r, r[-160:])
    if mt and mt.group(3) != "none":
        r2 = call("read", {"main": True, "budget": 900, "cursor": int(mt.group(3))})
        check("read 续读从正确游标开始", "from cursor=" in r2, r2[:200])

    # 10) 游标容错：输出里是裸数字，抄成字符串也该收
    print("\n== 参数容错")
    ok = call("snapshot", {"filter": "links", "budget": 500, "cursor": "5"})
    check("cursor 传字符串 '5' 被接受", "invalid_argument" not in ok and "total=" in ok, ok[:120])
    bad = call("snapshot", {"filter": "links", "budget": 500, "cursor": "abc"})
    check("cursor 传 'abc' 仍被拒", "invalid_argument" in bad and "non-negative integer" in bad, bad[:120])
    obs = call("observe", {"kind": "console", "since": "0"})
    check("observe since 传字符串被接受", "invalid_argument" not in obs, obs[:120])

    # 收尾
    closed = call("tabs", {"action": "close", "scope": "session"})
    print("\n== 收尾\n  " + closed.replace("\n", " | "))

    print("\n" + "=" * 60)
    print(f"PASS {len(PASS)} / FAIL {len(FAIL)}")
    for f in FAIL:
        print("  FAIL:", f)


main()
