# KI Plotter

Remplaçant de SignMaster Cut pour un plotter vinyle KI-870A. Tauri 2 : front TypeScript sans
framework (`src/`), commandes dans `src-tauri/src/lib.rs`, logique dans `crates/kp_*`.
Contrat complet : `docs/ARCHITECTURE.md`.

- Interface et messages d'erreur **en français** ; code et commentaires en anglais.
- Les types JSON existent deux fois : `src/model/types.ts` et les structs serde des crates
  (camelCase). Toute modification se fait des deux côtés.
- Unités : millimètres, repère document (x le long du rouleau, y en travers vers le bas).
  Le repère machine n'existe que dans `kp_geom::job`.
- `kp_link` et `kp_relay` ne dépendent pas de Tauri et doivent compiler sous Linux : le relais
  tourne sur un Raspberry Pi (`npm run build:pi` → `dist/ki-plotter-relay-pi/`, binaires musl
  statiques via cargo-zigbuild). Les fichiers de `deploy/raspberry-pi/` restent en fins de ligne LF.
- Test local du relais sans Pi : `ki-plotter-relay --listen 127.0.0.1:7870 --device file:<chemin>`.
- Le front tourne aussi dans un navigateur avec un faux backend (`src/backend.ts`) :
  `npm run dev`, utile pour tester l'interface sans l'appli.

Vérifications : `cargo test --workspace`, `cargo clippy --workspace --all-targets`,
`npx tsc --noEmit`, `npx vitest run`.
