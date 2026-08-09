#!/bin/sh

# Validate mandatory Cog execution evidence in one or more feature worklogs.
# Historical worklogs predate this policy, so callers pass only current entries.

set -eu

if [ "$#" -eq 0 ]; then
    echo "usage: $0 <worklog.md> [worklog.md ...]" >&2
    exit 2
fi

failed=0

check_worklog() {
    worklog=$1

    if [ ! -f "$worklog" ]; then
        echo "$worklog: file not found" >&2
        failed=1
        return
    fi

    require() {
        pattern=$1
        description=$2
        if ! grep -Eq -- "$pattern" "$worklog"; then
            echo "$worklog: missing $description" >&2
            failed=1
        fi
    }

    require '^## Cog execution evidence$' 'Cog execution evidence section'
    require '^- Graph id: `[^`]+`$' 'graph id'
    require '^### Initial render$' 'initial render heading'
    require '^frontier [0-9]+:' 'initial frontier render'
    require '^### Node execution$' 'node execution heading'
    require 'claimed.*closed.*output' 'claimed/closed node execution with output'
    require '^### Notes$' 'notes heading'
    require '^### Final status$' 'final status heading'
    require '^- Status: `complete`$' 'complete final status'
    require 'omega' 'omega in execution evidence'
}

for worklog in "$@"; do
    check_worklog "$worklog"
done

if [ "$failed" -ne 0 ]; then
    exit 1
fi

echo "Cog worklog evidence verified: $*"
