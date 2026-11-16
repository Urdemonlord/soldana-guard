#!/bin/bash
# Pin crates that need rustc > 1.75 (edition2024 manifests) down to the
# newest version whose rust_version <= 1.75, until anchor build succeeds.
cd ~/workspace/soldana-guard || exit 1
export PATH="$HOME/.cargo/bin:$HOME/.local/share/solana/install/active_release/bin:$PATH"

pick_version() {
  python3 - "$1" <<'PYEOF'
import json, sys, urllib.request
crate = sys.argv[1]
def parse(v):
    try:
        return tuple(int(x) for x in v.split("."))
    except Exception:
        return (99, 99, 99)
best = None
req = urllib.request.Request(
    f"https://crates.io/api/v1/crates/{crate}/versions?per_page=100",
    headers={"User-Agent": "soldana-guard-build-fix/1.0"},
)
with urllib.request.urlopen(req, timeout=30) as r:
    data = json.load(r)
for v in data["versions"]:
    if v.get("yanked"):
        continue
    rv = v.get("rust_version")
    if rv is None:
        continue
    if parse(rv) <= (1, 75, 0):
        if best is None or parse(v["num"]) > parse(best):
            best = v["num"]
print(best or "")
PYEOF
}

for i in $(seq 1 25); do
  if [ -f target/deploy/soldana_guard.so ] && [ -f target/deploy/mock_dex.so ]; then
    echo "BUILD OK: both .so files exist"
    ls -la target/deploy/*.so
    exit 0
  fi
  out=$(anchor build 2>&1)
  if [ -f target/deploy/soldana_guard.so ] && [ -f target/deploy/mock_dex.so ]; then
    echo "BUILD OK after attempt $i"
    ls -la target/deploy/*.so
    exit 0
  fi
  echo "$out" | tail -3
  # edition2024 manifest error: path is .../<crate>-<version>/Cargo.toml
  line=$(echo "$out" | grep -m1 "failed to parse manifest at")
  crate=""; ver=""
  if [ -n "$line" ]; then
    p=$(echo "$line" | sed -E 's/.*failed to parse manifest at `([^`]+)`.*/\1/')
    dir=$(basename "$(dirname "$p")")
    ver=$(echo "$dir" | grep -oE '[0-9]+\.[0-9]+\.[0-9]+.*$' | head -1)
    crate=${dir%-$ver}
  else
    line=$(echo "$out" | grep -m1 "cannot be built because it requires rustc")
    if [ -n "$line" ]; then
      crate=$(echo "$line" | sed -E 's/.*package `([^ ]+) v.*/\1/')
      ver=$(echo "$line" | sed -E 's/.*`[^ ]+ v([^`]+)`.*/\1/')
    fi
  fi
  if [ -z "$crate" ]; then
    echo "UNKNOWN FAILURE at attempt $i"
    echo "$out" | tail -30
    exit 1
  fi
  echo "Attempt $i: pinning $crate (was $ver)..."
  target_ver=$(pick_version "$crate")
  if [ -z "$target_ver" ]; then
    echo "No 1.75-compatible version found for $crate"
    exit 1
  fi
  echo "  -> $target_ver"
  if ! cargo update -p "$crate@$ver" --precise "$target_ver" 2>&1 | tail -2; then
    cargo update -p "$crate" --precise "$target_ver" 2>&1 | tail -2
  fi
  sed -i 's/^version = 4$/version = 3/' Cargo.lock
done
echo "Gave up after 25 attempts"
ls -la target/deploy/ 2>/dev/null
exit 1
