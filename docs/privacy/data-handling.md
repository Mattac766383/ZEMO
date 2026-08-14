# Traitement des données — Phase 0

Statut : **normatif pour le MVP**. Le principe directeur est local-first :
collecter le minimum, garder les données sur le poste et rendre tout transfert
externe visible et ponctuel.

## Décisions

### Corpus

Les originaux restent à leur emplacement. L’analyse ouvre les fichiers en
lecture seule et ne crée ni sidecar, ni ADS, ni dossier technique. Elle ne
renomme, ne déplace et ne supprime rien. Le produit n’importe pas une copie
complète du corpus dans son stockage privé.

L’utilisateur choisit explicitement les racines analysées. Sortir d’une racine,
rencontrer un reparse point ou perdre l’autorisation arrête la lecture.

### Catalogue local

Le catalogue peut contenir :

- chemins et noms, qui sont considérés sensibles ;
- identité Windows, taille, dates, type détecté et empreinte ;
- extraits strictement nécessaires, index de recherche et embeddings locaux ;
- propositions, statut logique `TO_REVIEW` et provenance ;
- journal d’Apply, de reprise et de consentement.

Il est stocké dans SQLite/SQLCipher. Les buffers d’extraction bruts restent en
mémoire ou dans un temporaire chiffré et sont libérés dès traitement. Le
catalogue est reconstruisible depuis une nouvelle analyse non mutante.

### Secrets

La clé SQLCipher est aléatoire et scellée via DPAPI pour l’utilisateur Windows
courant. Les clés de fournisseurs sont stockées dans le gestionnaire de secrets
de l’OS. Elles ne transitent pas dans l’UI après saisie et ne figurent jamais
dans les logs, exports ou messages d’erreur.

### IA locale et cloud

Un modèle local reçoit uniquement les données prévues par une capacité Rust. Une
requête cloud requiert un consentement ponctuel, après affichage du fournisseur,
de la finalité, des fichiers/champs, du volume, des redactions et de la politique
de conservation externe. Le consentement est lié au hash du payload final et
n’est ni réutilisable ni transformable en autorisation permanente.

Le journal local conserve la preuve minimale — requête, fournisseur, finalité,
catégories, volume, hash, décision et date — mais pas le payload ni la réponse en
clair par défaut.

### Télémétrie et diagnostics

Aucune télémétrie produit ni rapport de crash n’est envoyé par défaut. Les logs
standards utilisent des identifiants opaques et excluent contenu, extraits,
prompts, chemins complets, secrets et réponses IA. Un diagnostic enrichi exige
une action utilisateur, une prévisualisation et une redaction avant export.

## Cycle de vie

- **Collecte** : au premier besoin et dans les racines sélectionnées seulement.
- **Conservation** : métadonnées, index et plans tant que le workspace est
  enregistré ; journal tant qu’il est nécessaire à l’explication et au rollback.
- **Mise à jour** : par observations locales validées, via le writer unique.
- **Purge** : « oublier ce workspace » supprime uniquement les données privées de
  l’application après confirmation ; le corpus n’est jamais touché.
- **Révocation** : retirer un fournisseur efface son secret local et bloque les
  nouveaux appels, sans prétendre effacer les données déjà reçues par lui.
- **Sauvegarde** : uniquement chiffrée ; restauration sur le même compte ou via
  une procédure explicite de récupération de clé.

Les durées exactes du journal et des sauvegardes doivent être affichées dans les
paramètres avant la version publique. Une purge de journal invalide les rollbacks
qui en dépendent et doit l’annoncer clairement.

## Invariants

- analyse sans aucune mutation du corpus ;
- aucune copie cloud implicite et aucun consentement global ;
- minimisation avant chiffrement : chiffrer n’autorise pas à tout conserver ;
- workers sans réseau ; seule la passerelle IA peut émettre une requête modèle ;
- séparation entre suppression de données privées de l’application et garantie
  de ne jamais supprimer/écraser un objet du corpus ;
- `TO_REVIEW` reste une donnée logique locale.

## Conséquences

Le produit fonctionne hors ligne et limite l’impact d’une fuite de base ou de
logs. Les recherches enrichies peuvent être moins complètes si l’utilisateur
refuse de conserver des extraits ou d’utiliser le cloud. Les consentements
répétés ajoutent une friction assumée.

Les embeddings, empreintes, chemins et métadonnées peuvent révéler des
informations même sans contenu brut ; ils reçoivent donc le même niveau de
protection que le catalogue.

## Limites

- SQLCipher ne protège pas une session déverrouillée ou un processus compromis ;
- DPAPI lie par défaut l’accès au profil Windows et complique la migration vers
  une autre machine ;
- après envoi cloud, la suppression et la rétention dépendent aussi du
  fournisseur présenté à l’utilisateur ;
- les sauvegardes système externes à Supremacy restent sous le contrôle de l’OS
  ou de l’administrateur.

## Critères de sortie

- inventaire des champs persistés avec finalité et durée approuvé ;
- tests confirmant l’absence de contenu sensible dans logs, erreurs, temporaires,
  base/WAL chiffrés et rapports par défaut ;
- parcours de consentement testé pour refus, annulation, expiration et payload
  modifié ;
- commandes « oublier un workspace », révoquer un fournisseur et réinitialiser
  les données locales testées sans mutation du corpus ;
- politique de conservation et fournisseurs affichés dans le produit ;
- test réseau démontrant que les workers n’émettent aucun trafic.
