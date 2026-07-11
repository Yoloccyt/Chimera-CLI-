#!/usr/bin/env bash
set -euo pipefail

VERSION="${VERSION:-1.5.1-omega}"
BINARY_URL="https://github.com/Yoloccyt/Chimera-CLI-/releases/download/v${VERSION}/chimera-linux-x86_64"
PKG_DIR="chimera-cli_${VERSION}_amd64"
DEB_FILE="${PKG_DIR}.deb"

rm -rf "${PKG_DIR}" "${DEB_FILE}"
mkdir -p "${PKG_DIR}/usr/bin"
mkdir -p "${PKG_DIR}/DEBIAN"

# 私有仓库下载需要 GITHUB_TOKEN
curl_args=(-fsSL)
if [ -n "${GITHUB_TOKEN:-}" ]; then
  curl_args+=(-H "Authorization: Bearer ${GITHUB_TOKEN}")
fi

curl "${curl_args[@]}" -o "${PKG_DIR}/usr/bin/chimera" "${BINARY_URL}"
chmod +x "${PKG_DIR}/usr/bin/chimera"

sed "s/^Version: .*/Version: ${VERSION}/" packaging/apt/chimera-cli.control > "${PKG_DIR}/DEBIAN/control"

dpkg-deb --build "${PKG_DIR}"
rm -rf "${PKG_DIR}"

echo "Built ${DEB_FILE}"
