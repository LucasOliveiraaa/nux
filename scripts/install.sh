#!/usr/bin/env bash
#
# nux installer
#
#   curl -fsSL https://github.com/LucasOliveiraaa/nux/releases/latest/download/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- --global --yes
#
# Run with --help for all options.
set -euo pipefail

REPO="LucasOliveiraaa/nux"
APP="nux"
NUX_HOME_BIN="${HOME}/.nux/bin"   # where `nux sync` links installed programs

VERSION="${NUX_VERSION:-latest}"
INSTALL_DIR="${NUX_INSTALL_DIR:-}"
MODE=""            # user | system | custom (empty = ask)
ASSUME_YES=0
VERIFY=1
ACTION="install"   # install | uninstall

TMP_DIR=""
USE_SUDO=0

# ---------------------------------------------------------------- output ----
if [[ -t 1 && -z "${NO_COLOR:-}" ]]; then
    BOLD=$'\033[1m'; DIM=$'\033[2m'; RED=$'\033[31m'
    GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RESET=$'\033[0m'
else
    BOLD=""; DIM=""; RED=""; GREEN=""; YELLOW=""; RESET=""
fi

info() { printf '%s\n' "$*"; }
ok()   { printf '%s✔%s %s\n' "$GREEN" "$RESET" "$*"; }
warn() { printf '%s!%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%s✘ %s%s\n' "$RED" "$*" "$RESET" >&2; exit 1; }

have() { command -v "$1" >/dev/null 2>&1; }

usage() {
    cat <<USAGE
${BOLD}${APP} installer${RESET}

Usage: install.sh [options]

Options:
  --local, --user      Install for the current user (~/.local/bin)
  --global, --system   Install for all users (/usr/local/bin, may use sudo)
  --dir <path>         Install into a custom directory
  --version <x.y.z>    Install a specific version (default: latest)
  --no-verify          Skip SHA-256 checksum verification
  --uninstall          Remove a previously installed ${APP}
  -y, --yes            Never prompt; accept defaults
  -h, --help           Show this help

Environment:
  NUX_VERSION          Same as --version
  NUX_INSTALL_DIR      Same as --dir

Without --local/--global/--dir the installer asks interactively (when a
terminal is available) and otherwise defaults to a local install
(or a global one when run as root).
USAGE
}

# ----------------------------------------------------------- interaction ----
# Talks to the controlling terminal so prompts work even when the script is
# piped into bash (stdin is then the script itself).
can_prompt() {
    [[ $ASSUME_YES -eq 0 ]] && { : </dev/tty >/dev/tty; } 2>/dev/null
}

ask() { # ask "prompt" "default" -> echoes answer
    local answer=""
    if can_prompt; then
        read -r -p "$1" answer </dev/tty >/dev/tty || answer=""
    fi
    printf '%s' "${answer:-$2}"
}

ask_yes_no() { # ask_yes_no "question" default(y|n)
    local def="$2" hint="[y/N]" answer
    [[ "$def" == "y" ]] && hint="[Y/n]"
    if ! can_prompt; then
        [[ "$def" == "y" ]]
        return
    fi
    answer="$(ask "$1 $hint " "$def")"
    [[ "$answer" =~ ^[Yy] ]]
}

# --------------------------------------------------------------- helpers ----
cleanup() {
    if [[ -n "$TMP_DIR" ]]; then rm -rf "$TMP_DIR"; fi
}

parse_args() {
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --local|--user)    MODE="user" ;;
            --global|--system) MODE="system" ;;
            --dir)             [[ $# -ge 2 ]] || die "--dir needs a value"
                               INSTALL_DIR="$2"; MODE="custom"; shift ;;
            --dir=*)           INSTALL_DIR="${1#*=}"; MODE="custom" ;;
            --version)         [[ $# -ge 2 ]] || die "--version needs a value"
                               VERSION="$2"; shift ;;
            --version=*)       VERSION="${1#*=}" ;;
            --no-verify)       VERIFY=0 ;;
            --uninstall)       ACTION="uninstall" ;;
            -y|--yes)          ASSUME_YES=1 ;;
            -h|--help)         usage; exit 0 ;;
            *)                 usage >&2; die "Unknown option: $1" ;;
        esac
        shift
    done
    if [[ -n "$INSTALL_DIR" && -z "$MODE" ]]; then MODE="custom"; fi
    return 0
}

is_musl() {
    if have ldd && ldd --version 2>&1 | grep -qi musl; then return 0; fi
    compgen -G "/lib/ld-musl-*" >/dev/null 2>&1
}

detect_platform() {
    local arch libc
    [[ "$(uname -s)" == "Linux" ]] ||
        die "Unsupported OS: $(uname -s). Nux currently supports Linux only."

    case "$(uname -m)" in
        x86_64|amd64)  arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) die "Unsupported architecture: $(uname -m) (supported: x86_64, aarch64)." ;;
    esac

    if is_musl; then libc="musl"; else libc="gnu"; fi
    TARGET="${arch}-unknown-linux-${libc}"
    ASSET="${APP}-${TARGET}.tar.gz"
}

