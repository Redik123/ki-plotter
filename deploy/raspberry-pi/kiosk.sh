#!/bin/sh
# Affiche l'écran de contrôle du relais en plein écran sur le Raspberry Pi
# (écran tactile 3,5" sur les broches GPIO, ou tout autre écran).
#
# À lancer quand l'écran fonctionne déjà (la console du Pi s'y affiche) :
#   sudo sh kiosk.sh [utilisateur]
set -eu

if [ "$(id -u)" -ne 0 ]; then
  echo "Lancez ce script avec sudo : sudo sh kiosk.sh" >&2
  exit 1
fi
USER_NAME="${1:-${SUDO_USER:-pi}}"
HOME_DIR=$(getent passwd "$USER_NAME" | cut -d: -f6)
if [ -z "$HOME_DIR" ]; then
  echo "Utilisateur inconnu : $USER_NAME" >&2
  exit 1
fi

echo "Installation de l'affichage minimal (X, Openbox, Chromium)…"
apt-get update
apt-get install -y --no-install-recommends xserver-xorg x11-xserver-utils xinit openbox unclutter wget
apt-get install -y --no-install-recommends chromium-browser 2>/dev/null ||
  apt-get install -y --no-install-recommends chromium
BROWSER=$(command -v chromium-browser || command -v chromium)

# Connexion automatique de l'utilisateur sur la console.
if command -v raspi-config >/dev/null 2>&1; then
  raspi-config nonint do_boot_behaviour B2 || true
fi

# Openbox lance Chromium en plein écran sur la page du relais.
install -d -o "$USER_NAME" -g "$USER_NAME" "$HOME_DIR/.config/openbox"
cat > "$HOME_DIR/.config/openbox/autostart" <<EOF
# KI Plotter : écran de contrôle du relais
xset s off
xset s noblank
xset -dpms
unclutter -idle 0.5 -root &
# Attend que le relais réponde (démarrage du Pi).
for i in \$(seq 1 60); do wget -q -O /dev/null http://localhost:7870/status && break; sleep 1; done
"$BROWSER" --kiosk --noerrdialogs --disable-infobars --disable-session-crashed-bubble \\
  --incognito --overscroll-history-navigation=0 --disable-pinch \\
  --check-for-update-interval=31536000 --window-position=0,0 \\
  "http://localhost:7870/?kiosk=1" &
EOF
chown "$USER_NAME:$USER_NAME" "$HOME_DIR/.config/openbox/autostart"

# Démarre l'affichage à la connexion automatique sur l'écran (tty1).
PROFILE="$HOME_DIR/.bash_profile"
if ! grep -q "KI Plotter kiosque" "$PROFILE" 2>/dev/null; then
  cat >> "$PROFILE" <<'EOF'
# KI Plotter kiosque
if [ -z "${DISPLAY:-}" ] && [ "$(tty)" = /dev/tty1 ]; then
  exec startx /usr/bin/openbox-session -- -nocursor
fi
EOF
  chown "$USER_NAME:$USER_NAME" "$PROFILE"
fi

echo
echo "Écran de contrôle installé pour l'utilisateur $USER_NAME. Redémarrez : sudo reboot"
echo "Pour le retirer : supprimez le bloc « KI Plotter kiosque » de $PROFILE."
