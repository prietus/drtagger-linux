#!/usr/bin/env bash
# Development loop: the code lives on the Mac, builds and runs on the Linux VM.
#
#   scripts/vm.sh sync              rsync the repo to the VM
#   scripts/vm.sh cargo <args>      sync, then run cargo on the VM
#   scripts/vm.sh cli <args>        sync, then run drtagger-cli on the VM
#   scripts/vm.sh run [args]        sync, build and start the GTK app in the
#                                   VM's graphical session (log: target/app.log)
#   scripts/vm.sh sh <command>      run a shell command in the VM checkout
#
# DRTAGGER_VM (default alarm@alarm.local), DRTAGGER_VM_DIR
# (default Projects/drtagger-linux) and DRTAGGER_VM_WAYLAND (default
# wayland-1) override the target.
set -euo pipefail

VM=${DRTAGGER_VM:-alarm@alarm.local}
DIR=${DRTAGGER_VM_DIR:-Projects/drtagger-linux}
WAYLAND=${DRTAGGER_VM_WAYLAND:-wayland-1}
ROOT=$(cd "$(dirname "$0")/.." && pwd)

# The graphical session's sockets, so SSH-launched processes reach the
# compositor, the session bus and the keyring.
SESSION_ENV="export LANG=en_US.UTF-8 XDG_RUNTIME_DIR=/run/user/\$(id -u) WAYLAND_DISPLAY=$WAYLAND DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u)/bus GSETTINGS_SCHEMA_DIR=\$PWD/target/schemas RUST_ICU_MAJOR_VERSION_NUMBER=\$(pkg-config --modversion icu-i18n | cut -d. -f1)"

sync() {
    rsync -a --delete \
        --exclude /target --exclude .git --exclude '*.swp' --exclude .DS_Store \
        --exclude /.build --exclude /Vendor/ffmpeg/ffmpeg --exclude /Vendor/ffmpeg/ffprobe \
        --exclude /parity/MacDump/.build --exclude /parity/MacDump/.build-patched --exclude /parity/local \
        "$ROOT/" "$VM:$DIR/"
}

remote() {
    ssh -o BatchMode=yes "$VM" "cd $DIR && $SESSION_ENV && mkdir -p target/schemas && glib-compile-schemas --targetdir=target/schemas data && $*"
}

# Cargo resolves on the VM; keep the Mac's lock file in step.
pull_lock() { rsync -a "$VM:$DIR/Cargo.lock" "$ROOT/Cargo.lock"; }

quote() { [ $# -eq 0 ] || printf '%q ' "$@"; }

cmd=${1:-}
shift || true
case "$cmd" in
    sync) sync ;;
    cargo) sync; status=0; remote "cargo $(quote "$@")" || status=$?; pull_lock; exit $status ;;
    cli) sync; status=0; remote "cargo run -q -p drtagger-cli -- $(quote "$@")" || status=$?; pull_lock; exit $status ;;
    run)
        sync
        remote "cargo build -p drtagger && (pkill -x drtagger || true) && \
            for i in \$(seq 50); do pgrep -x drtagger > /dev/null || break; sleep 0.1; done && \
            (nohup env ${DRTAGGER_LANGUAGE:+LANGUAGE=$DRTAGGER_LANGUAGE} target/debug/drtagger $(quote "$@") > target/app.log 2>&1 &) && sleep 2 && \
            if pgrep -x drtagger > /dev/null; then echo 'drtagger running (log: target/app.log)'; else cat target/app.log; exit 1; fi"
        ;;
    sh) sync; remote "$*" ;;
    *) sed -n '2,15p' "$0"; exit 2 ;;
esac
