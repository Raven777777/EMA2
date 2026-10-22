#!/usr/bin/env bash
# 多源并发：一次 open 多个 URL，再用 batch js 并发跑 __E.digest()，返回统一摘要。
# 用法: ./batch.sh URL [URL...]
set -uo pipefail
cd "$(dirname "$0")" || exit 1
. ./_env.sh
if [ $# -lt 1 ]; then
  echo "usage: ./batch.sh URL [URL...]" >&2
  exit 1
fi
URLS="$(IFS=,; echo "$*")"
OUT="$("$EMA2" batch open --urls "$URLS" 2>&1)"
# 每个块头部是 url=，随后 id= 和一条重复的 url=；这里按「url -> id -> 明细」重排，
# 重复的 url 只保留头部那一行。
printf '%s\n' "$OUT" | awk '
  /^---$/ { next }
  /^url=/ { head = $0; next }
  /^id=/ { print head; print; next }
  /^title=/ || /^status=/ { print "  " $0; next }
  /^error=/ { print "  " $0; next }
'
IDS="$(printf '%s\n' "$OUT" | sed -n 's/^id=//p' | paste -sd, -)"
if [ -z "$IDS" ]; then
  echo "error=no_tabs"
  exit 1
fi
echo "ids=$IDS"
# 库 + 调用合成一个临时脚本，用 --file 投递：省掉 shell 里的多行引号拼接。
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT
cat lib.js > "$TMP"
echo "return __E.digest(600);" >> "$TMP"
"$EMA2" batch js --tab-ids "$IDS" --file "$TMP" --budget 1000 --timeout 20
