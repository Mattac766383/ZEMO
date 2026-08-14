# ADR-0003 — SQLite/SQLCipher et writer unique

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

Le catalogue contient chemins, identités, métadonnées, extraits, décisions et
journaux potentiellement sensibles. Plusieurs producteurs asynchrones doivent
l’alimenter sans créer de contention, de migrations concurrentes ni d’états
partiellement persistés.

## Décision

Le stockage local est **SQLite chiffré par SQLCipher**. Un acteur Rust unique
possède l’unique connexion en écriture et sérialise des commandes typées. Les
workers n’ouvrent jamais la base. Les lectures utilisent des connexions
`query_only`, des transactions courtes et des snapshots cohérents.

Le mode WAL est admis uniquement après vérification que la base, le WAL et les
fichiers temporaires sont bien chiffrés avec les paramètres SQLCipher retenus.
Les migrations sont versionnées, transactionnelles et exécutées avant le
démarrage du writer. Une seule instance applicative détient le verrou
d’exploitation du catalogue.

Sous Windows, une clé aléatoire dédiée est scellée pour l’utilisateur courant
avec DPAPI. Elle n’apparaît ni dans la configuration, ni dans les logs, ni dans
les sauvegardes en clair. La perte de cette clé rend le catalogue irrécupérable,
mais n’affecte pas le corpus.

## Invariants

- exactement un writer applicatif ; aucune écriture directe par l’UI ou un
  worker ;
- toute commande est atomique ou possède une machine d’état de reprise ;
- le catalogue reste une vue reconstruisible et ne fait jamais foi contre
  l’identité constatée sur NTFS ;
- les journaux d’Apply sont persistés avant toute mutation du corpus ;
- aucune donnée sensible n’est écrite dans une base, un WAL ou un fichier
  temporaire non chiffré.

## Conséquences

Le modèle simplifie l’ordre des événements, les migrations et la reprise après
crash. Il impose une file bornée, de la backpressure et des écritures groupées
pour ne pas bloquer l’analyse. Les requêtes longues doivent être paginées afin de
ne pas retenir le WAL.

SQLCipher ajoute une dépendance native, un coût de chiffrement, une gestion de
clé et une vérification de licence/distribution. Le chiffrement au repos ne
protège pas les données quand la session utilisateur et l’application sont
ouvertes.

## Limites et alternatives rejetées

- SQLite sans chiffrement est rejeté pour le catalogue de production.
- Un fichier JSON par objet ne fournit ni transactions multi-objets, ni
  migrations fiables.
- Plusieurs writers avec retry sont rejetés : ils déplaceraient la complexité
  vers chaque producteur et rendraient l’ordre des journaux ambigu.
- Le stockage sur partage réseau ou dossier synchronisé n’est pas supporté.

## Critères de sortie

- la distribution Windows embarque une version SQLCipher identifiée et conforme
  aux obligations de licence ;
- des tests confirment qu’une recherche de chaînes sensibles ne révèle rien dans
  la base, le WAL, les temporaires et les sauvegardes ;
- wrong-key, migration interrompue, crash en écriture et redémarrage sont testés ;
- un test de charge avec producteurs concurrents ne crée qu’un seul writer,
  applique la backpressure et conserve l’ordre attendu ;
- l’intégrité et la restauration d’une sauvegarde sont vérifiées avant
  activation de l’Apply.
