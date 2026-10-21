#!/usr/bin/env bash
# Install Aether for the current user (no sudo, except for the udev rule).
#
#   ./scripts/install.sh               install to ~/.local
#   ./scripts/install.sh --prefix DIR  install somewhere else
#   ./scripts/install.sh --no-udev     skip the udev rule (keyboard then needs it installed separately)
#   ./scripts/install.sh --no-build    install the existing release build as-is
#
# Every installed path is recorded in <prefix>/share/aether-he/install-manifest,
# which scripts/uninstall.sh reads to remove exactly what was installed.
set -euo pipefail

cd "$(dirname "$0")/.."

prefix="$HOME/.local"
udev=1
build=1
while (($#)); do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift ;;
        --no-udev) udev=0 ;;
        --no-build) build=0 ;;
        -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done
prefix="$(realpath -m "$prefix")"

bold=$'\e[1m'; green=$'\e[32m'; yellow=$'\e[33m'; off=$'\e[0m'
step() { printf '\n%s==> %s%s\n' "$bold" "$*" "$off"; }

# ---- build if needed ------------------------------------------------------
bins=(target/release/aether target/release/aether-gui)
stale=0
for b in "${bins[@]}"; do
    [[ -x $b ]] || { stale=1; continue; }
    if [[ -n $(find crates data Cargo.toml Cargo.lock -newer "$b" -print -quit) ]]; then stale=1; fi
done
if ((stale)); then
    if ((build)); then
        step "Building release binaries"
        ./scripts/build.sh --no-test
    else
        for b in "${bins[@]}"; do [[ -x $b ]] || { echo "missing $b; run scripts/build.sh" >&2; exit 1; }; done
        echo "${yellow}note: the release build is older than the sources (--no-build)${off}"
    fi
fi

# ---- files ----------------------------------------------------------------
share="$prefix/share/aether-he"
manifest="$share/install-manifest"
installed=()

put() { # put <mode> <src> <dest>
    install -D -m "$1" "$2" "$3"
    installed+=("$3")
}

step "Installing to $prefix"
put 755 target/release/aether "$prefix/bin/aether"
put 755 target/release/aether-gui "$prefix/bin/aether-gui"
while IFS= read -r -d '' f; do
    put 644 "$f" "$share/$f"
done < <(find data -type f -print0 | sort -z)
put 644 packaging/aether-he.desktop "$prefix/share/applications/aether-he.desktop"
put 644 packaging/aether-he.png "$prefix/share/icons/hicolor/512x512/apps/aether-he.png"

# ---- udev rule (sudo) -----------------------------------------------------
rule_dest=/etc/udev/rules.d/70-aether.rules
if ((udev)); then
    step "Installing the udev rule (needs sudo once)"
    if cmp -s packaging/70-aether.rules "$rule_dest"; then
        echo "already installed: $rule_dest"
    else
        sudo install -m 644 packaging/70-aether.rules "$rule_dest"
    fi
    # The rule installed by hand during bring-up has the same content under an older name.
    old=/etc/udev/rules.d/70-aula.rules
    if [[ -f $old ]] && cmp -s "$old" packaging/70-aether.rules; then
        sudo rm -f "$old"
        echo "removed the older duplicate $old"
    fi
    sudo udevadm control --reload
    sudo udevadm trigger --subsystem-match=hidraw
    installed+=("udev:$rule_dest")
fi

printf '%s\n' "# Aether install manifest: one installed path per line (read by scripts/uninstall.sh)" \
    "${installed[@]}" > "$manifest"


step "${green}Installed${off}"
echo "  ${#installed[@]} items, listed in $manifest"
echo "  Start it from your app menu (\"Aether HE\") or run: aether-gui"
case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "${yellow}  note: $prefix/bin is not on your PATH; run $prefix/bin/aether-gui or add it to PATH${off}" ;;
esac
((udev)) && echo "  If the keyboard was plugged in during install and isn't found, replug it once."
echo "  Remove with: ./scripts/uninstall.sh"
