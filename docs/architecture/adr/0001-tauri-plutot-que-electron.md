# ADR-0001 — Tauri 2 plutôt qu’Electron

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

Le produit historique repose sur Electron. La refondation doit réduire la
surface privilégiée JavaScript, rapprocher les contrôles du cœur Rust et
produire une application Windows distribuable sans embarquer Node.js dans
l’interface.

## Décision

Le client de bureau utilise **Tauri 2**, React et TypeScript. Sous Windows,
l’interface s’exécute dans WebView2. Le processus WebView n’accède ni à Node.js,
ni directement au système de fichiers, à SQLite, aux secrets ou au réseau IA.
Toutes les opérations privilégiées passent par des commandes Tauri explicites,
typées et validées en Rust.

Les capacités et plugins Tauri sont désactivés par défaut puis autorisés au plus
juste. Un plugin n’est adopté qu’après revue de sa surface IPC, de ses
dépendances et de ses permissions.

## Invariants

- l’UI est une frontière non fiable, même si son code est livré avec l’app ;
- aucune commande IPC générique de type `execute`, `read_path` ou `write_path` ;
- aucun chemin venant de TypeScript ne fait foi pour un Apply ;
- la politique métier et les consentements sont revérifiés côté Rust ;
- la migration n’affaiblit pas les garanties locales : analyse non mutante,
  workers sans réseau et Apply journalisé restent obligatoires.

## Conséquences

La distribution est plus petite et le cœur Rust peut partager ses types et
contrôles avec les workers. En contrepartie, le rendu dépend de la version
WebView2 qualifiée, les écarts Windows doivent être testés, et une partie du
processus principal Electron doit être réécrite plutôt que portée telle quelle.

Tauri n’est pas considéré « sûr par nature » : une commande trop large, une CSP
faible ou un plugin permissif recréerait la même surface d’attaque.

## Limites et alternatives rejetées

- **Conserver Electron** réduirait l’effort initial mais maintiendrait un runtime
  Node privilégié et deux autorités possibles, JavaScript et Rust.
- **Application Rust native** réduirait encore la surface Web, mais augmenterait
  le coût de migration de l’interface et l’écart avec l’écosystème React.
- Le MVP cible WebView2 Evergreen ; le choix d’un bootstrap ou runtime fixe doit
  être arrêté par la stratégie d’installation avant publication.

## Critères de sortie

- un installateur fonctionne sur des images propres de Windows 10 et 11 ;
- la stratégie d’installation/mise à jour WebView2 est documentée et testée ;
- l’inventaire IPC ne contient que des cas d’usage nommés et autorisés ;
- CSP, allowlists Tauri et absence d’API Node sont vérifiées automatiquement ;
- une tentative d’appel IPC forgé depuis la WebView est refusée côté Rust.
