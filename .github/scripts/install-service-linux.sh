#!/bin/sh
# ===========================================================================
#  Registers the `rustdesk` binary sitting next to this script as a systemd
#  service that starts at boot and is restarted when it exits.
#
#      sudo ./install-service.sh
#
#  Uninstall:
#      sudo systemctl disable --now rustdesk
#      sudo rm /etc/systemd/system/rustdesk.service
#
#  This runs the windowless daemon directly, with no arguments, so it needs no
#  desktop session: that is the mode this build starts the host server in. On a
#  machine that has a graphical desktop, a system service can still fail to
#  reach the session's display for capture - the packaged builds run
#  `rustdesk --server` from the desktop session for exactly that reason.
# ===========================================================================
set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN="$SELF_DIR/rustdesk"
UNIT_PATH=/etc/systemd/system/rustdesk.service

if [ "$(id -u)" != "0" ]; then
    echo "Run this as root: sudo $0" >&2
    exit 1
fi

if [ ! -x "$BIN" ]; then
    echo "The rustdesk binary is missing next to this script, or is not executable:" >&2
    echo "  $BIN" >&2
    echo "Unpack the whole archive and run this file from inside that directory." >&2
    exit 1
fi

if ! command -v systemctl >/dev/null 2>&1; then
    echo "systemctl was not found. This installer only supports systemd." >&2
    echo "On another init system, run this daemon from your own service unit." >&2
    exit 1
fi

cat > "$UNIT_PATH" <<EOF
[Unit]
Description=RustDesk Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=$BIN
Restart=always
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable rustdesk.service
systemctl restart rustdesk.service

echo
echo "================== result =================="
systemctl --no-pager --full status rustdesk.service || true
echo
echo "Installed and enabled at boot."
echo "  unit   : $UNIT_PATH"
echo "  binary : $BIN"
echo "  log    : journalctl -u rustdesk -f"
echo "  stop   : sudo systemctl disable --now rustdesk"