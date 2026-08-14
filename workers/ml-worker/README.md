# ML worker optionnel

Ce processus ne possède ni chemin vers le corpus ni accès réseau. Il communique
par JSON Lines sur `stdin/stdout`, refuse toute requête supérieure à 32 Mio et
ne télécharge jamais de modèle.

Par défaut, seule la commande `health` est disponible. Une capacité OCR, vision
ou reranking ne peut être activée qu'avec :

1. un modèle distribué séparément ;
2. un manifeste signé contenant sa révision, sa licence et son SHA-256 ;
3. un adaptateur testé qui reçoit uniquement des octets bornés ;
4. une évaluation du corpus golden avant activation.

L'absence de modèle est un état normal : l'élément reste logiquement
`TO_REVIEW`.
