#!/bin/sh
# Test stand-in for gst-inspect-1.0 (src-tauri/src/process/tests.rs).
# Exit 0 only for the exact argv "--exists autoaudiosink"; 1 for another
# element; 2 for any other argv.
if [ "$#" -ne 2 ] || [ "$1" != "--exists" ]; then exit 2; fi
if [ "$2" = "autoaudiosink" ]; then exit 0; fi
exit 1
