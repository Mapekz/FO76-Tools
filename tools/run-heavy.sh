#!/usr/bin/env bash
# Local Linux resource guard for memory-heavy development commands.
set -euo pipefail
umask 077

# This stage runs inside the scope before the requested command is executed.
if [[ ${1:-} == --inside-scope ]]; then
    expected=${2:-}
    info=${3:-}
    case $expected in
        5368709120|10737418240) ;;
        *) exit 64 ;;
    esac
    if (( $# < 4 )); then exit 64; fi
    shift 3
    group=
    while IFS=: read -r hierarchy _controllers path; do
        if [[ $hierarchy == 0 ]]; then group=$path; break; fi
    done < /proc/self/cgroup
    cgroup="/sys/fs/cgroup$group"
    for control in memory.max memory.swap.max memory.oom.group; do
        if [[ -z $group || ! -r $cgroup/$control ]]; then
            printf 'Memory controller is unavailable. Command was not run.\n' >&2
            exit 78
        fi
    done
    read -r actual < "$cgroup/memory.max"
    read -r swap < "$cgroup/memory.swap.max"
    read -r oom_group < "$cgroup/memory.oom.group"
    if [[ $actual != "$expected" || $swap != 0 || $oom_group != 1 ]]; then
        printf 'Memory protection is not active. Command was not run.\n' >&2
        exit 78
    fi
    if [[ ! -w $cgroup/cgroup.kill || ! -r $cgroup/cgroup.events ]]; then
        printf 'Cgroup lifecycle controls are unavailable. Command was not run.\n' >&2
        exit 78
    fi
    # Publish an exact kernel object identity before any user work can start.
    inode=$(stat -Lc '%i' "$cgroup")
    printf '%s\n%s\n' "$cgroup" "$inode" > "$info.new"
    mv -- "$info.new" "$info"
    exec "$@"
fi

usage() {
    printf 'Usage: %s {esm|cargo} command [args...]\n' "$0"
    printf 'esm: 5G; cargo: 10G; no swap; one heavy command per user.\n'
}
if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
if (( $# < 2 )); then usage >&2; exit 64; fi
case $1 in
    esm) limit=5G; expected=5368709120 ;;
    cargo) limit=10G; expected=10737418240 ;;
    *) usage >&2; exit 64 ;;
esac
shift
for dependency in systemd-run systemctl findmnt realpath stat; do
    if ! command -v "$dependency" >/dev/null; then
        printf 'Required Linux tool is unavailable: %s. Command was not run.\n' "$dependency" >&2
        exit 69
    fi
done

# XDG paths must be absolute; ignore invalid relative values as the spec requires.
cache_home=${XDG_CACHE_HOME:-$HOME/.cache}
[[ $cache_home == /* ]] || cache_home=$HOME/.cache
scratch=${FO76_SCRATCH_DIR:-$cache_home/fo76-tools/heavy}
mkdir -p -- "$scratch"
scratch=$(cd -- "$scratch" && pwd -P)
filesystem=$(findmnt --noheadings --output FSTYPE --target "$scratch")
case $filesystem in
    tmpfs|ramfs|'')
        printf 'Scratch must be a verified disk directory. Set FO76_SCRATCH_DIR. Command was not run.\n' >&2
        exit 78
        ;;
esac

# A stable unit name is an atomic per-user lock, held until every process exits.
# The systemd user manager itself supplies the per-user namespace.
unit=fo76-heavy.scope
owner="fo76-heavy-$$-$RANDOM"
if systemctl --user is-active --quiet "$unit"; then
    printf 'Another heavy command is running in %s. Command was not run.\n' "$unit" >&2
    exit 75
fi
run=$(mktemp -d "$scratch/run.XXXXXXXX")
mkdir "$run/tmp"
export TMPDIR="$run/tmp" TMP="$run/tmp" TEMP="$run/tmp"
log="$run/output.log"
script=$(realpath -- "${BASH_SOURCE[0]}")
runner_pid=
runner_pending=0
scope_info="$run/cgroup-info"
scope_fd=

open_scope() {
    if [[ -n $scope_fd ]]; then return 0; fi
    [[ -f $scope_info ]] || return 1
    local fields inode candidate_fd
    mapfile -t fields < "$scope_info" || return 2
    if (( ${#fields[@]} != 2 )) || [[ ${fields[0]} != /sys/fs/cgroup/* ]]; then return 2; fi
    [[ -d ${fields[0]} ]] || return 1
    if ! { exec {candidate_fd}<"${fields[0]}"; } 2>/dev/null; then return 2; fi
    if ! inode=$(stat -Lc '%i' "/proc/$$/fd/$candidate_fd"); then
        exec {candidate_fd}<&-
        return 2
    fi
    if [[ $inode != "${fields[1]}" ]]; then
        exec {candidate_fd}<&-
        return 1
    fi
    # Signal traps may reenter this function; publish only a verified handle.
    scope_fd=$candidate_fd
}
scope_live() {
    open_scope || return $?
    local key value
    [[ -e /proc/$$/fd/$scope_fd/cgroup.events ]] || return 1
    while read -r key value; do
        if [[ $key == populated ]]; then [[ $value == 1 ]]; return; fi
    done < "/proc/$$/fd/$scope_fd/cgroup.events"
    return 2
}
# shellcheck disable=SC2329 # Invoked by the cancellation trap via cancel.
kill_scope() {
    open_scope || return $?
    # This fd points to the original kernel object, even after its name is reused.
    { printf '1\n' > "/proc/$$/fd/$scope_fd/cgroup.kill"; } 2>/dev/null
}
# shellcheck disable=SC2329 # Invoked by the EXIT trap.
cleanup() {
    local live=0
    scope_live || live=$?
    if (( live != 1 )); then
        printf 'Scope completion is unverified; temporary files retained in %s.\n' "$run/tmp" >&2
    else
        rm -rf -- "$run/tmp"
    fi
}
# shellcheck disable=SC2329 # Invoked by the signal traps.
cancel() {
    trap '' INT TERM HUP
    kill_scope || true
    if (( runner_pending )); then
        runner_pid=${runner_pid:-${!:-}}
        # Bash's live job table excludes a runner already reaped by wait.
        if [[ -n $runner_pid && $(jobs -pr) == "$runner_pid" ]]; then
            kill -KILL "$runner_pid" 2>/dev/null || true
        fi
        # Startup may have published its cgroup between the first check and kill.
        kill_scope || true
        if [[ -n $runner_pid ]]; then wait "$runner_pid" 2>/dev/null || true; fi
    fi
    exit "$1"
}
trap cleanup EXIT
trap 'cancel 130' INT
trap 'cancel 143' TERM
trap 'cancel 129' HUP

printf 'Heavy command: %s cap, no swap; unit=%s; owner=%s; log=%s\n' "$limit" "$unit" "$owner" "$log"
# Atomic unit creation rejects a simultaneous startup even after the busy check.
# The inner stage verifies real controller settings; never retry uncapped.
runner_pending=1
systemd-run --user --scope --quiet --collect --unit="$unit" --description="$owner" \
    --expand-environment=no \
    --property="MemoryMax=$limit" --property=MemorySwapMax=0 --property=OOMPolicy=kill \
    bash "$script" --inside-scope "$expected" "$scope_info" "$@" <&0 >"$log" 2>&1 &
runner_pid=$!
status=0
wait "$runner_pid" || status=$?
runner_pending=0
runner_pid=
# The foreground program may exit while detached descendants remain alive.
# Retain both exclusivity and scratch until the entire owned scope is done.
while true; do
    live=0
    scope_live || live=$?
    if (( live == 1 )); then break; fi
    if (( live == 2 )); then
        printf 'Cannot verify scope completion; temporary files retained.\n' >&2
        status=78
        break
    fi
    sleep 0.1
done
printf 'Heavy command exit=%s; full log=%s\n' "$status" "$log"
if (( status != 0 )); then
    printf 'Failure excerpt, last 30 lines and at most 4000 bytes:\n' >&2
    tail -n 30 "$log" | tail -c 4000 >&2
fi
exit "$status"
