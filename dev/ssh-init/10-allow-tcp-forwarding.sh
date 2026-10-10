#!/bin/sh
# Runs inside the `ssh` container at start-up (linuxserver custom-cont-init.d hook).
# The linuxserver/openssh-server image ships sshd_config with AllowTcpForwarding=no;
# Tabler's SSH-tunnel tests need it on so the client can forward to postgres:5432 / mysql:3306.
CFG=/config/sshd/sshd_config
if [ -f "$CFG" ]; then
  if grep -q '^[#[:space:]]*AllowTcpForwarding' "$CFG"; then
    sed -i 's/^[#[:space:]]*AllowTcpForwarding.*/AllowTcpForwarding yes/' "$CFG"
  else
    echo 'AllowTcpForwarding yes' >> "$CFG"
  fi
  echo "[tabler] AllowTcpForwarding enabled in $CFG"
else
  echo "[tabler] $CFG not found; sshd_config not patched" >&2
fi
