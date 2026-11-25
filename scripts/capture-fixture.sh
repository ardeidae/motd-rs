#!/bin/sh
# Captures this machine's state as a motd-rs test fixture: a fake root
# filesystem with the files motd-rs reads, plus a fake.toml holding the
# syscall and systemd answers. The result is anonymised.
#
# Usage: capture-fixture.sh <name> [<output dir>]
#
# Run it on the machine to capture, as a normal user or as root (root sees a
# few more /proc entries). It needs only POSIX sh, awk, sed, stat and date;
# `ip` and `systemctl` are used when present. Nothing on the machine is
# changed: the raw capture goes to a temporary directory that is removed on
# exit, and only the anonymised copy is written to the output directory
# (default: ./<name>).
#
# Anonymisation, consistent across all files:
#   - hostname                    -> <name>  (the hostname file is always
#                                    replaced; elsewhere the word is kept when
#                                    it is the distro ID, e.g. "ubuntu")
#   - human user names (uid >= 1000 and session users) -> user1, user2, ...
#   - IPv4 addresses              -> 192.0.2.x, 198.51.100.x, 203.0.113.x
#                                    (one per /24, last byte kept), also in the
#                                    little-endian hex of /proc/net/*
#   - IPv6 global prefixes        -> 2001:db8:N::/64; interface identifiers
#                                    (which may embed a MAC address) are replaced
#   - interface names enx<MAC>    -> enx0200000000NN
#   - network filesystem servers  -> fileserverN.example
#   - device-mapper names (LVM volume groups often carry the hostname)
#                                 -> /dev/mapper/vgN
# Loopback, link-local and multicast addresses are kept. The script then
# searches the result for the original values and lists any file that still
# contains one. Always read the result before committing it: process names,
# unit descriptions and mount paths are copied as they are.
#
# The hex address decoding assumes a little-endian machine (x86, ARM, RISC-V),
# like the kernel's own /proc/net format on those machines.

set -eu

die() {
    printf 'capture-fixture: %s\n' "$*" >&2
    exit 1
}

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
    die "usage: $0 <name> [<output dir>]"
fi
name=$1
case $name in
    *[!A-Za-z0-9-]* | -* | '') die "name must be letters, digits and '-' (it becomes the hostname)" ;;
esac
out=${2:-./$name}
[ -e "$out" ] && die "$out already exists"
[ "$(uname -s)" = Linux ] || die "Linux only"

umask 022
raw=$(mktemp -d)
trap 'rm -rf "$raw"' EXIT INT TERM
mkdir "$raw/root"
R=$raw/root

# Copies a file into the capture, keeping its path. Missing or unreadable
# files are skipped: their absence is part of what the fixture records.
grab() {
    src=$1
    dst=$R${2:-$1}
    [ -r "$src" ] && [ -f "$src" ] || return 0
    mkdir -p "${dst%/*}"
    cat "$src" >"$dst" 2>/dev/null || rm -f "$dst"
}

# --- Files ------------------------------------------------------------------

for f in \
    /proc/sys/kernel/hostname /proc/sys/kernel/osrelease \
    /proc/uptime /proc/loadavg /proc/meminfo /proc/mdstat \
    /proc/net/route /proc/net/ipv6_route /proc/net/if_inet6 \
    /proc/net/tcp /proc/net/tcp6 /proc/net/udp /proc/net/udp6 \
    /proc/pressure/cpu /proc/pressure/memory /proc/pressure/io \
    /sys/devices/system/cpu/online \
    /run/reboot-required /run/reboot-required.pkgs \
    /proc/2/task/2/children \
    /etc/os-release /usr/lib/os-release; do
    grab "$f"
done

# `cat` reads its own mount table, which is the one motd-rs would see.
mkdir -p "$R/proc/self"
cat /proc/self/mountinfo >"$R/proc/self/mountinfo"

# Every process: field 3 of stat (state, for zombies) and comm (for
# `process:` services). Processes may exit while we read: grab skips them.
for d in /proc/[0-9]*; do
    # This script's own shell is not part of the machine.
    [ "${d#/proc/}" = "$$" ] && continue
    grab "$d/stat"
    grab "$d/comm"
done

