#!/usr/bin/env bash
# Pre-installation check for the Waz remote-server binary.
#
# Outputs a structured key=value summary to stdout. Exit code 0 indicates detection completed;
# non-zero indicates detection process failed, and the client will treat it as `status=unknown` and fail open.
#
# Important: The Waz Linux remote-server is now built statically linked with target `x86_64-unknown-linux-musl`
# by waz_release.yml (static-musl). The artifact does not depend on the host's dynamic libc, so it can run
# on any Linux x86_64 host — including older glibc distributions (CentOS 7 = 2.17, Amazon Linux 2 = 2.26,
# Ubuntu 20.04 / Debian 11 = 2.31) as well as musl-based distributions (Alpine, etc.).
#
# Since the binary is static, libc detection is no longer used as a gate, but is kept solely for telemetry.

set -u

# Legacy field: Retain required_glibc to be compatible with parsing logic of older clients.
# A static musl binary actually has no glibc floor, so this is just output for backward compatibility
# and no longer participates in the status determination below.
required_glibc="2.17"
echo "required_glibc=${required_glibc}"

# 1. Identify the libc family, and identify version in glibc scenarios (telemetry only, does not affect status).
libc_family="unknown"
libc_version=""

if version=$(getconf GNU_LIBC_VERSION 2>/dev/null); then
    # Output looks like: "glibc 2.35"
    libc_family="glibc"
    libc_version="${version##* }"
elif ldd_out=$(ldd --version 2>&1 | head -n1); then
    case "$ldd_out" in
        *musl*)   libc_family="musl"   ;;
        *uClibc*) libc_family="uclibc" ;;
        *)
            v=$(printf '%s\n' "$ldd_out" | grep -oE '[0-9]+\.[0-9]+' | head -n1)
            if [ -n "$v" ]; then
                libc_family="glibc"
                libc_version="$v"
            fi
            ;;
    esac
fi

echo "libc_family=${libc_family}"
[ -n "$libc_version" ] && echo "libc_version=${libc_version}"

# 2. Determine support status.
#
# remote-server is a static musl binary and does not link to host libc, so any glibc version
# (including below 2.35) as well as musl / uclibc hosts can run it. As long as it is successfully
# identified as a Linux x86_64 host, report `supported`; if no libc clues can be detected (lacking both
# getconf and ldd), fall back to `unknown` so the client can fail open and attempt installation as usual.
status="unknown"
reason=""

if [ "$libc_family" = "glibc" ] \
   || [ "$libc_family" = "musl" ] \
   || [ "$libc_family" = "uclibc" ] \
   || [ "$libc_family" = "bionic" ]; then
    status="supported"
fi

echo "status=${status}"
if [ -n "$reason" ]; then
    echo "reason=${reason}"
fi
