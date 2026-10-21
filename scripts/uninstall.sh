#!/usr/bin/env bash
# Remove what scripts/install.sh installed.
#
#   ./scripts/uninstall.sh               remove the app, keep your backups and settings
#   ./scripts/uninstall.sh --purge       also delete backups and GUI settings
#   ./scripts/uninstall.sh --prefix DIR  the prefix used at install time (default ~/.local)
#   ./scripts/uninstall.sh --yes         don't ask for confirmation
#
# The keyboard keeps its own settings (lighting, actuation) in its memory;
# uninstalling the app does not change the board.
set -euo pipefail

prefix="$HOME/.local"
purge=0
yes=0
while (($#)); do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift ;;
        --purge) purge=1 ;;
        --yes|-y) yes=1 ;;
        -h|--help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done
prefix="$(realpath -m "$prefix")"

bold=$'\e[1m'; green=$'\e[32m'; yellow=$'\e[33m'; off=$'\e[0m'

confirm() { # confirm <question>
    ((yes)) && return 0
    local reply
    read -r -p "$1 [y/N] " reply
    [[ $reply == [yY]* ]]
}

share="$prefix/share/aether-he"
manifest="$share/install-manifest"
# Backups and GUI settings live in the user data dir, whatever the prefix.
user_dir="${XDG_DATA_HOME:-$HOME/.local/share}/aether-he"

# ---- what to remove -------------------------------------------------------
files=()
rules=()
if [[ -f $manifest ]]; then
    while IFS= read -r line; do
        [[ -z $line || $line == \#* ]] && continue
        if [[ $line == udev:* ]]; then rules+=("${line#udev:}"); else files+=("$line"); fi
    done < "$manifest"
    files+=("$manifest")
else
    echo "${yellow}No install manifest in $share; looking for the default install paths.${off}"
    for f in "$prefix/bin/aether" "$prefix/bin/aether-gui" \
             "$prefix/share/applications/aether-he.desktop" \
             "$prefix/share/icons/hicolor/512x512/apps/aether-he.png"; do
        [[ -e $f ]] && files+=("$f")
    done
    if [[ -d $share/data ]]; then
        while IFS= read -r -d '' f; do files+=("$f"); done < <(find "$share/data" -type f -print0)
    fi
    [[ -f /etc/udev/rules.d/70-aether.rules ]] && rules+=(/etc/udev/rules.d/70-aether.rules)
fi

# Only ever delete Aether's own files: refuse anything outside the expected places.
for f in "${files[@]}"; do
    case "$f" in
        "$prefix"/bin/aether|"$prefix"/bin/aether-gui|"$share"/*|\
        "$prefix"/share/applications/aether-he.desktop|"$prefix"/share/icons/hicolor/*/apps/aether-he.png) ;;
        *) echo "refusing to remove unexpected manifest entry: $f" >&2; exit 1 ;;
    esac
done
for r in "${rules[@]}"; do
    [[ $r == /etc/udev/rules.d/70-aether.rules ]] || { echo "refusing to remove unexpected rule: $r" >&2; exit 1; }
done

backups=0
if [[ -d $user_dir/backups ]]; then backups=$(find "$user_dir/backups" -type f -name '*.json' | wc -l); fi

if ((${#files[@]} == 0 && ${#rules[@]} == 0 && !purge)); then
    echo "Nothing to uninstall."
    exit 0
fi

printf '%sWill remove:%s\n' "$bold" "$off"
for f in "${files[@]}"; do echo "  $f"; done
for r in "${rules[@]}"; do echo "  $r (sudo)"; done
if ((purge)); then
    echo "  $user_dir/backups ($backups backup files)"
    echo "  $user_dir/app.ron (GUI settings)"
else
    echo "Keeping: $user_dir/backups ($backups backups) and GUI settings (use --purge to delete them)"
fi
confirm "Continue?" || { echo "Cancelled."; exit 1; }

# ---- remove ---------------------------------------------------------------
for f in "${files[@]}"; do rm -f -- "$f"; done

if ((${#rules[@]})); then
    for r in "${rules[@]}"; do sudo rm -f -- "$r"; done
    sudo udevadm control --reload
    sudo udevadm trigger --subsystem-match=hidraw
fi

if ((purge)); then
    if ((backups > 0)) && ! confirm "Really delete $backups keyboard backups?"; then
        echo "Kept the backups."
    else
        rm -rf -- "$user_dir/backups"
    fi
    rm -f -- "$user_dir/app.ron"
fi

# Remove directories that are now empty (never anything with files left in it).
for d in "$share/data/layouts" "$share/data" "$share" "$user_dir" \
         "$prefix/share/icons/hicolor/512x512/apps"; do
    [[ -d $d ]] && rmdir --ignore-fail-on-non-empty -- "$d"
done

printf '\n%sUninstalled.%s The keyboard keeps its current lighting and actuation (stored on the board).\n' "$green" "$off"
if ((!purge && backups > 0)); then
    echo "Your backups are still in $user_dir/backups."
fi
