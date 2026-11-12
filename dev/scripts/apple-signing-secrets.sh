#!/bin/bash
# Gets the macOS build signed and notarized: puts the Apple secrets that
# .github/workflows/build.yml reads into the GitHub repository, so the app opens without the
# Gatekeeper steps. Run it yourself, in a terminal, and run the same command again after each
# thing it asks you to do:
#
#   bash dev/scripts/apple-signing-secrets.sh
#
# It needs a "Developer ID Application" certificate, which only the Apple team's Account Holder
# can make (paid Apple Developer Program). The first run writes a certificate request to the
# Desktop and opens Apple's page for it: you upload that file and download the certificate. The
# next run finds the download, checks it, and sets the secrets. Then it asks for the Apple ID and
# an app-specific password (account.apple.com ▸ Sign-In and Security ▸ App-Specific Passwords),
# which notarization needs.
#
# The passwords are typed in the terminal and go straight to `gh secret set`; nothing it reads is
# printed or logged. The private key of the request stays in ~/.config/tabler-signing (readable
# by you alone) until the certificate arrives, then moves into your login keychain if you say so.
#
# A certificate you already have, exported from Keychain Access as .p12:
#
#   bash dev/scripts/apple-signing-secrets.sh ~/Desktop/Tabler.p12
#
# The certificate is already in this Mac's login keychain (an earlier run kept it there), and the
# build cannot read the .p12 in the secrets ("MAC verification failed … wrong password?"): export
# it again, in the format the build's macOS reads, and replace the certificate secrets:
#
#   bash dev/scripts/apple-signing-secrets.sh --from-keychain
set -euo pipefail
umask 077