check_dependencies() {
    local missing=()
    have curl || have wget || missing+=("curl or wget")
    have tar     || missing+=("tar")
    have install || missing+=("install")
    if [[ ${#missing[@]} -gt 0 ]]; then
        die "Missing required command(s): ${missing[*]}"
    fi
}

# Nux builds software from source, so warn (don't fail) if its runtime
# prerequisites are missing.
check_runtime_prereqs() {
    local missing=()
    have git  || missing+=("git")
    have curl || missing+=("curl")
    if ! have cc && ! have gcc && ! have clang; then
        missing+=("a C compiler (gcc/clang)")
    fi
    if [[ ${#missing[@]} -gt 0 ]]; then
        warn "Nux needs these to build packages, but they weren't found: ${missing[*]}"
    fi
}

fetch() { # fetch <url> <dest>
    if have curl; then
        curl --fail --location --silent --show-error --retry 3 "$1" -o "$2"
    else
        wget --quiet --tries=3 -O "$2" "$1"
    fi
}

sha256_of() {
    if   have sha256sum; then sha256sum "$1" | awk '{print $1}'
    elif have shasum;    then shasum -a 256 "$1" | awk '{print $1}'
    elif have openssl;   then openssl dgst -sha256 "$1" | awk '{print $NF}'
    else return 1; fi
}

# ----------------------------------------------------------- destination ----
default_user_dir() { printf '%s' "${HOME}/.local/bin"; }

choose_destination() {
    if [[ -z "$MODE" ]]; then
        if can_prompt; then
            {
                echo
                echo "${BOLD}Where should ${APP} be installed?${RESET}"
                echo "  1) Local   ${DIM}$(default_user_dir) – current user only, no sudo${RESET}"
                echo "  2) Global  ${DIM}/usr/local/bin – all users, may ask for sudo${RESET}"
                echo "  3) Custom  ${DIM}pick any directory${RESET}"
            } >/dev/tty
            case "$(ask "Choose [1]: " "1")" in
                1|l|L) MODE="user" ;;
                2|g|G) MODE="system" ;;
                3|c|C) MODE="custom" ;;
                *)     die "Invalid choice." ;;
            esac
        elif [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
            MODE="system"
        else
            MODE="user"
        fi
    fi

    case "$MODE" in
        user)   INSTALL_DIR="$(default_user_dir)" ;;
        system) INSTALL_DIR="/usr/local/bin" ;;
        custom)
            if [[ -z "$INSTALL_DIR" ]]; then
                INSTALL_DIR="$(ask "Install directory: " "")"
                [[ -n "$INSTALL_DIR" ]] || die "No directory given."
            fi
            ;;
    esac

    INSTALL_DIR="${INSTALL_DIR/#\~/$HOME}"   # expand a typed leading ~
}

nearest_existing_dir() {
    local p="$1"
    while [[ ! -d "$p" && "$p" != "/" && "$p" != "." ]]; do p="$(dirname "$p")"; done
    printf '%s' "$p"
}

resolve_privileges() {
    local probe
    probe="$(nearest_existing_dir "$INSTALL_DIR")"
    USE_SUDO=0
    if [[ ! -w "$probe" ]]; then
        have sudo || die "${INSTALL_DIR} isn't writable and sudo is unavailable. Re-run as root or use --local."
        USE_SUDO=1
        info "${DIM}${INSTALL_DIR} needs elevated permissions; sudo will be used.${RESET}"
    fi
}

priv() { if [[ $USE_SUDO -eq 1 ]]; then sudo "$@"; else "$@"; fi; }

# -------------------------------------------------------------- download ----
release_url() { # release_url <file>
    if [[ "$VERSION" == "latest" ]]; then
        printf 'https://github.com/%s/releases/latest/download/%s' "$REPO" "$1"
    else
        printf 'https://github.com/%s/releases/download/v%s/%s' "$REPO" "${VERSION#v}" "$1"
    fi
}

download_and_verify() {
    info "Downloading ${APP} (${VERSION}) for ${TARGET}..."
    fetch "$(release_url "$ASSET")" "$TMP_DIR/$ASSET" ||
        die "Could not download ${ASSET}. Check that this version/platform exists:
  https://github.com/${REPO}/releases
Alternatively, build from source: cargo build --release"

    if [[ $VERIFY -eq 0 ]]; then
        warn "Skipping checksum verification (--no-verify)."
        return
    fi

    fetch "$(release_url SHA256SUMS)" "$TMP_DIR/SHA256SUMS" 2>/dev/null ||
        die "Could not download SHA256SUMS. Use --no-verify to install anyway."

    local expected actual
    expected="$(awk -v f="$ASSET" '$2 == f || $2 == "*" f {print $1}' "$TMP_DIR/SHA256SUMS")"
    [[ -n "$expected" ]] || die "No checksum listed for ${ASSET}."
    actual="$(sha256_of "$TMP_DIR/$ASSET")" ||
        die "No sha256 tool found (sha256sum/shasum/openssl). Use --no-verify to skip."

    [[ "$expected" == "$actual" ]] ||
        die "Checksum mismatch for ${ASSET}!
  expected: ${expected}
  actual:   ${actual}"
    ok "Checksum verified."
}