# logind sessions. The `*.ref` entries are FIFOs: `grab` keeps regular files only.
for f in /run/systemd/sessions/*; do
    grab "$f"
done

for z in /sys/class/thermal/thermal_zone*; do
    grab "$z/type"
    grab "$z/temp"
done
for h in /sys/class/hwmon/hwmon*; do
    grab "$h/name"
    for t in "$h"/temp*_input "$h"/temp*_label; do
        grab "$t"
    done
done

# Only the directory names matter (installed kernels).
for m in /lib/modules/*/; do
    [ -d "$m" ] || continue
    m=${m%/}
    mkdir -p "$R/lib/modules/${m##*/}"
    # git does not keep empty directories.
    : >"$R/lib/modules/${m##*/}/.gitkeep"
done

# --- fake.toml --------------------------------------------------------------

fake=$R/fake.toml
# A TOML basic string. Built character by character: backslashes in a gsub
# replacement are handled differently by gawk, mawk and busybox awk.
toml_escape='
function esc(s,    r, i, ch) {
    r = ""
    for (i = 1; i <= length(s); i++) {
        ch = substr(s, i, 1)
        if (ch == "\\" || ch == "\"") r = r "\\" ch
        else if (ch == "\t") r = r "\\t"
        else r = r ch
    }
    return "\"" r "\""
}'

{
    printf '# Faked answers for this fixture (see src/fake.rs).\n'
    printf '# Captured with scripts/capture-fixture.sh on %s.\n\n' "$(date -u +%Y-%m-%d)"
    printf '[sys]\n'
    printf 'machine = "%s"\n' "$(uname -m)"
    printf 'now = %s\n' "$(date +%s)"
    printf 'local_time = "%s"\n' "$(LC_ALL=C date +'%a %d %b %H:%M %Z')"
} >"$fake"

# IPv4 addresses as getifaddrs(3) returns them, with the IFF_UP and
# IFF_LOOPBACK flags from the link list.
if command -v ip >/dev/null 2>&1; then
    { ip -o link show 2>/dev/null; printf '%s\n' '--addr--'; ip -o -4 addr show 2>/dev/null; } |
        awk "$toml_escape"'
        $0 == "--addr--" { addr = 1; next }
        !addr {
            n = $2; sub(/:$/, "", n); sub(/@.*/, "", n)
            flags[n] = $3
            next
        }
        {
            n = $2; sub(/@.*/, "", n)
            for (i = 3; i < NF; i++) if ($i == "inet") { a = $(i + 1); break }
            split(a, p, "/")
            # An alias label (eth0:1) is the name getifaddrs reports.
            label = n
            for (i = 3; i <= NF; i++) if (index($i, n ":") == 1) label = $i
            f = flags[n]
            printf "\n[[sys.interfaces]]\nname = %s\naddr = \"%s\"\nprefix_len = %d\n", esc(label), p[1], p[2]
            printf "up = %s\nloopback = %s\n", (f ~ /[<,]UP[,>]/ ? "true" : "false"), (f ~ /LOOPBACK/ ? "true" : "false")
        }' >>"$fake"
elif [ -r /proc/net/fib_trie ] && [ -r /proc/net/route ]; then
    # Minimal systems and containers often have no `ip`. The kernel's routing
    # tables hold the same facts: every local address is a "/32 host LOCAL"
    # leaf of fib_trie, and the link route that contains it gives the
    # interface and the prefix length. Interfaces with an address are up.
    # Two interfaces on the same subnet cannot be told apart this way: the
    # first route wins and a warning says which address to check.
    printf 'capture-fixture: no `ip` command: interfaces read from /proc/net/fib_trie\n' >&2
    { cat /proc/net/route; printf '%s\n' '--trie--'; cat /proc/net/fib_trie; } |
        awk "$toml_escape"'
        function le(h, k) {    # byte k (1 = first) of a little-endian hex address
            return index("0123456789ABCDEF", substr(h, 9 - 2 * k, 1)) * 16 - 16 + index("0123456789ABCDEF", substr(h, 10 - 2 * k, 1)) - 1
        }
        function masked(x, m,    k) {    # x AND m for one byte of a netmask
            if (m == 255) return x
            for (k = 1; 256 - k > m; k *= 2) ;
            return int(x / k) * k
        }
        $0 == "--trie--" { trie = 1; next }
        !trie {
            if (FNR == 1 || $3 != "00000000") next
            nr++; rif[nr] = $1
            for (k = 1; k <= 4; k++) { rd[nr, k] = le(toupper($2), k); rm[nr, k] = le(toupper($8), k) }
            next
        }
        $1 == "|--" { leaf = $2; next }
        $1 == "/32" && $2 == "host" && $3 == "LOCAL" && !(leaf in seen) {
            seen[leaf] = 1
            split(leaf, a, ".")
            if (a[1] == 127) { name = "lo"; plen = 8; lo = "true" }
            else {
                name = ""; plen = -1; lo = "false"
                for (r = 1; r <= nr; r++) {
                    bits = 0; ok = 1
                    for (k = 1; k <= 4; k++) {
                        if (masked(a[k], rm[r, k]) != rd[r, k]) ok = 0
                        for (m = rm[r, k]; m > 0; m = (m * 2) % 256) bits++
                    }
                    if (ok && bits > plen) { name = rif[r]; plen = bits; tie = "" }
                    else if (ok && bits == plen && rif[r] != name) tie = rif[r]
                }
                if (tie != "")
                    printf "capture-fixture: %s is on the subnet of both %s and %s: check its interface in fake.toml\n", leaf, name, tie > "/dev/stderr"
                if (name == "") next
            }
            printf "\n[[sys.interfaces]]\nname = %s\naddr = \"%s\"\nprefix_len = %d\n", esc(name), leaf, plen
            printf "up = true\nloopback = %s\n", lo
        }' >>"$fake"
