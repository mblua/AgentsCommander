#!/usr/bin/env bash
set -euo pipefail
umask 077

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <exact-deb-path>" >&2
  exit 2
fi

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "linux DEB smoke can run only on Linux" >&2
  exit 2
fi

DEB_PATH="$(realpath -e -- "$1")"
if [[ ! -f "$DEB_PATH" ]]; then
  echo "DEB input is not a regular file" >&2
  exit 2
fi

RAW_PARENT="$(realpath -m -- "${RUNNER_TEMP:-${TMPDIR:-/tmp}}")"
ARTIFACT_DIR="${SMOKE_ARTIFACT_DIR:-$PWD/artifacts/linux-deb-smoke}"
ARTIFACT_PARENT="$(dirname -- "$ARTIFACT_DIR")"
mkdir -p -- "$ARTIFACT_PARENT"
if ! mkdir -- "$ARTIFACT_DIR"; then
  echo "smoke artifact directory already exists: $ARTIFACT_DIR" >&2
  exit 2
fi
chmod 0700 "$ARTIFACT_DIR"
RAW_ROOT="$(mktemp -d "$RAW_PARENT/agentscommander-deb-smoke.XXXXXX")"

ASSERTIONS="$ARTIFACT_DIR/assertions.txt"
PROCESS_STATUS="$ARTIFACT_DIR/process-status.txt"
: >"$ASSERTIONS"
: >"$PROCESS_STATUS"
chmod 0600 "$ASSERTIONS" "$PROCESS_STATUS"

declare -a GROUP_PID=()
declare -a GROUP_PGID=()
declare -a GROUP_START=()
declare -a GROUP_NAME=()
declare -a GROUP_REAPED=()
declare -a GROUP_STATUS=()

MASTER_TOKEN_COPY="$RAW_ROOT/master-token.value"
WEB_TOKEN_COPY="$RAW_ROOT/web-token.value"
RAW_FIRST_STDOUT="$RAW_ROOT/gui-first.stdout"
RAW_FIRST_STDERR="$RAW_ROOT/gui-first.stderr"
RAW_SECOND_STDOUT="$RAW_ROOT/gui-second.stdout"
RAW_SECOND_STDERR="$RAW_ROOT/gui-second.stderr"
FINAL_RESULT="failed"
CONFIG_DIR=""

assert_note() {
  printf '%s\n' "$1" >>"$ASSERTIONS"
}

fail() {
  assert_note "FAIL: $1"
  echo "linux DEB smoke failed: $1" >&2
  exit 1
}

