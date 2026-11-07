#!/usr/bin/env bash
# Правила iptables для подсетей Nixyon: контейнеры не ходят в частные сети и к
# облачным метаданным, а на сам хост из этих подсетей не проходит ничего. Встроенный
# DNS Docker работает внутри сетевого пространства контейнера и под запрет не попадает.
# apply добавляет правила (повторный запуск ничего не дублирует), remove убирает.
# Правила не переживают перезагрузку: их применяет служба nixyon-firewall, она же
# после этого запускает рабочий контейнер.
set -euo pipefail

SUBNETS=(10.231.0.0/24 10.232.0.0/24)
BLOCKED=(10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 169.254.0.0/16 100.64.0.0/10)
OWN=(10.231.0.0/24 10.232.0.0/24 10.233.0.0/24)

rule() {
  local action="$1"; shift
  if [[ "${action}" == apply ]]; then
    iptables -C "$@" 2>/dev/null || iptables -I "$@"
  else
    while iptables -C "$@" 2>/dev/null; do iptables -D "$@"; done
  fi
}

run() {
  local action="$1"
  for src in "${SUBNETS[@]}"; do
    for dst in "${BLOCKED[@]}"; do
      rule "${action}" DOCKER-USER -s "${src}" -d "${dst}" -j DROP
    done
    for own in "${OWN[@]}"; do
      rule "${action}" DOCKER-USER -s "${src}" -d "${own}" -j RETURN
    done
    rule "${action}" INPUT -s "${src}" -j DROP
  done
}

case "${1:-}" in
  apply | remove) run "$1" ;;
  *) echo "использование: firewall.sh apply|remove" >&2; exit 2 ;;
esac
