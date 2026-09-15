#!/bin/sh
# Downloads the newest Slipstream release for this machine, checks it against its published
# checksum, and runs its installer, which tells you what it will do before it writes anything.
#
#   sh -c "$(curl -fsSL https://raw.githubusercontent.com/peterwalker78/slipstream-desktop/main/install.sh)"
#
# Options (after a -- when run the way above):
#   --check          download and report what would happen; install nothing
#   --try            open it in a window first, then offer to install
#   --version vX.Y.Z install that release instead of the newest
#   --dir DIR        unpack there instead of the cache folder
#   --yes            don't ask anything; take every step the installer offers
set -eu

project=peterwalker78/slipstream-desktop
releases_api=https://api.github.com/repos/$project/releases

mode=install
version=
dir=
yes=
while [ $# -gt 0 ]; do
    case $1 in
        --check) mode=check ;;
        --yes | -y) yes=--yes ;;
        --try) mode=try ;;
        --version)
            version=${2:?--version needs a tag such as v0.5.0}
            shift
            ;;
        --dir)
            dir=${2:?--dir needs a folder}
            shift
            ;;
        -h | --help)
            sed -n '2,14p' "$0" 2>/dev/null | sed 's/^# \{0,1\}//' || cat <<'USAGE'
install.sh [--check] [--try] [--version vX.Y.Z] [--dir DIR]
USAGE
            exit 0
            ;;
        *)
            echo "install.sh: unknown option $1" >&2
            exit 2
            ;;
    esac
    shift
done

have() { command -v "$1" >/dev/null 2>&1; }

if [ "$(id -u)" -eq 0 ]; then
    echo "install.sh: run this as yourself, not with sudo." >&2
    echo "It asks for a password itself, for the few files outside your home." >&2
    exit 2
fi

arch=$(uname -m)
case $arch in
    x86_64) ;;
    *)
        echo "install.sh: there are only x86_64 downloads so far, and this is $arch." >&2
        echo "Build from source instead: https://github.com/$project#from-source" >&2
        exit 1
        ;;
esac

if have curl; then
    fetch() { curl -fsSL "$1"; }
    fetch_to() { curl -fL --progress-bar -o "$2" "$1"; }
elif have wget; then
    fetch() { wget -qO- "$1"; }
    fetch_to() { wget -q --show-progress -O "$2" "$1"; }
else
    echo "install.sh: neither curl nor wget is installed, so nothing can be downloaded." >&2
    exit 1
fi
for tool in tar sha256sum; do
    have "$tool" || { echo "install.sh: $tool isn't installed, and this needs it." >&2; exit 1; }
done

echo "Looking for the newest Slipstream release"
if [ -n "$version" ]; then
    tag=$version
else
    # The API lists releases newest first; betas count, and every release is one for now.
    tag=$(fetch "$releases_api?per_page=1" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1)
fi
if [ -z "$tag" ]; then
    echo "install.sh: GitHub didn't name a release. Try again, or download one by hand:" >&2
    echo "  https://github.com/$project/releases" >&2
    exit 1
fi

name=$(fetch "$releases_api/tags/$tag" \
    | sed -n 's/.*"name": *"\(slipstream-[^"]*-linux-'"$arch"'\.tar\.gz\)".*/\1/p' | head -n1)
if [ -z "$name" ]; then
    echo "install.sh: release $tag has no download for $arch." >&2
    exit 1
fi
base=https://github.com/$project/releases/download/$tag

work=${dir:-${XDG_CACHE_HOME:-$HOME/.cache}/slipstream-install}
mkdir -p "$work"
cd "$work"

echo "Downloading $name"
fetch_to "$base/$name" "$name"
fetch_to "$base/$name.sha256" "$name.sha256"
if ! sha256sum -c "$name.sha256" >/dev/null 2>&1; then
    echo "install.sh: $name doesn't match its published checksum, so it wasn't unpacked." >&2
    echo "Delete $work and try again." >&2
    exit 1
fi
echo "Checksum matches."

folder=${name%.tar.gz}
rm -rf "$folder"
tar xf "$name"
cd "$folder"

case $mode in
    check)
        exec sh scripts/install-session --check
        ;;
    try)
        sh scripts/try-slipstream || true
        echo
        printf 'Install Slipstream on the login screen now? [y/N] '
        read -r answer || answer=
        case $answer in
            y | Y | yes) ;;
            *)
                echo "Nothing was installed. It's unpacked in $work/$folder if you want it later."
                exit 0
                ;;
        esac
        ;;
esac

echo
exec sh scripts/install-session ${yes:+"$yes"}
