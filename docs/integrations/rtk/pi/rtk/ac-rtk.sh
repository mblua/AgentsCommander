#!/bin/bash
# ac-rtk.sh — rtk launcher that strips rtk's own "No hook installed" stderr
# banner at the source, before any caller-side merge (`2>&1`) can leak it into
# stdout. Transparent otherwise: same args, same stdout/stderr, same exit code.
exec rtk "$@" 2> >(grep --line-buffered -v 'No hook installed' >&2)
