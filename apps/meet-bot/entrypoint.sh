#!/bin/sh
set -e
Xvfb :99 -screen 0 1280x800x24 -nolisten tcp >/dev/null 2>&1 &
# AudioContext needs an output device; a null sink is enough.
pulseaudio --daemonize --exit-idle-time=-1 >/dev/null 2>&1 || true
pactl load-module module-null-sink sink_name=null >/dev/null 2>&1 || true
sleep 1
exec node /app/join.cjs
