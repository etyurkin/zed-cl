#!/usr/bin/env bash
# install-windows-msys2.sh - one-shot Windows installer for the zed-cl
# Common Lisp extension, run from an MSYS2 shell.
#
#   curl -fsSL https://raw.githubusercontent.com/etyurkin/zed-cl/master/scripts/install-windows-msys2.sh | bash
#
# What it does:
#   1. Installs MSYS2 packages it needs (git, curl, unzip)
#   2. Updates the Visual C++ runtime (outdated MSVCP140.dll makes the
#      wasi-sdk clang crash, which Zed reports as "Failed to resolve clang path")
#   3. Installs SBCL if missing (official MSI, UAC prompt)
#   4. Installs wasi-sdk 25 and sets WASI_SDK_PATH (Zed needs its clang to
#      compile the tree-sitter grammar when installing a dev extension)
#   5. Clones the zed-cl repo for the example files
#   6. Downloads the prebuilt extension package and native tools from the
#      latest GitHub release (nothing is compiled on this machine)
#   7. Prints the two manual steps left: install the dev extension in Zed
#      and open an example file.

set -euo pipefail

REPO="etyurkin/zed-cl"
SBCL_VERSION="2.6.7"
WASI_SDK_VERSION="25"
UP="$(cygpath -u "$USERPROFILE")"          # /c/Users/<you>
EXT_DIR="$UP/zed-cl-extension"             # the folder Zed installs from
CLONE_DIR="$UP/zed-cl"                     # repo clone, for the examples
WASI_DIR="$UP/wasi-sdk"
SBCL_PF="/c/Program Files/Steel Bank Common Lisp"

say()  { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
note() { printf '    %s\n' "$*"; }
die()  { printf '\n\033[1;31mERROR: %s\033[0m\n' "$*" >&2; exit 1; }

[ "$(uname -o 2>/dev/null)" = "Msys" ] || die "Run this from an MSYS2 shell."
[ -n "${USERPROFILE:-}" ] || die "USERPROFILE is not set."

# Download a named asset from the latest release. GitHub's browser download
# endpoint occasionally 504s; the API asset endpoint is the fallback.
RELEASE_JSON=""
fetch_asset() {
  local name="$1" out="$2" url id
  if [ -z "$RELEASE_JSON" ]; then
    RELEASE_JSON="$(curl -fsSL --retry 3 "https://api.github.com/repos/$REPO/releases/latest")" \
      || die "Could not query the latest $REPO release."
  fi
  url="$(printf '%s\n' "$RELEASE_JSON" | grep -o "\"browser_download_url\": *\"[^\"]*/$name\"" | grep -o 'https[^"]*' | head -1)"
  [ -n "$url" ] || die "Release asset $name not found."
  if ! curl -fsSL --retry 3 -o "$out" "$url"; then
    note "Direct download failed, retrying via the API asset endpoint..."
    id="$(printf '%s\n' "$RELEASE_JSON" | grep -B3 "\"name\": \"$name\"" | grep -m1 '"id":' | tr -dc '0-9')"
    curl -fsSL --retry 3 -H "Accept: application/octet-stream" \
      -o "$out" "https://api.github.com/repos/$REPO/releases/assets/$id" \
      || die "Could not download $name."
  fi
}

# Run an installer elevated and wait for it (UAC prompt appears on screen).
run_elevated() {
  local exe="$1"; shift
  powershell.exe -NoProfile -Command \
    "Start-Process -FilePath '$(cygpath -w "$exe")' -ArgumentList '$*' -Verb RunAs -Wait" \
    </dev/null
}

say "1/7 MSYS2 packages (git, curl, unzip)"
pacman -S --needed --noconfirm git curl unzip >/dev/null
note "ok"

