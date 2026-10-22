#!/usr/bin/env bash
# 定位 ema2 可执行文件：EMA2_BIN > PATH > 仓库内 bin/ema2(.exe)。
# 三个包装器都 source 本文件，查找顺序只在这里维护。
# 调用方必须先 cd 到本目录，相对回退路径才成立。
if [ -n "${EMA2_BIN:-}" ]; then
  EMA2="$EMA2_BIN"
elif command -v ema2 >/dev/null 2>&1; then
  EMA2="$(command -v ema2)"
elif [ -x ../../bin/ema2.exe ]; then
  EMA2="../../bin/ema2.exe"
elif [ -x ../../bin/ema2 ]; then
  EMA2="../../bin/ema2"
else
  echo "error=ema2_not_found hint=export EMA2_BIN=/path/to/ema2, put ema2 on PATH, or build rust/ into bin/" >&2
  exit 2
fi
export EMA2