proc_identity() {
  local pid="$1"
  local stat_line
  local after
  local -a fields

  [[ -r "/proc/$pid/stat" ]] || return 1
  IFS= read -r stat_line <"/proc/$pid/stat" || return 1
  [[ "$stat_line" == *") "* ]] || return 1
  after="${stat_line##*) }"
  read -r -a fields <<<"$after"
  [[ ${#fields[@]} -ge 20 ]] || return 1
  printf '%s %s %s\n' "${fields[0]}" "${fields[2]}" "${fields[19]}"
}

track_group() {
  local pid="$1"
  local name="$2"
  local identity=""
  local state=""
  local pgid=""
  local start=""
  local attempt

  for attempt in $(seq 1 50); do
    if identity="$(proc_identity "$pid")"; then
      read -r state pgid start <<<"$identity"
      break
    fi
    sleep 0.1
  done
  [[ -n "$identity" ]] || fail "$name exited before its process identity was captured"
  [[ "$state" != "Z" ]] || fail "$name exited before startup"
  [[ "$pgid" == "$pid" ]] || fail "$name did not become its own process-group leader"

  GROUP_PID+=("$pid")
  GROUP_PGID+=("$pgid")
  GROUP_START+=("$start")
  GROUP_NAME+=("$name")
  GROUP_REAPED+=("0")
  GROUP_STATUS+=("")
  printf '%s leader=%s pgid=%s start=%s\n' "$name" "$pid" "$pgid" "$start" \
    >>"$PROCESS_STATUS"
}

reap_group_leader() {
  local index="$1"
  local pid="${GROUP_PID[$index]}"
  local status

  if [[ "${GROUP_REAPED[$index]}" == "1" ]]; then
    return 0
  fi
  set +e
  wait "$pid"
  status=$?
  set -e
  GROUP_REAPED[$index]="1"
  GROUP_STATUS[$index]="$status"
  printf '%s reaped status=%s\n' "${GROUP_NAME[$index]}" "$status" >>"$PROCESS_STATUS"
}

wait_group_exit() {
  local index="$1"
  local max_ticks="$2"
  local pid="${GROUP_PID[$index]}"
  local expected_pgid="${GROUP_PGID[$index]}"
  local expected_start="${GROUP_START[$index]}"
  local identity=""
  local state=""
  local pgid=""
  local start=""
  local tick

  for tick in $(seq 1 "$max_ticks"); do
    if ! identity="$(proc_identity "$pid")"; then
      reap_group_leader "$index"
      return 0
    fi
    read -r state pgid start <<<"$identity"
    if [[ "$pgid" != "$expected_pgid" || "$start" != "$expected_start" ]]; then
      printf '%s wait-refused identity-changed\n' "${GROUP_NAME[$index]}" \
        >>"$PROCESS_STATUS"
      return 2
    fi
    if [[ "$state" == "Z" ]]; then
      reap_group_leader "$index"
      return 0
    fi
    sleep 0.1
  done
  return 1
}

signal_group_if_owned() {
  local index="$1"
  local signal="$2"
  local pid="${GROUP_PID[$index]}"
  local expected_pgid="${GROUP_PGID[$index]}"
  local expected_start="${GROUP_START[$index]}"
  local identity
  local state
  local pgid
  local start

  identity="$(proc_identity "$pid")" || return 1
  read -r state pgid start <<<"$identity"
  [[ "$state" != "Z" ]] || return 1
  if [[ "$pgid" != "$expected_pgid" || "$start" != "$expected_start" ]]; then
    printf '%s signal-refused identity-changed\n' "${GROUP_NAME[$index]}" \
      >>"$PROCESS_STATUS"
    return 2
  fi
  kill "-$signal" -- "-$expected_pgid"
}

stop_group() {
  local index="$1"

  if [[ "${GROUP_REAPED[$index]}" == "1" ]]; then
    return 0
  fi
  if signal_group_if_owned "$index" TERM; then
    printf '%s signal=TERM\n' "${GROUP_NAME[$index]}" >>"$PROCESS_STATUS"
  fi
  if wait_group_exit "$index" 100; then
    return 0
  fi
  if signal_group_if_owned "$index" KILL; then
    printf '%s signal=KILL\n' "${GROUP_NAME[$index]}" >>"$PROCESS_STATUS"
  fi
  if ! wait_group_exit "$index" 50; then
    printf '%s reap-timeout\n' "${GROUP_NAME[$index]}" >>"$PROCESS_STATUS"
    return 1
  fi
}

sanitize_one_output() {
  local source="$1"
  local destination="$2"

  [[ -f "$source" ]] || return 0
  python3 - "$source" "$destination" "$MASTER_TOKEN_COPY" "$WEB_TOKEN_COPY" <<'PY'
from pathlib import Path
import sys

source, destination, master_path, web_path = map(Path, sys.argv[1:])
tokens = []
for token_path in (master_path, web_path):
    if token_path.is_file():
        value = token_path.read_text(encoding="utf-8").strip()
        if value:
            tokens.append(value)

with source.open("r", encoding="utf-8", errors="backslashreplace") as src:
    with destination.open("w", encoding="utf-8", newline="") as dst:
        for line in src:
            if "[master-token]" in line or "[web-token]" in line:
                dst.write("[redacted-token-record]\n")
                continue
            for token in tokens:
                line = line.replace(token, "[redacted-token]")
            dst.write(line)
PY
  chmod 0600 "$destination"
}

sanitize_outputs() {
  sanitize_one_output "$RAW_FIRST_STDOUT" "$ARTIFACT_DIR/gui-first.stdout.sanitized"
  sanitize_one_output "$RAW_FIRST_STDERR" "$ARTIFACT_DIR/gui-first.stderr.sanitized"
  sanitize_one_output "$RAW_SECOND_STDOUT" "$ARTIFACT_DIR/gui-second.stdout.sanitized"
  sanitize_one_output "$RAW_SECOND_STDERR" "$ARTIFACT_DIR/gui-second.stderr.sanitized"
}

validate_artifacts_have_no_tokens() {
  python3 - "$ARTIFACT_DIR" "$MASTER_TOKEN_COPY" "$WEB_TOKEN_COPY" <<'PY'
from pathlib import Path
import sys

artifact_dir, master_path, web_path = map(Path, sys.argv[1:])
tokens = []
for token_path in (master_path, web_path):
    if token_path.is_file():
        value = token_path.read_bytes().strip()
        if value:
            tokens.append(value)

for artifact in artifact_dir.rglob("*"):
    if not artifact.is_file():
        continue
    data = artifact.read_bytes()
    for token in tokens:
        if token in data:
            raise SystemExit(f"recognized bearer token remained in {artifact.name}")
PY
}

cleanup() {
  local original_status=$?
  local cleanup_failed=0
  local index

  trap - EXIT INT TERM
  set +e
  for ((index = ${#GROUP_PID[@]} - 1; index >= 0; index--)); do
    stop_group "$index" || cleanup_failed=1
  done
  if [[ -n "$CONFIG_DIR" ]]; then
    if [[ -f "$CONFIG_DIR/master-token.txt" && ! -L "$CONFIG_DIR/master-token.txt" ]]; then
      install -m 0600 "$CONFIG_DIR/master-token.txt" "$MASTER_TOKEN_COPY" ||
        cleanup_failed=1
    fi
    if [[ -f "$CONFIG_DIR/web-token.txt" && ! -L "$CONFIG_DIR/web-token.txt" ]]; then
      install -m 0600 "$CONFIG_DIR/web-token.txt" "$WEB_TOKEN_COPY" ||
        cleanup_failed=1
    fi
  fi
  sanitize_outputs || cleanup_failed=1
  validate_artifacts_have_no_tokens || cleanup_failed=1
  if [[ "$FINAL_RESULT" == "passed" && $cleanup_failed -eq 0 && $original_status -eq 0 ]]; then
    printf 'result=passed\n' >>"$ASSERTIONS"
  else
    printf 'result=failed\n' >>"$ASSERTIONS"
  fi
  if [[ -n "$RAW_ROOT" && "$RAW_ROOT" == "$RAW_PARENT"/agentscommander-deb-smoke.* ]]; then
    rm -rf -- "$RAW_ROOT"
  fi
  if [[ $original_status -eq 0 && $cleanup_failed -ne 0 ]]; then
    original_status=1
  fi
  exit "$original_status"
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

ARCHIVE_ROOT="$(mktemp -d "$RAW_ROOT/archive.XXXXXX")"
dpkg-deb -x "$DEB_PATH" "$ARCHIVE_ROOT"
mapfile -t exact_archive_entries < <(
  dpkg-deb --fsys-tarfile "$DEB_PATH" | tar -tf - |
    awk '$0 == "usr/bin/agentscommander" ||
         $0 == "./usr/bin/agentscommander" { print }'
)
[[ ${#exact_archive_entries[@]} -eq 1 ]] ||
  fail "archive does not contain exactly one canonical usr/bin/agentscommander entry"
[[ -f "$ARCHIVE_ROOT/usr/bin/agentscommander" ]] ||
  fail "archive AgentsCommander payload is not a regular file"
[[ ! -L "$ARCHIVE_ROOT/usr/bin/agentscommander" ]] ||
  fail "archive AgentsCommander payload is a symlink"
[[ -x "$ARCHIVE_ROOT/usr/bin/agentscommander" ]] ||
  fail "archive AgentsCommander payload is not executable"
[[ ! -e "$ARCHIVE_ROOT/usr/bin/.agentscommander" ]] ||
  fail "archive contains forbidden /usr/bin/.agentscommander state"
mapfile -d '' -t native_payloads < <(
  find "$ARCHIVE_ROOT" -xdev -type f -name agentscommander -perm /111 -print0 | sort -z
)
[[ ${#native_payloads[@]} -eq 1 ]] ||
  fail "archive contains another executable agentscommander payload"
[[ "${native_payloads[0]}" == "$ARCHIVE_ROOT/usr/bin/agentscommander" ]] ||
  fail "archive native binary is not in the canonical path"
assert_note "archive-layout=passed"

PACKAGE_NAME="$(dpkg-deb -f "$DEB_PATH" Package)"
PACKAGE_VERSION="$(dpkg-deb -f "$DEB_PATH" Version)"
[[ -n "$PACKAGE_NAME" && -n "$PACKAGE_VERSION" ]] ||
  fail "package metadata is incomplete"
sudo apt-get install -y --no-install-recommends "$DEB_PATH"
[[ -f /usr/bin/agentscommander && ! -L /usr/bin/agentscommander &&
  -x /usr/bin/agentscommander ]] ||
  fail "installed /usr/bin/agentscommander is not a regular non-symlink executable"
dpkg-query -L "$PACKAGE_NAME" | grep -Fxq /usr/bin/agentscommander ||
  fail "installed package does not list /usr/bin/agentscommander"
PACKAGE_OWNER="$(dpkg-query -S /usr/bin/agentscommander)"
case "$PACKAGE_OWNER" in
  "$PACKAGE_NAME: /usr/bin/agentscommander" | "$PACKAGE_NAME":*": /usr/bin/agentscommander") ;;
  *) fail "installed binary is not owned by the expected package" ;;
esac
printf 'package=%s\nversion=%s\nbinary=/usr/bin/agentscommander\n' \
  "$PACKAGE_NAME" "$PACKAGE_VERSION" >>"$ARTIFACT_DIR/package.txt"
chmod 0600 "$ARTIFACT_DIR/package.txt"
assert_note "installed-layout=passed"

HOME_DIR="$(mktemp -d "$RAW_ROOT/home.XXXXXX")"
XDG_CONFIG_HOME="$(mktemp -d "$RAW_ROOT/config.XXXXXX")"
XDG_CACHE_HOME="$(mktemp -d "$RAW_ROOT/cache.XXXXXX")"
XDG_DATA_HOME="$(mktemp -d "$RAW_ROOT/data.XXXXXX")"
XDG_RUNTIME_DIR="$(mktemp -d "$RAW_ROOT/runtime.XXXXXX")"
PROJECT_PARENT="$(mktemp -d "$RAW_ROOT/projects.XXXXXX")"
PROJECT_DIR="$(mktemp -d "$PROJECT_PARENT/smoke-project.XXXXXX")"
MARKER_DIR="$(mktemp -d "$RAW_ROOT/markers.XXXXXX")"
chmod 0700 "$HOME_DIR" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" \
  "$XDG_DATA_HOME" "$XDG_RUNTIME_DIR" "$PROJECT_PARENT" "$PROJECT_DIR" "$MARKER_DIR"

mkdir -p "$HOME_DIR/.local/bin" "$HOME_DIR/bin" "$HOME_DIR/.cargo/bin"
chmod 0700 "$HOME_DIR/.local" "$HOME_DIR/.local/bin" "$HOME_DIR/bin" \
  "$HOME_DIR/.cargo" "$HOME_DIR/.cargo/bin"

SYSTEM_GIT_BEFORE="$(PATH=/usr/bin:/bin command -v git)"
[[ "$SYSTEM_GIT_BEFORE" == /usr/bin/git || "$SYSTEM_GIT_BEFORE" == /bin/git ]] ||
  fail "fixed parent PATH did not resolve system Git"

install -m 0755 /dev/stdin "$HOME_DIR/.local/bin/codex" <<'MARKER'
#!/usr/bin/env bash
set -euo pipefail
observed="$SMOKE_MARKER_DIR/codex-observed.txt"
{
  printf 'execution=ok\n'
  printf 'local_dir=%s\n' "${AGENTSCOMMANDER_LOCAL_DIR-}"
  printf 'path=%s\n' "${PATH-}"
  printf 'git=%s\n' "$(command -v git)"
} >"$observed"
chmod 0600 "$observed"
printf 'ran\n' >"$SMOKE_MARKER_DIR/codex-executed"
chmod 0600 "$SMOKE_MARKER_DIR/codex-executed"
printf 'codex-smoke-marker\n'
MARKER

install -m 0755 /dev/stdin "$HOME_DIR/.local/bin/git" <<'HOSTILE_GIT'
#!/usr/bin/env bash
set -euo pipefail
printf 'executed\n' >>"$SMOKE_MARKER_DIR/hostile-git-executed"
chmod 0600 "$SMOKE_MARKER_DIR/hostile-git-executed"
exec /usr/bin/git "$@"
HOSTILE_GIT

run_cli() {
  local label="$1"
  shift
  local stdout="$RAW_ROOT/cli-$label.stdout"
  local stderr="$RAW_ROOT/cli-$label.stderr"
  : >"$stdout"
  : >"$stderr"
  chmod 0600 "$stdout" "$stderr"
  env -i \
    HOME="$HOME_DIR" \
    USER="$(id -un)" \
    LOGNAME="$(id -un)" \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    PATH=/usr/bin:/bin \
    XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
    XDG_CACHE_HOME="$XDG_CACHE_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    SMOKE_MARKER_DIR="$MARKER_DIR" \
    /usr/bin/agentscommander "$@" >"$stdout" 2>"$stderr"
}

run_cli coding-agent coding-agent add \
  --id smoke-codex \
  --label "Smoke Codex" \
  --command codex \
  --backend local ||
  fail "coding-agent add failed"
run_cli new-project new-project "$PROJECT_DIR" ||
  fail "new-project failed"
assert_note "pre-gui-cli=passed"

CONFIG_DIR="$XDG_CONFIG_HOME/agentscommander"
EXPECTED_CHILD_PATH="$HOME_DIR/.local/bin:$HOME_DIR/bin:$HOME_DIR/.cargo/bin:/usr/bin:/bin"
EXPECTED_HOSTILE_GIT="$HOME_DIR/.local/bin/git"

launch_gui() {
  local label="$1"
  local stdout="$2"
  local stderr="$3"
  local pid

  : >"$stdout"
  : >"$stderr"
  chmod 0600 "$stdout" "$stderr"
  setsid env -i \
    HOME="$HOME_DIR" \
    USER="$(id -un)" \
    LOGNAME="$(id -un)" \
    SHELL=/bin/bash \
    LANG=C.UTF-8 \
    LC_ALL=C.UTF-8 \
    PATH=/usr/bin:/bin \
    TMPDIR="$RAW_ROOT" \
    XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
    XDG_CACHE_HOME="$XDG_CACHE_HOME" \
    XDG_DATA_HOME="$XDG_DATA_HOME" \
    XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
    GDK_BACKEND=x11 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    SMOKE_MARKER_DIR="$MARKER_DIR" \
    dbus-run-session -- xvfb-run -a --server-args="-screen 0 1280x800x24" \
    /usr/bin/agentscommander >"$stdout" 2>"$stderr" &
  pid=$!
  track_group "$pid" "$label"
  LAST_GROUP_INDEX=$((${#GROUP_PID[@]} - 1))
}

launch_gui "first-gui" "$RAW_FIRST_STDOUT" "$RAW_FIRST_STDERR"
FIRST_GROUP_INDEX="$LAST_GROUP_INDEX"

readiness_met=0
INSTANCE_DIR=""
OUTBOX_DIR=""
DAEMON_PID=""
for _ in $(seq 1 400); do
  if [[ -f "$CONFIG_DIR/daemon.pid" &&
    -f "$CONFIG_DIR/master-token.txt" &&
    -f "$CONFIG_DIR/web-token.txt" &&
    -f "$CONFIG_DIR/app-outbox-path.txt" &&
    -f "$CONFIG_DIR/gui-instance.lock" &&
    -f "$CONFIG_DIR/coding-agent-mutation.lock" &&
    -d "$CONFIG_DIR/instances" ]]; then
    DAEMON_PID="$(<"$CONFIG_DIR/daemon.pid")"
    if [[ "$DAEMON_PID" =~ ^[1-9][0-9]*$ ]] && kill -0 "$DAEMON_PID" 2>/dev/null; then
      mapfile -d '' -t instance_entries < <(
        find "$CONFIG_DIR/instances" -mindepth 1 -maxdepth 1 -print0 | sort -z
      )
      if [[ ${#instance_entries[@]} -eq 1 &&
        -d "${instance_entries[0]}" &&
        ! -L "${instance_entries[0]}" ]]; then
        instance_name="$(basename -- "${instance_entries[0]}")"
        if [[ ! "$instance_name" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]]; then
          sleep 0.15
          continue
        fi
        INSTANCE_DIR="$CONFIG_DIR/instances/$instance_name"
        OUTBOX_DIR="$INSTANCE_DIR/outbox"
        POINTER_VALUE="$(<"$CONFIG_DIR/app-outbox-path.txt")"
        if [[ -d "$OUTBOX_DIR" && "$POINTER_VALUE" == "$OUTBOX_DIR" ]]; then
          readiness_met=1
          break
        fi
      fi
    fi
  fi
  sleep 0.15
done
[[ "$readiness_met" == "1" ]] || fail "first GUI did not reach finite readiness"

install -m 0600 "$CONFIG_DIR/master-token.txt" "$MASTER_TOKEN_COPY"
install -m 0600 "$CONFIG_DIR/web-token.txt" "$WEB_TOKEN_COPY"
assert_note "gui-readiness=passed"

PROJECT_BASENAME="$(basename -- "$PROJECT_DIR")"
run_cli create-agent create-agent \
  --project "$PROJECT_BASENAME" \
  --name smoke-agent \
  --description smoke \
  --launch smoke-codex ||
  fail "create-agent launch request failed"

marker_ready=0
for _ in $(seq 1 400); do
  if [[ -f "$MARKER_DIR/codex-executed" &&
    -f "$MARKER_DIR/codex-observed.txt" ]]; then
    marker_ready=1
    break
  fi
  sleep 0.15
done
[[ "$marker_ready" == "1" ]] || fail "local Codex marker did not execute"

grep -Fxq 'execution=ok' "$MARKER_DIR/codex-observed.txt" ||
  fail "Codex marker did not record execution"
grep -Fxq "local_dir=$CONFIG_DIR" "$MARKER_DIR/codex-observed.txt" ||
  fail "Codex child received the wrong AGENTSCOMMANDER_LOCAL_DIR"
grep -Fxq "path=$EXPECTED_CHILD_PATH" "$MARKER_DIR/codex-observed.txt" ||
  fail "Codex child PATH ordering was incorrect"
grep -Fxq "git=$EXPECTED_HOSTILE_GIT" "$MARKER_DIR/codex-observed.txt" ||
  fail "Codex child did not resolve the prepended Git marker"
[[ ! -e "$MARKER_DIR/hostile-git-executed" ]] ||
  fail "AgentsCommander internal Git inherited the child-only PATH"
SYSTEM_GIT_AFTER="$(PATH=/usr/bin:/bin command -v git)"
[[ "$SYSTEM_GIT_AFTER" == "$SYSTEM_GIT_BEFORE" ]] ||
  fail "parent-shell Git resolution changed"
assert_note "codex-child-path=passed"
assert_note "internal-git-path-isolation=passed"

check_private_dir() {
  local path="$1"
  [[ -d "$path" && ! -L "$path" ]] || fail "private directory shape is unsafe"
  [[ "$(stat -c '%u' "$path")" == "$(id -u)" ]] ||
    fail "private directory owner is incorrect"
  [[ "$(stat -c '%a' "$path")" == "700" ]] ||
    fail "private directory mode is not 0700"
}

check_private_file() {
  local path="$1"
  [[ -f "$path" && ! -L "$path" ]] || fail "private file shape is unsafe"
  [[ "$(stat -c '%u' "$path")" == "$(id -u)" ]] ||
    fail "private file owner is incorrect"
  [[ "$(stat -c '%h' "$path")" == "1" ]] ||
    fail "private file has more than one link"
  [[ "$(stat -c '%a' "$path")" == "600" ]] ||
    fail "private file mode is not 0600"
}

check_private_dir "$CONFIG_DIR"
check_private_dir "$CONFIG_DIR/instances"
check_private_dir "$INSTANCE_DIR"
check_private_dir "$OUTBOX_DIR"
for fixed_name in \
  gui-instance.lock \
  coding-agent-mutation.lock \
  app.log \
  app.log.1 \
  app.log.2 \
  app.log.3 \
  app.log.4 \
  app.log.5 \
  settings.json \
  settings.pre-384-v1.json \
  web-token.txt \
  master-token.txt \
  app-outbox-path.txt \
  daemon.pid; do
  if [[ -e "$CONFIG_DIR/$fixed_name" ]]; then
    check_private_file "$CONFIG_DIR/$fixed_name"
  fi
done
assert_note "linux-private-modes=passed"

for forbidden in \
  /usr/bin/.agentscommander \
  "$HOME_DIR/.agentscommander" \
  "$HOME_DIR/.agentscommander-new" \
  "$HOME_DIR/.agentscommander-new-dev"; do
  [[ ! -e "$forbidden" ]] || fail "legacy config path was created"
done
assert_note "legacy-paths-absent=passed"

write_snapshot() {
  local output="$1"
  local file
  local relative
  local kind
  local mode
  local identity
  local digest

  : >"$output"
  chmod 0600 "$output"
  for file in \
    "$CONFIG_DIR/master-token.txt" \
    "$CONFIG_DIR/web-token.txt" \
    "$CONFIG_DIR/app-outbox-path.txt" \
    "$CONFIG_DIR/daemon.pid"; do
    printf 'fixed\t%s\t%s\t%s\n' \
      "$(basename -- "$file")" \
      "$(stat -c '%d:%i:%a' "$file")" \
      "$(sha256sum "$file" | awk '{print $1}')" >>"$output"
  done
  printf 'instance\t.\t%s\n' "$(stat -c '%d:%i:%a' "$INSTANCE_DIR")" >>"$output"
  while IFS= read -r -d '' file; do
    relative="${file#"$INSTANCE_DIR"/}"
    if [[ -L "$file" ]]; then
      fail "instance snapshot found an unsafe symlink"
    elif [[ -d "$file" ]]; then
      kind="directory"
      mode="$(stat -c '%a' "$file")"
      identity="$(stat -c '%d:%i' "$file")"
      printf 'tree\t%s\t%s\t%s\t%s\n' "$relative" "$kind" "$mode" "$identity" \
        >>"$output"
    elif [[ -f "$file" ]]; then
      kind="file"
      mode="$(stat -c '%a' "$file")"
      identity="$(stat -c '%d:%i' "$file")"
      digest="$(sha256sum "$file" | awk '{print $1}')"
      printf 'tree\t%s\t%s\t%s\t%s\t%s\n' \
        "$relative" "$kind" "$mode" "$identity" "$digest" >>"$output"
    else
      fail "instance snapshot found an unsafe special file"
    fi
  done < <(find "$INSTANCE_DIR" -mindepth 1 -print0 | sort -z)
}

SNAPSHOT="$RAW_ROOT/readiness.snapshot"
SNAPSHOT_NEXT="$RAW_ROOT/readiness.snapshot.next"
write_snapshot "$SNAPSHOT"
snapshot_stable=0
for _ in $(seq 1 50); do
  sleep 0.15
  write_snapshot "$SNAPSHOT_NEXT"
  if cmp -s "$SNAPSHOT" "$SNAPSHOT_NEXT"; then
    snapshot_stable=1
    break
  fi
  mv -f "$SNAPSHOT_NEXT" "$SNAPSHOT"
done
[[ "$snapshot_stable" == "1" ]] || fail "live instance snapshot did not stabilize"
install -m 0600 "$SNAPSHOT" "$ARTIFACT_DIR/readiness-snapshot.txt"

launch_gui "second-gui" "$RAW_SECOND_STDOUT" "$RAW_SECOND_STDERR"
SECOND_GROUP_INDEX="$LAST_GROUP_INDEX"
if ! wait_group_exit "$SECOND_GROUP_INDEX" 200; then
  fail "second GUI did not exit within its finite deadline"
fi
[[ "${GROUP_STATUS[$SECOND_GROUP_INDEX]}" == "0" ]] ||
  fail "second GUI did not return exit status 0"

write_snapshot "$SNAPSHOT_NEXT"
cmp -s "$SNAPSHOT" "$SNAPSHOT_NEXT" ||
  fail "second GUI changed live readiness state"
assert_note "second-gui-exit-zero=passed"
assert_note "second-gui-state-unchanged=passed"

[[ ! -e "$MARKER_DIR/hostile-git-executed" ]] ||
  fail "hostile Git wrapper executed during the smoke"
SYSTEM_GIT_FINAL="$(PATH=/usr/bin:/bin command -v git)"
[[ "$SYSTEM_GIT_FINAL" == "$SYSTEM_GIT_BEFORE" ]] ||
  fail "parent-shell Git resolution changed after second launch"

stop_group "$FIRST_GROUP_INDEX" || fail "first GUI process group did not stop cleanly"
FINAL_RESULT="passed"
