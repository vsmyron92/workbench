#!/bin/sh
# Installs the Workbench binary from a release archive into ~/.local/bin (or $PREFIX/bin).
# Run it from the unpacked archive: ./install.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="${PREFIX:-$HOME/.local}/bin"

if [ ! -x "$here/workbench" ]; then
  echo "install.sh: no workbench binary next to this script ($here)" >&2
  exit 1
fi

mkdir -p "$bin"
# Copy, then rename over the old binary: a running Workbench keeps its file.
cp "$here/workbench" "$bin/.workbench.new"
chmod 0755 "$bin/.workbench.new"
mv -f "$bin/.workbench.new" "$bin/workbench"
echo "Installed $("$bin/workbench" --version) to $bin/workbench"

case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "Note: $bin is not on your PATH. Add it, e.g. in ~/.profile: export PATH=\"$bin:\$PATH\"" ;;
esac

if command -v systemctl >/dev/null 2>&1 && systemctl --user is-active --quiet workbench.service 2>/dev/null; then
  echo "The Workbench service is running. Restart it to use the new version:"
  echo "  systemctl --user restart workbench.service"
else
  echo "Start it with:  workbench serve --open"
  echo "Or with your session:  workbench service install --enable"
fi
