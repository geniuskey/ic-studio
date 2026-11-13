#!/bin/sh
# IC Studio installer for macOS (Apple silicon) and Linux (x86_64).
#
#   curl -fsSL https://github.com/geniuskey/ic-studio/releases/latest/download/install.sh | sh
#
# Installs the app (ic-studio) and the command line tool (ics, CLI and MCP server) from a
# GitHub release into one folder, links them into ~/.local/bin, adds a desktop entry on
# Linux and registers the MCP server (ics mcp) with Claude Code when `claude` is on the PATH.
#
# Options (environment variables):
#   ICS_VERSION      release tag to install, e.g. v0.2.0 (default: latest)
#   ICS_INSTALL_DIR  install folder (default: ~/.local/share/ic-studio)
#   ICS_UNINSTALL=1  remove what this script installed

set -eu

REPO="geniuskey/ic-studio"
BINS="ic-studio ics"
INSTALL_DIR="${ICS_INSTALL_DIR:-$HOME/.local/share/ic-studio}"
BIN_DIR="$HOME/.local/bin"
DESKTOP_FILE="$HOME/.local/share/applications/ic-studio.desktop"

say() { printf '==> %s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

download() {
    if has curl; then
        curl -fL --proto '=https' --tlsv1.2 --progress-bar -o "$2" "$1"
    elif has wget; then
        wget -q --show-progress -O "$2" "$1" 2>/dev/null || wget -q -O "$2" "$1"
    else
        err "curl or wget is required"
    fi
}

uninstall() {
    say "Uninstalling IC Studio"
    for b in $BINS; do
        link="$BIN_DIR/$b"
        if [ -L "$link" ] && [ "$(readlink "$link")" = "$INSTALL_DIR/$b" ]; then
            rm -f "$link"
        fi
    done
    if [ -d "$INSTALL_DIR" ]; then
        rm -rf "$INSTALL_DIR"
        echo "Removed $INSTALL_DIR"
    fi
    if [ -f "$DESKTOP_FILE" ]; then
        rm -f "$DESKTOP_FILE"
        echo "Removed $DESKTOP_FILE"
    fi
    if has claude; then
        if claude mcp remove --scope user ic-studio </dev/null >/dev/null 2>&1; then
            echo "Removed the ic-studio MCP server from Claude Code"
        fi
    fi
    echo "IC Studio is uninstalled. Projects and settings are kept."
}

main() {
    if [ "${ICS_UNINSTALL:-}" = "1" ]; then
        uninstall
        return
    fi

    os="$(uname -s)"
    arch="$(uname -m)"
    case "$os/$arch" in
        Linux/x86_64 | Linux/amd64) platform="linux-x86_64" ;;
        Darwin/arm64 | Darwin/aarch64) platform="macos-aarch64" ;;
        Darwin/x86_64)
            err "macOS on Intel (x86_64) is not built. Build from source: https://github.com/$REPO/blob/main/docs/install.md" ;;
        *)
            err "unsupported platform $os/$arch. Release builds exist for Linux x86_64 and macOS arm64; see https://github.com/$REPO/blob/main/docs/install.md to build from source." ;;
    esac

    has tar || err "tar is required"

    asset="ic-studio-$platform.tar.gz"
    if [ -n "${ICS_VERSION:-}" ]; then
        url="https://github.com/$REPO/releases/download/$ICS_VERSION/$asset"
    else
        url="https://github.com/$REPO/releases/latest/download/$asset"
    fi

    tmp="$(mktemp -d 2>/dev/null || mktemp -d -t ic-studio)"
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' INT TERM

    say "Downloading $url"
    download "$url" "$tmp/$asset" || err "download failed: $url"

    say "Extracting"
    mkdir -p "$tmp/unpacked"
    tar -xzf "$tmp/$asset" -C "$tmp/unpacked"
    for b in $BINS; do
        [ -f "$tmp/unpacked/$b" ] || err "the archive does not contain $b"
    done

    say "Installing to $INSTALL_DIR"
    mkdir -p "$INSTALL_DIR" "$BIN_DIR"
    for b in $BINS; do
        # Replace the file instead of overwriting it in place, so a running copy keeps working.
        cp "$tmp/unpacked/$b" "$INSTALL_DIR/$b.new"
        chmod 755 "$INSTALL_DIR/$b.new"
        mv -f "$INSTALL_DIR/$b.new" "$INSTALL_DIR/$b"
        ln -sf "$INSTALL_DIR/$b" "$BIN_DIR/$b"
    done

    if [ "$os" = "Darwin" ]; then
        xattr -dr com.apple.quarantine "$INSTALL_DIR" 2>/dev/null || true
    fi

    if [ "$os" = "Linux" ]; then
        mkdir -p "$(dirname "$DESKTOP_FILE")"
        cat >"$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=IC Studio
Comment=Full-custom IC design: schematic, simulation and layout
Exec="$INSTALL_DIR/ic-studio" %F
Terminal=false
Categories=Development;Electronics;Engineering;
EOF
        if has update-desktop-database; then
            update-desktop-database "$(dirname "$DESKTOP_FILE")" >/dev/null 2>&1 || true
        fi
    fi

    ics="$INSTALL_DIR/ics"
    mcp_status=""
    if has claude; then
        say "Registering the MCP server with Claude Code"
        claude mcp remove --scope user ic-studio </dev/null >/dev/null 2>&1 || true
        if claude mcp add --scope user ic-studio -- "$ics" mcp </dev/null >/dev/null 2>&1; then
            mcp_status="registered with Claude Code as \"ic-studio\" (user scope)"
        else
            mcp_status="registration with Claude Code failed"
        fi
    fi

    echo
    echo "IC Studio is installed."
    echo "  Folder:   $INSTALL_DIR (linked into $BIN_DIR)"
    echo "  App:      ic-studio"
    echo "  CLI:      ics --help   (MCP server: ics mcp)"
    if [ -n "$mcp_status" ]; then
        echo "  MCP:      $mcp_status"
    fi
    case "$mcp_status" in
        "" | *failed*)
            echo "  MCP:      to register with Claude Code, run"
            echo "              claude mcp add --scope user ic-studio -- \"$ics\" mcp"
            echo "            for other MCP clients, add"
            echo "              { \"mcpServers\": { \"ic-studio\": { \"command\": \"$ics\", \"args\": [\"mcp\"] } } }"
            ;;
    esac
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            echo "  PATH:     $BIN_DIR is not on your PATH. Add this line to your shell profile"
            echo "            (~/.zshrc, ~/.bashrc or ~/.profile) and open a new terminal:"
            echo "              export PATH=\"\$HOME/.local/bin:\$PATH\""
            ;;
    esac
}

main "$@"
