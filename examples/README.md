# EMA2 脚本示例

EMA2 本身只是「MCP over HTTP ↔ Edge 扩展」的桥，**不含业务脚本**；
任何语言用 HTTP POST 就能驱动真实浏览器。这个目录放脚本示例。

v1.12 工具面仍是 11 个：`tabs / open / read / snapshot / act / wait / js / observe / files / shot / batch`，
输出是行式紧凑文本，错误是 `error=<code> msg=... hint=...`。
参数与输出协议见 [../docs/TOOLS.md](../docs/TOOLS.md)。

## 前置条件

1. `bin\ema2.exe install`：首次在 `edge://extensions` 里“加载解压缩的扩展”，
   选择 install 打印的目录；之后在授权页勾选终端并点一次「授权此浏览器」。
2. `bin\ema2.exe status` 显示 `"extension_connected": true` 即就绪。
3. 手工调用 HTTP 时从 `%LOCALAPPDATA%\EMA2\state.json` 取 `token`，
   或用 `EDGE_MCP_TOKEN` 覆盖。

## scrape_baike.py —— 抓百度百科并输出 Markdown

只用 Python 标准库（3.10+），不需要 `pip install`。

```bash
python examples/scrape_baike.py Python
python examples/scrape_baike.py 量子计算 -o examples/output/quantum.md
python examples/scrape_baike.py https://baike.baidu.com/item/DeepSeek --keep-tab --max-chars 100000
EDGE_MCP_TOKEN=xxx python examples/scrape_baike.py Python
```

输出默认写到 `examples/output/<词条>.md`（已加入 `.gitignore`）。

| 参数 | 说明 |
|---|---|
| `entry` | 词条名（`Python`）或完整 URL |
| `-o, --out` | 指定输出路径 |
| `--max-chars` | 正文上限（默认 60000，超出会截断并提示） |
| `--wait` | 等待词条渲染的最长秒数（默认 15） |
| `--keep-tab` | 抓完保留标签页 |
| `--keep-refs` | 保留 `[1]` 这类引用角标（默认去掉） |
| `--json` | 额外输出原始结构化数据 `.json` |

脚本内部的调用顺序：

```
initialize                         → 拿到 Mcp-Session-Id
open        {url, wait:"dom"}       → 返回 id/url/title（行式）
wait        {tabId, selector:".J-lemma-content", timeout}
js          {tabId, code, budget:200000} → 页面主世界结构化抽取，返回紧凑 JSON
tabs        {action:"close", tabId} → 清理标签页
```

## weather.js —— 查天气

```bash
node examples/weather.js "Tokyo"
node examples/weather.js "New York" --json
node examples/weather.js "Shanghai" --keep-tab
```

内部走 `open → wait → read → tabs close`，默认读完关闭标签页。

## 直接调用 MCP 的最小示例（curl）

中继默认要求鉴权，所有请求都需要 `-H "Authorization: Bearer $EDGE_MCP_TOKEN"`。

```bash
# 1) 握手，记下响应头里的 Mcp-Session-Id
curl -i -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}' \
  http://127.0.0.1:8765/mcp

# 2) 打开页面
curl -X POST -H "Authorization: Bearer $EDGE_MCP_TOKEN" -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"open","arguments":{"url":"https://example.com"}}}' \
  http://127.0.0.1:8765/mcp

# 3) 快照 → 按 ref 点击
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"snapshot","arguments":{}}}' \
  http://127.0.0.1:8765/mcp
curl -X POST -H "Content-Type: application/json" -H "Mcp-Session-Id: <sid>" \
  -d '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"act","arguments":{"action":"click","ref":"e1@d3"}}}' \
  http://127.0.0.1:8765/mcp
```

完整工具列表见根目录 README。
