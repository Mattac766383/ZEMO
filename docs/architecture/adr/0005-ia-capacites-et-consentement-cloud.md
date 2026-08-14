# ADR-0005 — IA par capacités et cloud par consentement ponctuel

- Statut : **accepté pour le MVP**
- Date : 2026-08-09

## Contexte

Les contenus analysés peuvent contenir des secrets et des instructions hostiles.
Un modèle local ou distant peut halluciner une action. Une clé API ou un accord
global ne constitue pas un consentement valable à transmettre un fichier
particulier.

## Décision

Toute interaction IA est médiée par la passerelle Rust. Elle reçoit une
**capacité** non transférable décrivant : le cas d’usage, les identités de
fichiers autorisées, les champs ou plages d’octets, le budget, les
transformations de redaction, l’échéance et l’identifiant de requête. L’absence
de capacité vaut refus.

Un modèle ne reçoit pas de chemin exploitable et ne peut pas appeler le système
de fichiers. Sa sortie est une proposition non fiable, revalidée par le domaine.
Les workers d’extraction et d’opérations n’ont aucune pile ni permission réseau ;
seule la passerelle IA peut effectuer l’egress nécessaire.

Pour chaque requête cloud, l’UI présente après construction du payload final :

- fournisseur et modèle ;
- finalité ;
- catégories de données, fichiers concernés et volume transmis ;
- redactions appliquées et risques résiduels ;
- lien vers la politique de conservation du fournisseur.

Le bouton d’envoi crée un consentement lié au hash du payload et à cette requête
uniquement. Il expire après usage ou annulation et ne peut pas devenir « toujours
autoriser ». Toute modification du fournisseur, du payload ou de la finalité
requiert un nouveau consentement.

## Invariants

- local-first et refus par défaut : aucune panne cloud ne bloque l’analyse locale ;
- aucune donnée du corpus ne quitte le poste sans consentement ponctuel valide ;
- une sortie IA ne déclenche jamais directement un Apply ;
- prompts, réponses et secrets fournisseur ne sont pas journalisés en clair ;
- les clés API résident dans le gestionnaire de secrets de l’OS ;
- les workers restent sans réseau, y compris lorsqu’un document tente de charger
  une ressource distante.

## Conséquences

La provenance et le motif de chaque envoi deviennent auditables et une injection
de prompt ne suffit pas à obtenir des données supplémentaires. En contrepartie,
les échanges cloud ont une friction volontaire, les payloads doivent être
prévisualisables et l’orchestration doit supporter refus, expiration et mode
hors-ligne.

Le consentement réduit le risque d’exfiltration mais ne contrôle pas ce que le
fournisseur conserve après réception. La minimisation et la redaction restent
obligatoires.

## Limites et alternatives rejetées

- une préférence persistante « cloud autorisé » est rejetée ;
- donner au modèle un outil générique de lecture ou de shell est rejeté ;
- les modèles locaux utilisent eux aussi des capacités, mais sans étape de
  consentement cloud ;
- mises à jour applicatives et autres flux réseau éventuels relèvent d’une
  politique séparée et ne doivent jamais réutiliser la capacité IA.

## Critères de sortie

- une capture réseau prouve zéro egress de corpus sans consentement ;
- consentement expiré, rejoué ou associé à un payload modifié est refusé ;
- les workers échouent à ouvrir une socket dans leur environnement de production ;
- des tests d’injection de prompt ne permettent ni lecture hors capacité, ni
  Apply, ni extension de budget ;
- l’historique local permet d’expliquer qui a consenti, à quoi et quand, sans
  conserver le contenu transmis en clair.
