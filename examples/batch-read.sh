#!/usr/bin/env sh
# 一次命令并发读取多个页面。
# 用法：bash examples/batch-read.sh https://a.com https://b.com
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: $0 <url> [url...]" >&2
  exit 1
fi

urls=$(printf '%s,' "$@" | sed 's/,$//')
exec ema2 batch read --urls "$urls" --mode md --budget 8000
