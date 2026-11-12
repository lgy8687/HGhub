#!/usr/bin/env bash
# 从 src-tauri/src 同步纯 Rust 核心模块，只做两处去 Tauri 化。
# 上游源码保持不动，重复执行安全。
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE/../src-tauri/src"
DST="$HERE/src/core"
mkdir -p "$DST"

for f in net media session network_policy lines playback_log logging; do
  cp "$SRC/$f.rs" "$DST/$f.rs"
done

python3 - "$DST" <<'PY'
import pathlib, sys
d = pathlib.Path(sys.argv[1])

p = d / "media.rs"
s = p.read_text(encoding="utf-8")
old = "use tauri::http::{Response, StatusCode};"
assert old in s, "media.rs 的 tauri::http 引用变了，需要人工核对"
p.write_text(s.replace(old, "use http::{Response, StatusCode};"), encoding="utf-8")

p = d / "playback_log.rs"
s = p.read_text(encoding="utf-8")
n = s.count("#[tauri::command]\n")
assert n > 0, "playback_log.rs 里没找到 tauri::command，需要人工核对"
p.write_text(s.replace("#[tauri::command]\n", ""), encoding="utf-8")
print(f"synced 7 modules; media.rs 1 import replaced; playback_log.rs {n} attributes removed")
PY
