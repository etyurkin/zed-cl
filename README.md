# Common Lisp Extension for Zed

Common Lisp language support for the Zed editor with integrated LSP server and Jupyter kernel support.

## Features

- **LSP Features**: Syntax highlighting, autocomplete, hover documentation, goto-definition
- **Smart Completion**: Type-aware parameter snippets, package-qualified completions
- **Multi-Package Support**: Package labels, user symbols prioritized
- **Interactive REPL**: Built-in REPL with `Ctrl+Shift+Enter`, shared state across files
- **Rich Output**: Display markdown, tables, images, and JSON inline
- **Jupyter Compatible**: Optional Jupyter Lab/Notebook support
- **Cross-Platform**: Works on macOS and Linux (not tested)

## Prerequisites

**Required for all features:**

1. **Common Lisp Implementation** (one of):
   - **SBCL** (recommended - full feature support):
     - macOS: `brew install sbcl`
     - Linux: `apt install sbcl` / `dnf install sbcl` / `pacman -S sbcl`

   - **ECL** (alternative - ~90% feature parity):
     - macOS: `brew install ecl`
     - Linux: `apt install ecl` / `dnf install ecl` / `pacman -S ecl`

   Configure your preferred implementation via `~/.zed-cl/config.json` (see Configuration section below).

**Note:** Quicklisp and required dependencies will be installed automatically during the build process (`make build` or `make setup-quicklisp`).

## Development Installation

```bash
git clone https://github.com/etyurkin/zed-cl
cd zed-cl
make build
```

In Zed:
1. Open command palette (`Cmd+Shift+P`)
2. Run "zed: install dev extension"
3. Select the cloned directory

## Configuration

All configuration is stored in `~/.zed-cl/config.json` using profiles:

```json
{
  "active_profile": "sbcl",
  "profiles": {
    "sbcl": {
      "lisp_impl": "sbcl",
      "system_index": "system-index.db",
      "completion_package_whitelist": [
        "CORE-KEYWORDS",
        "COMMON-LISP",
        "COMMON-LISP-USER"
      ]
    }
  }
}
```

### Profile Settings

Each profile can configure:
- `lisp_impl` - Common Lisp implementation (`"sbcl"` or `"ecl"`)
- `system_index` - System index database filename (in `~/.zed-cl/`)
- `completion_package_whitelist` - Packages to show in completions

### Multiple Profiles

Create different profiles for different workflows:

```json
{
  "active_profile": "sbcl-full",
  "profiles": {
    "sbcl-full": {
      "lisp_impl": "sbcl",
      "system_index": "sbcl-complete.db",
      "completion_package_whitelist": ["CORE-KEYWORDS", "COMMON-LISP", "COMMON-LISP-USER"]
    },
    "ecl-dev": {
      "lisp_impl": "ecl",
      "system_index": "ecl-packages.db",
      "completion_package_whitelist": ["COMMON-LISP", "COMMON-LISP-USER"]
    },
    "minimal": {
      "lisp_impl": "sbcl",
      "system_index": "system-index.db",
      "completion_package_whitelist": ["COMMON-LISP"]
    }
  }
}
```

Switch profiles by changing `active_profile` and restarting Zed.

### Completion Package Whitelist

Control which packages appear in completions. By default (when not set), shows all user-defined packages plus COMMON-LISP and KEYWORD.

Special values:
- `"CORE-KEYWORDS"` - Only core keywords (excludes system keywords)
- `"ALL-KEYWORDS"` - All keywords including system ones

## Using the Extension

### LSP Features

Open any `.lisp` file and get:
- Autocomplete for Common Lisp built-ins and your code
- Hover documentation
- Goto-definition
- Package-aware completions

### Interactive REPL

**Inline evaluation:**
1. Open a `.lisp` file
2. Select code or position cursor in a form
3. Press `Ctrl+Shift+Enter` to evaluate
4. See results inline

**Terminal REPL (for development):**
1. Open command palette (`Cmd+Shift+P`)
2. Type "Tasks: Spawn"
3. Select "Common Lisp REPL"
4. Get an interactive REPL in a terminal tab

