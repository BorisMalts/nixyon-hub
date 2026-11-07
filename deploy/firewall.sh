#!/usr/bin/env bash
# Правила iptables для подсетей Nixyon: контейнеры не ходят в частные сети и к
# облачным метаданным, а на сам хост из этих подсетей не проходит ничего. Встроенный
# DNS Docker работает внутри сетевого пространства контейнера и под запрет не попадает.
# apply добавляет правила (повторный запуск ничего не дублирует), remove убирает.
# Исключение одно: подсеть с выходом в интернет ходит на внешний адрес хоста по порту
# 443, как любой человек из интернета. Так CI берёт модели с зеркала хаба: хост
# перенаправляет такой запрос на прокси в частной сети, и без исключения его режет запрет.
# Исключение сверяет исходный адрес и порт запроса, а не адрес прокси, который меняется
# при перезапуске контейнера.
# Правила не переживают перезагрузку: их применяет служба nixyon-firewall, она же
# после этого запускает рабочий контейнер.
set -euo pipefail

SUBNETS=(10.231.0.0/24 10.232.0.0/24)
BLOCKED=(10.0.0.0/8 172.16.0.0/12 192.168.0.0/16 169.254.0.0/16 100.64.0.0/10)
OWN=(10.231.0.0/24 10.232.0.0/24 10.233.0.0/24)
EGRESS=10.232.0.0/24

# Внешний IPv4 хоста: адрес, с которого хост сам выходит в интернет.
public_ip() {
  ip -4 route get 1.1.1.1 | awk '{ for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1) }'
}

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
    if [[ "${src}" == "${EGRESS}" ]]; then
      local ip
      ip="$(public_ip)"
      if [[ "${ip}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        rule "${action}" DOCKER-USER -s "${src}" -p tcp -m conntrack \
          --ctorigdst "${ip}" --ctorigdstport 443 -j RETURN
      else
        echo "внешний адрес хоста не найден, CI не достанет до зеркала моделей" >&2
      fi
    fi
    rule "${action}" INPUT -s "${src}" -j DROP
  done
}

case "${1:-}" in
  apply | remove) run "$1" ;;
  *) echo "использование: firewall.sh apply|remove" >&2; exit 2 ;;
esac
