# Snowflake test keys

Throwaway RSA keys for the snowflake lane's unit tests (JWT claims, fingerprint, encrypted
PKCS#8). They sign nothing real. Made with OpenSSL 3:

```bash
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out rsa_key.p8
openssl pkcs8 -topk8 -v2 aes-256-cbc -in rsa_key.p8 -out rsa_key_encrypted.p8 -passout pass:tabler-test
openssl rsa -in rsa_key.p8 -traditional -out rsa_key_pkcs1.pem
openssl rsa -in rsa_key.p8 -pubout -out rsa_key.pub
# The fingerprint Snowflake shows as RSA_PUBLIC_KEY_FP (Snowflake's documented command):
openssl rsa -in rsa_key.p8 -pubout -outform DER | openssl dgst -sha256 -binary | openssl enc -base64
# vJ5HJhsgorWJFKft6LsG4A6Tator6dqNsrtQ1Za0BTE=
```
