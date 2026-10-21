# Relais KI Plotter pour Raspberry Pi

Le Raspberry Pi reste branché en USB au plotter. Les PC envoient les découpes par le
réseau, depuis n'importe quelle pièce. Tout est préparé dans KI Plotter sur le PC ; le Pi
garde les travaux et les envoie au plotter.

Compatible Raspberry Pi 2, 3, 4 et 5, avec Raspberry Pi OS 32 ou 64 bits (Lite suffit).

## Comment ça marche

- **File d'attente** (par défaut) : un travail envoyé depuis le PC attend sur le Pi. On va
  au plotter, on met le vinyle en place, puis on appuie sur **Couper** sur l'écran du Pi.
  Les travaux en attente sont gardés sur la carte SD, même après une coupure de courant.
- **Direct** : le travail est coupé dès qu'il arrive (réglable sur l'écran).
- L'écran de contrôle est une page web : `http://ki-plotter.local:7870/`. Elle s'utilise
  depuis le petit écran tactile du Pi, un téléphone ou un PC :
  - **Couper** un travail en attente, ou le supprimer ;
  - **Arrêter** une découpe en cours ;
  - **Coupe test** : le carré de 25 mm, avec les réglages du dernier envoi du PC ;
  - **Recouper** le dernier travail (une copie de plus, ou après une erreur) ;
  - **Réglages** : mode, port du plotter, débit, contrôle de flux, nom.

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
tension du Pi. À la fin, il affiche l'adresse de l'écran de contrôle.

## 3. Écran tactile 3,5" (facultatif)

1. Branchez l'écran sur les broches GPIO et installez **son pilote** en suivant la notice
   du fabricant (elle dépend du modèle : Waveshare, « 3.5inch RPi LCD », etc.). Continuez
   quand la console du Pi s'affiche sur le petit écran et que le tactile répond.
2. Installez l'affichage plein écran :

   ```bash
   ssh -t utilisateur@ki-plotter.local "cd ki-plotter-relay-pi && sudo sh kiosk.sh"
   ```

3. Redémarrez le Pi (`sudo reboot`). L'écran de contrôle s'ouvre tout seul, sans curseur.

## 4. Utiliser depuis KI Plotter

**Découper** → *Envoyer vers* : **Raspberry Pi (réseau)** → **Rechercher**. L'appli
indique si le relais est prêt et s'il est en mode file d'attente. Après l'envoi :
« En attente sur le Pi ✓ ». Il reste à lancer la découpe sur l'écran.

## Dépannage

| Problème | À faire |
|---|---|
| « Aucun relais trouvé » | Le PC et le Pi doivent être sur le même réseau. Tapez l'adresse du Pi à la main (`ki-plotter.local` ou son IP). |
| « Plotter absent » sur l'écran | Plotter éteint ou câble USB débranché. Vérifiez avec `ki-plotter-relay --list-ports`. |
| Le plotter ne réagit pas | Sur l'écran, **Réglages** : essayez *Contrôle de flux : Aucun*, puis un autre débit. |
| Voir ce qui se passe | `journalctl -u ki-plotter-relay -f` |

Réglages de base : `/etc/ki-plotter-relay.conf`, puis `sudo systemctl restart ki-plotter-relay`.
Ceux de l'écran sont dans `/var/lib/ki-plotter-relay/settings.conf`.

**Mise à jour** : recopiez le dossier et relancez `sudo sh install.sh` (la configuration et
la file d'attente sont conservées).

**Désinstaller** :

```bash
sudo systemctl disable --now ki-plotter-relay
sudo rm /etc/systemd/system/ki-plotter-relay.service /usr/local/bin/ki-plotter-relay /etc/ki-plotter-relay.conf
```

Le relais accepte les travaux de tout appareil du réseau local. Il ne doit pas être exposé
sur Internet.
