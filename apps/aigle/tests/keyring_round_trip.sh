#!/bin/sh
# Run only against a temporary Secret Service on a private session bus.
set -eu
if [ "${AIGLE_TEST_PRIVATE_BUS:-}" != 1 ]; then
    exec dbus-run-session -- env AIGLE_TEST_PRIVATE_BUS=1 sh "$0"
fi
test_root=$(mktemp -d)
keyring_pid=
cleanup() {
    if [ -n "$keyring_pid" ]; then kill "$keyring_pid" 2>/dev/null || true; fi
    rm -rf "$test_root"
}
trap cleanup EXIT HUP INT TERM
export XDG_DATA_HOME="$test_root/data"
export XDG_CONFIG_HOME="$test_root/config"
export XDG_CACHE_HOME="$test_root/cache"
export XDG_RUNTIME_DIR="$test_root/runtime"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
printf 'aigle-test-keyring-password\n' > "$test_root/password"
gnome-keyring-daemon --foreground --unlock --components=secrets \
    --control-directory="$XDG_RUNTIME_DIR/keyring" < "$test_root/password" > "$test_root/daemon.log" 2>&1 &
keyring_pid=$!
# Wait for the private daemon rather than letting D-Bus activate another instance.
i=0
until gdbus call --session --dest org.freedesktop.DBus \
    --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.NameHasOwner \
    org.freedesktop.secrets | rg -q true; do
    i=$((i + 1))
    if [ "$i" -ge 50 ]; then cat "$test_root/daemon.log"; exit 1; fi
    sleep 0.1
done
export AIGLE_TEST_ISOLATED_KEYRING=1
cargo test -p aigle-qt-bridge credentials::tests::isolated_keyring_round_trip --lib -- --ignored
