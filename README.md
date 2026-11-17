# Fast Copy

[![CI](https://github.com/andriantenaina/Fast-Copy/actions/workflows/ci.yml/badge.svg)](https://github.com/andriantenaina/Fast-Copy/actions/workflows/ci.yml) [![Release](https://img.shields.io/github/v/release/andriantenaina/Fast-Copy?include_prereleases)](https://github.com/andriantenaina/Fast-Copy/releases)

**Copies de fichiers très rapides, vérifiées et reprenables — Windows 10/11 et macOS 12+.**
*Very fast, verified, resumable file copies for Windows 10/11 and macOS 12+.* ([English below](#english))

- ⚡ **Rapide** : lecture et écriture en parallèle (pipeline à tampons alignés), E/S sans cache pour les gros fichiers, workers parallèles pour les milliers de petits fichiers, clonage instantané sur APFS/ReFS, profils adaptés (NVMe, disque dur, clé USB, réseau).
- 🛡️ **Aucune perte de données** : écriture dans un fichier temporaire, vidage sur le disque, **relecture sans cache et comparaison d'empreinte (XXH3-128)**, puis renommage atomique. Un fichier existant n'est jamais écrasé à moitié. En mode « déplacer », la source n'est supprimée qu'une fois la copie vérifiée et durable.
- ⏯️ **Reprise automatique** : un journal enregistre l'avancement ; après une clé arrachée, une coupure de courant ou un Ctrl+C, `fcopy resume` repart du dernier point sûr.
- 💾 **Clés USB** :
  - **FAT32** (limite 4 Go par fichier) : proposition de **formater en exFAT** (lisible *et* inscriptible sur Windows et Mac), ou **découpage automatique** des gros fichiers en morceaux `.001`, `.002`… recollés automatiquement à la copie inverse, ou sans Fast Copy grâce aux scripts fournis.
  - **NTFS sur Mac** (lecture seule) : détecté, avec proposition de formater en exFAT.
- 🔤 **Noms compatibles** entre Windows et Mac : Unicode NFC, caractères interdits, noms réservés, doublons de casse — tout est signalé.
- 🖱️ **Intégration au système** : menu clic droit de l'Explorateur (Windows 10, et menu moderne de Windows 11) et Actions rapides du Finder.

## Utilisation (ligne de commande)

```sh
fcopy copy  D:\Vidéos E:\              # copie vérifiée (par défaut)
fcopy move  ~/Projets /Volumes/CLE     # déplacer : source supprimée après vérification
fcopy copy  film.mkv E:\ --verify fast # plus rapide, sans relecture
fcopy copy  dossier E:\ --on-conflict newer --eject
fcopy resume                           # reprendre la dernière copie interrompue
fcopy jobs                             # lister les copies interrompues
fcopy verify D:\Photos E:\             # comparer octet par octet
fcopy drive list                       # lecteurs, système de fichiers, bus
fcopy drive format E:\ --label MACLE   # formater en exFAT (EFFACE TOUT, confirmation)
fcopy split gros.iso E:\               # découper manuellement
fcopy join  E:\gros.iso.fcsplit.json C:\ # recoller
fcopy bench gros.iso E:\ --compare     # mesurer le débit réel
fcopy shell install                    # menus clic droit
```

Codes de sortie : `0` succès · `1` terminé avec erreurs · `2` bloqué/échec · `130` annulé (reprenable).
`--json` produit une ligne JSON par événement, pour l'utiliser depuis des scripts.

| Niveau de vérification | Ce qui est garanti |
|---|---|
| `fast` | empreinte calculée à la lecture, pas de relecture |
| `full` (défaut) | destination relue **depuis le disque** (cache contourné) et comparée |
| `paranoid` | `full` + relecture de la source |

## Mesures (machine de développement)

| Scénario (2 Gio + 5 000 petits fichiers) | Fast Copy | robocopy |
|---|---|---|
| Gros fichier, NVMe → NVMe, **avec vérification** | **4,1 s** | 7,6 s (sans vérification) |
| 5 000 petits fichiers, NVMe → NVMe, **avec vérification** | 3,7 s | 3,0 s (`/MT:8`, sans vérification ni vidage) |
| Gros fichier, NVMe → disque dur 5 400 tr/min | 28-31 s | 25 s (sans vidage) |

robocopy rend la main avant que ses données soient vidées sur le disque ; Fast Copy ne déclare un fichier copié qu'une fois ses données sur le disque et vérifiées. Lancez `fcopy bench` sur votre matériel.

## Comment ça marche

```
crates/fc-core      moteur (planification, pipeline, vérification, journal, volumes, formatage, découpage)
crates/fc-cli       binaire `fcopy`
crates/fc-helper    assistant élevé (UAC) : formatage diskpart, clichés VSS
crates/fc-shellext-win  extension COM du menu contextuel de Windows 11
app/                interface graphique Tauri v2 (Svelte 5)
packaging/          installeurs, paquet « sparse » Windows, winget, Homebrew
```

Choix techniques détaillés (avec sources) : E/S sans cache alignées (`FILE_FLAG_NO_BUFFERING`, `F_NOCACHE`), vidage durable (`FlushFileBuffers`, `F_FULLFSYNC`), préallocation sans exposer d'anciennes données (jamais `SetFileValidData`), exFAT en MBR avec clusters ≤ 1 Mo (macOS refuse de monter les clusters ≥ 2 Mo), morceaux de 4 095 Mio compatibles 7-Zip.

## Installer

Téléchargez la dernière version sur la page [Releases](https://github.com/andriantenaina/Fast-Copy/releases) : installeur Windows (`-setup.exe` ou `.msi`), DMG macOS universel, ou archives de la CLI `fcopy`.

## Compiler

```sh
cargo build --release -p fc-cli -p fc-helper     # CLI
cargo test -p fc-core -p fc-cli                  # tests
cd app && npm ci && npm run tauri build          # application graphique
```

Windows : Visual Studio Build Tools (C++) et le SDK Windows. macOS : Xcode Command Line Tools.
Applications non signées : sur macOS 12, clic droit ▸ Ouvrir au premier lancement ; sur Windows, SmartScreen ▸ « Informations complémentaires » ▸ « Exécuter quand même ».

## Licence

MIT ou Apache-2.0, au choix.

---

## English

Fast Copy copies huge files and huge numbers of files as fast as your hardware allows, and **never reports success for data that is not verified on disk**:
- temp file → flush → cache-bypassing re-read and XXH3-128 compare → atomic rename;
- crash-safe journal with automatic resume (`fcopy resume`);
- moves delete sources only after verification.

For USB drives:
- **FAT32** (4 GiB per-file limit): reformat to **exFAT** (read/write on both Windows and macOS), or split large files automatically into 7-Zip-compatible chunks. Chunks are rejoined automatically on copy-back, or without the tool using the bundled `copy /b` / `cat` scripts.
- **NTFS** drives, which macOS can only read, are detected and the tool offers the exFAT reformat.

Download builds from [Releases](https://github.com/andriantenaina/Fast-Copy/releases). Run `fcopy --help` for all commands. Licensed MIT OR Apache-2.0.
