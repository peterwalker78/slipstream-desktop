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
#   --uninstall      remove what the installer wrote (add --purge for your settings too)
set -eu

project=peterwalker78/slipstream-desktop
releases_api=https://api.github.com/repos/$project/releases

mode=install
version=
dir=
yes=
purge=
while [ $# -gt 0 ]; do
    case $1 in
        --check) mode=check ;;
        --yes | -y) yes=--yes ;;
        --try) mode=try ;;
        --uninstall) mode=uninstall ;;
        --purge) purge=--purge ;;
        --version)
            version=${2:?--version needs a tag such as v0.5.0}
            shift
            ;;
        --dir)
            dir=${2:?--dir needs a folder}
            shift
            ;;
        -h | --help)
            # Run as sh -c "$(curl ...)", $0 is no file, so there's no header to show.
            if [ -f "$0" ]; then
                sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            else
                echo "install.sh [--check] [--try] [--version vX.Y.Z] [--dir DIR] [--yes] [--uninstall [--purge]]"
            fi
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
        echo "Build from source instead: https://github.com/$project/blob/main/guide/install.md#building-it-yourself" >&2
        exit 1
        ;;
esac

# GitHub allows a limited number of unnamed requests from one address each hour. A token in the
# environment, which continuous integration has and people generally don't, lifts that.
token=${GH_TOKEN:-${GITHUB_TOKEN:-}}
if have curl; then
    fetch() { curl -fsSL ${token:+-H "Authorization: Bearer $token"} "$1"; }
    fetch_to() { curl -fL --progress-bar ${token:+-H "Authorization: Bearer $token"} -o "$2" "$1"; }
elif have wget; then
    fetch() { wget -qO- ${token:+--header="Authorization: Bearer $token"} "$1"; }
    fetch_to() { wget -q --show-progress ${token:+--header="Authorization: Bearer $token"} -O "$2" "$1"; }
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
    echo "install.sh: GitHub didn't name a release. It may be limiting how often this address can" >&2
    echo "ask, in which case a few minutes is enough. Failing that, download one by hand:" >&2
    echo "  https://github.com/$project/releases" >&2
    exit 1
fi

# The file is named after the release, so the name is worked out from the tag rather than asked
# for. GitHub's by-tag view of a release has been seen reporting a release as having no files at
# all, for a long time, while those same files downloaded perfectly well from the address below —
# so asking it whether a download exists is a question that can be answered wrongly. Downloading
# it and finding out is not.
name=slipstream-${tag#v}-linux-$arch.tar.gz
base=https://github.com/$project/releases/download/$tag

work=${dir:-${XDG_CACHE_HOME:-$HOME/.cache}/slipstream-install}
mkdir -p "$work"
cd "$work"

echo "Downloading $name"
if ! fetch_to "$base/$name" "$name"; then
    echo >&2
    echo "install.sh: release $tag has no download for $arch." >&2
    echo "  If it was published in the last few minutes its files may still be going up." >&2
    echo "  Otherwise, see what that release has:" >&2
    echo "  https://github.com/$project/releases/tag/$tag" >&2
    exit 1
fi
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
    uninstall)
        # Any release's installer can take away what an earlier one wrote: it removes what the
        # list it left behind names, and nothing else.
        exec sh scripts/install-session --uninstall ${purge:+"$purge"} ${yes:+"$yes"}
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
