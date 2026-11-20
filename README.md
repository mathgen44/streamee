# Streamee

Interface légère pour streamers. Première fonction : **un pad de sons qui partent directement dans ton micro**,
avec ta voix, dans n'importe quel jeu (chat de proximité), Discord ou OBS, **sans micro virtuel**.

## Comment ça marche

Les solutions habituelles (Virtual Audio Cable, VoiceMeeter) ajoutent un faux micro qu'il faut choisir partout,
et elles entrent souvent en conflit avec d'autres logiciels audio (SteelSeries GG…).

Streamee fait autrement : il pose un petit **filtre audio Windows (APO)** sur ton vrai micro.
Les jeux et logiciels continuent d'utiliser ton micro habituel ; le filtre y ajoute simplement les sons du pad.

```
Streamee (pad)  ──►  tampon partagé  ──►  filtre APO sur le micro  ──►  jeu / Discord / OBS
       │            (ProgramData)          (voix + sons)
       └──►  retour casque (tu t'entends aussi)
```

- `apo/` — le filtre (DLL C++, chargé par `audiodg.exe`). S'il ne trouve pas Streamee, il laisse passer le micro tel quel.
- `scripts/` — installation et désinstallation du filtre sur un micro, avec sauvegarde des réglages d'origine.
- `src-tauri/` — l'application (Rust) : décodage des sons, mixage, envoi au filtre, retour casque, raccourcis globaux.
- `ui/` — l'interface du pad (HTML/CSS/JS, sans framework).

## Utilisation

1. Lance Streamee, glisse des sons dans la fenêtre (MP3, WAV, FLAC, OGG, M4A…).
2. **⚙ Réglages → Choisir le micro…** : Windows demande l'autorisation administrateur, puis tu choisis ton micro.
   Le service audio redémarre (deux secondes de coupure).
3. La pastille en haut passe au vert (« en direct ») dès qu'un logiciel utilise le micro : les sons y partent.

- Clic sur un pad : lecture. Clic droit (ou mode **✎ Édition**) : nom, couleur, volume, boucle, comportement au second appui, raccourci.
- **Raccourcis globaux** : ils marchent même quand le jeu a le focus (`Ctrl+Alt+Retour` coupe tout par défaut).
- Deux volumes : 🎙 ce que les autres entendent, 🎧 ce que tu entends.
- **Retirer de tous les micros** remet exactement les réglages d'origine.

## Limites connues

- Windows 10/11 uniquement.
- Si le micro avait déjà un effet propre au fabricant (hors chaîne « composite »), il est mis de côté pendant que Streamee est installé, et remis à la désinstallation.
- Un logiciel qui ouvre le micro en **mode exclusif** ou en mode « brut » (raw) contourne tous les filtres Windows : rare pour les jeux et Discord.
- Le filtre doit être réinstallé si Windows recrée le périphérique (nouveau pilote, autre port USB).

## Compiler

Prérequis : Visual Studio 2022 (C++), CMake, Rust stable.

```powershell
cmake -S apo -B apo/build -A x64
cmake --build apo/build --config Release
cd src-tauri
cargo test
npx @tauri-apps/cli@2 dev     # ou : build, pour l'installateur
```

Le workflow GitHub Actions construit l'installateur à chaque push et le publie en brouillon sur les tags `v*`.

## Licence

MIT — mathgen44
