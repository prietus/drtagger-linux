#!/usr/bin/env bash
# Screenshot of the VM's session after optionally selecting an album in the
# running app and activating a window action:
#   scripts/shot.sh <out.png> [album path] [action] [scroll fraction]
set -euo pipefail
VM=${DRTAGGER_VM:-alarm@alarm.local}
out=$1; album=${2:-}; action=${3:-}; scroll=${4:-}
ssh "$VM" "bash -s -- $(printf '%q ' "$album" "$action" "$scroll")" <<'REMOTE'
set -e
album=$1; action=$2; scroll=$3
export XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-1 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus
obj=/us/priet/drtagger/window/1
call() { gdbus call --session --dest us.priet.drtagger --object-path $obj --method org.gtk.Actions.Activate "$@" >/dev/null; }
if [ -n "$album" ]; then
  arg=$(python3 -c 'import sys;print("[<%s>]" % repr(sys.argv[1]).replace("\\x27","\x27"))' "$album" 2>/dev/null || true)
  arg=$(python3 - "$album" <<'PY'
import sys
s = sys.argv[1].replace("\\", "\\\\").replace("'", "\\'")
print("[<'" + s + "'>]")
PY
)
  call select-album "$arg" '{}'
fi
[ -n "$action" ] && call "$action" '[]' '{}'
[ -n "$scroll" ] && { sleep 0.5; call scroll-inspector "[<double $scroll>]" '{}'; }
sleep 1.5
grim /home/alarm/scratch/shot.png
REMOTE
scp -q "$VM:scratch/shot.png" "$out"
