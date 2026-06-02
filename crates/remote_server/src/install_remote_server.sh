#!/usr/bin/env bash
# Installs the Waz CLI binary on the remote host, used for remote-server-proxy.
#
# setup.rs will replace these placeholders at runtime:
#   {download_base_url}     - e.g., https://github.com/zerx-lab/warp/releases/latest/download
#   {install_dir}           - e.g., ~/.waz/remote-server
#   {binary_name}           - e.g., waz-oss
#   {version_suffix}        - e.g., -v0.2026..., empty if there is no release tag
#   {staging_tarball_path}  - SCP fallback pre-uploaded tarball path, empty for normal download paths
set -e

arch=$(uname -m)
case "$arch" in
  x86_64|amd64)  arch_name=x86_64 ;;
  aarch64|arm64) arch_name=aarch64 ;;
  *) echo "unsupported arch: $arch" >&2; exit 2 ;;
esac

os_kernel=$(uname -s)
case "$os_kernel" in
  Darwin) os_name=macos ;;
  Linux)  os_name=linux ;;
  *) echo "unsupported OS: $os_kernel" >&2; exit 2 ;;
esac

install_dir="{install_dir}"
case "$install_dir" in
  "~"|"~/"*) install_dir="${HOME}${install_dir#\~}" ;;
esac
mkdir -p "$install_dir"

tmpdir=$(mktemp -d "$install_dir/.install.XXXXXX")
# Clean up the staging directory on a best-effort basis. Failure here must not override the actual installation result:
# When trap is triggered, either the binary has already been moved to the final path, or the script has failed
# for other reasons. The errors of the latter are more worth exposing to the caller.
cleanup() {
  rm -rf "$tmpdir" 2>/dev/null || true
}
trap cleanup EXIT

staging_tarball_path="{staging_tarball_path}"
if [ -n "$staging_tarball_path" ]; then
  case "$staging_tarball_path" in
    "~"|"~/"*) staging_tarball_path="${HOME}${staging_tarball_path#\~}" ;;
  esac
  mv "$staging_tarball_path" "$tmpdir/waz.tar.gz"
else
  url="{download_base_url}/waz-$os_name-$arch_name.tar.gz"
  if command -v curl >/dev/null 2>&1; then
    curl -fSL --connect-timeout 15 "$url" -o "$tmpdir/waz.tar.gz"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$tmpdir/waz.tar.gz" "$url"
  else
    echo "error: neither curl nor wget is available" >&2
    exit 3
  fi
fi

tar -xzf "$tmpdir/waz.tar.gz" -C "$tmpdir"

bin="$tmpdir/{binary_name}"
if [ ! -f "$bin" ]; then
  bin=$(find "$tmpdir" -type f \( -name 'waz-oss' -o -name 'warp-oss' -o -name 'oz*' \) ! -path "$tmpdir/resources/*" ! -name '*.tar.gz' | head -n1)
fi
if [ -z "$bin" ]; then echo "no binary found in tarball" >&2; exit 1; fi
chmod +x "$bin"
mv "$bin" "$install_dir/{binary_name}{version_suffix}"
