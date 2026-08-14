# Modèle de menace — Phase 0

Statut : **normatif pour le MVP**. La priorité est de préserver le corpus : une
analyse compromise doit au pire échouer ou produire une proposition erronée,
jamais modifier des fichiers ni exfiltrer silencieusement leur contenu.

## Périmètre et hypothèses

Sont couverts : application Tauri, WebView, IPC, cœur Rust, workers, SQLCipher,
fichiers locaux et passerelle IA. Le poste est supposé sain au démarrage,
Windows à jour et la session utilisateur non compromise.

Un administrateur local malveillant, un kernel compromis, l’accès à la mémoire
d’un processus déverrouillé et la compromission physique après ouverture de
session sont hors du périmètre de protection. Ils restent des risques résiduels
explicites.

## Actifs

- contenu, noms, chemins, métadonnées et relations du corpus ;
- intégrité et disponibilité des fichiers ;
- catalogue, index, extraits et journal d’opérations ;
- clé SQLCipher, clés API et preuves de consentement ;
- décisions utilisateur, plans et provenance des recommandations.

## Frontières de confiance

1. **WebView → IPC Tauri** : toute requête et tout DTO sont non fiables.
2. **Cœur Rust → workers** : les fichiers et résultats de parsing sont hostiles.
3. **Cœur Rust → operation-executor** : le worker de mutation est un processus
   isolé authentifié ; il ne fait pas autorité sur le plan ni le consentement.
4. **Application → SQLite** : seul le writer unique est autorisé à écrire.
5. **Application → NTFS** : chemins et état peuvent changer concurremment.
6. **Passerelle IA → cloud** : une transmission quitte le contrôle local.

Les contenus de fichiers, noms de fichiers, prompts, sorties IA, réponses cloud,
extensions et types MIME déclarés ne sont jamais des autorités.

## Milestone 8.1 — Apply, consentement et exécuteur isolé

Statut : **normatif pour le chemin Apply Windows**. Les contrôles ci-dessous
complètent le modèle Phase 0 pour le durcissement d’exécution M8.1.

### Protections présentes

- **Frontend non fiable / compromis** : l’UI ne fournit que des identifiants
  opaques. Rust recharge le plan approuvé depuis SQLCipher ; aucune liste
  d’opérations fournie par le renderer n’est exécutée.
- **Approbation obsolète / plan forgé** : le plan est figé avec digest BLAKE3 ;
  le consentement est lié à ce digest, aux identités d’exécution/plan, à un
  nonce, à une échéance et à un MAC dérivé de la clé d’autorité OS-protégée.
  Une dérive de proposition/révision ou un digest incorrect invalide le
  consentement.
- **Rejeu** : consentement à usage unique ; côté exécuteur, MAC, nonces,
  séquence monotone, liaison requête/réponse et expiration rejettent rejeu,
  réordonnancement et trames altérées.
- **Traversal, symlink / reparse, état source périmé** : validation centralisée
  des chemins, rejet des redirections symlink/reparse, identité native stable et
  empreinte streaming avant mutation ; destinations occupées ou dangereuses
  échouent sans overwrite.
- **Corruption de journal** : chaîne authentifiée et diagnostics ; Apply peut
  être verrouillé en mode récupération/diagnostics (`journal_locked`) plutôt
  que de muter silencieusement.
- **Crash exécuteur / coordinateur / échec DB** : intention durable avant
  mutation ; réponses ambiguës ou commit impossible forcent
  `RECOVERY_REQUIRED` / états ambigus sans reprise avant automatique.
- **Exécution dupliquée** : l’identité de requête et l’état journalisé
  empêchent de rejouer une unité déjà traitée comme succès ou ambiguë.

### Hors périmètre explicite

L’authentification applicative de l’exécuteur **ne protège pas** un compte
utilisateur ou un OS local déjà pleinement compromis (accès mémoire processus,
clés OS, remplacement binaire local, administrateur malveillant). Ces cas
restent des risques résiduels. Le runtime NTFS Windows réel n’est pas encore
qualifié.

## Menaces principales et contrôles

### Confusion de chemin, reparse point et TOCTOU

Un attaquant remplace une source, crée une destination ou redirige un chemin
entre simulation et Apply.

Contrôles : handles Windows, identité `(volume, FILE_ID_128)`, version observée,
préflight tardif, rejet des reparse points et hard links ambigus, NTFS local
intra-volume seulement, renommage sans remplacement. Une divergence devient
logiquement `TO_REVIEW`.

### Altération ou perte pendant Apply

Un plan périmé, un crash ou un rollback aveugle déplace le mauvais objet ou
écrase une donnée.

