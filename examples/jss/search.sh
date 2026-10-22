#!/usr/bin/env bash
# 搜索原语：拼 Bing 查询 -> open -> 解析 SERP -> 输出 rank|title|url|snippet
# 用法: ./search.sh "查询词"            （默认保留标签页）
#       ./search.sh --close "查询词"    （读完关闭标签页）
set -uo pipefail
cd "$(dirname "$0")" || exit 1
. ./_env.sh
CLOSE=0
if [ "${1:-}" = "--close" ]; then CLOSE=1; shift; fi
if [ $# -lt 1 ]; then
  echo "usage: ./search.sh [--close] \"query\"" >&2
  exit 1
fi
RAW="$*"
# 逐字符百分号编码：只保留 URL 非保留字符，空格与 + ? = % # & " 全部转义。
enc() {
  local s="$1" out="" c i
  for ((i = 0; i < ${#s}; i++)); do
    c="${s:i:1}"
    case "$c" in
      [A-Za-z0-9._~-]) out+="$c" ;;
      *) out+="$(printf '%s' "$c" | od -An -tx1 -v | tr -d ' \n' | sed 's/../%&/g')" ;;
    esac
  done
  printf '%s' "$out"
}
Q="$(enc "$RAW")"
OUT="$("$EMA2" open "https://www.bing.com/search?q=$Q" --timeout 25 2>&1)"
ID="$(printf '%s\n' "$OUT" | sed -n 's/^id=//p')"
ST="$(printf '%s\n' "$OUT" | sed -n 's/^status=//p')"
if [ -z "$ID" ]; then
  echo "error=no_tab"
  printf '%s\n' "$OUT"
  exit 1
fi
echo "query=$RAW tab=$ID status=$ST"
# serp.js 自身按 5000 字符封顶并回报 results/total，预算留出信封余量。
"$EMA2" js --file serp.js --tab "$ID" --budget 6000
if [ "$CLOSE" = "1" ]; then
  "$EMA2" tabs close "$ID" >/dev/null 2>&1 || true
fi
