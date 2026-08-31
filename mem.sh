#!/bin/bash

for p in $(pgrep -x 'Xephyr|opusdm-hub|opusdm-lister'); do
  awk '/^Pss:/ {s+=$2} END {print FILENAME, s" kB"}' /proc/$p/smaps_rollup
done

# Group total
pids=$(pgrep -x 'Xephyr|opusdm-hub|opusdm-lister' | tr '\n' ',')
awk '/^Pss:/ {s+=$2} END {printf "%.1f MiB PSS\n", s/1024}' \
  /proc/{$(echo $pids | sed 's/,$//')}/smaps_rollup 2>/dev/null
