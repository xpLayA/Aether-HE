#!/usr/bin/env bash
# Remove what scripts/install.sh installed.
#
#   ./scripts/uninstall.sh               remove the app, keep your backups and settings
#   ./scripts/uninstall.sh --purge       also delete backups and GUI settings
#   ./scripts/uninstall.sh --prefix DIR  the prefix used at install time (default ~/.local)
#   ./scripts/uninstall.sh --yes         don't ask for confirmation (keeps keyboard settings)
#   ./scripts/uninstall.sh --keep-settings     leave the keyboard as it is now
#   ./scripts/uninstall.sh --restore-original  put the keyboard back to how it was before
#                                              Aether first changed it (its oldest backup)
#
# The keyboard stores its lighting and actuation itself. Without --restore-original
# you are asked which you want when the keyboard is connected.
set -euo pipefail

prefix="$HOME/.local"
purge=0
yes=0
keyboard=ask   # ask | keep | restore
while (($#)); do
    case "$1" in
        --prefix) prefix="${2:?--prefix needs a directory}"; shift ;;
        --purge) purge=1 ;;
        --yes|-y) yes=1 ;;
        --keep-settings) keyboard=keep ;;
        --restore-original) keyboard=restore ;;
        -h|--help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
        *) echo "unknown option: $1 (try --help)" >&2; exit 2 ;;
    esac
    shift
done
prefix="$(realpath -m "$prefix")"
repo="$(cd "$(dirname "$0")/.." && pwd)"
((yes)) && [[ $keyboard == ask ]] && keyboard=keep

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
gui_state="${XDG_CONFIG_HOME:-$HOME/.config}/aether-he/gui.json"

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

# "Start at login" from the GUI's Device page.
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/aether-he.desktop"
[[ -f $autostart ]] && files+=("$autostart")

# Only ever delete Aether's own files: refuse anything outside the expected places.
for f in "${files[@]}"; do
    case "$f" in
        "$prefix"/bin/aether|"$prefix"/bin/aether-gui|"$share"/*|\
        "$prefix"/share/applications/aether-he.desktop|"$prefix"/share/icons/hicolor/*/apps/aether-he.png|"$autostart") ;;
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

# ---- keyboard settings: keep, or restore the original (oldest backup) -----
# Decided before anything is removed, while the aether tool still exists.
aether=""
for b in "$prefix/bin/aether" "$repo/target/release/aether"; do
    if [[ -x $b ]]; then aether=$b; break; fi
done
original=""
no_restore=""
if [[ -z $aether ]]; then
    no_restore="the aether tool is not installed or built"
else
    original=$("$aether" backups 2>/dev/null | awk 'NR == 1 { print $1 }') || original=""
    if [[ -z $original ]]; then
        no_restore="there are no backups"
    elif ! "$aether" info >/dev/null 2>&1; then
        no_restore="the keyboard is not connected (or the Aula web driver has it open)"
    fi
fi
original_date=""
[[ -n $original ]] && original_date=$(date -d "@$original" '+%Y-%m-%d %H:%M')

if [[ -n $no_restore ]]; then
    if [[ $keyboard == restore ]]; then
        echo "Cannot restore the original keyboard settings: $no_restore. Nothing was removed." >&2
        exit 1
    fi
    [[ $keyboard == ask ]] && echo "${yellow}Keyboard settings will be kept: $no_restore.${off}"
    keyboard=keep
elif [[ $keyboard == ask ]]; then
    printf '%sKeyboard settings:%s\n' "$bold" "$off"
    echo "  [k] keep the current lighting and actuation (default)"
    echo "  [r] restore the original settings, from before Aether first changed them ($original_date)"
    read -r -p "Choose [k/r]: " reply
    if [[ $reply == [rR]* ]]; then keyboard=restore; else keyboard=keep; fi
fi

printf '%sWill remove:%s\n' "$bold" "$off"
for f in "${files[@]}"; do echo "  $f"; done
for r in "${rules[@]}"; do echo "  $r (sudo)"; done
if ((purge)); then
    echo "  $user_dir/backups ($backups backup files)"
    echo "  $gui_state (GUI settings)"
else
    echo "Keeping: $user_dir/backups ($backups backups) and GUI settings (use --purge to delete them)"
fi
if [[ $keyboard == restore ]]; then
    echo "Keyboard: restore the original settings from $original_date"
else
    echo "Keyboard: keep its current settings"
fi
confirm "Continue?" || { echo "Cancelled."; exit 1; }

if [[ $keyboard == restore ]]; then
    echo
    "$aether" restore --original || { echo "Restoring the keyboard failed; nothing was uninstalled." >&2; exit 1; }
fi

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
    rm -f -- "$gui_state" "$user_dir/app.ron"   # app.ron: written by versions before the Slint GUI
fi

# Remove directories that are now empty (never anything with files left in it).
for d in "$share/data/layouts" "$share/data" "$share" "$user_dir" \
         "$prefix/share/icons/hicolor/512x512/apps"; do
    [[ -d $d ]] && rmdir --ignore-fail-on-non-empty -- "$d"
done

if [[ $keyboard == restore ]]; then
    printf '\n%sUninstalled.%s The keyboard is back to its original settings (%s).\n' "$green" "$off" "$original_date"
else
    printf '\n%sUninstalled.%s The keyboard keeps its current lighting and actuation (stored on the board).\n' "$green" "$off"
fi
if ((!purge && backups > 0)); then
    echo "Your backups are still in $user_dir/backups."
fi
