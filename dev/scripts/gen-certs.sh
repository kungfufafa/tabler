#!/bin/sh
# Generates the dev-only TLS material for the `pg-ssl` / `mysql-ssl` compose services and the
# client-certificate tests in src-tauri/tests/verify_servers.rs. Output: dev/certs/ (gitignored).
#
#   sh dev/scripts/gen-certs.sh          # (re)create everything
#
# Layout (all PEM unless said otherwise):
#   ca.crt / ca.key                 root CA "Tabler Dev Root CA" (servers trust only this)
#   int.crt / int.key               intermediate CA, signs the client certificates
#   server.crt / server.key         server cert signed by the root; SAN = DNS:localhost, pg-ssl,
#                                   mysql-ssl (deliberately NO IP SAN: verify-full to 127.0.0.1
#                                   must fail, to localhost must pass)
#   server-chain.crt                server.crt + ca.crt
#   client.crt                      leaf only, CN=tabler_cert (signed by the intermediate)
#   client-chain.crt                leaf + intermediate (what a client must send)
#   client-chain-reversed.crt       intermediate + leaf (wrong order)
#   client.key                      PKCS#8 ("BEGIN PRIVATE KEY", OpenSSL 3 default)
#   client-pkcs1.key                PKCS#1 ("BEGIN RSA PRIVATE KEY")
#   client-encrypted.key            encrypted PKCS#8 ("BEGIN ENCRYPTED PRIVATE KEY", pass "tabler")
#   client-key.der                  PKCS#8 DER (binary)
#   client-combined.pem             key + chain in one file
#   client.p12                      PKCS#12 bundle (key + chain, password "tabler")
#   client-ec.crt / client-ec.key   EC P-256 leaf + chain, SEC1 key ("BEGIN EC PRIVATE KEY")
#   other.crt / other.key           CN=someone_else (valid chain, wrong user for `cert` auth)
#   full.crt / full.key             CN=tabler_full (pg-ssl: password + clientcert=verify-full)
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
OUT="$ROOT/dev/certs"
mkdir -p "$OUT"
cd "$OUT"

DAYS=3650
SUBJ_PREFIX="/O=Tabler Dev"

cat > ext.cnf <<'CNF'
[ca]
basicConstraints = critical, CA:TRUE
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
[int]
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[server]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost, DNS:pg-ssl, DNS:mysql-ssl
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
[client]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = clientAuth
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
CNF

rsa_key() { openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$1" 2>/dev/null; }

# Root and intermediate CAs.
rsa_key ca.key
openssl req -x509 -new -key ca.key -days "$DAYS" -subj "$SUBJ_PREFIX/CN=Tabler Dev Root CA" \
  -config ext.cnf -extensions ca -out ca.crt
rsa_key int.key
openssl req -new -key int.key -subj "$SUBJ_PREFIX/CN=Tabler Dev Client CA" -out int.csr
openssl x509 -req -in int.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days "$DAYS" \
  -extfile ext.cnf -extensions int -out int.crt 2>/dev/null

# Server certificate (signed by the root).
rsa_key server.key
openssl req -new -key server.key -subj "$SUBJ_PREFIX/CN=localhost" -out server.csr
openssl x509 -req -in server.csr -CA ca.crt -CAkey ca.key -CAcreateserial -days "$DAYS" \
  -extfile ext.cnf -extensions server -out server.crt 2>/dev/null
cat server.crt ca.crt > server-chain.crt

# Client certificates (signed by the intermediate).
client() { # <name> <CN> <keyfile already generated>
  openssl req -new -key "$3" -subj "$SUBJ_PREFIX/CN=$2" -out "$1.csr"
  openssl x509 -req -in "$1.csr" -CA int.crt -CAkey int.key -CAcreateserial -days "$DAYS" \
    -extfile ext.cnf -extensions client -out "$1.crt" 2>/dev/null
}
rsa_key client.key
client client tabler_cert client.key
cat client.crt int.crt > client-chain.crt
cat int.crt client.crt > client-chain-reversed.crt
openssl rsa -in client.key -traditional -out client-pkcs1.key 2>/dev/null
openssl pkcs8 -topk8 -in client.key -v2 aes-256-cbc -passout pass:tabler -out client-encrypted.key
openssl pkcs8 -topk8 -nocrypt -in client.key -outform DER -out client-key.der
cat client.key client-chain.crt > client-combined.pem
openssl pkcs12 -export -inkey client.key -in client.crt -certfile int.crt -passout pass:tabler \
  -name tabler_cert -out client.p12

openssl ecparam -name prime256v1 -genkey -noout -out client-ec.key
client client-ec-leaf tabler_cert client-ec.key
cat client-ec-leaf.crt int.crt > client-ec.crt
rm -f client-ec-leaf.crt

rsa_key other.key
client other-leaf someone_else other.key
cat other-leaf.crt int.crt > other.crt
rm -f other-leaf.crt

rsa_key full.key
client full-leaf tabler_full full.key
cat full-leaf.crt int.crt > full.crt
rm -f full-leaf.crt

rm -f ./*.csr ./*.srl ext.cnf
chmod 600 ./*.key client-key.der client-combined.pem client.p12
echo "certificates written to $OUT:"
ls "$OUT"
