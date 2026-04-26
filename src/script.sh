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

# Swap may be absent on containers / hosts with no swap; emit empty value
# in that case so the binary records it as "metric unavailable".
free | awk '/^Swap:/ {if ($2 > 0) printf "swap_pct=%.0f\n", $3/$2*100; else print "swap_pct="}'

# Process count via ps. NR-1 strips the header row; using `wc -l` would
# over-count by one and trip thresholds spuriously.
ps -e | awk 'END{print "proc_count=" NR-1}'

# Primary IPv4 / IPv6. `hostname -I` returns all assigned global addresses
# space-separated; we take the first. Empty value when unavailable (minimal
# distros without `hostname -I`, hosts with no global address, etc).
ip_addr="$(hostname -I 2>/dev/null | awk '{print $1}')"
echo "ip_addr=${ip_addr}"