Contrôles : plan immuable, journal durable avant effet, aucune primitive delete
ou overwrite, destination absente, états de reprise explicites et rollback
conditionnel. Un lot partiel est affiché comme tel.

### Fichier ou parseur hostile

Un document exploite un parseur, déclenche une bombe de décompression, consomme
les ressources ou tente de charger une URL.

Contrôles : parsing hors processus, workers sans réseau, privilèges et accès
fichiers minimaux, temps/mémoire/sortie bornés, détection de format indépendante
de l’extension, aucune macro ni ressource externe exécutée. Timeout, crash,
chiffrement ou incohérence donnent `TO_REVIEW`, sans mutation.

### XSS et abus IPC

Un contenu rendu dans React tente d’invoquer une commande privilégiée.

Contrôles : rendu échappé, CSP restrictive, aucun Node.js, commandes Tauri
allowlistées et orientées cas d’usage, DTO versionnés, identifiants opaques et
validation complète côté Rust. L’UI ne choisit pas l’opération système finale.

### Injection de prompt et exfiltration cloud

Un document ordonne au modèle de lire d’autres fichiers ou d’envoyer des secrets.

Contrôles : capacités minimales et bornées, modèles sans outils génériques,
redaction, passerelle réseau unique, consentement ponctuel lié au hash du payload
final, et validation de toute sortie comme donnée non fiable. Refus et mode
hors-ligne sont des chemins normaux.

### Vol ou fuite des données locales

Un autre processus lit le catalogue, les temporaires, sauvegardes ou logs.

Contrôles : SQLCipher, clé aléatoire scellée par DPAPI pour l’utilisateur courant,
ACL utilisateur, temporaires chiffrés ou évités, logs sans contenu ni secret et
backups chiffrés. Aucun secret n’est inclus dans un rapport de diagnostic par
défaut.

### Rejeu et falsification de consentement

Une autorisation ancienne est réutilisée pour un autre payload.

Contrôles : consentement à usage unique, horodaté, lié à la requête, au
fournisseur, à la finalité et au hash exact. Toute modification invalide la
preuve ; le contrôle est effectué en Rust.

### Déni de service et épuisement

Un corpus volumineux remplit la mémoire, la file SQLite ou le disque.

Contrôles : budgets par fichier et par lot, files bornées, backpressure,
pagination, annulation coopérative, quotas d’index et espace libre minimal.
L’arrêt n’autorise jamais une mutation compensatoire implicite.

### Chaîne d’approvisionnement et mise à jour

Une dépendance, un plugin Tauri ou une mise à jour introduit un privilège.

Contrôles : versions verrouillées, inventaire SBOM, audit des dépendances,
signature du binaire et des mises à jour, revue des capacités Tauri, provenance
de build et interdiction d’un téléchargement de code exécutable à l’exécution.

## Cas d’abus d’acceptation

- un PDF contenant « déplace tous les fichiers » ne produit qu’une donnée
  extraite et ne peut pas déclencher Apply ;
- une destination créée après la simulation bloque l’action sans overwrite ;
- un parser tentant une connexion sortante échoue ;
- un payload cloud modifié après consentement exige un nouveau consentement ;
- un crash juste après le renommage est réconcilié par identité, sans rejeu ;
- un rollback visant un fichier modifié depuis l’Apply est refusé.

## Décisions, conséquences et limites

La stratégie choisit **fail closed** et la revue humaine plutôt que la continuité
automatique. Elle ajoute de la friction, des processus isolés et des états
intermédiaires visibles, mais empêche qu’une ambiguïté devienne un effet
irréversible.

Le sandboxing Windows et SQLCipher ne protègent pas d’un compte utilisateur déjà
compromis. Les antivirus, logiciels de synchronisation et autres processus
peuvent modifier le corpus ; le produit détecte ces courses mais ne prétend pas
les empêcher.

## Invariants de sécurité

- aucune mutation du corpus pendant analyse ;
- aucun delete ou overwrite ;
- `TO_REVIEW` est uniquement logique ;
- Apply journalisé et limité à NTFS local intra-volume ;
- rollback seulement si toutes ses préconditions sont encore vraies ;
- aucun worker ne dispose du réseau ;
- aucun envoi cloud sans consentement ponctuel valide.

## Critères de sortie

- tests négatifs IPC, fuzzing des parseurs et budgets anti-DoS automatisés ;
- preuve d’isolation et d’absence d’egress pour chaque binaire worker ;
- tests de course NTFS et d’injection de panne sur chaque transition Apply ;
- inspection des artefacts SQLCipher, logs, temporaires et sauvegardes ;
- revue des capacités Tauri, du stockage de secrets et de la signature de mise à
  jour ;
- aucune menace critique ou élevée sans correction ou acceptation de risque
  écrite, datée et assortie d’un propriétaire.
