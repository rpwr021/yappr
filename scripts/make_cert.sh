#!/usr/bin/env bash
# Create a stable self-signed code-signing identity ("Yappr Self-Signed") in the
# login keychain, so build_app.sh produces a constant signature across rebuilds
# and macOS keeps your Input Monitoring / Accessibility grants.
#
# Run once. Safe to re-run (skips if the cert already exists).
set -euo pipefail

NAME="Yappr Self-Signed"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if security find-certificate -c "$NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo "'$NAME' already exists in the login keychain."
  exit 0
fi

tmp="$(mktemp -d)"
cat > "$tmp/cert.cnf" <<CNF
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
CNF

openssl req -x509 -newkey rsa:2048 -keyout "$tmp/k.key" -out "$tmp/c.crt" \
  -days 3650 -nodes -config "$tmp/cert.cnf" >/dev/null 2>&1
# legacy PKCS12 MAC so macOS `security` can import it (OpenSSL 3 default fails)
openssl pkcs12 -export -inkey "$tmp/k.key" -in "$tmp/c.crt" \
  -out "$tmp/c.p12" -name "$NAME" -passout pass:yappr -legacy -macalg sha1 >/dev/null 2>&1

security import "$tmp/c.p12" -k "$KEYCHAIN" -P "yappr" -T /usr/bin/codesign
rm -rf "$tmp"
echo "Created '$NAME'. build_app.sh will now sign Yappr with a stable identity."
