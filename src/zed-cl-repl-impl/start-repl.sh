#!/usr/bin/env bash
# start-repl.sh - Multi-implementation launcher for master REPL
#
# Detects available Common Lisp implementation and starts master REPL
# Default priority: SBCL > ECL
#
# Environment Variables:
#   ZED_CL_LISP_IMPL - Force specific implementation (sbcl, ecl)
#   Example: ZED_CL_LISP_IMPL=ecl zed

set -e

# Get script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Detect Lisp implementation
detect_lisp() {
    # Check if user specified an implementation
    if [ -n "$ZED_CL_LISP_IMPL" ]; then
        if command -v "$ZED_CL_LISP_IMPL" &> /dev/null; then
            echo "$ZED_CL_LISP_IMPL"
            return
        else
            echo "WARNING: ZED_CL_LISP_IMPL=$ZED_CL_LISP_IMPL but $ZED_CL_LISP_IMPL not found in PATH" >&2
            echo "Falling back to auto-detection" >&2
        fi
    fi

    # Auto-detect with priority: SBCL > ECL
    if command -v sbcl &> /dev/null; then
        echo "sbcl"
    elif command -v ecl &> /dev/null; then
        echo "ecl"
    else
        echo ""
    fi
}

LISP=$(detect_lisp)

if [ -z "$LISP" ]; then
    echo "ERROR: No supported Common Lisp implementation found" >&2
    echo "Please install either SBCL or ECL:" >&2
    echo "  macOS:  brew install sbcl  (or)  brew install ecl" >&2
    echo "  Ubuntu: sudo apt install sbcl  (or)  sudo apt install ecl" >&2
    exit 1
fi

echo "[Launcher] Using $LISP" >&2

# Start master REPL with detected implementation
case "$LISP" in
    sbcl)
        exec sbcl --noinform --no-userinit \
            --load "$SCRIPT_DIR/start-master-repl.lisp"
        ;;
    ecl)
        exec ecl -norc \
            -load "$SCRIPT_DIR/start-master-repl.lisp"
        ;;
    *)
        echo "ERROR: Unsupported implementation: $LISP" >&2
        exit 1
        ;;
esac