say() { printf '%s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

STATE=${TABLER_SIGNING_HOME:-$HOME/.config/tabler-signing}
DESKTOP=${TABLER_SIGNING_DESKTOP:-$HOME/Desktop}
DOWNLOADS=${TABLER_SIGNING_DOWNLOADS:-$HOME/Downloads}
KEY="$STATE/developer-id.key"
REQUEST_NAME="Tabler.certSigningRequest"
APPLE_PAGE="https://developer.apple.com/account/resources/certificates/add"
CERT_SECRETS="APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_TEAM_ID"
NOTARY_SECRETS="APPLE_ID APPLE_PASSWORD"

command -v gh > /dev/null || fail "the GitHub CLI (gh) is not installed"
REPO=$(gh repo view --json nameWithOwner -q .nameWithOwner 2> /dev/null) ||
  fail "not in a GitHub repository gh can see (run \`gh auth login\`)"

WORK=$(mktemp -d)
KEYCHAIN="$WORK/check.keychain-db"
cleanup() {
  security delete-keychain "$KEYCHAIN" > /dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

# --- the repository's secrets (GitHub never gives their values back) --------------------------

secret_names() { gh secret list --repo "$REPO" --json name -q '.[].name'; }
has_secrets() {
  local have name
  have=$(secret_names)
  for name in "$@"; do
    grep -qx "$name" <<< "$have" || return 1
  done
}
set_secret() { gh secret set "$1" --repo "$REPO" > /dev/null; }
report() {
  local have name
  have=$(secret_names)
  say "Secrets of $REPO:"
  for name in $CERT_SECRETS $NOTARY_SECRETS; do
    if grep -qx "$name" <<< "$have"; then say "  set      $name"; else say "  missing  $name"; fi
  done
}

# --- asking ----------------------------------------------------------------------------------

# A line typed in the terminal (or read from a pipe); `secret` is not shown while typed.
ask() {
  local value=""
  if [ -t 0 ]; then
    if [ "${2:-}" = secret ]; then
      read -r -s -p "$1: " value || true
      printf '\n' >&2
    else
      read -r -p "$1: " value || true
    fi
  else
    read -r value || true
  fi
  printf '%s' "$value"
}

# --- the certificate --------------------------------------------------------------------------

# "Developer ID Application: <name> (<team id>)" of a certificate in PEM, or nothing.
identity_of() {
  openssl x509 -in "$1" -noout -subject -nameopt multiline 2> /dev/null |
    sed -n 's/^ *commonName *= *\(Developer ID Application: .*\)$/\1/p'
}
team_of() { printf '%s' "$1" | sed -n 's/.*(\([A-Z0-9]*\))$/\1/p'; }
public_key_of_request() { openssl rsa -in "$KEY" -pubout 2> /dev/null | openssl sha256; }
public_key_of_cert() { openssl x509 -in "$1" -noout -pubkey 2> /dev/null | openssl sha256; }

# Any certificate file as PEM (Apple's download is DER).
to_pem() {
  openssl x509 -inform DER -in "$1" -out "$2" 2> /dev/null || openssl x509 -in "$1" -out "$2" 2> /dev/null
}

# The certificate Apple issued for the request: `$1` when given, else the newest .cer in
# Downloads made from this request's key. Prints the path of its PEM copy.
find_certificate() {
  local file pem="$WORK/cert.pem" want
  want=$(public_key_of_request)
  if [ -n "${1:-}" ]; then
    to_pem "$1" "$pem" || fail "not a certificate: $1"
    [ "$(public_key_of_cert "$pem")" = "$want" ] ||
      fail "$1 was not made from the request on this Mac (upload $DESKTOP/$REQUEST_NAME)"
    printf '%s' "$pem"
    return
  fi
  # Newest first.
  while IFS= read -r file; do
    [ -n "$file" ] || continue
    to_pem "$file" "$pem" || continue
    if [ "$(public_key_of_cert "$pem")" = "$want" ]; then
      printf '%s' "$pem"
      return
    fi
  done << EOF
$(ls -t "$DOWNLOADS"/*.cer 2> /dev/null || true)
EOF
  return 1
}

# --- the .p12 the build imports -------------------------------------------------------------

# The build's `security import` (an older macOS than this one, maybe) reads only the old PKCS#12
# algorithms: SHA-1 MAC and 3DES. A .p12 made by OpenSSL 3 (AES-256, a SHA-256 MAC) imports fine
# here and fails there with "MAC verification failed (wrong password?)".
is_legacy_p12() {
  local info
  # OpenSSL names the MAC ("MAC: sha1"); LibreSSL only the bag algorithms.
  info=$(P=$2 openssl pkcs12 -in "$1" -info -noout -passin env:P 2>&1) ||
    info=$(P=$2 /usr/bin/openssl pkcs12 -in "$1" -info -noout -passin env:P 2>&1) || return 1
  ! grep -qiE "PBES2|AES" <<< "$info" || return 1
  if grep -qi "^MAC:" <<< "$info"; then
    grep -qi "^MAC: sha1," <<< "$info"
  else
    grep -qi "pbeWithSHA1And3-KeyTripleDES" <<< "$info"
  fi
}

# The certificate and private key of `identity` from a .p12 of any format (several identities
# allowed), as one PEM file `$4` with the key first and the certificate after it.
pem_of_identity() {
  local p12=$1 password=$2 identity=$3 out=$4 all="$WORK/all.pem" n file cn want
  P=$password /usr/bin/openssl pkcs12 -in "$p12" -nodes -passin env:P -out "$all" 2> /dev/null ||
    P=$password openssl pkcs12 -in "$p12" -nodes -passin env:P -out "$all" 2> /dev/null ||
    P=$password openssl pkcs12 -legacy -in "$p12" -nodes -passin env:P -out "$all" 2> /dev/null ||
    fail "could not read the .p12: wrong password?"
  # One file per PEM block.
  awk -v dir="$WORK" '
    /^-----BEGIN / { n++; file = sprintf("%s/block-%03d.pem", dir, n) }
    file { print > file }
    /^-----END / { close(file); file = "" }
  ' "$all"
  rm -f "$all"
  : > "$WORK/id-cert.pem"
  for file in "$WORK"/block-*.pem; do
    grep -q "BEGIN CERTIFICATE" "$file" || continue
    cn=$(identity_of "$file")
    [ "$cn" = "$identity" ] && cp "$file" "$WORK/id-cert.pem" && break
  done
  [ -s "$WORK/id-cert.pem" ] || fail "the export holds no \"$identity\" certificate"
  want=$(public_key_of_cert "$WORK/id-cert.pem")
  for file in "$WORK"/block-*.pem; do
    grep -q "PRIVATE KEY" "$file" || continue
    if [ "$(openssl pkey -in "$file" -pubout 2> /dev/null | openssl sha256)" = "$want" ]; then
      cat "$file" "$WORK/id-cert.pem" > "$out"
      rm -f "$WORK"/block-*.pem
      return
    fi
  done
  rm -f "$WORK"/block-*.pem
  fail "the export holds \"$identity\" without its private key"
}

# A new .p12 at `$2` (password `$3`) of the PEM key + certificate `$1`, with Apple's Developer ID
# intermediates, in the old algorithms (system LibreSSL).
legacy_p12() {
  local chain=()
  security find-certificate -a -c "Developer ID Certification Authority" -p > "$WORK/chain.pem" 2> /dev/null || true
  if [ -s "$WORK/chain.pem" ]; then
    chain=(-certfile "$WORK/chain.pem")
  fi
  P=$3 /usr/bin/openssl pkcs12 -export -in "$1" ${chain[@]+"${chain[@]}"} -out "$2" -passout env:P \
    -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1 ||
    fail "could not make the .p12"
}

# Sets the four certificate secrets from a .p12 and its password, once the same import the build
# does (into a keychain of its own, removed again) shows it holds the certificate and its key.
publish_p12() {
  local p12=$1 password=$2 identity=$3 team held
  is_legacy_p12 "$p12" "$password" ||
    fail "the .p12 uses PKCS#12 algorithms the build's macOS cannot read (nothing was changed)"
  security create-keychain -p check "$KEYCHAIN"
  security import "$p12" -k "$KEYCHAIN" -f pkcs12 -P "$password" -T /usr/bin/codesign > /dev/null 2>&1 ||
    fail "could not read the .p12: wrong password, or not a certificate with its private key"
  held=$(security find-identity "$KEYCHAIN")
  grep -qF "\"$identity\"" <<< "$held" ||
    fail "the .p12 does not hold \"$identity\" with its private key"
  team=$(team_of "$identity")
  [ -n "$team" ] || fail "could not read the team id from \"$identity\""
  /usr/bin/openssl base64 -A -in "$p12" | set_secret APPLE_CERTIFICATE
  printf '%s' "$password" | set_secret APPLE_CERTIFICATE_PASSWORD
  printf '%s' "$identity" | set_secret APPLE_SIGNING_IDENTITY
  printf '%s' "$team" | set_secret APPLE_TEAM_ID
  mkdir -p "$STATE"
  printf '%s\n' "$team" > "$STATE/team-id"
  say "Certificate: $identity"
  say "The build signs with it from the next push to main."
}

# --- notarization -----------------------------------------------------------------------------

notarization() {
  local team apple_id password
  team=$(cat "$STATE/team-id" 2> /dev/null || true)
  [ -n "$team" ] || fail "the team id is not known here: run this script with the certificate first"
  say ""
  say "Last step, notarization (without it Gatekeeper still stops the app the first time):"
  say "  1. Open  account.apple.com  ▸ Sign-In and Security ▸ App-Specific Passwords ▸ +"
  say "  2. Name it \"Tabler\" and copy the password it shows (like abcd-efgh-ijkl-mnop)."
  say "Leave the Apple ID empty to do this later (run this command again then)."
  apple_id=$(ask "Apple ID (the e-mail address you sign in to Apple with)")
  if [ -z "$apple_id" ]; then
    say "Skipped: the app is signed but not notarized until this step is done."
    return 1
  fi
  password=$(ask "App-specific password" secret)
  [ -n "$password" ] || fail "no app-specific password given: nothing was changed"
  say "Checking the sign-in with Apple…"
  xcrun notarytool history --apple-id "$apple_id" --password "$password" --team-id "$team" \
    > /dev/null 2> "$WORK/notary.err" || {
    sed 's/^/  /' "$WORK/notary.err" >&2
    fail "Apple refused that sign-in: the notarization secrets were not changed"
  }
  printf '%s' "$apple_id" | set_secret APPLE_ID
  printf '%s' "$password" | set_secret APPLE_PASSWORD
}

finish() {
  say ""
  report
  if has_secrets $CERT_SECRETS $NOTARY_SECRETS; then
    say "Done: the next push to main builds a signed, notarized app."
  fi
}

# --- a .p12 exported from Keychain Access -----------------------------------------------------

if [ $# -gt 0 ] && [ "${1##*.}" = p12 ]; then
  [ -f "$1" ] || fail "no such file: $1 (run this script without a file to make the certificate first)"
  password=$(ask "Password of the .p12" secret)
  [ -n "$password" ] || fail "the .p12 needs a password (the build cannot import one without)"
  security create-keychain -p check "$WORK/peek.keychain-db"
  security import "$1" -k "$WORK/peek.keychain-db" -f pkcs12 -P "$password" > /dev/null 2>&1 ||
    fail "could not read the .p12: wrong password, or not a certificate with its private key"
  identity=$(security find-identity "$WORK/peek.keychain-db" |
    sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -n 1)
  security delete-keychain "$WORK/peek.keychain-db" > /dev/null 2>&1 || true
  [ -n "$identity" ] || fail "the .p12 holds no \"Developer ID Application\" certificate"
  # Keychain Access may export newer algorithms than the build reads: packed again either way.
  pem_of_identity "$1" "$password" "$identity" "$WORK/identity.pem"
  P12_PASSWORD=$(openssl rand -hex 24)
  legacy_p12 "$WORK/identity.pem" "$WORK/developer-id.p12" "$P12_PASSWORD"
  rm -f "$WORK/identity.pem"
  publish_p12 "$WORK/developer-id.p12" "$P12_PASSWORD" "$identity"
  notarization || true
  finish
  exit 0
fi

# --- the certificate in the login keychain ----------------------------------------------------

if [ "${1:-}" = --from-keychain ]; then
  LOGIN="$HOME/Library/Keychains/login.keychain-db"
  identity=$(security find-identity -v -p codesigning "$LOGIN" |
    sed -n 's/.*"\(Developer ID Application: [^"]*\)".*/\1/p' | head -n 1)
  [ -n "$identity" ] ||
    fail "no \"Developer ID Application\" certificate with its key in the login keychain"
  say "Certificate: $identity"
  say "macOS asks for your login password to export it (once per key; \"Allow\" is enough)."
  export_password=$(openssl rand -hex 24)
  security export -k "$LOGIN" -t identities -f pkcs12 -P "$export_password" \
    -o "$WORK/login.p12" > /dev/null || fail "the keychain did not export the certificate"
  pem_of_identity "$WORK/login.p12" "$export_password" "$identity" "$WORK/identity.pem"
  rm -f "$WORK/login.p12"
  P12_PASSWORD=$(openssl rand -hex 24)
  legacy_p12 "$WORK/identity.pem" "$WORK/developer-id.p12" "$P12_PASSWORD"
  rm -f "$WORK/identity.pem"
  publish_p12 "$WORK/developer-id.p12" "$P12_PASSWORD" "$identity"
  finish
  exit 0
fi

# --- the guided way ---------------------------------------------------------------------------

if has_secrets $CERT_SECRETS; then
  if has_secrets $NOTARY_SECRETS; then
    finish
    exit 0
  fi
  say "The certificate is in place; notarization is not."
  notarization || true
  finish
  exit 0
fi

ask_apple() {
  say ""
  say "What you do now, on Apple's page (it opens in your browser):"
  say "  1. Sign in with the Apple ID that is the team's Account Holder."
  say "  2. Pick \"Developer ID Application\" (under Software) and click Continue."
  say "  3. Leave \"G2 Sub-CA\" as it is. Click \"Choose File\" and pick"
  say "       Desktop ▸ $REQUEST_NAME"
  say "     then click Continue."
  say "  4. Click Download. Leave the file where it lands (Downloads)."
  say ""
  say "Then run this same command again:"
  say "  bash dev/scripts/apple-signing-secrets.sh"
  if [ -z "${TABLER_SIGNING_NO_OPEN:-}" ]; then open "$APPLE_PAGE" 2> /dev/null || say "Open: $APPLE_PAGE"; fi
}

if [ ! -f "$KEY" ]; then
  mkdir -p "$STATE"
  openssl req -new -newkey rsa:2048 -nodes -keyout "$KEY" -out "$STATE/request.csr" \
    -subj "/CN=Tabler Developer ID/C=US" > /dev/null 2>&1 || fail "could not make the certificate request"
  cp "$STATE/request.csr" "$DESKTOP/$REQUEST_NAME"
  say "Made the certificate request: Desktop ▸ $REQUEST_NAME"
  ask_apple
  exit 0
fi

if [ $# -gt 0 ]; then
  # A certificate file named on the command line: it is the one, or the run stops.
  PEM=$(find_certificate "$1")
elif ! PEM=$(find_certificate); then
  say "No certificate made from the request is in $DOWNLOADS yet."
  [ -f "$DESKTOP/$REQUEST_NAME" ] || cp "$STATE/request.csr" "$DESKTOP/$REQUEST_NAME"
  ask_apple
  exit 1
fi

IDENTITY=$(identity_of "$PEM")
if [ -z "$IDENTITY" ]; then
  say "The certificate that was downloaded is:"
  openssl x509 -in "$PEM" -noout -subject | sed 's/^/  /'
  fail "it is not a \"Developer ID Application\" certificate: pick that kind on Apple's page (step 2)"
fi

# The certificate, its key and Apple's Developer ID intermediates, under a password nobody needs
# to know: the build reads it from the secret next to the .p12.
P12="$WORK/developer-id.p12"
P12_PASSWORD=$(openssl rand -hex 24)
cat "$KEY" "$PEM" > "$WORK/identity.pem"
legacy_p12 "$WORK/identity.pem" "$P12" "$P12_PASSWORD"
rm -f "$WORK/identity.pem"
publish_p12 "$P12" "$P12_PASSWORD" "$IDENTITY"

# The key has done its work here: it goes into the login keychain (where Xcode would have put
# it, for signing a build on this Mac) or it is thrown away. GitHub holds the copy the build uses.
say ""
keep=$(ask "Keep the certificate in this Mac's login keychain too? [Y/n]")
case "$keep" in
  n | N | no | No) say "Not kept on this Mac." ;;
  *)
    if security import "$P12" -k "$HOME/Library/Keychains/login.keychain-db" -f pkcs12 \
      -P "$P12_PASSWORD" -T /usr/bin/codesign > /dev/null 2>&1; then
      say "Kept: Keychain Access ▸ My Certificates ▸ $IDENTITY"
    else
      say "Could not add it to the login keychain (the build is not affected)."
    fi
    ;;
esac
rm -f "$KEY" "$STATE/request.csr" "$DESKTOP/$REQUEST_NAME"

notarization || true
finish
