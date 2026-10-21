# KI Plotter

Logiciel de découpe pour plotter vinyle, écrit pour remplacer SignMaster Cut sur un
**KI-870A**. Windows, Tauri 2 (Rust + TypeScript).

## Ce qu'il fait

- **Texte** avec les polices installées sur le PC (crénage, espacement, interligne,
  alignement), conversion en courbes.
- **Import** SVG, DXF (AutoCAD) et PLT/HPGL (anciens fichiers SignMaster), glisser-déposer
  dans la fenêtre.
- **Vectorisation** d'images PNG/JPG/BMP/GIF avec seuil réglable et aperçu.
- **Échenillage** : cadre et lignes qui s'arrêtent aux lettres.
- **Mosaïque** : découpe d'un grand dessin en panneaux avec recouvrement, rangés sur le rouleau.
- **Découpe** : compensation de lame, dépassement, ordre optimisé (intérieurs d'abord),
  miroir pour le flex, aperçu et simulation de la lame, envoi par port série (USB),
  Raspberry Pi sur le réseau, imprimante Windows (RAW) ou fichier `.plt`, test de découpe.

## Lancer

```bash
npm install
npm run tauri dev
```

Version installable (`src-tauri/target/release/bundle/nsis/`) :

```bash
npm run tauri build
```

Tests : `cargo test --workspace` (Rust) et `npx tsc --noEmit` (typage du front).

## Premier essai sur le KI-870A

1. Brancher le plotter, l'allumer, fermer SignMaster (il bloque le port COM).
2. **Découper** → *Sortie* : choisir le port COM du plotter (souvent « USB-SERIAL CH340 »).
3. Choisir **Test (carré 25 mm)** et envoyer. Si rien ne sort : essayer le contrôle de flux
   *Aucun*, puis d'autres débits (9600, 38400, 115200).
4. Comparer la découpe avec l'aperçu : si le triangle est inversé, cocher/décocher
   *Inverser Y* (ou *Inverser X*, *Échanger X et Y*) dans *Réglages machine*.
5. Mesurer le carré : s'il fait 24,8 mm au lieu de 25, mettre l'échelle à 25 ÷ 24,8.
6. Coins arrondis → augmenter le décalage de lame ; petites « oreilles » aux coins → le réduire.

## Organisation

Voir [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

```
src/             interface (éditeur, dialogues)
src-tauri/       application Tauri (commandes)
crates/kp_geom   géométrie, préparation du travail, HPGL/DMPL, échenillage, mosaïque
crates/kp_import polices et texte, SVG, DXF, PLT, vectorisation
crates/kp_link   envoi au plotter (série, imprimante, fichier, relais réseau)
crates/kp_relay  service du Raspberry Pi (ki-plotter-relay)
deploy/raspberry-pi  service systemd, configuration, installation sur le Pi
```

## Licence

[GPL-3.0](LICENSE) : libre d'utiliser, d'étudier et de modifier le logiciel ; les versions
modifiées qui sont distribuées doivent rester sous GPL-3.0, avec leur code source.

## Plotter dans une autre pièce : relais Raspberry Pi

Un Raspberry Pi (2 à 5) reste branché en USB au plotter et reçoit les découpes par le
réseau. Dans **Découper**, choisir *Envoyer vers* **Raspberry Pi (réseau)** puis
**Rechercher** : le Pi est trouvé automatiquement sur le réseau local.

Préparer le paquet à copier sur le Pi (binaires 32 et 64 bits + script d'installation) :

```bash
npm run build:pi
```

Le résultat est dans `dist/ki-plotter-relay-pi/`. L'installation sur le Pi est décrite dans
[deploy/raspberry-pi/LISEZMOI.md](deploy/raspberry-pi/LISEZMOI.md). Il faut une seule fois :
`rustup target add aarch64-unknown-linux-musl armv7-unknown-linux-musleabihf`,
`cargo install cargo-zigbuild` et `pip install ziglang`.