else
    printf 'capture-fixture: no `ip` command and no /proc/net/fib_trie: [[sys.interfaces]] left empty\n' >&2
fi

# statvfs(3) for each mount point, through `stat -f`. Skipped: autofs (the
# stat would trigger the automount) and pseudo filesystems that hold no
# data. Mount points containing a tab or a newline are skipped too.
have_timeout=
command -v timeout >/dev/null 2>&1 && have_timeout=1
awk '{
    for (i = 7; i <= NF; i++) if ($i == "-") break
    t = $(i + 1)
    if (t ~ /^(autofs|proc|sysfs|cgroup|cgroup2|devpts|mqueue|debugfs|tracefs|securityfs|pstore|bpf|configfs|fusectl|hugetlbfs|binfmt_misc|efivarfs|selinuxfs|rpc_pipefs|nsfs)$/) next
    mp = $5
    if (mp ~ /\\011|\\012/) next
    gsub(/\\040/, " ", mp); gsub(/\\134/, "\\", mp)
    print mp
}' "$R/proc/self/mountinfo" | sort -u | while IFS= read -r mp; do
    if [ -n "$have_timeout" ]; then
        s=$(timeout 2 stat -f -c '%S %b %f %a %c %d' -- "$mp" 2>/dev/null) || continue
    else
        s=$(stat -f -c '%S %b %f %a %c %d' -- "$mp" 2>/dev/null) || continue
    fi
    printf '%s\t%s\n' "$mp" "$s"
done | awk -F '\t' "$toml_escape"'
{
    split($2, s, " ")
    # awk numbers are doubles: the products are exact up to 2^53 bytes (8 PiB).
    printf "\n[sys.statvfs.%s]\n", esc($1)
    printf "total_bytes = %.0f\nfree_bytes = %.0f\navail_bytes = %.0f\n", s[1] * s[2], s[1] * s[3], s[1] * s[4]
    printf "total_inodes = %.0f\nfree_inodes = %.0f\n", s[5], s[6]
}' >>"$fake"

# systemd: every service, plus every failed unit of any type. No
# [systemd] table at all means "no systemd" (containers, OpenRC).
if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    {
        systemctl list-units --all --type=service --no-legend --plain --full --no-pager 2>/dev/null || true
        systemctl list-units --state=failed --no-legend --plain --full --no-pager 2>/dev/null || true
    } | awk "$toml_escape"'
        NF >= 4 && !seen[$1]++ {
            d = $0
            for (i = 1; i <= 4; i++) sub(/^[ \t]*[^ \t]+/, "", d)
            sub(/^[ \t]+/, "", d)
            printf "\n[[systemd.units]]\nname = %s\ndescription = %s\n", esc($1), esc(d)
            printf "load_state = %s\nactive_state = %s\nsub_state = %s\n", esc($2), esc($3), esc($4)
        }' >>"$fake"
fi

# --- Anonymisation ----------------------------------------------------------

