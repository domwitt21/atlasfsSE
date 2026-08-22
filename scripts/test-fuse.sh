#!/bin/sh
set -eu

atlas_binary=${1:-target/debug/atlas}
test_root=$(mktemp -d /tmp/atlasfs-fuse-live.XXXXXX)
repository=$test_root/repository
mountpoint=$test_root/mount
log=$test_root/mount.log
mkdir "$mountpoint"

cleanup() {
    fusermount3 -u "$mountpoint" 2>/dev/null || true
    case "$test_root" in
        /tmp/atlasfs-fuse-live.*) rm -rf "$test_root" ;;
    esac
}
trap cleanup EXIT INT TERM

"$atlas_binary" init "$repository" >/dev/null
"$atlas_binary" mount "$repository" "$mountpoint" >"$log" 2>&1 &
mount_pid=$!

ready=0
for _attempt in 1 2 3 4 5; do
    if mountpoint -q "$mountpoint"; then
        ready=1
        break
    fi
    sleep 1
done
if [ "$ready" -ne 1 ]; then
    cat "$log"
    exit 1
fi

mkdir "$mountpoint/docs"
printf atlas-fuse-live >"$mountpoint/docs/file"
sync "$mountpoint/docs/file"
test "$(cat "$mountpoint/docs/file")" = atlas-fuse-live
mv "$mountpoint/docs/file" "$mountpoint/docs/renamed"
test -f "$mountpoint/docs/renamed"
rm "$mountpoint/docs/renamed"
rmdir "$mountpoint/docs"
fusermount3 -u "$mountpoint"
wait "$mount_pid"
echo "live FUSE mount test passed"
