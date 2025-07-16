#!/bin/sh
set -eu

APP_NAME="meerai"
REPO="packvisionxyz/meerai"
BASE_URL="${MEERAI_DOWNLOAD_BASE_URL:-https://github.com/$REPO/releases}"
VERSION="${MEERAI_VERSION:-latest}"
NO_MODIFY_PATH="${MEERAI_NO_MODIFY_PATH:-}"
QUIET="${MEERAI_QUIET:-0}"

log() {
    if [ "$QUIET" != "1" ]; then
        printf '%s\n' "$*"
    fi
}

err() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || err "required command '$1' was not found"
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        -q|--quiet)
            QUIET=1
            ;;
        -v|--version)
            [ "$#" -ge 2 ] || err "--version requires a value"
            VERSION="$2"
            shift
            ;;
        -h|--help)
            cat <<'EOF'
Install meerai

USAGE:
    install.sh [OPTIONS]

OPTIONS:
    -v, --version <VERSION>  Install a specific version tag (e.g. 0.1.0 or v0.1.0)
    -q, --quiet              Suppress informational output
    -h, --help               Print this help message

ENV:
    MEERAI_INSTALL_DIR         Override destination bin directory
    MEERAI_VERSION             Override version to install (default: latest)
    MEERAI_NO_MODIFY_PATH      Set to non-empty to skip modifying shell profiles
    MEERAI_DOWNLOAD_BASE_URL   Override GitHub Releases base URL
EOF
            exit 0
            ;;
        *)
            err "unknown option: $1"
            ;;
    esac
    shift
done

need_cmd uname
need_cmd tar
need_cmd mktemp
need_cmd chmod
need_cmd mkdir
need_cmd mv

if command -v curl >/dev/null 2>&1; then
    DOWNLOADER="curl"
elif command -v wget >/dev/null 2>&1; then
    DOWNLOADER="wget"
else
    err "either 'curl' or 'wget' is required to download $APP_NAME"
fi

download_file() {
    url="$1"
    dest="$2"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl --proto '=https' --tlsv1.2 -LsSf "$url" -o "$dest" \
            || err "failed to download $url"
    else
        wget --https-only -qO "$dest" "$url" \
            || err "failed to download $url"
    fi
}

verify_sha256() {
    archive_path="$1"
    checksum_path="$2"
    expected=$(awk '{print $1}' "$checksum_path")
    [ -n "$expected" ] || err "empty checksum file: $checksum_path"

    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$archive_path" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$archive_path" | awk '{print $1}')
    elif command -v openssl >/dev/null 2>&1; then
        actual=$(openssl dgst -sha256 -r "$archive_path" | awk '{print $1}')
    else
        err "cannot verify checksum: need sha256sum, shasum, or openssl"
    fi

    [ "$actual" = "$expected" ] || err "checksum mismatch for $archive_path (expected $expected, got $actual)"
}

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)

    case "$os" in
        Linux)
            case "$arch" in
                x86_64|amd64)
                    printf '%s' "x86_64-unknown-linux-gnu"
                    ;;
                aarch64|arm64)
                    printf '%s' "aarch64-unknown-linux-gnu"
                    ;;
                *)
                    err "unsupported Linux architecture: $arch"
                    ;;
            esac
            ;;
        Darwin)
            case "$arch" in
                x86_64)
                    if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
                        printf '%s' "aarch64-apple-darwin"
                    else
                        printf '%s' "x86_64-apple-darwin"
                    fi
                    ;;
                arm64|aarch64)
                    printf '%s' "aarch64-apple-darwin"
                    ;;
                *)
                    err "unsupported macOS architecture: $arch"
                    ;;
            esac
            ;;
        *)
            err "unsupported operating system: $os"
            ;;
    esac
}

resolve_install_dir() {
    if [ -n "${MEERAI_INSTALL_DIR:-}" ]; then
        printf '%s' "$MEERAI_INSTALL_DIR"
    elif [ -n "${XDG_BIN_HOME:-}" ]; then
        printf '%s' "$XDG_BIN_HOME"
    elif [ -n "${XDG_DATA_HOME:-}" ]; then
        printf '%s' "$XDG_DATA_HOME/../bin"
    else
        printf '%s' "$HOME/.local/bin"
    fi
}

