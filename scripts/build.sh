#!/usr/bin/env bash
# Build Aether (Rust): runs the tests, then builds the `aether` CLI and the
# `aether-gui` desktop app.
#
#   ./scripts/build.sh            tests + release build
#   ./scripts/build.sh --no-test  skip the tests
#   ./scripts/build.sh --debug    faster, unoptimised build
set -euo pipefail

cd "$(dirname "$0")/.."

profile=release
run_tests=1
for arg in "$@"; do
    case "$arg" in
        --debug) profile=debug ;;
        --no-test) run_tests=0 ;;
        -h|--help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
        *) echo "unknown option: $arg (try --help)" >&2; exit 2 ;;
    esac
done

bold=$'\e[1m'; green=$'\e[32m'; yellow=$'\e[33m'; red=$'\e[31m'; off=$'\e[0m'
step() { printf '\n%s==> %s%s\n' "$bold" "$*" "$off"; }

# ---- prerequisites ---------------------------------------------------------
step "Checking prerequisites"
missing=()
command -v cargo >/dev/null || missing+=(rust)
command -v pkg-config >/dev/null || missing+=(pkgconf)
if command -v pkg-config >/dev/null; then
    pkg-config --exists hidapi-hidraw || missing+=(hidapi)
    pkg-config --exists libudev || missing+=(systemd-libs)
fi
if ((${#missing[@]})); then
    echo "${red}Missing: ${missing[*]}${off}"
    echo "Install them with:"
    echo "    sudo pacman -S --needed ${missing[*]}"
    exit 1
fi
echo "cargo $(cargo --version | cut -d' ' -f2), hidapi $(pkg-config --modversion hidapi-hidraw)"

# ---- tests -----------------------------------------------------------------
if ((run_tests)); then
    step "Running tests"
    cargo test --workspace --quiet
fi

# ---- build -----------------------------------------------------------------
step "Building ($profile)"
if [[ $profile == release ]]; then
    cargo build --workspace --release
else
    cargo build --workspace
fi

bin="target/$profile"
step "${green}Done${off}"
echo "  GUI: $bin/aether-gui"
echo "  CLI: $bin/aether   (try: $bin/aether info)"

# ---- keyboard access -------------------------------------------------------
rule=/etc/udev/rules.d/70-aether.rules
if [[ -f $rule ]] && cmp -s "$rule" packaging/70-aether.rules; then
    echo
    echo "udev rule installed: the keyboard is accessible without sudo."
elif ls /etc/udev/rules.d/*aula*.rules >/dev/null 2>&1; then
    echo
    echo "${yellow}An older udev rule is installed ($(ls /etc/udev/rules.d/*aula*.rules)); it works,"
    echo "but scripts/install.sh will replace it with $rule.${off}"
else
    echo
    echo "${yellow}The keyboard needs a udev rule before it can be opened without sudo:${off}"
    echo "    sudo cp packaging/70-aether.rules /etc/udev/rules.d/"
    echo "    sudo udevadm control --reload && sudo udevadm trigger"
    echo "then unplug and replug the keyboard."
fi

echo
echo "Run the app:   ./$bin/aether-gui"
echo "Install it:    ./scripts/install.sh   (app menu entry + udev rule; remove with ./scripts/uninstall.sh)"
echo "Close the Aula web driver in your browser first if it is open."
