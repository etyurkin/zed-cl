.PHONY: all build build-lsp build-kernel build-repl-tui build-extension clean test install-dev help verify-lisp verify setup-grammar setup-quicklisp build-system-index build-system-index-if-missing build-indexer bundle register-kernel check

# Rust binary names
LSP_BIN := zed-cl-lsp
KERNEL_BIN := zed-cl-kernel
REPL_TUI_BIN := zed-cl-repl

# Grammar settings
GRAMMAR_REPO := https://github.com/tree-sitter-grammars/tree-sitter-commonlisp
GRAMMAR_DIR := grammars/commonlisp

# Index settings
SBCL_SRC_DIR := $(shell brew --prefix sbcl 2>/dev/null || echo "/usr/local")/share/sbcl/src
INDEX_DIR := $(HOME)/.zed-cl
SYSTEM_INDEX := $(INDEX_DIR)/system-index.db
INDEXER_BIN := src/zed-cl-index/target/release/zed-cl-index

# Build directories
LSP_DIR := src/zed-cl-lsp
KERNEL_DIR := src/zed-cl-kernel
REPL_TUI_DIR := src/zed-cl-repl
LSP_BUILD_DIR := $(LSP_DIR)/target/release
KERNEL_BUILD_DIR := $(KERNEL_DIR)/target/release
REPL_TUI_BUILD_DIR := $(REPL_TUI_DIR)/target/release

# Extension binary directory
EXTENSION_BIN_DIR := bin

