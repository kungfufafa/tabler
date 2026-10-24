#!/bin/sh
# dev/scripts/win-env.sh -- the cargo environment for Windows (Git Bash). Source it from the repo
# root before cargo commands:
#
#   . dev/scripts/win-env.sh
#   export CARGO_TARGET_DIR=C:/t/s1; . dev/scripts/win-env.sh     # another build folder
#
# Every variable keeps a value that is already set. Set an override in its own statement, as
# above: bash makes `VAR=x . script` a temporary assignment and drops it, and the default with it,
# when the script returns.
# - OpenSSL: openssl-sys cannot build its vendored copy with Git Bash's perl (it lacks
#   Locale::Maketext::Simple), so it links the prebuilt static one in C:/t/openssl.
# - CARGO_TARGET_DIR: a short path (DuckDB's C++ build hits MSVC's MAX_PATH under the repo's
#   src-tauri/target), and not the folder `tauri dev` builds in, which Windows locks while the
#   app runs.

export OPENSSL_NO_VENDOR="${OPENSSL_NO_VENDOR:-1}"
export OPENSSL_STATIC="${OPENSSL_STATIC:-1}"
export OPENSSL_DIR="${OPENSSL_DIR:-C:/t/openssl}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-C:/t/s0}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-8}"

if [ ! -d "$OPENSSL_DIR/lib" ]; then
  printf '[win-env] warning: no OpenSSL build in %s (set OPENSSL_DIR)\n' "$OPENSSL_DIR" >&2
fi