All evaluations share a single REPL environment - definitions are automatically available in autocomplete.

**Direct Terminal Connection (Advanced):**

For debugging or advanced use, connect directly to the master REPL socket:

```bash
./scripts/connect-repl.sh
```

This connects via Unix domain socket to the shared REPL using the configured Lisp implementation.

## Building a System Index (Optional)

Goto-definition works out-of-the-box for your workspace code. To enable goto-definition for external libraries (Quicklisp packages, SBCL built-ins, etc.), build a system index.

### Quick Start

The extension bundles an indexer tool at `~/.zed/extensions/installed/zed-cl/<version>/bin/zed-cl-index`.

**For SBCL users - index SBCL sources:**
```bash
# Using Makefile (indexes SBCL built-ins)
make build-system-index

# Or manually
~/.zed/extensions/installed/zed-cl/<version>/bin/zed-cl-index \
  --source-dir /path/to/sbcl/src/code \
  --output ~/.zed-cl/system-index.db \
  --default-package COMMON-LISP
```

**For all users - index Quicklisp packages:**
```bash
# Example: Index Alexandria
zed-cl-index \
  --source-dir ~/quicklisp/dists/quicklisp/software/alexandria-<version> \
  --output ~/.zed-cl/system-index.db \
  --default-package ALEXANDRIA
```

### Finding SBCL Source

**macOS (Homebrew):**
```bash
$(brew --prefix sbcl)/share/sbcl/src
# Usually: /opt/homebrew/share/sbcl/src or /usr/local/share/sbcl/src
```

**Linux:**
```bash
/usr/share/sbcl/src           # Debian/Ubuntu
/usr/share/sbcl-source/src    # Some distributions
```

### Indexer Commands

**Build an index:**
```bash
zed-cl-index \
  --source-dir <PATH>           # Directory containing .lisp files (searches recursively)
  --output <DB_FILE>            # Output database file (appends if exists)
  --default-package <PACKAGE>   # Default package for symbols without (in-package ...)
```

**Query an index:**
```bash
zed-cl-index \
  --query \
  --database <DB_FILE>          # Database to query
  --symbol <SYMBOL>             # Symbol name (e.g., MAPCAR, FORMAT) [required]
  --package <PACKAGE>           # Package name (e.g., SB-IMPL) [optional - searches all packages if omitted]
```

### Examples

**Index SBCL standard library:**
```bash
# Core runtime (list functions, sequences, etc.)
zed-cl-index \
  --source-dir /opt/homebrew/share/sbcl/src/code \
  --output ~/.zed-cl/system-index.db \
  --default-package COMMON-LISP

# CLOS/MOP (classes, methods, generic functions)
zed-cl-index \
  --source-dir /opt/homebrew/share/sbcl/src/pcl \
  --output ~/.zed-cl/system-index.db \
  --default-package COMMON-LISP

# Interpreter
zed-cl-index \
  --source-dir /opt/homebrew/share/sbcl/src/interpreter \
  --output ~/.zed-cl/system-index.db \
  --default-package COMMON-LISP
```

**Index Quicklisp libraries:**
```bash
# Alexandria
zed-cl-index \
  --source-dir ~/quicklisp/dists/quicklisp/software/alexandria-20241012-git \
  --output ~/.zed-cl/system-index.db \
  --default-package ALEXANDRIA

# Iterate
zed-cl-index \
  --source-dir ~/quicklisp/dists/quicklisp/software/iterate-1.5.3 \
  --output ~/.zed-cl/system-index.db \
  --default-package ITERATE
```

**Query the index:**
```bash
# Find MAPCAR in a specific package
zed-cl-index --query \
  --database ~/.zed-cl/system-index.db \
  --package SB-IMPL \
  --symbol MAPCAR

# Output:
# Looking up: SB-IMPL::MAPCAR
#
# Found 1 definition(s):
#
#   [1] function in SB-IMPL
#       File: /opt/homebrew/share/sbcl/src/code/list.lisp
#       Position: line 1388, char 19

# Find MAPCAR in all packages (omit --package)
zed-cl-index --query \
  --database ~/.zed-cl/system-index.db \
  --symbol MAPCAR

# Output:
# Looking up: MAPCAR (in all packages)
#
# Found 1 definition(s) across packages:
#
#   [1] function in SB-IMPL
#       File: /opt/homebrew/share/sbcl/src/code/list.lisp
#       Position: line 1388, char 19
```

