#!/usr/bin/env bash
# AI Activity Monitor - install on macOS (Apple silicon). The launchd
# counterpart of install.ps1: builds the frontend and the three binaries, then
# registers per-user LaunchAgents for the collector and dashboard (at login,
# kept alive) and the 9 PM daily report.
#
#   ./install.sh              build + install + start
#   ./install.sh --uninstall  stop and remove the LaunchAgents (data is kept)
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
agents="$HOME/Library/LaunchAgents"
data="$HOME/Library/Application Support/AIMonitor"
labels=(com.aimonitor.collector com.aimonitor.dashboard com.aimonitor.report)

unload_all() {
    for label in "${labels[@]}"; do
        launchctl bootout "gui/$(id -u)/$label" 2>/dev/null || true
    done
}

if [[ "${1:-}" == "--uninstall" ]]; then
    unload_all
    for label in "${labels[@]}"; do rm -f "$agents/$label.plist"; done
    echo "Uninstalled. Data left in: $data"
    exit 0
fi

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Missing: $1" >&2
        echo "  Install it, then re-run this script: $2" >&2
        exit 1
    fi
}
require cargo "https://rustup.rs"
require trunk "brew install trunk   (or: cargo install trunk --locked)"

# rust-toolchain.toml pins the toolchain and brings the wasm32 target along;
# rustup installs both on first use.

# Build the frontend BEFORE the native binaries: aimon-dashboard embeds
# frontend/aimon-ui/dist into itself at compile time (rust-embed), so a stale
# or missing dist/ means a stale or missing UI in the built binary.
(cd "$here/frontend/aimon-ui" && trunk build --release)
cargo build --release --manifest-path "$here/Cargo.toml" -p aimon-collector -p aimon-dashboard -p aimon-report

bin="$here/target/release"
mkdir -p "$agents" "$data"

# write_agent LABEL BINARY SCHEDULE_XML
write_agent() {
    cat >"$agents/$1.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$1</string>
    <key>ProgramArguments</key><array><string>$bin/$2</string></array>
    <key>WorkingDirectory</key><string>$here</string>
    <key>StandardOutPath</key><string>$data/$2.log</string>
    <key>StandardErrorPath</key><string>$data/$2.log</string>
$3
</dict>
</plist>
EOF
}

keep_alive='    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><true/>'
write_agent com.aimonitor.collector aimon-collector "$keep_alive"
write_agent com.aimonitor.dashboard aimon-dashboard "$keep_alive"
write_agent com.aimonitor.report aimon-report '    <key>StartCalendarInterval</key><dict><key>Hour</key><integer>21</integer><key>Minute</key><integer>0</integer></dict>'

unload_all
for label in "${labels[@]}"; do
    launchctl bootstrap "gui/$(id -u)" "$agents/$label.plist"
done

echo "Installed. Data: $data  |  Report now: $bin/aimon-report"
sleep 1
open "http://127.0.0.1:8765"
