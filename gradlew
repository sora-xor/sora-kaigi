#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
android_dir="$repo_root/clients/android"
wrapper="$android_dir/gradlew"

if [[ ! -x "$wrapper" ]]; then
  echo "error: Android wrapper not found at $wrapper" >&2
  echo "run: (cd clients/android && gradle wrapper --gradle-version 8.13)" >&2
  exit 1
fi

if [[ "${KAIGI_ANDROID_SKIP_JAVA21:-0}" != "1" ]] && command -v /usr/libexec/java_home >/dev/null 2>&1; then
  if java21_home="$(/usr/libexec/java_home -v 21 2>/dev/null)"; then
    export JAVA_HOME="$java21_home"
    export PATH="$JAVA_HOME/bin:$PATH"
  fi
fi

exec "$wrapper" -p "$android_dir" "$@"
