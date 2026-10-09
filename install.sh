#!/bin/sh
# Installs the sobre binary from a GitHub release, on Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/malochapo/sobre-ai/main/install.sh | sh -s -- --dense
#
# It downloads the archive for this platform and SHA256SUMS from the release,
# refuses an archive whose checksum does not match, and copies the binary into
# ~/.local/bin (or --dir). It needs no root, changes no shell file, and runs
# only `sobre --version` once installed. On Windows, use install.ps1.
set -eu

REPO="malochapo/sobre-ai"
GITHUB_URL="https://github.com"
API_URL="https://api.github.com"

usage() {
    cat <<'EOF'
Install sobre from a GitHub release.

Usage: install.sh [--version <tag>] [--dense] [--dir <path>]

  --version <tag>  the release to install, e.g. v0.1.0-rc.1 (default: the
                   latest release, or the newest pre-release while there is
                   no release yet)
  --dense          the build with local embeddings (on Linux it needs glibc
                   2.28 or later; there is none for Intel macOS)
  --dir <path>     where to put the binary (default: ~/.local/bin)
  -h, --help       show this help
EOF
}

say() {
    printf '%s\n' "$*"
}

die() {
    printf 'sobre install: %s\n' "$*" >&2
    exit 1
}

# fetch <url> <file>: download with curl, or with wget when there is no curl.
fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL --retry 3 -o "$2" "$1"
    else
        wget -q -O "$2" "$1"
    fi
}

# The first "tag_name" in a GitHub API answer, or nothing.
json_tag() {
    grep -o '"tag_name": *"[^"]*"' "$1" | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'
}

# The tag of the latest release or, while there is none, of the newest
# pre-release. Listing the releases also covers a failed first request: the
# newest stable release in the list wins.
resolve_tag() {
    if fetch "$API_URL/repos/$REPO/releases/latest" "$tmp/latest.json" 2>/dev/null; then
        latest=$(json_tag "$tmp/latest.json")
        if [ -n "$latest" ]; then
            say "$latest"
            return
        fi
    fi
    fetch "$API_URL/repos/$REPO/releases?per_page=100" "$tmp/releases.json" ||
        die "could not list the releases of $GITHUB_URL/$REPO (GitHub API unreachable or rate-limited); pass --version <tag>"
    # Each release's tag, then its pre-release flag, newest release first.
    grep -oE '"(tag_name|prerelease)": *("[^"]*"|true|false)' "$tmp/releases.json" |
        sed 's/^"\([a-z_]*\)": *"\{0,1\}\([^"]*\)"\{0,1\}$/\1 \2/' |
        awk '
            $1 == "tag_name" { tag = $2; next }
            $1 == "prerelease" && tag != "" {
                if (first == "") first = tag
                if ($2 == "false") { print tag; found = 1; exit }
                tag = ""
            }
            END { if (!found && first != "") print first }
        '
}

# The C library: "<major> <minor>" for glibc, "musl", or nothing when it
# cannot be told.
libc_version() {
    if out=$(getconf GNU_LIBC_VERSION 2>/dev/null); then
        printf '%s\n' "$out" | sed -n 's/^glibc \([0-9][0-9]*\)\.\([0-9][0-9]*\).*/\1 \2/p'
        return
    fi
    out=$(ldd --version 2>&1 || true)
    case "$out" in
        *musl*) say musl ;;
        *GLIBC* | *"GNU libc"*)
            printf '%s\n' "$out" | head -n 1 | sed -n 's/.* \([0-9][0-9]*\)\.\([0-9][0-9]*\)$/\1 \2/p'
            ;;
    esac
}

# The dense build loads ONNX Runtime at run time and targets glibc 2.28.
check_glibc() {
    libc=$(libc_version)
    case "$libc" in
        musl)
            die "the dense build needs glibc 2.28 or later, and this system uses musl; install without --dense"
            ;;
        "")
            say "warning: could not tell the glibc version; the dense build needs glibc 2.28 or later" >&2
            ;;
        *)
            major=${libc% *}
            minor=${libc#* }
            if [ "$major" -lt 2 ] || { [ "$major" -eq 2 ] && [ "$minor" -lt 28 ]; }; then
                die "the dense build needs glibc 2.28 or later, and this system has glibc $major.$minor; install without --dense"
            fi
            ;;
    esac
}

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{ print $1 }'
    else
        shasum -a 256 "$1" | awk '{ print $1 }'
    fi
}