### Multiple System Indexes (Advanced)

Create different indexes for different projects:

```bash
# Minimal: Just SBCL core
zed-cl-index \
  --source-dir /path/to/sbcl/src/code \
  --output ~/.zed-cl/system-sbcl-only.db \
  --default-package COMMON-LISP

# Full: SBCL + all your Quicklisp libraries
zed-cl-index \
  --source-dir /path/to/sbcl/src/code \
  --output ~/.zed-cl/system-full.db \
  --default-package COMMON-LISP
# ... add more libraries
```

**Switch between indexes** by changing the `system_index` field in your active profile in `~/.zed-cl/config.json` and restarting the extension.

### How It Works

- **User code**: Automatically indexed when you open/save `.lisp` files → `~/.zed-cl/user-index.db`
- **System libraries**: Manually indexed using `zed-cl-index` → `~/.zed-cl/system-index.db` (or custom name)
- **Goto-definition search order**:
  1. User index (your workspace code)
  2. System index (SBCL + libraries you indexed)

## Development Commands

```bash
# Build
make build          # Build Rust binaries and extension
make dev            # Development mode
make check          # Type check only
make test           # Run tests

# Jupyter
make install-jupyter  # Register kernel for Jupyter
make verify          # Verify SBCL installation

# Maintenance
make clean          # Clean build artifacts
make help           # Show all commands
```

### Process Management

**List running processes:**
```bash
# Count each process type
ps aux | grep -c '[z]ed-cl-kernel'      # Count kernels
ps aux | grep -c '[z]ed-cl-lsp'        # Count LSP servers
ps aux | grep -c 'master-repl'         # Count master REPLs

# Show detailed process list
ps aux | grep -E 'zed-cl-kernel|zed-cl-lsp|master-repl' | grep -v grep
```

**Kill processes:**
```bash
# Kill all zed-cl processes
pkill -f 'zed-cl-kernel' && pkill -f 'zed-cl-lsp' && pkill -f 'master-repl'

# Kill individual components
pkill -f 'zed-cl-kernel'        # Kill only kernels
pkill -f 'zed-cl-lsp'          # Kill only LSP
pkill -f 'master-repl'         # Kill only master REPL

# Force kill if needed
pkill -9 -f 'zed-cl-kernel' && pkill -9 -f 'zed-cl-lsp' && pkill -9 -f 'master-repl'
```

## Log Locations

**LSP Server:**
- `/tmp/cl-zed-lsp.log` - LSP debug logs
- `/tmp/master-repl.log` - Master REPL logs

**Zed Application:**
- `~/Library/Logs/Zed/Zed.log` (macOS)
- `~/.local/share/zed/logs/Zed.log` (Linux)

**Database Indexes:**
- `~/.zed-cl/system-index.db` - System libraries (SBCL + manually indexed packages)
- `~/.zed-cl/user-index.db` - User workspace code (auto-indexed)

## Architecture

```
      ┌───────────────────────┐
      │  Master REPL Process  │
      │ (Unix domain sockets) │
      └──────────┬────────────┘
                 │
       ┌─────────┼──────────┐
       │         │          │
   ┌───▼───┐  ┌──▼───┐  ┌───▼───┐
   │Console│  │ Zed  │  │ Zed   │
   │ REPL  │  │ REPL │  │ LSP   │
   └───────┘  └──────┘  └───────┘
```

All components connect to a single master REPL via Unix domain sockets for true state sharing. Code evaluated in any component is immediately available in all others.

## License

MIT

## Links

- [Zed Editor](https://zed.dev/)
- [Common Lisp HyperSpec](http://www.lispworks.com/documentation/HyperSpec/Front/index.htm)
- [SBCL](http://www.sbcl.org/)