# Values to replace with words: the hostname, then human users.
host=$(cat /proc/sys/kernel/hostname)
# The distro's ID (`ubuntu`, `debian`) is also the default user or hostname
# of some images. It identifies nobody, and replacing it would rename units
# such as ubuntu-advantage.service, so it is never replaced.
osid=$(awk -F= '$1 == "ID" { gsub(/"/, "", $2); print $2; exit }' /etc/os-release /usr/lib/os-release 2>/dev/null || true)
{
    printf 'host\t%s\n' "$host"
    case $host in *.*) printf 'host\t%s\n' "${host%%.*}" ;; esac
    { getent passwd 2>/dev/null || cat /etc/passwd; } |
        awk -F: '$3 >= 1000 && $3 < 65534 { print "user\t" $1 }'
    for f in "$R"/run/systemd/sessions/*; do
        if [ -f "$f" ]; then
            awk -F= '$1 == "USER" { print "user\t" $2 }' "$f"
        fi
    done
} >"$raw/words"

(cd "$R" && find . -type f | sed 's|^\./||' | sort) >"$raw/files"
mkdir -p "$out"
(cd "$R" && find . -type d) | while IFS= read -r d; do mkdir -p "$out/$d"; done

awk -v name="$name" -v osid="$osid" -v root="$R" -v out="$out" -v secrets="$raw/secrets" -v hexsecrets="$raw/hexsecrets" '
function hexval(h,    i, v) {
    v = 0
    h = tolower(h)
    for (i = 1; i <= length(h); i++) v = v * 16 + index("0123456789abcdef", substr(h, i, 1)) - 1
    return v
}
# `_` separates words: RHEL names its volume group rhel_<hostname>.
function isword(c) { return c ~ /[A-Za-z0-9]/ }

# Replaces whole-word occurrences of `old` in `s`.
function repword(s, old, new,    r, i, before, after) {
    r = ""
    while ((i = index(s, old)) > 0) {
        before = i > 1 ? substr(s, i - 1, 1) : ""
        after = substr(s, i + length(old), 1)
        if ((before == "" || !isword(before)) && (after == "" || !isword(after))) {
            r = r substr(s, 1, i - 1) new
        } else {
            r = r substr(s, 1, i + length(old) - 1)
        }
        s = substr(s, i + length(old))
    }
    return r s
}
function words(s,    k) {
    for (k = 1; k <= nwords; k++) s = repword(s, wold[k], wnew[k])
    return s
}

# IPv4: one TEST-NET block per original /24, the last byte kept.
function keep4(a, b) { return a == 0 || a == 127 || a >= 224 || (a == 169 && b == 254) }
function map4(a, b, c, d,    key) {
    if (keep4(a, b)) return a "." b "." c "." d
    key = a "." b "." c
    if (!(key in net4)) {
        n4++
        net4[key] = n4 == 1 ? "192.0.2" : n4 == 2 ? "198.51.100" : n4 == 3 ? "203.0.113" : "10.99." (n4 - 4)
    }
    print a "." b "." c "." d > secrets
    return net4[key] "." d
}
# 8 hex digits, little-endian (the /proc/net/{route,tcp,udp} format).
function map4hex(h, upper,    a, b, c, d, r, p) {
    a = hexval(substr(h, 7, 2)); b = hexval(substr(h, 5, 2))
    c = hexval(substr(h, 3, 2)); d = hexval(substr(h, 1, 2))
    if (keep4(a, b)) return h
    print h > hexsecrets
    r = map4(a, b, c, d)
    split(r, p, ".")
    r = sprintf("%02x%02x%02x%02x", p[4], p[3], p[2], p[1])
    return upper ? toupper(r) : r
}
# Dotted IPv4 addresses inside free text.
function dotted(s,    r, m, ip, p, before, after) {
    r = ""
    while (match(s, /[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/)) {
        ip = substr(s, RSTART, RLENGTH)
        before = RSTART > 1 ? substr(s, RSTART - 1, 1) : ""
        after = substr(s, RSTART + RLENGTH, 1)
        split(ip, p, ".")
        if (before !~ /[0-9.]/ && after !~ /[0-9.]/ && p[1] < 256 && p[2] < 256 && p[3] < 256 && p[4] < 256)
            ip = map4(p[1] + 0, p[2] + 0, p[3] + 0, p[4] + 0)
        r = r substr(s, 1, RSTART - 1) ip
        s = substr(s, RSTART + RLENGTH)
    }
    return r s
}

# IPv6, 32 lower-case hex digits in network order.
function map6(h,    pre, iid, v4) {
    if (h ~ /^0+$/ || h == "00000000000000000000000000000001" || substr(h, 1, 2) == "ff") return h
    if (substr(h, 1, 24) == "00000000000000000000ffff") {
        v4 = map4(hexval(substr(h, 25, 2)), hexval(substr(h, 27, 2)), hexval(substr(h, 29, 2)), hexval(substr(h, 31, 2)))
        split(v4, p6, ".")
        return substr(h, 1, 24) sprintf("%02x%02x%02x%02x", p6[1], p6[2], p6[3], p6[4])
    }
    pre = substr(h, 1, 16); iid = substr(h, 17, 16)
    if (pre !~ /^fe[89ab]/ && pre !~ /^0+$/) {
        if (!(pre in pre6)) { npre++; pre6[pre] = "20010db8" sprintf("%04x", npre) "0000"; print pre > hexsecrets }
        pre = pre6[pre]
    }
    # Small identifiers (::1, ::a) say nothing about the machine.
    if (iid !~ /^000000000000/) {
        if (!(iid in iid6)) { niid++; iid6[iid] = "0000000000" sprintf("%06x", 4096 + niid); print iid > hexsecrets }
        iid = iid6[iid]
    }
    return pre iid
}
# /proc/net/{tcp6,udp6}: four 32-bit words, each in host (little-endian) order.
function swap6(h,    r, w, k) {
    r = ""
    for (k = 0; k < 4; k++) {
        w = substr(h, 8 * k + 1, 8)
        r = r substr(w, 7, 2) substr(w, 5, 2) substr(w, 3, 2) substr(w, 1, 2)
    }
    return r
}
function sock(f, v6,    p, a, port) {
    p = index(f, ":"); a = substr(f, 1, p - 1); port = substr(f, p)
    if (v6) return toupper(swap6(map6(tolower(swap6(a))))) port
    return map4hex(a, 1) port
}

# Network filesystem source: keep the path, replace the server.
function netsource(src,    at, host, rest, i) {
    if (substr(src, 1, 2) == "//") {
        i = index(substr(src, 3), "/")
        host = i ? substr(src, 3, i - 1) : substr(src, 3)
        rest = i ? substr(src, 3 + i - 1) : ""
        return "//" server(host) rest
    }
    i = index(src, ":")
    if (!i) return src
    host = substr(src, 1, i - 1); rest = substr(src, i)
    at = ""
    if ((i = index(host, "@")) > 0) { at = substr(host, 1, i); host = substr(host, i + 1) }
    return at server(host) rest
}
function server(h,    p) {
    if (split(h, p, ".") == 4 && h ~ /^[0-9.]+$/) return map4(p[1] + 0, p[2] + 0, p[3] + 0, p[4] + 0)
    if (!(h in servers)) { nserv++; servers[h] = "fileserver" nserv ".example"; print h > secrets }
    return servers[h]
}

BEGIN {
    # Word replacements: hostname first, then users in order of appearance.
    while ((getline line < (root "/../words")) > 0) {
        split(line, w, "\t")
        if (w[2] == "" || w[2] == "root" || w[2] == osid || (w[2] in seenword)) continue
        seenword[w[2]] = 1
        nwords++
        wold[nwords] = w[2]
        wnew[nwords] = w[1] == "host" ? name : "user" (++nusers)
        if (w[2] != wnew[nwords]) print w[2] > secrets
    }
    # Interface names that embed a MAC address (enx001122334455).
    while ((getline f < (root "/../files")) > 0) {
        if (f !~ /^(proc\/net\/|fake\.toml)/) continue
        path = root "/" f
        while ((getline line < path) > 0)
            while (match(line, /enx[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]/)) {
                mac = substr(line, RSTART, RLENGTH)
                if (!(mac in seenword)) {
                    seenword[mac] = 1; nwords++; wold[nwords] = mac
                    wnew[nwords] = sprintf("enx0200000000%02x", ++nmac)
                    print mac > secrets
                }
                line = substr(line, RSTART + RLENGTH)
            }
        close(path)
    }
    close(root "/../files")

    while ((getline f < (root "/../files")) > 0) {
        path = root "/" f
        dest = out "/" f
        n = 0
        while ((getline line < path) > 0) {
            n++
            if (f == "proc/sys/kernel/hostname") line = name
            else if (f == "proc/net/route" && n > 1) {
                split(line, c, "\t")
                line = c[1] "\t" map4hex(c[2], 1) "\t" map4hex(c[3], 1)
                for (k = 4; k in c; k++) line = line "\t" c[k]
            } else if (f ~ /^proc\/net\/(tcp|udp)6?$/ && n > 1) {
                v6 = f ~ /6$/
                split(line, c, " ")
                p = index(line, c[2])
                lead = substr(line, 1, p - 1)
                rest = substr(line, p + length(c[2]))
                rest = substr(rest, index(rest, c[3]) + length(c[3]))
                line = lead sock(c[2], v6) " " sock(c[3], v6) rest
            } else if (f == "proc/net/if_inet6") {
                line = map6(substr(line, 1, 32)) substr(line, 33)
            } else if (f == "proc/net/ipv6_route") {
                line = map6(substr(line, 1, 32)) substr(line, 33, 4) map6(substr(line, 37, 32)) substr(line, 69, 4) map6(substr(line, 73, 32)) substr(line, 105)
            } else if (f == "proc/self/mountinfo") {
                k = split(line, c, " ")
                for (i = 7; i <= k; i++) if (c[i] == "-") break
                if (c[i + 1] ~ /^(nfs|nfs4|cifs|smb3|smbfs|ceph|glusterfs|fuse\.sshfs|fuse\.glusterfs)$/) {
                    p = index(line, " - " c[i + 1] " " c[i + 2])
                    line = substr(line, 1, p - 1) " - " c[i + 1] " " netsource(c[i + 2]) substr(line, p + length(" - " c[i + 1] " " c[i + 2]))
                } else if (index(c[i + 2], "/dev/mapper/") == 1) {
                    if (!(c[i + 2] in dm)) { dm[c[i + 2]] = "/dev/mapper/vg" (++ndm); print substr(c[i + 2], 13) > secrets }
                    p = index(line, " - " c[i + 1] " " c[i + 2] " ")
                    line = substr(line, 1, p - 1) " - " c[i + 1] " " dm[c[i + 2]] substr(line, p + length(" - " c[i + 1] " " c[i + 2]))
                }
                line = dotted(line)
            } else if (f ~ /^run\/systemd\/sessions\// && line ~ /^REMOTE_HOST=/) {
                v = substr(line, 13)
                line = "REMOTE_HOST=" (v ~ /^[0-9.]+$/ ? dotted(v) : server(v))
            } else if (f == "fake.toml") {
                line = dotted(line)
            }
            # os-release must stay true even when the hostname is "debian".
            if (f !~ /os-release$|^proc\/sys\/kernel\/osrelease$/) line = words(line)
            print line > dest
        }
        close(path)
        close(dest)
        # Keep empty files empty (e.g. /run/reboot-required).
        if (n == 0) printf "" > dest
        close(dest)
    }
}' </dev/null

# --- Self-check -------------------------------------------------------------

leaks=
if [ -s "$raw/secrets" ]; then
    # os-release is left as it is, even when the hostname is a distro name.
    leaks=$(grep -rlwF -f "$raw/secrets" "$out" 2>/dev/null |
        grep -v -e '/os-release$' -e '/proc/sys/kernel/osrelease$' || true)
fi
if [ -s "$raw/hexsecrets" ]; then
    # /proc files mix cases: search both.
    { cat "$raw/hexsecrets"; tr 'a-f' 'A-F' <"$raw/hexsecrets"; tr 'A-F' 'a-f' <"$raw/hexsecrets"; } | sort -u >"$raw/hex"
    leaks="$leaks
$(grep -rlF -f "$raw/hex" "$out/proc/net" "$out/fake.toml" 2>/dev/null || true)"
fi
leaks=$(printf '%s\n' "$leaks" | sed '/^$/d' | sort -u)

files=$(find "$out" -type f | wc -l)
printf 'capture-fixture: %s files written to %s\n' "$files" "$out"
if [ -n "$leaks" ]; then
    printf 'capture-fixture: WARNING: original values remain in:\n%s\n' "$leaks" >&2
    printf 'capture-fixture: fix them by hand before using the fixture.\n' >&2
    exit 2
fi
printf 'capture-fixture: no original hostname, user name or address found. Still read the files before committing.\n'
printf 'capture-fixture: to copy it: tar czf %s.tar.gz -C %s .\n' "$name" "$out"