main() {
    version=""
    dense=0
    dir=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --version)
                [ $# -ge 2 ] || die "--version needs a tag, e.g. v0.1.0-rc.1"
                version=$2
                shift 2
                ;;
            --version=*)
                version=${1#*=}
                shift
                ;;
            --dense)
                dense=1
                shift
                ;;
            --dir)
                [ $# -ge 2 ] || die "--dir needs a path"
                dir=$2
                shift 2
                ;;
            --dir=*)
                dir=${1#*=}
                shift
                ;;
            -h | --help)
                usage
                exit 0
                ;;
            *)
                die "unknown option '$1' (see --help)"
                ;;
        esac
    done

    if [ -n "$version" ]; then
        case "$version" in
            v*) ;;
            *) version="v$version" ;;
        esac
        case "$version" in
            *[!A-Za-z0-9._-]*) die "'$version' is not a release tag" ;;
        esac
    fi

    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Linux)
            case "$arch" in
                x86_64 | amd64) cpu=x86_64 ;;
                aarch64 | arm64) cpu=aarch64 ;;
                *) die "there is no sobre build for Linux on $arch, only for x86_64 and aarch64" ;;
            esac
            if [ "$dense" = 1 ]; then
                target="$cpu-unknown-linux-gnu"
            else
                target="$cpu-unknown-linux-musl"
            fi
            ;;
        Darwin)
            # A shell running under Rosetta reports x86_64 on Apple silicon.
            if [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
                arch=arm64
            fi
            case "$arch" in
                arm64 | aarch64) target=aarch64-apple-darwin ;;
                x86_64)
                    [ "$dense" = 0 ] ||
                        die "there is no dense build for Intel macOS (Microsoft ships no ONNX Runtime for it): install without --dense, or build from source"
                    target=x86_64-apple-darwin
                    ;;
                *) die "there is no sobre build for macOS on $arch" ;;
            esac
            ;;
        MINGW* | MSYS* | CYGWIN* | Windows_NT)
            die "on Windows, run install.ps1 from PowerShell (see the README)"
            ;;
        *)
            die "there is no sobre build for $os, only for Linux, macOS and Windows"
            ;;
    esac

    if [ "$dense" = 1 ]; then
        if [ "$os" = Linux ]; then
            check_glibc
        fi
        asset="sobre-dense-$target.tar.gz"
    else
        asset="sobre-$target.tar.gz"
    fi

    command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1 ||
        die "this script needs curl or wget"
    command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 ||
        die "this script needs sha256sum or shasum to check the download"
    command -v tar >/dev/null 2>&1 || die "this script needs tar"

    if [ -z "$dir" ]; then
        [ -n "${HOME:-}" ] || die "HOME is not set: pass --dir <path>"
        dir="$HOME/.local/bin"
    fi

    tmp=$(mktemp -d "${TMPDIR:-/tmp}/sobre-install.XXXXXX") ||
        die "could not create a temporary directory"
    trap 'rm -rf "$tmp"' EXIT
    trap 'exit 1' HUP INT TERM

    if [ -z "$version" ]; then
        version=$(resolve_tag)
        [ -n "$version" ] || die "no sobre release found at $GITHUB_URL/$REPO/releases"
    fi

    base="$GITHUB_URL/$REPO/releases/download/$version"
    say "Installing sobre $version ($asset)"
    fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS" ||
        die "could not download $base/SHA256SUMS: is $version a sobre release?"
    fetch "$base/$asset" "$tmp/$asset" ||
        die "could not download $base/$asset"

    expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1; exit }' "$tmp/SHA256SUMS" |
        tr 'A-F' 'a-f')
    [ -n "$expected" ] || die "SHA256SUMS of $version lists no $asset"
    actual=$(sha256_of "$tmp/$asset")
    [ "$actual" = "$expected" ] ||
        die "checksum mismatch for $asset (expected $expected, got $actual): nothing was installed"

    mkdir "$tmp/extract"
    tar -xzf "$tmp/$asset" -C "$tmp/extract" || die "could not extract $asset"
    [ -f "$tmp/extract/sobre" ] || die "$asset holds no 'sobre' binary"

    mkdir -p "$dir" || die "could not create $dir"
    dir=$(CDPATH='' cd -- "$dir" && pwd)
    dest="$dir/sobre"
    # Copied beside the old binary, then renamed over it: a running sobre (an
    # agent's MCP server) cannot be overwritten in place.
    cp "$tmp/extract/sobre" "$dir/.sobre.new.$$" || die "could not write to $dir"
    chmod 755 "$dir/.sobre.new.$$"
    mv -f "$dir/.sobre.new.$$" "$dest" || die "could not replace $dest"

    installed=$("$dest" --version 2>&1) || die "installed $dest, but it does not run: $installed"
    say "Installed $installed at $dest"

    case ":${PATH:-}:" in
        *":$dir:"*)
            found=$(command -v sobre 2>/dev/null || true)
            if [ -n "$found" ] && [ "$found" != "$dest" ]; then
                say ""
                say "Note: $found comes before $dest on your PATH."
            fi
            ;;
        *)
            say ""
            say "$dir is not on your PATH. Add it in your shell profile, for example:"
            say "  export PATH=\"$dir:\$PATH\""
            ;;
    esac

    say ""
    say "Next: run 'sobre init' to connect your agents, then restart them."
}

# Everything runs from here, once the whole script has been read: a download
# cut short cannot run half of it.
main "$@"