add_to_path_file() {
    profile="$1"
    line="$2"
    if [ -f "$profile" ] && grep -Fqs "$line" "$profile"; then
        return 0
    fi
    if [ -f "$profile" ] || [ "$3" = "create" ]; then
        mkdir -p "$(dirname "$profile")"
        printf '\n%s\n' "$line" >> "$profile"
        return 0
    fi
    return 1
}

update_shell_path() {
    bin_dir="$1"
    case ":$PATH:" in
        *":$bin_dir:"*)
            return 0
            ;;
    esac
    [ -z "$NO_MODIFY_PATH" ] || return 0

    export_line="export PATH=\"$bin_dir:\$PATH\""
    updated=""

    shell_name=$(basename "${SHELL:-sh}")
    case "$shell_name" in
        zsh)
            if add_to_path_file "$HOME/.zshrc" "$export_line" "create"; then
                updated="$HOME/.zshrc"
            fi
            ;;
        bash)
            if [ -f "$HOME/.bashrc" ]; then
                add_to_path_file "$HOME/.bashrc" "$export_line" "skip" && updated="$HOME/.bashrc"
            elif [ -f "$HOME/.bash_profile" ]; then
                add_to_path_file "$HOME/.bash_profile" "$export_line" "skip" && updated="$HOME/.bash_profile"
            else
                add_to_path_file "$HOME/.profile" "$export_line" "create" && updated="$HOME/.profile"
            fi
            ;;
        fish)
            fish_line="fish_add_path \"$bin_dir\""
            if add_to_path_file "${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish" "$fish_line" "create"; then
                updated="${XDG_CONFIG_HOME:-$HOME/.config}/fish/config.fish"
            fi
            ;;
        *)
            if add_to_path_file "$HOME/.profile" "$export_line" "create"; then
                updated="$HOME/.profile"
            fi
            ;;
    esac

    if [ -n "$updated" ]; then
        log "Added $bin_dir to PATH in $updated"
        log "Restart your shell or run: export PATH=\"$bin_dir:\$PATH\""
    else
        log "Add $bin_dir to your PATH to run '$APP_NAME'"
    fi
}

check_requirement() {
    cmd="$1"
    reason="$2"
    setup="$3"
    if ! command -v "$cmd" >/dev/null 2>&1; then
        printf 'warning: missing requirement '\''%s'\'' (%s)\n' "$cmd" "$reason" >&2
        printf '         %s\n' "$setup" >&2
    fi
}

check_requirements() {
    # BEGIN GENERATED REQUIREMENTS
    check_requirement 'git' 'tracks changed files in the TUI File Changes panel' 'Install Git (https://git-scm.com/downloads) and ensure '\''git'\'' is on your PATH.'
    # END GENERATED REQUIREMENTS
}

TARGET=$(detect_target)
ARCHIVE="$APP_NAME-$TARGET.tar.gz"
INSTALL_DIR=$(resolve_install_dir)

if [ "$VERSION" = "latest" ]; then
    ARCHIVE_URL="$BASE_URL/latest/download/$ARCHIVE"
    CHECKSUM_URL="$BASE_URL/latest/download/$ARCHIVE.sha256"
else
    case "$VERSION" in
        v*) TAG="$VERSION" ;;
        *) TAG="v$VERSION" ;;
    esac
    ARCHIVE_URL="$BASE_URL/download/$TAG/$ARCHIVE"
    CHECKSUM_URL="$BASE_URL/download/$TAG/$ARCHIVE.sha256"
fi

TMP_DIR=$(mktemp -d)
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT INT TERM

log "Downloading $APP_NAME ($TARGET)..."
download_file "$ARCHIVE_URL" "$TMP_DIR/$ARCHIVE"
download_file "$CHECKSUM_URL" "$TMP_DIR/$ARCHIVE.sha256"

verify_sha256 "$TMP_DIR/$ARCHIVE" "$TMP_DIR/$ARCHIVE.sha256"

tar -xzf "$TMP_DIR/$ARCHIVE" -C "$TMP_DIR"
[ -f "$TMP_DIR/$APP_NAME" ] || err "archive did not contain expected binary '$APP_NAME'"

mkdir -p "$INSTALL_DIR"
chmod +x "$TMP_DIR/$APP_NAME"
mv -f "$TMP_DIR/$APP_NAME" "$INSTALL_DIR/$APP_NAME"

log "Installed $APP_NAME to $INSTALL_DIR/$APP_NAME"
update_shell_path "$INSTALL_DIR"
check_requirements
log "Run '$APP_NAME' to start."
