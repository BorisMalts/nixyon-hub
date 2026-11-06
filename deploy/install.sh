#!/usr/bin/env bash
# Установка и обновление стека Nixyon на сервере, запуск от root.
#   install.sh prepare <публичный адрес> <ssh-адрес репозитория Nixyon>
#       папка состояния /opt/nixyon, токен агента, ключ развёртывания; печатает
#       открытый ключ, его добавляют в GitHub: репозиторий, Settings, Deploy keys,
#       только чтение
#   install.sh up
#       клон Nixyon для образа сборки Android, образы, правила сети, запуск, проверка
#   install.sh user-add <почта>
#       учётная запись, пароль из stdin
#   install.sh runner-register
#       регистрация раннера GitHub Actions; одноразовый токен из stdin, его выдаёт
#       владелец репозитория: Settings, Actions, Runners, New self-hosted runner
#   install.sh debug-keystore
#       отладочный ключ подписи APK из stdin (обычно ~/.android/debug.keystore с
#       ноутбука), чтобы APK из хаба ставился поверх собранного локально
set -euo pipefail

STATE=/opt/nixyon
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_UID=1000
COMPOSE=(docker compose -p nixyon -f "${HERE}/compose.yaml" --env-file "${STATE}/.env")

die() {
  echo "ошибка: $*" >&2
  exit 1
}

[[ "$(id -u)" == 0 ]] || die "запускать от root"

