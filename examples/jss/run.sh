#!/usr/bin/env bash
# 单标签页调用：把 lib.js 与表达式拼成一次 js 调用，避免两步往返。
# 用法: ./run.sh '<JS 表达式>' [ema2 js 的其它参数，如 --tab ID --budget N]
#       ./run.sh --raw '<多语句 JS，需自带 return>' [参数...]
# 例:   ./run.sh "__E.grep('CVE',2,40)" --tab 1618223388
set -uo pipefail
cd "$(dirname "$0")" || exit 1
. ./_env.sh
MODE="expr"
if [ "${1:-}" = "--raw" ]; then MODE="raw"; shift; fi
if [ $# -lt 1 ]; then
  echo "usage: ./run.sh [--raw] '<JS expression>' [--tab ID] [--budget N] [--timeout S]" >&2
  exit 1
fi
EXPR="$1"; shift
LIB="$(cat lib.js)"
if [ "$MODE" = "raw" ]; then
  BODY="$EXPR"
else
  BODY="return ( $EXPR );"
fi
exec "$EMA2" js "$LIB
$BODY" "$@"
