# What the watch asks of this Plugin, and the case it answers.
PLUGIN=com.yoke.hw-monitor
UNIT=hw-monitor
CASE=yoke-plugin-hw-monitor:the-watch.01

# std: yoke-plugin-hw-monitor:the-watch.01
probe() { # <instance>
  sleep 2
  local said; said="$(yoke unit show "$UNIT" --instance "$1" 2>&1)" || { echo "the unit cannot be read: $said"; return 1; }
  grep -q '"state": "Running"' <<<"$said" || { echo "the unit is not Running: $said"; return 1; }
}