prepare() {
  local origin="${1:-}" repo="${2:-}"
  [[ "${origin}" == https://* ]] || die "первый аргумент: публичный адрес вида https://nixyon.example.org"
  [[ "${repo}" == git@github.com:* ]] || die "второй аргумент: ssh-адрес вида git@github.com:владелец/репозиторий.git"
  install -d -m 700 "${STATE}" "${STATE}/secrets"
  if [[ ! -f "${STATE}/secrets/agent_token" ]]; then
    openssl rand -hex 32 > "${STATE}/secrets/agent_token"
  fi
  chmod 444 "${STATE}/secrets/agent_token"
  if [[ ! -f "${STATE}/secrets/deploy_key" ]]; then
    ssh-keygen -q -t ed25519 -N '' -C "nixyon-deploy" -f "${STATE}/secrets/deploy_key"
  fi
  chown "${WORKSPACE_UID}:${WORKSPACE_UID}" "${STATE}/secrets/deploy_key"
  chmod 600 "${STATE}/secrets/deploy_key"
  umask 077
  cat > "${STATE}/.env" <<ENV
NIXYON_PUBLIC_ORIGIN=${origin}
NIXYON_REPO_URL=${repo}
NIXYON_STATE=${STATE}
ENV
  echo "открытый ключ развёртывания (добавить в GitHub, только чтение):"
  cat "${STATE}/secrets/deploy_key.pub"
}

clone_source() {
  local repo
  repo="$(grep '^NIXYON_REPO_URL=' "${STATE}/.env" | cut -d= -f2-)"
  local ssh_cmd="ssh -i ${STATE}/secrets/deploy_key -o IdentitiesOnly=yes -o StrictHostKeyChecking=yes -o UserKnownHostsFile=${HERE}/github_known_hosts"
  if [[ -d "${STATE}/src/.git" ]]; then
    GIT_SSH_COMMAND="${ssh_cmd}" git -C "${STATE}/src" pull --ff-only
  else
    GIT_SSH_COMMAND="${ssh_cmd}" git clone "${repo}" "${STATE}/src"
  fi
}

install_firewall_unit() {
  install -m 755 "${HERE}/firewall.sh" /usr/local/sbin/nixyon-firewall
  cat > /etc/systemd/system/nixyon-firewall.service <<UNIT
[Unit]
Description=Nixyon: правила сети рабочего контейнера
After=docker.service
Requires=docker.service

[Service]
Type=oneshot
ExecStart=/usr/local/sbin/nixyon-firewall apply
ExecStartPost=-/usr/bin/docker start nixyon_workspace nixyon_runner
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
UNIT
  systemctl daemon-reload
  systemctl enable --now nixyon-firewall.service
}

# Общий пул для рабочего контейнера и раннера: вместе не больше 14 ядер и 24 ГБ,
# остальное остаётся проду.
install_slice() {
  cat > /etc/systemd/system/nixyon.slice <<UNIT
[Unit]
Description=Nixyon: общий пул ядер и памяти сборки и тестов

[Slice]
CPUQuota=1400%
MemoryMax=24G
MemorySwapMax=0
UNIT
  systemctl daemon-reload
  systemctl start nixyon.slice
}

# Адрес репозитория для раннера выводится из ssh-адреса: git@github.com:a/b.git
# превращается в https://github.com/a/b.
ensure_runner_url() {
  grep -q '^NIXYON_RUNNER_URL=' "${STATE}/.env" && return
  local repo path
  repo="$(grep '^NIXYON_REPO_URL=' "${STATE}/.env" | cut -d= -f2-)"
  path="${repo#git@github.com:}"
  path="${path%.git}"
  [[ "${path}" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || die "не получилось вывести адрес репозитория из ${repo}"
  echo "NIXYON_RUNNER_URL=https://github.com/${path}" >> "${STATE}/.env"
}

KEYSTORE="${STATE}/secrets/debug.keystore"

# Образ nixyon-android:dev собран из репозитория Nixyon, поэтому keytool в нём запускается
# как чужой код: под gVisor, без сети, не от root и с доступом только к пустой временной
# папке. Папку секретов он не видит.
keytool_in_image() {
  local dir="$1"
  shift
  docker run --rm --runtime=runsc --network none --user "${WORKSPACE_UID}:${WORKSPACE_UID}" \
    -v "${dir}:/work" --entrypoint keytool nixyon-android:dev "$@"
}

scratch_dir() {
  local dir
  dir="$(mktemp -d)"
  chown "${WORKSPACE_UID}:${WORKSPACE_UID}" "${dir}"
  chmod 700 "${dir}"
  echo "${dir}"
}

place_keystore() {
  local from="$1"
  [[ -f "${from}" && ! -L "${from}" ]] || die "ключ не получился обычным файлом"
  install -m 600 -o "${WORKSPACE_UID}" -g "${WORKSPACE_UID}" "${from}" "${KEYSTORE}.new"
  # Файл смонтирован в контейнер по inode: пишем поверх, а не заменяем, чтобы
  # работающий контейнер сразу видел новый ключ.
  if [[ -f "${KEYSTORE}" && ! -L "${KEYSTORE}" ]]; then
    cat "${KEYSTORE}.new" > "${KEYSTORE}"
    rm -f "${KEYSTORE}.new"
  else
    mv -f "${KEYSTORE}.new" "${KEYSTORE}"
  fi
}

# Без ключа Gradle создал бы свой при каждом новом томе, и APK перестали бы ставиться
# поверх друг друга; поэтому ключ живёт в папке состояния.
ensure_debug_keystore() {
  [[ -f "${KEYSTORE}" ]] && return
  local dir
  dir="$(scratch_dir)"
  trap 'rm -rf "${dir}"' EXIT
  keytool_in_image "${dir}" -genkeypair -noprompt \
    -keystore /work/debug.keystore -storepass android -alias androiddebugkey -keypass android \
    -dname "CN=Android Debug,O=Android,C=US" -keyalg RSA -keysize 2048 -validity 10000 > /dev/null
  place_keystore "${dir}/debug.keystore"
  rm -rf "${dir}"
  trap - EXIT
}

debug_keystore() {
  [[ -d "${STATE}/secrets" ]] || die "сначала install.sh prepare"
  local dir
  dir="$(scratch_dir)"
  trap 'rm -rf "${dir}"' EXIT
  head -c 65536 > "${dir}/in.keystore"
  [[ -s "${dir}/in.keystore" ]] || die "ключ пустой"
  chown "${WORKSPACE_UID}:${WORKSPACE_UID}" "${dir}/in.keystore"
  keytool_in_image "${dir}" -list -keystore /work/in.keystore -storepass android \
    -alias androiddebugkey > /dev/null \
    || die "это не отладочный ключ Android (пароль android, псевдоним androiddebugkey)"
  place_keystore "${dir}/in.keystore"
  echo "ключ подписи установлен"
}

# Зеркало моделей: пишет рабочий контейнер, хаб только читает. Владелец корня тома
# ставится заранее, иначе его задаёт тот контейнер, который смонтирует том первым.
prepare_models_mirror() {
  docker volume create \
    --label com.docker.compose.project=nixyon \
    --label com.docker.compose.volume=models-mirror \
    nixyon_models-mirror > /dev/null
  docker run --rm --user 0 --network none -v nixyon_models-mirror:/mirror nixyon-android:dev \
    sh -c "chown ${WORKSPACE_UID}:${WORKSPACE_UID} /mirror && chmod 755 /mirror"
}

# Наборы для обучения: том пишет только рабочий контейнер, владелец корня ставится
# заранее по той же причине, что у зеркала моделей.
prepare_training_data() {
  docker volume create \
    --label com.docker.compose.project=nixyon \
    --label com.docker.compose.volume=training-data \
    nixyon_training-data > /dev/null
  docker run --rm --user 0 --network none -v nixyon_training-data:/data nixyon-android:dev \
    sh -c "chown ${WORKSPACE_UID}:${WORKSPACE_UID} /data && chmod 755 /data"
}

up() {
  [[ -f "${STATE}/.env" ]] || die "сначала install.sh prepare"
  docker network inspect nixyon_edge > /dev/null 2>&1 || die "нет сети nixyon_edge: сначала выкатить Onyxar с блоком Nixyon"
  docker info --format '{{json .Runtimes}}' | grep -q runsc || die "нет рантайма runsc (gVisor)"
  clone_source
  docker build -t nixyon-android:dev "${STATE}/src/docker/android"
  ensure_runner_url
  ensure_debug_keystore
  # Наборы записей для проверки качества кладёт владелец, рабочий контейнер читает их
  # только на чтение.
  install -d -m 755 "${STATE}/datasets"
  install_slice
  install_firewall_unit
  prepare_models_mirror
  prepare_training_data
  "${COMPOSE[@]}" up -d --build --remove-orphans
  for _ in $(seq 1 30); do
    if [[ "$(docker inspect -f '{{.State.Health.Status}}' nixyon_hub 2> /dev/null)" == healthy ]]; then
      echo "хаб работает"
      "${COMPOSE[@]}" ps
      return
    fi
    sleep 5
  done
  "${COMPOSE[@]}" logs --tail 50 hub >&2
  die "хаб не поднялся"
}

user_add() {
  local email="${1:-}"
  [[ -n "${email}" ]] || die "нужна почта"
  "${COMPOSE[@]}" exec -T hub nixyon-hub user add "${email}"
}

runner_register() {
  docker exec -i nixyon_runner nixyon-runner-register
  echo "раннер зарегистрирован, через полминуты появится в GitHub: Settings, Actions, Runners"
}

case "${1:-}" in
  prepare) shift; prepare "$@" ;;
  up) up ;;
  user-add) shift; user_add "$@" ;;
  runner-register) runner_register ;;
  debug-keystore) debug_keystore ;;
  *) die "использование: install.sh prepare <адрес> <репозиторий> | up | user-add <почта> | runner-register | debug-keystore" ;;
esac
