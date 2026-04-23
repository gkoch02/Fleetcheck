#!/bin/sh
# Emits one key=value pair per line. Empty value means "metric unavailable".
set -u

awk '{print "uptime_secs=" int($1)}' /proc/uptime

df -P / | awk 'NR==2 {gsub("%","",$5); print "disk_pct=" $5}'

if [ -r /sys/class/thermal/thermal_zone0/temp ]; then
    awk '{print "temp_millic=" $1}' /sys/class/thermal/thermal_zone0/temp
else
    echo "temp_millic="
fi

awk '{print "load_1m=" $1}' /proc/loadavg

free | awk '/^Mem:/ {printf "mem_pct=%.0f\n", $3/$2*100}'
