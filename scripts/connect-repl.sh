#!/bin/bash
# Connect to the master REPL via Unix domain socket

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# Read Lisp implementation from active profile in config
CONFIG_FILE="$HOME/.zed-cl/config.json"

if [ -f "$CONFIG_FILE" ] && command -v jq >/dev/null 2>&1; then
    # Use jq if available (common on most systems)
    ACTIVE_PROFILE=$(jq -r '.active_profile // "sbcl"' "$CONFIG_FILE" 2>/dev/null)
    LISP_IMPL=$(jq -r ".profiles.\"$ACTIVE_PROFILE\".lisp_impl // \"sbcl\"" "$CONFIG_FILE" 2>/dev/null)
else
    # Fallback to sbcl if no config or jq not available
    LISP_IMPL="sbcl"
fi

# Check if rlwrap is available
if command -v rlwrap >/dev/null 2>&1; then
    RLWRAP="rlwrap -C commonlisp"
else
    RLWRAP=""
fi

# Run the Lisp client with the configured implementation
case "$LISP_IMPL" in
    sbcl)
        exec $RLWRAP sbcl --script "$SCRIPT_DIR/connect-repl.lisp"
        ;;
    ecl)
        exec $RLWRAP ecl -shell "$SCRIPT_DIR/connect-repl.lisp"
        ;;
    *)
        echo "Error: Unknown Lisp implementation: $LISP_IMPL"
        echo "Supported: sbcl, ecl"
        exit 1
        ;;
esac
