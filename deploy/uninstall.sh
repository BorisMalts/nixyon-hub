#!/usr/bin/env bash
# Полное удаление стека Nixyon с сервера, запуск от root: контейнеры, тома, образы,
# правила сети, служба systemd, папка /opt/nixyon. Блок Caddy и сеть nixyon_edge в
# Onyxar убираются отдельным PR, DNS-записи удаляет владелец.
set -euo pipefail

STATE=/opt/nixyon
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

[[ "$(id -u)" == 0 ]] || { echo "ошибка: запускать от root" >&2; exit 1; }

if [[ -f "${STATE}/.env" ]]; then
  docker compose -p nixyon -f "${HERE}/compose.yaml" --env-file "${STATE}/.env" down --volumes --rmi all --remove-orphans
fi
docker image rm nixyon-android:dev 2> /dev/null || true
if systemctl list-unit-files nixyon-firewall.service > /dev/null 2>&1; then
  systemctl disable --now nixyon-firewall.service || true
  rm -f /etc/systemd/system/nixyon-firewall.service
  systemctl daemon-reload
fi
"${HERE}/firewall.sh" remove
rm -f /usr/local/sbin/nixyon-firewall
if [[ -f /etc/systemd/system/nixyon.slice ]]; then
  systemctl stop nixyon.slice || true
  rm -f /etc/systemd/system/nixyon.slice
  systemctl daemon-reload
fi
rm -rf "${STATE}"
echo "Nixyon удалён. Осталось: PR в Onyxar без блока Caddy и сети nixyon_edge, DNS-записи nixyon."
