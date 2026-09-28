#!/usr/bin/env bash
# One-time setup: create a local self-signed code signing certificate.
# This ensures macOS TCC recognises the app across builds and updates.
#
# Run: bash scripts/setup-codesign.sh
# (No $99 Apple Developer account required.)

set -euo pipefail

CERT_NAME="WorkReview Self-Signed"
KEYCHAIN=~/Library/Keychains/login.keychain-db

if security find-certificate -c "$CERT_NAME" "$KEYCHAIN" &>/dev/null; then
    echo "[OK] Certificate '$CERT_NAME' already exists in login keychain."
    exit 0
fi

echo "[1/3] Generating self-signed certificate (valid 10 years)..."

openssl req -x509 \
    -newkey rsa:2048 \
    -keyout /tmp/wr_key.pem \
    -out /tmp/wr_cert.pem \
    -days 3650 \
    -nodes \
    -subj "/CN=$CERT_NAME/O=WorkReview/C=CN" \
    -config <(cat <<'EOF'
[req]
distinguished_name = req_dn
prompt = no
x509_extensions = v3_cs

[req_dn]
CN = WorkReview Self-Signed
O = WorkReview
C = CN

[v3_cs]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
subjectKeyIdentifier = hash
EOF
    ) 2>/dev/null

echo "[2/3] Importing into login keychain..."

# macOS 的 security 只认传统 PKCS#12（RC2/3DES + SHA1）。Homebrew 的 OpenSSL 3.x 默认用
# AES-256-CBC/SHA-256 导出，security import 会报
# "MAC verification failed during PKCS12 import (wrong password?)"（密码其实是对的）。
# 故优先用系统自带 LibreSSL（/usr/bin/openssl，输出传统格式），缺失时退回 OpenSSL 3 的 -legacy。
export_pkcs12() {
    if [ -x /usr/bin/openssl ]; then
        /usr/bin/openssl pkcs12 -export \
            -out /tmp/wr_cert.p12 \
            -inkey /tmp/wr_key.pem \
            -in /tmp/wr_cert.pem \
            -passout pass:workreview 2>/dev/null && return 0
    fi

    openssl pkcs12 -export -legacy \
        -out /tmp/wr_cert.p12 \
        -inkey /tmp/wr_key.pem \
        -in /tmp/wr_cert.pem \
        -passout pass:workreview 2>/dev/null
}

export_pkcs12

security import /tmp/wr_cert.p12 \
    -k "$KEYCHAIN" \
    -P workreview \
    -T /usr/bin/codesign 2>&1

rm -f /tmp/wr_key.pem /tmp/wr_cert.pem /tmp/wr_cert.p12

echo "[3/3] Verifying..."
if security find-certificate -c "$CERT_NAME" "$KEYCHAIN" &>/dev/null; then
    echo "[OK] Certificate installed. Tauri builds will now use stable code signing."
    echo "     tauri.conf.json signingIdentity should be: \"$CERT_NAME\""
else
    echo "[WARN] Certificate was not found after import. Check Keychain Access manually."
    exit 1
fi
