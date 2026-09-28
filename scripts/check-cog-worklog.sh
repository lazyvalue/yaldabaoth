#!/bin/sh

# Validate the Cog evidence in one or more worklogs. The graph itself holds the
# per-node outputs and notes; the worklog only needs to point at it and prove it
# reached `complete`. Historical worklogs predate this format — pass only current
# entries.

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

    require '\*\*Graph:\*\* `[^`<>]+`' 'graph id in the header'
    require '^## Shipped$' 'Shipped section'
    require '^## Caveats$' 'Caveats section'
    require '^## Cog$' 'Cog section'
    require '^- Status: `complete`$' 'complete final status'
    require '^frontier [0-9]+: omega \[done\]' 'final render with omega done'
}

for worklog in "$@"; do
    check_worklog "$worklog"
done

if [ "$failed" -ne 0 ]; then
    exit 1
fi

echo "Cog worklog evidence verified: $*"
