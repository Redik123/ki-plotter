#!/bin/sh
# Installe (ou met à jour) le relais KI Plotter sur le Raspberry Pi.
# Usage : sudo sh install.sh
set -eu
cd "$(dirname "$0")"

if [ "$(id -u)" -ne 0 ]; then
  echo "Lancez ce script avec sudo : sudo sh install.sh" >&2
  exit 1
fi

case "$(uname -m)" in
  aarch64 | arm64) BIN=ki-plotter-relay-aarch64 ;;
  armv7l | armv8l) BIN=ki-plotter-relay-armv7 ;;
  *)
    echo "Ce Raspberry Pi ($(uname -m)) n'est pas pris en charge : il faut un Pi 2, 3, 4 ou 5." >&2
    exit 1
    ;;
esac

install -m 0755 "$BIN" /usr/local/bin/ki-plotter-relay

if [ -f /etc/ki-plotter-relay.conf ]; then
  echo "Configuration existante conservée : /etc/ki-plotter-relay.conf"
else
  install -m 0644 ki-plotter-relay.conf /etc/ki-plotter-relay.conf
fi

install -m 0644 ki-plotter-relay.service /etc/systemd/system/ki-plotter-relay.service
systemctl daemon-reload
systemctl enable ki-plotter-relay.service >/dev/null
systemctl restart ki-plotter-relay.service
sleep 1

if systemctl is-active --quiet ki-plotter-relay.service; then
  IP=$(hostname -I 2>/dev/null | awk '{print $1}')
  echo
  echo "Relais KI Plotter installé et démarré."
  echo "  Écran de contrôle : http://${IP:-<adresse du Pi>}:7870/   (ou http://$(hostname).local:7870/)"
  echo "  Dans KI Plotter : Découper > Envoyer vers « Raspberry Pi (réseau) » > Rechercher."
  echo "  Écran tactile sur le Pi : sudo sh kiosk.sh (une fois son pilote installé)."
  echo
  /usr/local/bin/ki-plotter-relay --list-ports | sed 's/^/  Port détecté : /' || true
else
  echo "Le service n'a pas démarré. Détails :" >&2
  journalctl -u ki-plotter-relay.service -n 20 --no-pager >&2 || true
  exit 1
fi
