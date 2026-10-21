# KI Plotter — architecture

Logiciel de découpe pour plotter vinyle (KI-870A), pour remplacer SignMaster Cut.

## Vue d'ensemble

```
src/            Interface (TypeScript, Vite) : éditeur, aperçu, dialogues
src-tauri/      Application Tauri 2 : commandes qui appellent les crates
crates/kp_geom  Géométrie et préparation du travail de découpe (aucune dépendance Tauri)
crates/kp_import Polices/texte, SVG, DXF, PLT, vectorisation d'images
crates/kp_link  Envoi au plotter : port série, imprimante Windows (RAW), fichier, relais
                réseau (client et protocole du relais)
crates/kp_relay Service « ki-plotter-relay » du Raspberry Pi branché au plotter
deploy/raspberry-pi  Service systemd, configuration, installation, script de compilation
```

Le PC **génère** le travail (HPGL/DMPL) ; l'**envoi** passe par une *destination*
interchangeable. Le Raspberry Pi ne fait que transmettre : il reçoit les octets du travail
et les envoie au plotter avec le même code `kp_link` que le PC.

### Briques réutilisées

| Besoin | Crate |
|---|---|
| Courbes, aplatissement | `kurbo` |
| Import SVG | `usvg` |
| Import DXF | `dxf` |
| Texte (mise en forme, crénage) | `rustybuzz` (+ `ttf-parser`) |
| Vectorisation d'images | `vtracer` |
| Lignes coupées par des formes (échenillage, mosaïque) | `i_overlay` |
| Port série | `serialport` |
| Imprimante Windows RAW | `windows` (winspool) |

Protocole DMPL : chaînes reprises d'Inkcut (`;:HAEC1`, `U x,y`, `D x,y`, `V`, `BP`).

## Repère et unités

Tout est en **millimètres**, dans le repère du document :

