#!/usr/bin/env python3
"""通过 EMA2 抓取百度百科词条，输出 Markdown 文档。

EMA2 对外只是一个标准 MCP over HTTP 端点（默认 http://127.0.0.1:8765/mcp），
所以任何语言、不装任何 SDK 都能脚本化操作真实 Edge 浏览器。本脚本只用
Python 标准库，可以当作「用 EMA2 写爬虫」的最小模板：

    initialize  ->  创建 session
    open              打开词条页（返回 id/url/title）
    wait              等 .J-lemma-content 出现
    js                在页面主世界里结构化抽取（heading/code/li/text/信息框）
    tabs close        清理标签页

用法：
    python examples/scrape_baike.py Python
    python examples/scrape_baike.py 量子计算 -o examples/output/quantum.md
    python examples/scrape_baike.py https://baike.baidu.com/item/DeepSeek --keep-tab --max-chars 100000
    EDGE_MCP_TOKEN=xxx python examples/scrape_baike.py Python     # 中继开了 token 时

前置条件：
    1. bin\\ema2.exe run（或 scripts\\start-bridge.cmd）
    2. Edge 里已加载 extension/，且 ema2.exe status 显示 extension_connected: true

环境变量：
    EMA2_MCP_URL    覆盖 MCP 地址（默认 http://127.0.0.1:8765/mcp）
    EDGE_MCP_TOKEN  中继启用鉴权时的 Bearer token
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path

PROTOCOL_VERSION = "2025-06-18"
DEFAULT_OUTPUT_DIR = Path(__file__).resolve().parent / "output"

# 在页面里做结构化抽取（而不是直接 innerText）：这样代码块、列表、标题能分开，
# 输出 Markdown 时不会把代码里的 "# 注释" 当成标题，也不会丢代码缩进。
EXTRACT_JS = r"""
(() => {
  // 不能用逗号选择器：document.querySelector 返回文档顺序里的第一个元素，
  // 外层 wrapper 排在 .J-lemma-content 之前，会把导航/点赞等噪声一起抓进来。
  const pickFirst = (selectors) => {
    for (const selector of selectors) {
      const el = document.querySelector(selector);
      if (el) return el;
    }
    return null;
  };
  const root = pickFirst(['.J-lemma-content', '.lemma-content', '#J-lemma-main-wrapper', '.main-content'])
    || document.body;
  const summaryEl = pickFirst(['.J-summary', '[class*="lemmaSummary"]', '.lemma-summary']);
  const h1 = pickFirst(['h1', '[class*="lemmaTitle"]']);

  const infobox = [];
  const infoRoot = pickFirst(['[class*="basicInfo"]', '.basic-info', '.J-basic-info']);
  if (infoRoot) {
    infoRoot.querySelectorAll('dt').forEach(dt => {
      const dd = dt.nextElementSibling;
      if (dd && dd.tagName === 'DD') infobox.push([dt.innerText.trim(), dd.innerText.trim()]);
    });
  }

  const BLOCK_TAGS = /^(DIV|SECTION|ARTICLE|P|UL|OL|LI|TABLE|THEAD|TBODY|TR|TD|TH|H1|H2|H3|H4|H5|H6|PRE|BLOCKQUOTE|DL|DT|DD|FIGURE|FIGCAPTION|MAIN|HEADER|FOOTER|ASIDE|FORM)$/;
  const SKIP_TAGS = /^(SCRIPT|STYLE|NOSCRIPT|IFRAME|VIDEO|AUDIO|SVG|CANVAS)$/;
  const blocks = [];
  const pushText = (type, text) => {
    const value = (text || '').replace(/\u200b/g, '').trim();
    if (value) blocks.push({ type, text: value });
  };

  const walk = (element) => {
    for (const child of element.children) {
      const tag = child.tagName;
      if (SKIP_TAGS.test(tag) || tag === 'H1') continue;
      if (tag === 'PRE') {
        const code = (child.innerText || '').replace(/^\n+/, '').replace(/\s+$/, '');
        if (code.trim()) blocks.push({ type: 'code', text: code });
        continue;
      }
      if (tag === 'H2' || tag === 'H3') {
        pushText('heading', child.innerText);
        blocks[blocks.length - 1].level = tag === 'H2' ? 2 : 3;
        continue;
      }
      if (tag === 'LI') {
        pushText('li', child.innerText);
        continue;
      }
      const hasBlockChild = [...child.children].some(grand => BLOCK_TAGS.test(grand.tagName));
      if (!hasBlockChild) {
        pushText('text', child.innerText);
        continue;
      }
      walk(child);
    }
  };
  walk(root);

  return {
    title: (h1 && h1.innerText.trim()) || document.title,
    docTitle: document.title,
    url: location.href,
    summary: summaryEl ? summaryEl.innerText.trim() : '',
    infobox,
    blocks,
    chars: blocks.reduce((sum, block) => sum + block.text.length, 0),
  };
})()
"""

NOISE_LINES = {
    "播报", "编辑", "目录", "赞", "收藏", "讨论", "分享", "复制链接", "订阅更新",
    "上传视频", "中文", "更多", "展开",
}

# 判断一段文本是不是代码：百科的示例代码没有 <pre>，是逐行 <div>，
# 只能按行首特征识别（缩进、注释、关键字、赋值/调用）。
CODE_LINE = re.compile(
    r"^\s*(?:#|def\s|class\s|import\s|from\s|return|print\(|for\s|while\s|"
    r"if\s|elif\s|else\s*:|try\s*:|except|with\s|@|"
    r"[A-Za-z_][\w.]*\s*[=()])"
)


def is_code_block(text: str) -> bool:
    lines = [line for line in text.splitlines() if line.strip()]
    if not lines:
        return False
    hits = sum(1 for line in lines if CODE_LINE.match(line))
    if len(lines) == 1:
        return hits == 1
    return hits == len(lines) or hits >= max(2, int(len(lines) * 0.6))


class McpError(RuntimeError):
    """EMA2 / MCP 调用失败。"""


class Ema2Client:
    """极简 MCP Streamable HTTP 客户端（仅标准库）。"""

    def __init__(self, url: str | None = None, token: str | None = None, timeout: float = 90.0):
        self.url = (url or os.environ.get("EMA2_MCP_URL") or "http://127.0.0.1:8765/mcp").strip()
        token = token if token is not None else os.environ.get("EDGE_MCP_TOKEN")
        self.token = token.strip() if isinstance(token, str) and token.strip() else None
        self.timeout = timeout
        self.session_id: str | None = None
        self._next_id = 0

    def _id(self) -> int:
        self._next_id += 1
        return self._next_id

    def _post(self, payload: dict) -> dict | None:
        request = urllib.request.Request(
            self.url,
            data=json.dumps(payload).encode("utf-8"),
            method="POST",
        )
        request.add_header("Content-Type", "application/json")
        if self.session_id:
            request.add_header("Mcp-Session-Id", self.session_id)
        if self.token:
            request.add_header("Authorization", f"Bearer {self.token}")

        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                session_id = response.headers.get("Mcp-Session-Id")
                if session_id:
                    self.session_id = session_id
                raw = response.read().decode("utf-8", "replace")
        except urllib.error.HTTPError as exc:
            body = exc.read().decode("utf-8", "replace").strip()
            hint = ""
            if exc.code == 401:
                hint = "（中继启用了 EDGE_MCP_TOKEN，请设置同名环境变量）"
            elif exc.code == 415:
                hint = "（Content-Type 必须是 application/json）"
            elif exc.code == 403:
                hint = "（Host/Origin 不是回环地址，被 EMA2 拒绝）"
            raise McpError(f"HTTP {exc.code}{hint}: {body or exc.reason}") from None
        except urllib.error.URLError as exc:
            raise McpError(
                f"无法连接 EMA2（{self.url}）：{exc.reason}；先运行 bin\\ema2.exe run"
            ) from None

        if not raw.strip():
            return None
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            raise McpError(f"EMA2 返回了非 JSON 响应：{raw[:200]}") from None

    def initialize(self) -> dict:
        response = self._post({
            "jsonrpc": "2.0",
            "id": self._id(),
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "ema2-baike-scraper", "version": "1.1.0"},
            },
        })
        if not response or "result" not in response:
            raise McpError(f"initialize 失败：{response}")
        if not self.session_id:
            raise McpError("EMA2 没有返回 Mcp-Session-Id 响应头")
        return response["result"]

    def call_tool(self, name: str, arguments: dict) -> str:
        response = self._post({
            "jsonrpc": "2.0",
            "id": self._id(),
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        if response is None:
            raise McpError(f"{name}: EMA2 返回空响应")
        if "error" in response:
            error = response["error"] or {}
            raise McpError(f"{name}: {error.get('message') or error}")
        result = response.get("result") or {}
        texts = [item.get("text", "") for item in result.get("content", []) if item.get("type") == "text"]
        if result.get("isError"):
            raise McpError(f"{name}: {' '.join(texts).strip() or '工具执行失败'}")
        return "\n".join(texts)

    def call_tool_json(self, name: str, arguments: dict):
        text = self.call_tool(name, arguments)
        try:
            return json.loads(text)
        except json.JSONDecodeError:
            raise McpError(f"{name}: 期望 JSON，收到 {text[:200]!r}") from None


def baike_url(entry: str) -> str:
    if entry.startswith(("http://", "https://")):
        return entry
    return "https://baike.baidu.com/item/" + urllib.parse.quote(entry, safe="")


def output_field(text: str, key: str) -> str | None:
    """从 v1.4 的行式输出里取 `key=value`。"""
    match = re.search(rf"^{re.escape(key)}=(.*)$", text or "", re.MULTILINE)
    return match.group(1) if match else None


def scrape(client: Ema2Client, entry: str, wait: float, keep_tab: bool, max_chars: int) -> dict:
    url = baike_url(entry)
    opened = client.call_tool("open", {"url": url, "wait": "dom"})
    tab_id = output_field(opened, "id")
    if not tab_id:
        raise McpError(f"open 没有返回 tabId：{opened[:200]}")
    print(f"· 打开标签页 {tab_id}: {url}")
    try:
        try:
            client.call_tool("wait", {"tabId": int(tab_id), "selector": ".J-lemma-content, #J-lemma-main-wrapper", "timeout": wait})
        except McpError as exc:
            raise McpError(f"等待词条渲染失败：{exc}") from None
        data = client.call_tool_json("js", {"tabId": int(tab_id), "code": EXTRACT_JS, "budget": 200_000})
    finally:
        if keep_tab:
            print(f"· 保留标签页 {tab_id}（--keep-tab）")
        else:
            try:
                client.call_tool("tabs", {"action": "close", "tabId": int(tab_id)})
                print(f"· 关闭标签页 {tab_id}")
            except McpError as exc:
                print(f"· 关闭标签页失败：{exc}", file=sys.stderr)

    data["requestedUrl"] = url
    blocks = data.get("blocks") or []
    full_chars = sum(len(block.get("text") or "") for block in blocks)
    data["fullChars"] = full_chars
    if full_chars > max_chars:
        kept, used = [], 0
        for block in blocks:
            text = block.get("text") or ""
            if used + len(text) > max_chars:
                data["truncated"] = True
                break
            kept.append(block)
            used += len(text)
        data["blocks"] = kept
    return data


def clean_text(value: str, keep_refs: bool) -> str:
    value = value.replace("\u200b", "").replace("\xa0", " ")
    value = re.sub(r"[ \t]+", " ", value).strip()
    if not keep_refs:
        value = re.sub(r"\s*\[\d+(?:\s*-\s*\d+)?\]", "", value).strip()
    return value


@dataclass
class Section:
    level: int
    title: str
    blocks: list[dict] = field(default_factory=list)


def build_sections(data: dict, keep_refs: bool) -> list[Section]:
    sections: list[Section] = [Section(level=2, title="概述")]
    for block in data.get("blocks") or []:
        kind = block.get("type")
        raw = block.get("text") or ""
        if kind == "heading":
            title = clean_text(raw, keep_refs)
            if title and title not in NOISE_LINES:
                sections.append(Section(level=int(block.get("level") or 2), title=title))
            continue
        if kind == "code":
            if raw.strip():
                sections[-1].blocks.append({"type": "code", "text": raw.rstrip()})
            continue
        raw_lines = raw.splitlines()
        if kind == "text" and is_code_block(raw):
            # 相邻的代码行合并成一个代码块，保留缩进（不做空白折叠）。
            merged = "\n".join(line.rstrip() for line in raw_lines if line.strip())
            previous = sections[-1].blocks[-1] if sections[-1].blocks else None
            if previous and previous["type"] == "code":
                previous["text"] = previous["text"] + "\n" + merged
            else:
                sections[-1].blocks.append({"type": "code", "text": merged})
            continue
        lines = [clean_text(line, keep_refs) for line in raw_lines]
        lines = [line for line in lines if line and line not in NOISE_LINES]
        if not lines:
            continue
        sections[-1].blocks.append({"type": kind or "text", "text": "\n".join(lines)})
    return [section for section in sections if section.title != "概述" or section.blocks]


def render_markdown(data: dict, keep_refs: bool) -> str:
    title = (data.get("title") or "百度百科词条").strip()
    url = data.get("url") or data.get("requestedUrl") or ""
    sections = build_sections(data, keep_refs)
    summary = clean_text(data.get("summary") or "", keep_refs)
    infobox = [
        (clean_text(key, keep_refs), clean_text(value, keep_refs))
        for key, value in (data.get("infobox") or [])
    ]
    paragraphs = sum(len(section.blocks) for section in sections)

    out: list[str] = [f"# {title}", ""]
    out.append(f"- **来源**：[{url}]({url})")
    out.append(
        f"- **抓取时间**：{datetime.now().strftime('%Y-%m-%d %H:%M:%S')}（EMA2 · Edge 真实浏览器）"
    )
    out.append(
        f"- **统计**：正文 {data.get('fullChars', 0)} 字符，{len(sections)} 个章节，{paragraphs} 个内容块"
    )
    if data.get("truncated"):
        out.append("- **注意**：正文超过 `--max-chars` 限制，已截断")
    out.append("")

    if summary:
        out += ["## 摘要", ""]
        out += [line for line in summary.split("\n") if line.strip()]
        out.append("")

    if infobox:
        out += ["## 基本信息", "", "| 字段 | 内容 |", "| --- | --- |"]
        for key, value in infobox:
            out.append(f"| {key or '—'} | {(value or '—').replace('|', '/')} |")
        out.append("")

    for section in sections:
        if section.title != "概述":
            out += [f"{'#' * min(max(section.level, 2), 6)} {section.title}", ""]
        for block in section.blocks:
            if block["type"] == "code":
                code = block["text"].replace("```", "'''")
                out += ["```", code, "```", ""]
                continue
            if block["type"] == "li":
                for line in block["text"].splitlines():
                    out.append(f"- {line.lstrip('-•· ').strip()}")
                out.append("")
                continue
            for line in block["text"].splitlines():
                if is_code_block(line):
                    # 孤立的单行代码（附近没有连续代码块）渲染成行内代码。
                    out += [f"`{line.strip().replace(chr(96), chr(39))}`", ""]
                    continue
                # 行首的 # 会被 Markdown 当成标题，转义掉。
                line = re.sub(r"^(#{1,6})", r"\1", line)
                is_short = len(line) <= 32
                ends_sentence = bool(re.search(r"[。！？!?：:；;，,、.]$", line))
                out.append(f"- {line}" if is_short and not ends_sentence else line)
                out.append("")
    return "\n".join(out).rstrip() + "\n"


def slugify(entry: str) -> str:
    if entry.startswith(("http://", "https://")):
        entry = urllib.parse.unquote(urllib.parse.urlparse(entry).path.rsplit("/", 1)[-1]) or "baike"
    slug = re.sub(r"[^\w\u4e00-\u9fff-]+", "_", entry, flags=re.UNICODE).strip("_")
    return slug or "baike"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="用 EMA2 控制真实 Edge 抓取百度百科并输出 Markdown",
    )
    parser.add_argument("entry", help="词条名（如 Python）或完整 URL")
    parser.add_argument("-o", "--out", help="输出 Markdown 路径（默认 examples/output/<词条>.md）")
    parser.add_argument("--wait", type=float, default=15.0, help="等待词条渲染的最长秒数（默认 15）")
    parser.add_argument("--max-chars", type=int, default=60000, help="正文最大字符数（默认 60000）")
    parser.add_argument("--keep-tab", action="store_true", help="抓取后保留标签页")
    parser.add_argument("--keep-refs", action="store_true", help="保留 [1] 这类引用角标")
    parser.add_argument("--json", action="store_true", help="同时输出一份原始 .json")
    args = parser.parse_args(argv)

    client = Ema2Client()
    try:
        info = client.initialize()
        print(f"· 已连接 EMA2（协议 {info.get('protocolVersion')}，会话 {(client.session_id or '')[:24]}…）")
        data = scrape(client, args.entry, args.wait, args.keep_tab, args.max_chars)
    except McpError as exc:
        print(f"错误：{exc}", file=sys.stderr)
        return 1

    markdown = render_markdown(data, args.keep_refs)
    out_path = Path(args.out) if args.out else DEFAULT_OUTPUT_DIR / f"{slugify(args.entry)}.md"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(markdown, encoding="utf-8")
    print(f"✓ 已写入 {out_path}（{len(markdown)} 字符）")

    if args.json:
        json_path = out_path.with_suffix(".json")
        json_path.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"✓ 原始数据 {json_path}")

    sections = build_sections(data, args.keep_refs)
    titles = [section.title for section in sections]
    print(f"· 标题：{data.get('title')}")
    print(f"· 章节：{' / '.join(titles[:10])}{' …' if len(titles) > 10 else ''}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
