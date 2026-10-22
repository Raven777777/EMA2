# EMA2 示例

EMA2 没有 MCP、token 或客户端配置；所有示例都直接调用 `ema2` 子命令。
参数与错误码见 [../docs/TOOLS.md](../docs/TOOLS.md)，一键真机验收见
[`../scripts/verify.py`](../scripts/verify.py)。

## 前置条件

```bash
bin/ema2.exe server
bin/ema2.exe status        # relay=running + extension=connected
```

Edge 扩展的 popup 端口必须是 **7744**（或与 `--port` 一致）。
示例要在真实、前台可见的 Edge 窗口里跑（无头/后台窗口不支持截图与输入）。

## form.ema2 —— 登录/表单脚本

```bash
bin/ema2.exe open https://example.com/login
bin/ema2.exe snapshot
# 把 ref（e15/e16/e30）替换成 snapshot 返回的实际值
bin/ema2.exe act --script examples/form.ema2
```

脚本语法（完整版以 `ema2 act --help` 为准）：

- 每行一条动作；空行和 `#` 注释忽略；
- 只允许 act 动作 + `await` + `waitFor`，不能写 `open/read/wait`；
- `waitFor` 独立成行，接受 `url=` / `status=` / `timeout=`，或页面条件
  `text=` / `textGone=` / `selector=`（一次一个，与 `ema2 wait` 同义）；
- `await 10s` 等上一个动作的导航/页面稳定；
- `type` 在 caret/selection 处插入，`fill` 覆盖。

## batch-read.sh —— 一次读多个页面

```bash
bash examples/batch-read.sh https://example.com https://example.org
```

内部是：

```bash
ema2 batch read --urls "u1,u2" --mode md --budget 8000
```

## extract.js —— 页面内结构化抽取

```bash
bin/ema2.exe open https://example.com
bin/ema2.exe js --file examples/extract.js
```

`js` 先做 compile-only 探测：能按表达式编译就执行一次，否则按 async 函数体执行一次，
所以文件里可以直接 `return ...`；多行代码必须显式 `return`（见 `ema2 js --help`）。

## jss/ —— 检索扩展的参考实现

```bash
cd examples/jss
./search.sh "Rust 2024 edition"       # 搜索并结构化输出
./run.sh "__E.outline(30)" --tab <ID> # 先看骨架再决定读哪段
./batch.sh "https://a.example/" "https://b.example/"
```

`lib.js` 只往页面注入 `window.__E`（`grep/outline/links/main/digest/meta/tables`），
搜索、并发、摘要都在外围脚本里，核心一行没动。完整说明与实测记录见
[`jss/README.md`](jss/README.md)；脚本用 `EMA2_BIN` → `PATH` → `bin/ema2(.exe)` 定位 exe。

## 上传文件

```bash
bin/ema2.exe open https://example.com/upload
bin/ema2.exe snapshot                     # 找到 input[type=file] 的 ref
bin/ema2.exe files upload C:/tmp/a.pdf --ref e8
```

`upload` 会把文件字节随请求下发，页面侧用 `DataTransfer` 赋给 `input.files`，
不依赖 debugger；单次 ≤20 个文件、载荷 ≤5 MiB。

## 典型循环

```bash
ema2 server
ema2 open https://example.com --replace
ema2 read --mode md
ema2 snapshot
ema2 act click e1 --wait-for "url=status status=200 timeout=5s"
ema2 observe network --since 0     # 续读时把上一次输出里的 next 数字传给 --since
ema2 shot --annotate
ema2 tabs close --session
ema2 server stop
```