extract_binary() {
    mkdir -p "$TMP_DIR/extract"
    tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR/extract"
    EXTRACTED_BIN="$(find "$TMP_DIR/extract" -type f -name "$APP" | head -n 1)"
    [[ -n "$EXTRACTED_BIN" ]] || die "Expected binary '${APP}' was not found in the archive."
}

# --------------------------------------------------------------- install ----
install_binary() {
    local dest="${INSTALL_DIR}/${APP}" staged="${INSTALL_DIR}/.${APP}.new.$$"

    if [[ -e "$dest" ]]; then
        info "Existing installation found at ${dest}; upgrading."
    fi

    priv mkdir -p "$INSTALL_DIR"
    # Stage, then rename: replaces the binary atomically.
    priv install -m 755 "$EXTRACTED_BIN" "$staged"
    priv mv -f "$staged" "$dest"
    ok "Installed ${APP} to ${dest}"

    local v
    if v="$("$dest" --version 2>/dev/null)"; then info "${DIM}${v}${RESET}"; fi
}

shell_rc_and_line() { # args: dir ; sets RC_FILE and RC_LINE
    local dir="$1"
    case "$(basename "${SHELL:-}")" in
        zsh)  RC_FILE="${ZDOTDIR:-$HOME}/.zshrc"
              RC_LINE="export PATH=\"${dir}:\$PATH\"" ;;
        bash) RC_FILE="$HOME/.bashrc"
              RC_LINE="export PATH=\"${dir}:\$PATH\"" ;;
        fish) RC_FILE="$HOME/.config/fish/config.fish"
              RC_LINE="fish_add_path \"${dir}\"" ;;
        *)    RC_FILE=""; RC_LINE="export PATH=\"${dir}:\$PATH\"" ;;
    esac
}

ensure_on_path() { # args: dir description
    local dir="$1" what="$2"
    case ":${PATH}:" in *":${dir}:"*) return 0 ;; esac

    warn "${dir} (${what}) is not in your PATH."
    shell_rc_and_line "$dir"
    if [[ -z "$RC_FILE" ]]; then
        info "Add it manually:  ${RC_LINE}"
        return 0
    fi
    if [[ -f "$RC_FILE" ]] && grep -qF "$dir" "$RC_FILE" 2>/dev/null; then
        info "${RC_FILE} already mentions it; restart your shell."
        return 0
    fi
    if ask_yes_no "Add it to ${RC_FILE}?" "y"; then
        mkdir -p "$(dirname "$RC_FILE")"
        { printf '\n# Added by the %s installer\n%s\n' "$APP" "$RC_LINE"; } >>"$RC_FILE"
        ok "Updated ${RC_FILE} (restart your shell or: source ${RC_FILE})"
    else
        info "Add it yourself:  ${RC_LINE}"
    fi
}

warn_if_shadowed() {
    local found
    found="$(command -v "$APP" 2>/dev/null || true)"
    if [[ -n "$found" && "$found" != "${INSTALL_DIR}/${APP}" ]]; then
        warn "Another '${APP}' is first on your PATH: ${found}"
        warn "It will shadow the one just installed in ${INSTALL_DIR}."
    fi
}

# ------------------------------------------------------------- uninstall ----
do_uninstall() {
    local dirs=() found=0 d f
    if [[ -n "$INSTALL_DIR" ]]; then
        dirs+=("${INSTALL_DIR/#\~/$HOME}")
    else
        dirs+=("$(default_user_dir)" "/usr/local/bin")
    fi

    for d in "${dirs[@]}"; do
        f="${d}/${APP}"
        [[ -f "$f" ]] || continue
        found=1
        if ask_yes_no "Remove ${f}?" "y"; then
            if [[ -w "$d" ]]; then rm -f "$f"
            elif have sudo; then sudo rm -f "$f"
            else die "Cannot remove ${f}: no permission and sudo unavailable."; fi
            ok "Removed ${f}"
        fi
    done

    if [[ $found -eq 0 ]]; then info "No ${APP} installation found in: ${dirs[*]}"; fi
    if [[ -d "${HOME}/.nux" ]]; then
        info "${DIM}Your packages and data in ~/.nux were left untouched; delete that folder to remove them.${RESET}"
    fi
}

# ------------------------------------------------------------------ main ----
main() {
    parse_args "$@"
    detect_platform

    if [[ "$ACTION" == "uninstall" ]]; then
        do_uninstall
        return
    fi

    check_dependencies
    choose_destination
    resolve_privileges

    TMP_DIR="$(mktemp -d)"
    trap cleanup EXIT

    download_and_verify
    extract_binary
    install_binary

    if [[ "$MODE" != "system" ]]; then ensure_on_path "$INSTALL_DIR" "nux itself"; fi
    ensure_on_path "$NUX_HOME_BIN" "programs installed by nux"
    warn_if_shadowed
    check_runtime_prereqs

    echo
    info "Get started:"
    info "  ${BOLD}nux init hello-env && cd hello-env && nux sync${RESET}"
    info "Docs: https://github.com/${REPO}"
}

# Called last, so a truncated `curl | bash` can never run half a script.
main "$@"