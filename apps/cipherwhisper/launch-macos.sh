#!/bin/sh
# Exit after dispatch so another Finder open can reopen the running setup UI.
app_binary_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
"$app_binary_dir/cipherwhisper-runtime" "$@" </dev/null >/dev/null 2>&1 &
