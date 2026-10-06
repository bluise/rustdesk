#!/bin/sh
# ===========================================================================
#  Registers the `rustdesk` binary sitting next to this script as a launchd
#  daemon that starts at boot and is restarted when it exits.
#
#      sudo ./install-service.sh
#
#  Uninstall:
#      sudo launchctl bootout system /Library/LaunchDaemons/rustdesk.plist
#      sudo rm /Library/LaunchDaemons/rustdesk.plist
#
#  Two macOS specifics to keep in mind:
#   * Screen Recording is a per-program permission, so this binary has to be
#     granted it under System Settings -> Privacy & Security -> Screen
#     Recording before a session can show anything.
#   * A LaunchDaemon runs outside any login session, which is the unattended
#     case. To capture a logged-in desktop, run `rustdesk --server` from that
#     session instead - that is what the packaged builds do.
# ===========================================================================
set -eu

SELF_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN="$SELF_DIR/rustdesk"
PLIST=/Library/LaunchDaemons/rustdesk.plist

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

if ! command -v launchctl >/dev/null 2>&1; then
    echo "launchctl was not found; this installer only supports macOS." >&2
    exit 1
fi

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>rustdesk</string>
    <key>ProgramArguments</key>
    <array>
        <string>$BIN</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>5</integer>
</dict>
</plist>
EOF

chown root:wheel "$PLIST"
chmod 644 "$PLIST"

# bootout fails when nothing is loaded yet, which is the normal first run.
launchctl bootout system "$PLIST" 2>/dev/null || true
launchctl bootstrap system "$PLIST"
launchctl enable system/rustdesk

echo
echo "================== result =================="
launchctl print system/rustdesk | head -n 20 || true
echo
echo "Installed as a LaunchDaemon, starting at boot."
echo "  plist  : $PLIST"
echo "  binary : $BIN"
echo "  stop   : sudo launchctl bootout system $PLIST"
echo
echo "Remember to allow Screen Recording for: $BIN"