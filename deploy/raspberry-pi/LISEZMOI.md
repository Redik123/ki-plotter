# Relais KI Plotter pour Raspberry Pi

Le Raspberry Pi reste branché en USB au plotter. Les PC envoient les découpes par le
réseau, depuis n'importe quelle pièce. Le Pi ne fait que transmettre : tout est préparé
dans KI Plotter sur le PC.

Compatible Raspberry Pi 2, 3, 4 et 5, avec Raspberry Pi OS 32 ou 64 bits (Lite suffit).

## 1. Préparer le Pi (une fois)

Avec **Raspberry Pi Imager**, installez Raspberry Pi OS Lite. Dans les réglages
(roue dentée) :

- nom d'hôte : `ki-plotter` (le Pi sera joignable en `ki-plotter.local`) ;
- activez **SSH** et choisissez un nom d'utilisateur et un mot de passe ;
- renseignez le Wi-Fi, ou branchez un câble réseau.

Démarrez le Pi et branchez le plotter sur un de ses ports USB.

## 2. Installer le relais

Depuis le PC, dans le dossier qui **contient** `ki-plotter-relay-pi` (le dossier `dist`
du projet), remplacez `utilisateur` par le vôtre :

```bash
scp -r ki-plotter-relay-pi utilisateur@ki-plotter.local:
```

```bash
ssh -t utilisateur@ki-plotter.local "cd ki-plotter-relay-pi && sudo sh install.sh"
```

Le script installe le service `ki-plotter-relay`. Il démarre tout seul à chaque mise sous
tension du Pi. À la fin, il affiche l'adresse de la page d'état
(`http://ki-plotter.local:7870/`), que vous pouvez ouvrir depuis un téléphone.

## 3. Utiliser depuis KI Plotter

**Découper** → *Envoyer vers* : **Raspberry Pi (réseau)** → **Rechercher**. Le relais
trouvé s'affiche avec le port du plotter. Le débit et le contrôle de flux se règlent dans
la même fenêtre, comme pour un plotter branché sur le PC.

## Dépannage

| Problème | À faire |
|---|---|
| « Aucun relais trouvé » | Le PC et le Pi doivent être sur le même réseau. Tapez l'adresse du Pi à la main (`ki-plotter.local` ou son IP). |
| « Aucun plotter détecté » | Plotter éteint ou câble USB débranché. Vérifiez avec `ki-plotter-relay --list-ports`. |
| Le plotter ne réagit pas | Essayez *Contrôle de flux : Aucun*, puis un autre débit, dans la fenêtre Découper. |
| Voir ce qui se passe | `journalctl -u ki-plotter-relay -f` |

Réglages : `/etc/ki-plotter-relay.conf`, puis `sudo systemctl restart ki-plotter-relay`.

**Mise à jour** : recopiez le dossier et relancez `sudo sh install.sh` (la configuration est
conservée).

**Désinstaller** :

```bash
sudo systemctl disable --now ki-plotter-relay
sudo rm /etc/systemd/system/ki-plotter-relay.service /usr/local/bin/ki-plotter-relay /etc/ki-plotter-relay.conf
```

Le relais accepte les travaux de tout appareil du réseau local. Il ne doit pas être exposé
sur Internet.