# Colors
GREEN := \033[0;32m
BLUE := \033[0;34m
YELLOW := \033[0;33m
NC := \033[0m

all: setup-grammar setup-quicklisp build

# Install Quicklisp and dependencies
setup-quicklisp:
	@echo "$(BLUE)Checking Quicklisp installation...$(NC)"
	@if ! which sbcl > /dev/null 2>&1; then \
		echo "$(YELLOW)⚠ SBCL not found, skipping Quicklisp setup$(NC)"; \
		echo "$(YELLOW)  Install SBCL with: brew install sbcl (macOS) or apt install sbcl (Linux)$(NC)"; \
		exit 0; \
	fi
	@if sbcl --noinform --non-interactive --eval '(if (find-package :quicklisp) (sb-ext:exit :code 0) (sb-ext:exit :code 1))' 2>/dev/null; then \
		echo "$(GREEN)✓ Quicklisp already installed$(NC)"; \
	else \
		echo "$(BLUE)Installing Quicklisp...$(NC)"; \
		curl -O https://beta.quicklisp.org/quicklisp.lisp && \
		sbcl --noinform --non-interactive \
			--load quicklisp.lisp \
			--eval '(quicklisp-quickstart:install)' \
			--eval '(ql:add-to-init-file)' && \
		rm quicklisp.lisp && \
		echo "$(GREEN)✓ Quicklisp installed and added to SBCL init$(NC)"; \
	fi
	@echo "$(BLUE)Loading required Lisp dependencies...$(NC)"
	@sbcl --noinform --non-interactive \
		--eval '(let ((quicklisp-init (merge-pathnames "quicklisp/setup.lisp" (user-homedir-pathname)))) \
		          (when (probe-file quicklisp-init) (load quicklisp-init)))' \
		--eval '(ql:quickload :cl-json :silent t)' \
		--eval '(sb-ext:exit :code 0)' 2>&1 | grep -v "^;" || true
	@echo "$(GREEN)✓ Dependencies loaded (cl-json)$(NC)"

# Fetch tree-sitter grammar for local development
setup-grammar:
	@if [ ! -d "$(GRAMMAR_DIR)" ]; then \
		echo "$(BLUE)Fetching tree-sitter-commonlisp grammar...$(NC)"; \
		mkdir -p grammars && \
		git clone --depth 1 $(GRAMMAR_REPO) $(GRAMMAR_DIR) && \
		echo "$(GREEN)✓ Grammar fetched to $(GRAMMAR_DIR)$(NC)"; \
	else \
		echo "$(GREEN)✓ Grammar already present at $(GRAMMAR_DIR)$(NC)"; \
	fi

# Build system symbol index (conditional - only if missing)
build-system-index-if-missing: build-indexer
	@if [ -f "$(SYSTEM_INDEX)" ]; then \
		echo "$(GREEN)✓ System index already exists: $(SYSTEM_INDEX)$(NC)"; \
	else \
		echo "$(BLUE)Building system symbol index (code, pcl, interpreter only)...$(NC)"; \
		if [ ! -d "$(SBCL_SRC_DIR)" ]; then \
			echo "$(YELLOW)⚠ SBCL source not found at $(SBCL_SRC_DIR)$(NC)"; \
			echo "$(YELLOW)  Install SBCL with: brew install sbcl (macOS)$(NC)"; \
			echo "$(YELLOW)  Continuing without system index (goto-definition will work for workspace code only)$(NC)"; \
		else \
			mkdir -p $(INDEX_DIR); \
			$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/code --output $(SYSTEM_INDEX) --default-package COMMON-LISP; \
			$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/pcl --output $(SYSTEM_INDEX) --default-package COMMON-LISP; \
			$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/interpreter --output $(SYSTEM_INDEX) --default-package COMMON-LISP; \
			echo "$(GREEN)✓ System index built: $(SYSTEM_INDEX)$(NC)"; \
			du -h $(SYSTEM_INDEX) | awk '{print "  Size: " $$1}'; \
		fi \
	fi

# Build system symbol index (force rebuild - deletes existing index first)
build-system-index: build-indexer
	@echo "$(BLUE)Building system symbol index (code, pcl, interpreter only)...$(NC)"
	@if [ ! -d "$(SBCL_SRC_DIR)" ]; then \
		echo "$(YELLOW)⚠ SBCL source not found at $(SBCL_SRC_DIR)$(NC)"; \
		echo "$(YELLOW)  Install SBCL with: brew install sbcl (macOS)$(NC)"; \
		exit 1; \
	fi
	@if [ -f "$(SYSTEM_INDEX)" ]; then \
		echo "  Removing old index..."; \
		rm -f $(SYSTEM_INDEX); \
	fi
	@mkdir -p $(INDEX_DIR)
	@echo "  Indexing src/code/ (runtime functions)..."
	@$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/code --output $(SYSTEM_INDEX) --default-package COMMON-LISP
	@echo "  Indexing src/pcl/ (CLOS)..."
	@$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/pcl --output $(SYSTEM_INDEX) --default-package COMMON-LISP
	@echo "  Indexing src/interpreter/..."
	@$(INDEXER_BIN) --source-dir $(SBCL_SRC_DIR)/interpreter --output $(SYSTEM_INDEX) --default-package COMMON-LISP
	@echo "$(GREEN)✓ System index built: $(SYSTEM_INDEX)$(NC)"
	@du -h $(SYSTEM_INDEX) | awk '{print "  Size: " $$1}'
	@echo "$(YELLOW)Note: Close Zed before rebuilding index to avoid lock conflicts$(NC)"

# Build the indexer tool
build-indexer: setup-grammar
	@echo "$(BLUE)Building indexer tool...$(NC)"
	@cargo build --release --manifest-path=src/zed-cl-index/Cargo.toml
	@echo "$(GREEN)✓ Indexer built$(NC)"

help:
	@echo "Common Lisp Zed Extension - Build Commands"
	@echo ""
	@echo "Main targets:"
	@echo "  make build             - Build LSP, kernel, REPL, and extension (release)"
	@echo "  make install-dev       - Show instructions for dev installation in Zed"
	@echo "  make clean             - Clean build artifacts"
	@echo "  make verify            - Verify prerequisites (SBCL, etc.)"
	@echo ""
	@echo "Individual components:"
	@echo "  make build-lsp         - Build only LSP server"
	@echo "  make build-kernel      - Build only Jupyter kernel"
	@echo "  make build-repl-tui    - Build only TUI REPL client"
	@echo "  make build-indexer     - Build only symbol indexer"
	@echo "  make build-extension   - Build only Zed extension WASM"
	@echo ""
	@echo "System index (optional):"
	@echo "  make build-system-index - Build SBCL system symbol index"
	@echo ""
	@echo "Setup:"
	@echo "  make setup-quicklisp   - Install Quicklisp and dependencies"
	@echo ""
	@echo "Other:"
	@echo "  make test              - Run tests"
	@echo "  make check             - Check code without building"
	@echo "  make verify-lisp       - Verify REPL Lisp files compile"
	@echo ""

build: setup-grammar setup-quicklisp verify-lisp build-system-index-if-missing build-lsp build-kernel build-repl-tui bundle build-extension register-kernel
	@echo "$(GREEN)✓ Build complete!$(NC)"
	@echo ""
	@echo "Binaries:"
	@echo "  LSP:       $(LSP_BUILD_DIR)/$(LSP_BIN)"
	@echo "  Kernel:    $(KERNEL_BUILD_DIR)/$(KERNEL_BIN)"
	@echo "  TUI REPL:  $(REPL_TUI_BUILD_DIR)/$(REPL_TUI_BIN)"
	@echo "  Extension: extension.wasm"
	@echo ""
	@echo "Extension bundles:"
	@echo "  LSP:       $(EXTENSION_BIN_DIR)/$(LSP_BIN)"
	@echo "  Kernel:    $(EXTENSION_BIN_DIR)/$(KERNEL_BIN)"
	@echo "  TUI REPL:  $(EXTENSION_BIN_DIR)/$(REPL_TUI_BIN)"
	@echo ""
	@du -h $(LSP_BUILD_DIR)/$(LSP_BIN) $(KERNEL_BUILD_DIR)/$(KERNEL_BIN) $(REPL_TUI_BUILD_DIR)/$(REPL_TUI_BIN) extension.wasm | awk '{print "  " $$2 ": " $$1}'

build-lsp:
	@echo "$(BLUE)Building LSP server...$(NC)"
	@cargo build --release --manifest-path=$(LSP_DIR)/Cargo.toml
	@echo "$(GREEN)✓ LSP server built$(NC)"

build-kernel:
	@echo "$(BLUE)Building Jupyter kernel...$(NC)"
	@cargo build --release --manifest-path=$(KERNEL_DIR)/Cargo.toml
	@echo "$(GREEN)✓ Jupyter kernel built$(NC)"

build-repl-tui:
	@echo "$(BLUE)Building TUI REPL client...$(NC)"
	@cargo build --release --manifest-path=$(REPL_TUI_DIR)/Cargo.toml
	@echo "$(GREEN)✓ TUI REPL client built$(NC)"

build-extension:
	@echo "$(BLUE)Building Zed extension WASM...$(NC)"
	@rustup target add wasm32-wasip1 2>/dev/null || true
	@cargo build --release --target wasm32-wasip1
	@cp target/wasm32-wasip1/release/zed_commonlisp.wasm extension.wasm
	@echo "$(GREEN)✓ Extension WASM built$(NC)"

check: setup-quicklisp verify-lisp
	@echo "$(BLUE)Checking LSP server...$(NC)"
	@cargo check --manifest-path=$(LSP_DIR)/Cargo.toml
	@echo "$(BLUE)Checking Jupyter kernel...$(NC)"
	@cargo check --manifest-path=$(KERNEL_DIR)/Cargo.toml
	@echo "$(GREEN)✓ All checks passed$(NC)"

verify-lisp:
	@echo "$(BLUE)Verifying REPL Lisp files...$(NC)"
	@if ! which sbcl > /dev/null 2>&1; then \
		echo "$(YELLOW)⚠ SBCL not found, skipping Lisp verification$(NC)"; \
		exit 0; \
	fi
	@sbcl --noinform --non-interactive \
		--eval '(handler-case \
		          (progn \
		            (let ((quicklisp-init (merge-pathnames "quicklisp/setup.lisp" (user-homedir-pathname)))) \
		              (when (probe-file quicklisp-init) (load quicklisp-init))) \
		            (require :asdf) \
		            (push (truename "src/zed-cl-repl-impl/") asdf:*central-registry*) \
		            (asdf:load-system :zed-cl/master-repl) \
		            (funcall (intern "EVAL-FORMS-FROM-CODE" :zed-cl.master-repl) "(+ 1 2)" nil nil nil) \
		            (sb-ext:exit :code 0)) \
		        (error (e) \
		          (format *error-output* "✗ REPL Lisp verification failed: ~A~%" e) \
		          (sb-ext:exit :code 1)))' 2>&1 | { grep -v "^;" || true; }; \
		if [ $${PIPESTATUS[0]} -eq 0 ]; then \
			echo "$(GREEN)✓ REPL Lisp files compiled successfully$(NC)"; \
		else \
			exit 1; \
		fi

test:
	@echo "$(BLUE)Running tests...$(NC)"
	@cargo test --manifest-path=$(LSP_DIR)/Cargo.toml
	@cargo test --manifest-path=$(KERNEL_DIR)/Cargo.toml
	@echo "$(GREEN)✓ Tests passed$(NC)"

# Bundle binaries for Zed extension
bundle: build-lsp build-kernel build-repl-tui build-indexer
	@echo "$(BLUE)Bundling binaries for extension...$(NC)"
	@mkdir -p $(EXTENSION_BIN_DIR)
	@cp $(LSP_BUILD_DIR)/$(LSP_BIN) $(EXTENSION_BIN_DIR)/
	@cp $(KERNEL_BUILD_DIR)/$(KERNEL_BIN) $(EXTENSION_BIN_DIR)/
	@cp $(REPL_TUI_BUILD_DIR)/$(REPL_TUI_BIN) $(EXTENSION_BIN_DIR)/
	@cp $(INDEXER_BIN) $(EXTENSION_BIN_DIR)/
	@xattr -cr $(EXTENSION_BIN_DIR) 2>/dev/null || true
	@codesign --force --deep --sign - $(EXTENSION_BIN_DIR)/$(LSP_BIN) 2>/dev/null || echo "Warning: codesign failed for LSP binary"
	@codesign --force --deep --sign - $(EXTENSION_BIN_DIR)/$(KERNEL_BIN) 2>/dev/null || echo "Warning: codesign failed for kernel binary"
	@codesign --force --deep --sign - $(EXTENSION_BIN_DIR)/$(REPL_TUI_BIN) 2>/dev/null || echo "Warning: codesign failed for TUI REPL binary"
	@codesign --force --deep --sign - $(EXTENSION_BIN_DIR)/zed-cl-index 2>/dev/null || echo "Warning: codesign failed for indexer binary"
	@echo "$(GREEN)✓ Binaries bundled in $(EXTENSION_BIN_DIR)/$(NC)"

clean:
	@echo "$(YELLOW)Cleaning build artifacts...$(NC)"
	@cargo clean --manifest-path=$(LSP_DIR)/Cargo.toml
	@cargo clean --manifest-path=$(KERNEL_DIR)/Cargo.toml
	@rm -rf $(EXTENSION_BIN_DIR)
	@rm -rf target/
	@rm -rf grammars/
	@rm -f extension.wasm
	@# Unregister kernel
	@if [ -d ~/.local/share/jupyter/kernels/commonlisp-zed ]; then \
		rm -rf ~/.local/share/jupyter/kernels/commonlisp-zed; \
		echo "$(YELLOW)  ✓ Kernel unregistered from ~/.local/share/jupyter/kernels/$(NC)"; \
	elif [ -d ~/Library/Jupyter/kernels/commonlisp-zed ]; then \
		rm -rf ~/Library/Jupyter/kernels/commonlisp-zed; \
		echo "$(YELLOW)  ✓ Kernel unregistered from ~/Library/Jupyter/kernels/$(NC)"; \
	fi
	@echo "$(GREEN)✓ Clean complete$(NC)"

# Install extension for local development
install-dev: build
	@echo "$(GREEN)✓ Extension built with embedded binaries!$(NC)"
	@echo ""
	@echo "To install: In Zed, run: Cmd+Shift+P → 'zed: install dev extension' → select this directory"
	@echo ""
	@echo "The extension.wasm (9.2M) contains:"
	@echo "  - LSP binary (6.3M) - Autocomplete, hover, symbol info"
	@echo "  - Kernel binary (2.8M) - Code execution (works in Zed + Jupyter)"
	@echo "  - WASM code (~140K)"
	@echo ""
	@echo "No external Jupyter installation needed for Zed REPL!"
	@echo "Just open a .lisp file and press Ctrl+Shift+Enter to run code."

# Register kernel for Zed (required for REPL to work)
register-kernel: bundle
	@echo "$(BLUE)Registering kernel for Zed...$(NC)"
	@# Determine Jupyter directory (macOS vs Linux)
	@if [ "$$(uname)" = "Darwin" ]; then \
		JUPYTER_DIR=~/Library/Jupyter/kernels/commonlisp-zed; \
	else \
		JUPYTER_DIR=~/.local/share/jupyter/kernels/commonlisp-zed; \
	fi; \
	mkdir -p "$$JUPYTER_DIR"; \
	KERNEL_PATH=$$(cd $(EXTENSION_BIN_DIR) && pwd)/$(KERNEL_BIN); \
	sed "s|KERNEL_PATH_PLACEHOLDER|$$KERNEL_PATH|g" kernels/commonlisp/kernel.json > "$$JUPYTER_DIR/kernel.json"; \
	echo "$(GREEN)✓ Kernel registered at $$JUPYTER_DIR for Zed REPL$(NC)"

# Verify prerequisites
verify:
	@echo "$(BLUE)Verifying installation...$(NC)"
	@echo ""
	@echo "Checking SBCL..."
	@which sbcl > /dev/null && echo "$(GREEN)  ✓ SBCL found: $$(sbcl --version 2>&1 | head -1)$(NC)" || (echo "$(YELLOW)  ✗ SBCL not found$(NC)" && echo "    Install: brew install sbcl (macOS) or apt install sbcl (Linux)")
	@echo ""
	@echo "Checking Jupyter (optional for standalone use)..."
	@which jupyter > /dev/null && echo "$(GREEN)  ✓ Jupyter found: $$(jupyter --version 2>&1 | head -1)$(NC)" || echo "$(YELLOW)  ℹ Jupyter not installed (optional)$(NC)"
	@echo ""
	@echo "Extension status:"
	@if [ -f extension.wasm ]; then \
		echo "$(GREEN)  ✓ extension.wasm built ($$(du -h extension.wasm | awk '{print $$1}'))$(NC)"; \
	else \
		echo "$(YELLOW)  ✗ extension.wasm not found (run 'make build')$(NC)"; \
	fi
