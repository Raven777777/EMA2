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

脚本语法：

- 每行一条动作；空行和 `#` 注释忽略；
- 含空格的值用双引号；
- `waitFor` 独立成行，挂在上一动作后；
- `await 10s` 等上一个动作的导航/页面稳定；
- `type` 追加，`fill` 覆盖。

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

`js` 先按表达式执行；遇到语法错误再按 async 函数体执行，所以文件里可以直接
`return ...`。

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
ema2 observe network --since next
ema2 shot --annotate
ema2 tabs close --session
ema2 server stop
```
