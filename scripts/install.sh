#!/bin/sh
# Relava CLI installer for macOS and Linux
# Usage: curl -fsSL https://raw.githubusercontent.com/relava/relava/main/scripts/install.sh | sh
#
# Environment variables:
#   RELAVA_INSTALL_DIR  Override install directory (default: ~/.relava/bin)
#   RELAVA_VERSION      Install a specific version (default: latest)

set -eu

REPO="relava/relava"
BINARY_NAME="relava"
DEFAULT_INSTALL_DIR="${HOME}/.relava/bin"

# --- Helpers ---

info() {
    printf "\033[1;34m==>\033[0m %s\n" "$1"
}

success() {
    printf "\033[1;32m==>\033[0m %s\n" "$1"
}

error() {
    printf "\033[1;31merror:\033[0m %s\n" "$1" >&2
    exit 1
}

warn() {
    printf "\033[1;33mwarning:\033[0m %s\n" "$1" >&2
}

need_cmd() {
    if ! command -v "$1" > /dev/null 2>&1; then
        error "need '$1' (command not found)"
    fi
}

# --- Platform detection ---

detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "macos" ;;
        *)       error "unsupported operating system: $(uname -s). Relava supports Linux and macOS." ;;
    esac
}

detect_arch() {
    case "$(uname -m)" in
        x86_64|amd64)  echo "x86_64" ;;
        aarch64|arm64) echo "aarch64" ;;
        *)             error "unsupported architecture: $(uname -m). Relava supports x86_64 and aarch64." ;;
    esac
}

get_target() {
    local os="$1"
    local arch="$2"

    case "${os}-${arch}" in
        linux-x86_64)   echo "x86_64-unknown-linux-gnu" ;;
        linux-aarch64)   echo "aarch64-unknown-linux-gnu" ;;
        macos-x86_64)   echo "x86_64-apple-darwin" ;;
        macos-aarch64)  echo "aarch64-apple-darwin" ;;
        *)              error "unsupported platform: ${os} ${arch}" ;;
    esac
}

# --- Version resolution ---

get_latest_version() {
    local url="https://api.github.com/repos/${REPO}/releases/latest"
    local response

    if command -v curl > /dev/null 2>&1; then
        response=$(curl -fsSL "$url" 2>/dev/null) || error "failed to fetch latest release from GitHub. Check your network connection."
    elif command -v wget > /dev/null 2>&1; then
        response=$(wget -qO- "$url" 2>/dev/null) || error "failed to fetch latest release from GitHub. Check your network connection."
    else
        error "need 'curl' or 'wget' to download files"
    fi

    # Extract tag_name from JSON without requiring jq
    echo "$response" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -1
}

# --- Download ---

download() {
    local url="$1"
    local output="$2"

    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$url" -o "$output" || error "download failed: $url"
    elif command -v wget > /dev/null 2>&1; then
        wget -qO "$output" "$url" || error "download failed: $url"
    else
        error "need 'curl' or 'wget' to download files"
    fi
}

# --- Main ---

main() {
    info "Installing Relava CLI..."

    # Check dependencies
    need_cmd tar
    need_cmd uname

    # Detect platform
    local os
    os=$(detect_os)
    local arch
    arch=$(detect_arch)
    local target
    target=$(get_target "$os" "$arch")

    info "Detected platform: ${os} ${arch} (${target})"

    # Resolve version
    local version="${RELAVA_VERSION:-}"
    if [ -z "$version" ]; then
        info "Fetching latest release..."
        version=$(get_latest_version)
        if [ -z "$version" ]; then
            error "could not determine latest version. Set RELAVA_VERSION to install a specific version."
        fi
    fi

    info "Installing version: ${version}"

    # Build download URL
    local archive_name="${BINARY_NAME}-${version}-${target}.tar.gz"
    local download_url="https://github.com/${REPO}/releases/download/${version}/${archive_name}"

    # Download to temp directory
    local tmp_dir
    tmp_dir=$(mktemp -d)
    trap 'rm -rf "$tmp_dir"' EXIT

    info "Downloading ${archive_name}..."
    download "$download_url" "${tmp_dir}/${archive_name}"

    # Extract
    info "Extracting..."
    tar -xzf "${tmp_dir}/${archive_name}" -C "$tmp_dir"

    # Find binary (handles flat or nested archive structures)
    local binary_path
    binary_path=$(find "$tmp_dir" -name "$BINARY_NAME" -type f | head -1)
    if [ -z "$binary_path" ]; then
        error "binary '${BINARY_NAME}' not found in archive"
    fi

    # Determine install directory
    local install_dir="${RELAVA_INSTALL_DIR:-$DEFAULT_INSTALL_DIR}"
    mkdir -p "$install_dir"

    # Install binary
    mv "$binary_path" "${install_dir}/${BINARY_NAME}"
    chmod +x "${install_dir}/${BINARY_NAME}"

    # Verify installation
    if "${install_dir}/${BINARY_NAME}" --version > /dev/null 2>&1; then
        local installed_version
        installed_version=$("${install_dir}/${BINARY_NAME}" --version 2>/dev/null || echo "unknown")
        success "Relava CLI installed successfully! (${installed_version})"
    else
        success "Relava CLI installed to ${install_dir}/${BINARY_NAME}"
    fi

    # Check if install dir is in PATH
    case ":${PATH}:" in
        *":${install_dir}:"*)
            # Already in PATH
            ;;
        *)
            echo ""
            warn "${install_dir} is not in your PATH."
            echo ""
            echo "Add it to your shell profile:"
            echo ""
            echo "  # bash (~/.bashrc) or zsh (~/.zshrc):"
            echo "  export PATH=\"${install_dir}:\$PATH\""
            echo ""
            echo "  # fish (~/.config/fish/config.fish):"
            echo "  fish_add_path ${install_dir}"
            echo ""
            echo "Then restart your shell or run: source <your-profile>"
            ;;
    esac
}

main "$@"
