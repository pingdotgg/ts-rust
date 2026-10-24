#!/bin/bash
# thpguard3: print what a host reports about THP, free page reporting and the balloon. Read-only.
# usage: sysinfo.sh   (uses sudo -n for dmesg when it can)
s() { echo "== $1"; shift; "$@" 2>&1 | head -40; }
s host bash -c 'echo "$(hostname) up $(cut -d" " -f1 /proc/uptime)s nproc $(nproc) kernel $(uname -r)"; grep -m1 "model name" /proc/cpuinfo'
s virt bash -c 'systemd-detect-virt 2>/dev/null; cat /sys/class/dmi/id/sys_vendor /sys/class/dmi/id/product_name 2>/dev/null; grep -m1 -o hypervisor /proc/cpuinfo; lscpu 2>/dev/null | grep -i "hypervisor vendor"'
s cmdline cat /proc/cmdline
s thp bash -c 'for f in enabled defrag shmem_enabled khugepaged/defrag; do echo "$f: $(cat /sys/kernel/mm/transparent_hugepage/$f)"; done'
s page_reporting bash -c 'ls /sys/module/page_reporting/parameters/ 2>/dev/null && cat /sys/module/page_reporting/parameters/* ; echo "free_page_reporting module dir: $(ls -d /sys/module/page_reporting 2>/dev/null || echo none)"'
s init_on_alloc bash -c 'grep -o "init_on_alloc=[01]\|init_on_free=[01]" /proc/cmdline; zcat /proc/config.gz 2>/dev/null | grep -E "INIT_ON_ALLOC_DEFAULT_ON|PAGE_REPORTING|VIRTIO_BALLOON|HYPERV_BALLOON" ; grep -sE "INIT_ON_ALLOC_DEFAULT_ON|CONFIG_PAGE_REPORTING|VIRTIO_BALLOON|HYPERV_BALLOON" /boot/config-$(uname -r)'
s virtio bash -c 'for d in /sys/bus/virtio/devices/*; do [ -e "$d" ] || continue; dev=$(cat $d/device); drv=$(basename "$(readlink -f $d/driver 2>/dev/null)"); f=$(cat $d/features); echo "$(basename $d) device=$dev driver=$drv features=$f"; if [ "$dev" = 0x0005 ]; then echo "  balloon bits: must_tell_host=${f:0:1} stats_vq=${f:1:1} deflate_on_oom=${f:2:1} free_page_hint=${f:3:1} page_poison=${f:4:1} reporting=${f:5:1}"; fi; done'
s hv_balloon bash -c 'ls /sys/bus/vmbus/drivers/ 2>/dev/null; ls /sys/bus/vmbus/drivers/hv_balloon 2>/dev/null | head -5'
s modules bash -c 'lsmod 2>/dev/null | grep -iE "balloon|virtio_mem|hv_" ; grep -E "balloon|virtio_mem" /proc/modules'
s dmesg bash -c 'd=$(dmesg 2>/dev/null || sudo -n dmesg 2>/dev/null); echo "$d" | grep -iE "page.report|balloon|virtio_mem|cold memory|hv_balloon|Dynamic Memory|init_on_alloc|Hypervisor detected|Booting paravirtualized|Command line" | head -20'
s memory bash -c 'free -m; grep -E "MemTotal|MemFree|^Cached|AnonHugePages" /proc/meminfo; cat /proc/buddyinfo'
s free2m awk '{ for (i = 14; i <= NF; i++) t += $i * (2 ^ (i - 5)) * 4 } END { printf "%.0f MiB free in 2 MiB blocks\n", t / 1024 }' /proc/buddyinfo
