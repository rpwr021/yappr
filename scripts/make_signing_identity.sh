#!/usr/bin/env bash
# Create a STABLE self-signed code-signing identity for Yappr.
#
# Why: macOS ties TCC grants (Microphone, Input Monitoring) to the app's code
# signature. Ad-hoc signatures change every rebuild, so the OS keeps dropping
# the grants. A stable identity keeps the signature constant across rebuilds, so
# permissions stick once granted.
#
# The existing "Yappr Self-Signed" was imported as a CERTIFICATE ONLY (no
# private key), so `security find-identity -p codesigning` can't see it and
# build_app.sh falls back to ad-hoc. This script generates the key + cert
# together and imports both, producing a real signing identity.
#
#   ./scripts/make_signing_identity.sh        # create if missing
#   ./scripts/make_signing_identity.sh --force # recreate
set -euo pipefail

NAME="Yappr Self-Signed"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"
P12_PASSWORD="yappr"

if [ "${1:-}" != "--force" ] \
  && security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "$NAME"; then
  echo "identity '$NAME' already present and usable:"
  security find-identity -v -p codesigning "$KEYCHAIN" | grep "$NAME"
  exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# OpenSSL config: self-signed leaf with the codeSigning extended key usage.
# critical EKU + digitalSignature is what `codesign` requires.
cat >"$tmp/req.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = v3
prompt = no
[dn]
CN = $NAME
[v3]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF

echo "generating key + self-signed code-signing cert..."
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$tmp/key.pem" -out "$tmp/cert.pem" \
  -days 3650 -config "$tmp/req.cnf" >/dev/null 2>&1

# OpenSSL 3's default PKCS#12 encryption/MAC is not understood reliably by
# macOS `security import`. Use its legacy format and a non-empty password;
# otherwise `security` misleadingly reports "MAC verification failed".
openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
  -name "$NAME" -out "$tmp/identity.p12" \
  -passout "pass:$P12_PASSWORD" -legacy -macalg sha1 >/dev/null 2>&1

echo "importing into login keychain (allowing codesign to use it)..."
# -T grants codesign access without an interactive prompt at sign time.
security import "$tmp/identity.p12" -k "$KEYCHAIN" -P "$P12_PASSWORD" \
  -T /usr/bin/codesign >/dev/null

# Trust the cert for code signing so Gatekeeper/codesign accept it locally.
# (May prompt once for your login password.)
security add-trusted-cert -d -r trustRoot \
  -p codeSign -k "$KEYCHAIN" "$tmp/cert.pem" 2>/dev/null \
  || echo "note: could not auto-trust; codesign still works for local ad-hoc-equivalent use"

# Let codesign reuse the key without prompting on every build.
security set-key-partition-list -S apple-tool:,apple: -k "" "$KEYCHAIN" >/dev/null 2>&1 || true

echo
if security find-identity -v -p codesigning "$KEYCHAIN" | grep -q "$NAME"; then
  echo "success — '$NAME' is now a usable code-signing identity:"
  security find-identity -v -p codesigning "$KEYCHAIN" | grep "$NAME"
  echo
  echo "Next: ./scripts/build_app.sh will sign with it automatically."
  echo "First launch after re-signing will re-prompt for Microphone once, then stick."
else
  echo "ERROR: identity still not visible to codesigning. Check Keychain Access." >&2
  exit 1
fi