say "2/7 Visual C++ runtime"
VC_VER="$(powershell.exe -NoProfile -Command \
  "(Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\x64' -ErrorAction SilentlyContinue).Version" \
  </dev/null 2>/dev/null | tr -d '\r' || true)"
VC_MINOR="${VC_VER#v14.}"; VC_MINOR="${VC_MINOR%%.*}"
if [ -n "$VC_VER" ] && [ "${VC_MINOR:-0}" -ge 38 ] 2>/dev/null; then
  note "already current ($VC_VER)"
else
  note "installing latest redistributable (approve the UAC prompt)..."
  curl -fsSL --retry 3 -o /tmp/vc_redist.x64.exe https://aka.ms/vs/17/release/vc_redist.x64.exe
  run_elevated /tmp/vc_redist.x64.exe /install /quiet /norestart
  note "ok"
fi

say "3/7 SBCL"
if command -v sbcl >/dev/null 2>&1 || [ -x "$SBCL_PF/sbcl.exe" ]; then
  note "already installed"
else
  note "downloading SBCL $SBCL_VERSION (approve the UAC prompt)..."
  curl -fsSL --retry 3 -o /tmp/sbcl.msi \
    "https://downloads.sourceforge.net/project/sbcl/sbcl/$SBCL_VERSION/sbcl-$SBCL_VERSION-x86-64-windows-binary.msi"
  powershell.exe -NoProfile -Command \
    "Start-Process msiexec -ArgumentList '/i','$(cygpath -w /tmp/sbcl.msi)','/qn' -Verb RunAs -Wait" </dev/null
  [ -x "$SBCL_PF/sbcl.exe" ] || die "SBCL install did not complete."
  note "ok"
fi
# Put SBCL on the *user* PATH (never 'setx PATH %PATH%;...' - that copies the
# machine PATH into the user PATH and truncates at 1024 chars).
powershell.exe -NoProfile -Command "
  \$dir = 'C:\\Program Files\\Steel Bank Common Lisp'
  \$p = [Environment]::GetEnvironmentVariable('PATH','User')
  if (\$p -notlike ('*' + \$dir + '*')) {
    [Environment]::SetEnvironmentVariable('PATH', \"\$p;\$dir\", 'User')
  }" </dev/null
note "on user PATH"

say "4/7 wasi-sdk $WASI_SDK_VERSION (Zed uses its clang to compile the grammar)"
if [ -x "$WASI_DIR/bin/clang.exe" ] && "$WASI_DIR/bin/clang.exe" --version >/dev/null 2>&1; then
  note "already installed and working"
else
  note "downloading (~500 MB, be patient)..."
  curl -fsSL --retry 3 -o /tmp/wasi-sdk.tar.gz \
    "https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-$WASI_SDK_VERSION/wasi-sdk-$WASI_SDK_VERSION.0-x86_64-windows.tar.gz"
  mkdir -p "$WASI_DIR"
  tar -xzf /tmp/wasi-sdk.tar.gz -C "$WASI_DIR" --strip-components=1
  "$WASI_DIR/bin/clang.exe" --version >/dev/null 2>&1 \
    || die "wasi-sdk clang crashes. This usually means the VC++ runtime update in step 2 has not taken effect - reboot and rerun this script."
  note "ok"
fi
setx WASI_SDK_PATH "$(cygpath -w "$WASI_DIR")" >/dev/null
# A half-written cache from an earlier failed Zed download shadows ours.
rm -rf "$(cygpath -u "$LOCALAPPDATA")/Zed/extensions/build/wasi-sdk"* 2>/dev/null || true
note "WASI_SDK_PATH set"

say "5/7 zed-cl repository (examples live here)"
if [ -d "$CLONE_DIR/.git" ]; then
  git -C "$CLONE_DIR" pull --ff-only >/dev/null 2>&1 || true
  note "updated existing clone"
else
  git clone --depth 1 "https://github.com/$REPO.git" "$CLONE_DIR" >/dev/null 2>&1
  note "cloned to $CLONE_DIR"
fi

say "6/7 extension package + native tools (latest release, prebuilt)"
fetch_asset "zed-cl-extension.zip" /tmp/zed-cl-extension.zip
rm -rf "$EXT_DIR" && mkdir -p "$EXT_DIR"
unzip -q /tmp/zed-cl-extension.zip -d "$EXT_DIR"
note "extension package: $EXT_DIR"
fetch_asset "zed-cl-windows-x86_64.zip" /tmp/zed-cl-natives.zip
mkdir -p "$UP/.zed-cl/bin"
unzip -oq /tmp/zed-cl-natives.zip -d "$UP/.zed-cl/bin"
note "native tools: $USERPROFILE\\.zed-cl\\bin (the extension can also fetch these itself)"

say "7/7 Zed"
if [ -x "$(cygpath -u "$LOCALAPPDATA")/Programs/Zed/Zed.exe" ]; then
  note "found"
else
  note "Zed is not installed - get it from https://zed.dev/download, then continue below."
fi

cat <<EOF

============================================================
 zed-cl is staged. Two manual steps remain, inside Zed:
============================================================

 1. Start Zed FRESH from the Start menu or Explorer
    (a Zed that was already running does not see the new
    PATH and WASI_SDK_PATH; quit it completely first).

 2. Ctrl+Shift+P -> "zed: install dev extension"
    -> select this folder:  $USERPROFILE\\zed-cl-extension
    The first install compiles the grammar (needs a minute).

 Then try it:
   - Open  $USERPROFILE\\zed-cl\\examples\\common-lisp-examples.lisp
   - Put the cursor on a form and press Ctrl+Shift+Enter.
     First run downloads nothing extra and starts SBCL as a
     shared REPL; results appear right under each form.
   - More examples in the same folder: my-utils.lisp,
     using-shared-repl.lisp, rich-output-examples.lisp.

 If something misbehaves:
   - "Failed to resolve clang path"  -> reboot (VC++ runtime),
     rerun this script, restart Zed.
   - eval does nothing               -> Ctrl+Shift+P ->
     "repl: refresh kernelspecs", reopen the .lisp file.
   - logs: %LOCALAPPDATA%\\Zed\\logs\\Zed.log
           %USERPROFILE%\\.zed-cl\\logs\\
============================================================
EOF