- `x` le long du rouleau (sens d'avance du vinyle), à partir de 0 ;
- `y` en travers du rouleau, **vers le bas** à l'écran ; la laize va de `y = 0` à
  `y = mediaWidth`.

Le passage au repère machine se fait seulement dans `kp_geom::job` (voir plus bas).

## Format d'échange (JSON, camelCase)

Les types TypeScript sont dans `src/model/types.ts`, les types Rust de chemin dans
`crates/kp_geom/src/path.rs`. Toute modification se fait des deux côtés.

```ts
type Seg = {t:"M",x,y} | {t:"L",x,y} | {t:"C",x1,y1,x2,y2,x,y} | {t:"Z"}
type PathData = Seg[]                // plusieurs sous-chemins, chacun commence par M
type Mat = [a,b,c,d,e,f]             // x' = a x + c y + e ; y' = b x + d y + f
interface Shape { path, transform: Mat, fillRule: "nonzero"|"evenodd" }
```

Le backend applique `transform` lui-même : le front n'envoie que ses objets tels quels.

## Commandes Tauri

Noms en snake_case côté Rust, arguments en camelCase côté JS (`invoke("text_outline", { req })`).
Les erreurs sont des `String` en français, affichables telles quelles.

| Commande | Arguments | Retour |
|---|---|---|
| `list_fonts` | — | `FontInfo[]` |
| `text_outline` | `req: TextRequest` | `TextOutline` |
| `import_file` | `path` (svg, dxf, plt/hpgl) | `Imported` |
| `load_image` | `path` | `ImageInfo` |
| `trace_image` | `req: TraceRequest` | `Imported` |
| `make_weeding` | `req: WeedRequest` | `PathData` |
| `make_tiles` | `req: TileRequest` | `Tile[]` |
| `build_job` | `req: JobRequest` | `JobResult` |
| `list_ports` | — | `PortInfo[]` |
| `list_printers` | — | `string[]` |
| `send_job` | `req: SendRequest` | — (événements `job-progress`, `job-finished`) |
| `cancel_job` | — | — |
| `read_text_file` / `write_text_file` | `path` (+ `contents`) | `string` / — |

## Préparation du travail (`kp_geom::job::build_job`)

1. Aplatir chaque `Shape` (transform appliquée) en polylignes, tolérance `cut.tolerance`.
2. `cut.mirror` : miroir sur `x` dans la boîte du travail (flex thermocollant).
3. Repère machine en mm. Référence `R` :
   - `fromOrigin = true` : la boîte englobante du travail ;
   - `fromOrigin = false` : `x ∈ [0, maxX du travail]`, `y ∈ [0, mediaWidth]`.

   Avec `u = x - R.minX`, `v = y - R.minY`, `W`, `H` la taille de `R` :
   `X = flipX ? W - u : u`, `Y = flipY ? H - v : v`, puis échange si `swapXY`.
4. Ordre (si `cut.optimize`) : intérieurs avant extérieurs (trous et petites pièces
   avant le contour qui les libère), puis plus proche voisin depuis l'origine ; un chemin
   fermé démarre au sommet le plus proche.
5. Compensation de lame (`bladeOffset > 0`) : le centre de la lame est décalé de
   `bladeOffset` dans le sens de la marche ; aux angles de plus de `cornerThreshold`
   degrés, arc de pivot autour du coin. Dépassement `overcut` sur les chemins fermés.
6. `passes` répétitions de chaque chemin.
7. Écriture : unités `unitsPerMm × scaleX/scaleY`, entiers.
   - HPGL : `IN;` [`VS{speed};FS{force};`] puis `PU x,y;PD x,y,x,y…;` (au plus
     `maxPointsPerCommand` points par `PD`, 0 = illimité), fin `PU0,0;` ou, si
     `feedAfter > 0`, `PU{maxX+feedAfter},0;`.
   - DMPL (expérimental) : `;:HAEC1 ` [`V{speed} BP{force} `] puis `U x,y ` / `D x,y `.
8. Retour : `data`, `moves` (trajet réel de la lame, repassé en coordonnées document pour
   l'aperçu — miroir compris, avec le dernier déplacement lame levée), `stats`
   (`pathCount` sans les passes ; longueurs et durée avec toutes les passes).

Valeurs par défaut (KI-870A, à confirmer sur la machine) : HPGL, 40 unités/mm,
`flipY = true`, `swapXY = false`, `flipX = false`, lame 0,25 mm, dépassement 1 mm,
vitesse/pression non envoyées (réglages du panneau).

## Échenillage (`make_weeding`)

Cadre rectangulaire à `margin` mm autour des formes (si `box`), plus `horizontal` et
`vertical` lignes réparties dans le cadre, coupées **seulement hors des formes**
(union des formes, chacune avec sa règle de remplissage), arrêtées à `gap` mm des
formes. Résultat : un `PathData` en coordonnées document.

## Mosaïque (`make_tiles`)

Grille sur la boîte englobante des formes : colonnes de `tileWidth` (le long du
rouleau, 0 = pas de découpe), rangées de `tileHeight` (en travers, 0 = pas de découpe).
Chaque panneau déborde de `overlap` sur ses voisins de droite et du bas ; le dernier
panneau (et le débord) s'arrête à la boîte englobante du dessin. Les chemins
sont découpés au rectangle du panneau ; `frame` ajoute le rectangle. Coordonnées
document d'origine : c'est le front qui range les panneaux sur le rouleau.

## Envoi (`kp_link`)

```ts
Destination = {kind:"serial", port, baud, flow:"none"|"hardware"|"software"}
            | {kind:"printer", name}      // Windows, mode RAW
            | {kind:"file", path}
            | {kind:"network", host, port, device, baud, flow}   // relais Raspberry Pi
```

`send` est bloquant (le Tauri le lance dans un thread), envoie par blocs de
`chunkSize` octets avec `chunkDelayMs` entre les blocs, rapporte la progression et
s'arrête entre deux blocs si on annule.

## Relais réseau (`kp_link::relay`, `kp_relay`)

HTTP/1.1 sur le port **7870**, une requête par connexion :

| Requête | Réponse |
|---|---|
| `GET /status` | `RelayStatus` : nom, mode, occupé, progression, plotter, ports série, dernier travail, nombre en attente |
| `POST /job?name=&width=&height=&seconds=&settings=&device=&baud=&flow=&chunkSize=&chunkDelayMs=` (corps = octets du travail) | `200` puis des lignes JSON. Mode file d'attente : `{"type":"queued","id","position"}`. Mode direct : `{"type":"progress","sent","total"}` … `{"type":"done","ok","cancelled","error"}`. Sinon `{"error":"…"}` (`409` si occupé) |
| `POST /cancel` | arrête le travail en cours entre deux blocs |
| `GET /` | écran de contrôle (tactile 480×320, téléphone, PC) |
| `GET /api/state` | état complet pour l'écran : en cours, file, dernier résultat, travail à recouper, réglages, ports |
| `POST /api/start?id=` | coupe un travail de la file (sans `id` : le suivant) |
| `POST /api/delete?id=` | retire un travail de la file |
| `POST /api/recut`, `POST /api/test`, `POST /api/cancel` | recouper le dernier, coupe test 25 mm, arrêter |
| `POST /api/settings` (JSON `{name, mode, device, baud, flow}`) | enregistre les réglages de l'écran |

Découverte : datagramme UDP `KI-PLOTTER-RELAY?` diffusé sur le port 7870 de chaque réseau
du PC, réponse `RelayInfo` en JSON.

Un seul fil de travail découpe, un travail à la fois. La file, le dernier travail, les
réglages de l'écran et les réglages machine du dernier envoi du PC (pour la coupe test,
construite sur le Pi avec `kp_geom`) sont gardés dans le dossier d'état
(`/var/lib/ki-plotter-relay`). Priorité des réglages : ligne de commande, écran
(`settings.conf`), `/etc/ki-plotter-relay.conf`.

Côté PC, la destination `network` envoie le travail avec son nom, sa taille et les
réglages `{cut, machine}`, puis suit la progression en mode direct ou rend
`SendOutcome::Queued` (« En attente sur le Pi »). Le port, le débit et le contrôle de flux
sont ceux du Pi. Si le PC se déconnecte, le Pi termine quand même la découpe. `device`
vaut `auto` (premier port USB série, sinon `/dev/usb/lp*`), un chemin, ou `file:<chemin>`
(tests).
