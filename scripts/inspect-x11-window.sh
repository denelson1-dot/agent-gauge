#!/usr/bin/env bash
set -eu

window_id="${1:-}"

if [ -z "$window_id" ]; then
  window_id="$(wmctrl -lx | awk 'BEGIN { IGNORECASE = 1 } /agent-gauge.Agent-gauge/ && $0 !~ /Settings/ { print $1; exit }')"
fi

if [ -z "$window_id" ]; then
  echo "Agent Gauge widget window not found. Start the app or pass its X11 window ID." >&2
  exit 1
fi

echo "Agent Gauge X11 window: $window_id"
xprop -id "$window_id" \
  WM_CLASS \
  _NET_WM_NAME \
  _NET_WM_STATE \
  _NET_WM_DESKTOP \
  _NET_WM_WINDOW_TYPE
