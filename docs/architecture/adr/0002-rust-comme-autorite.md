# ADR-0002 — Rust comme autorité

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

React, les parseurs, les modèles IA et les chemins affichés peuvent être
compromis, obsolètes ou simplement incohérents. Répartir les décisions entre
TypeScript, des workers et Rust rendrait les garanties de sécurité impossibles à
prouver.

## Décision

Rust est l’unique autorité pour :

- l’identité et l’état des fichiers ;
- les transitions d’analyse, de simulation, d’Apply et de rollback ;
- la validation des racines, préconditions et destinations ;
- la politique de capacités IA et la preuve du consentement cloud ;
- la persistance, le journal d’opérations et les règles de confidentialité.

TypeScript gère la présentation et envoie des intentions via des DTO versionnés.
L’application Rust recharge l’état faisant foi et prend la décision. Les workers
renvoient des observations non fiables ; ils ne modifient ni le corpus ni la
base et n’ont aucun accès réseau.

Le domaine Rust reste indépendant de Tauri, SQLite et Windows. Des adaptateurs
implémentent ses ports et les erreurs métier sont explicites, sérialisables et
stables pour l’UI.

## Invariants

- aucune validation de sécurité ne dépend uniquement du frontend ;
- un identifiant opaque référence les objets côté Rust ; un chemin affiché
  n’accorde aucune autorisation ;
- toute transition métier invalide est refusée avant effet externe ;
- les sorties de workers et de modèles sont bornées, validées et traitées comme
  des données, jamais comme des commandes ;
- les capacités sont minimales, temporaires et limitées à un cas d’usage.

## Conséquences

Les invariants deviennent testables sans WebView ni base réelle et la même
politique s’applique à tous les adaptateurs. Cela ajoute des conversions DTO,
une gestion stricte des versions IPC et parfois une duplication de types entre
Rust et TypeScript. Cette duplication doit être générée ou couverte par des
tests de contrat, pas compensée par une commande IPC permissive.

Rust réduit certaines classes d’erreurs mémoire, mais ne protège ni d’une règle
métier incorrecte, ni d’un mauvais appel système, ni d’une dépendance vulnérable.

## Limites et alternatives rejetées

- Une logique partagée frontend/backend est admise seulement pour le rendu
  non normatif, par exemple le tri visuel.
- Les scripts de plugins ne peuvent pas étendre les privilèges du cœur.
- Mettre l’autorité dans la base via des triggers seuls est rejeté : les
  préconditions système de fichiers ne sont pas observables par SQLite.

## Critères de sortie

- les couches de domaine et d’application se testent sans Tauri ;
- chaque commande IPC possède un schéma, une version et des tests négatifs ;
- des tests prouvent qu’un chemin forgé, un état périmé et une sortie worker
  malformée ne déclenchent aucun effet ;
- les dépendances ne créent aucun chemin du domaine vers les adaptateurs ;
- les états et erreurs structurés suffisent à expliquer un refus dans l’UI.
