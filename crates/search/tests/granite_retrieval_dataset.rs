//! Labeled semantic retrieval evaluation dataset — Supremacy M9.1 Step 3.
//! Designed to be hard for pure lexical overlap; embeddings should matter.

pub struct EvalDoc {
    pub id: &'static str,
    pub text: &'static str,
}

pub struct EvalQuery {
    pub text: &'static str,
    pub relevant: &'static [&'static str],
    pub hard_negatives: &'static [&'static str],
}

pub fn docs() -> Vec<EvalDoc> {
    vec![
        // --- Roofing / toiture (Dupont chantier) ---
        EvalDoc {
            id: "scan_2023_04_12_001",
            text: "Devis réfection couverture zinc et tuiles plates — client Dupont SARL, \
                   chantier 14 rue des Lilas. Remplacement liteaux, écran HPV, \
                   zinguerie gouttières. Montant HT 18 450 €. Validité 60 jours.",
        },
        EvalDoc {
            id: "IMG_8841",
            text: "Compte-rendu intervention toiture Dupont : fuite au faîtage côté nord, \
                   reprise solins cheminée, contrôle étanchéité velux. \
                   Matériaux fournis Point P. Fin des travaux prévue vendredi.",
        },
        EvalDoc {
            id: "doc_tmp_roof_en",
            text: "Roof estimate for Dupont Ltd — strip and re-tile south pitch, \
                   replace damaged underlayment, install ridge ventilation. \
                   Labour and materials £12,900. Site access via rear alley.",
        },
        // --- Dupont CV / identity (hard negative for roofing) ---
        EvalDoc {
            id: "CV_Dupont_v3_final",
            text: "Jean Dupont — curriculum vitae. Développeur full-stack (Rust, React, TypeScript). \
                   Expérience : startup fintech 2019–2024, open-source embeddings. \
                   Recherche poste senior backend Île-de-France. Contact : j.dupont@example.com",
        },
        EvalDoc {
            id: "lettre_motiv_dupont",
            text: "Madame, Monsieur, je suis Jean Dupont, ingénieur logiciel. \
                   Motivé par votre offre d'architecte cloud. \
                   Pièce jointe : CV détaillé et références GitHub.",
        },
        // --- Point P: invoice vs marketing brochure ---
        EvalDoc {
            id: "FAC_PP_78421",
            text: "Facture Point P n°78421 — client Dupont SARL. \
                   Tuiles terre cuite 24×36, liteaux, crochets, membrane HPV. \
                   Total TTC 1 437,82 €. Règlement 30 jours. Livraison chantier Lilas.",
        },
        EvalDoc {
            id: "brochure_pointp_print",
            text: "Catalogue Point P printemps : promotions tuiles, isolants, \
                   et outillage pro. Offre fidélité +10% sur la gamme couverture. \
                   Magasins partenaires — sans montant ni numéro de commande.",
        },
        EvalDoc {
            id: "BL_pointp_884",
            text: "Bon de livraison Point P — palette tuiles et liteaux déposée \
                   rue des Lilas pour chantier Dupont. Signature réception OK. \
                   Réf. commande liée facture 78421.",
        },
        // --- Other suppliers / DIY (confusion with Point P) ---
        EvalDoc {
            id: "ticket_lm_jardin",
            text: "Ticket Leroy Merlin — sécateur, terreau, pots céramique. \
                   Total 89,90 €. Magasin Brétigny. Achat particulier jardinage.",
        },
        EvalDoc {
            id: "FAC_acme_en",
            text: "Invoice ACME Building Supplies — OSB sheets, membrane, ridge caps. \
                   Order for Dupont Ltd site. Total 1 437.82 EUR VAT included. \
                   Payment terms net 30.",
        },
        EvalDoc {
            id: "devis_bigmat_charpente",
            text: "Devis BigMat — fermettes et sablières pour extension garage Martin. \
                   Hors chantier Dupont. Montant 4 200 € HT. Non lié à Point P.",
        },
        // --- Insurance habitation ---
        EvalDoc {
            id: "contrat_assur_hab_2022",
            text: "Contrat multirisque habitation — résidence principale 14 rue des Lilas. \
                   Garanties incendie, dégâts des eaux, responsabilité civile. \
                   Franchise 250 €. Assureur MAIF, échéance annuelle mars.",
        },
        EvalDoc {
            id: "sinistre_fuite_toiture",
            text: "Déclaration de sinistre : infiltrations après orage, plafond chambre nord. \
                   Lié dommages toiture. Photos jointes. Ouverture dossier assurance habitation.",
        },
        EvalDoc {
            id: "attestation_rc_pro_dupont",
            text: "Attestation RC professionnelle Dupont SARL — activité couverture-zinguerie. \
                   Validité calendaire. Document entreprise, pas le contrat maison privée.",
        },
        // --- Tax / impôts ---
        EvalDoc {
            id: "avis_impot_2023",
            text: "Avis d'impôt sur le revenu 2023 — foyer fiscal personnel. \
                   Revenus déclarés salaires + revenus fonciers. \
                   Solde à payer 1 120 €. Référence fiscale non professionnelle.",
        },
        EvalDoc {
            id: "liasse_tva_sarl",
            text: "Déclaration TVA CA3 — Dupont SARL, régime réel. \
                   Opérations imposables trimestre, crédit de TVA matériaux Point P. \
                   Document comptable société.",
        },
        EvalDoc {
            id: "cerfa_2042_notes",
            text: "Brouillon notes pour déclaration 2042 : dons associations, \
                   frais réels domicile-travail, crédits d'impôt transition énergétique \
                   (isolation combles). Usage personnel.",
        },
        // --- Other projects / clients (hard negatives vs Dupont roof) ---
        EvalDoc {
            id: "chantier_martin_extension",
            text: "Planning chantier Martin — extension garage ossature bois, \
                   hors toiture Dupont. Suivi béton, charpente BigMat, \
                   menuiseries PVC. Chef de chantier : Amélie R.",
        },
        EvalDoc {
            id: "devis_bernard_facade",
            text: "Devis ravalement façade Bernard SAS — enduit monocouche, \
                   échafaudage 3 semaines. Aucune intervention couverture. \
                   Adresse 8 avenue Victor Hugo.",
        },
        EvalDoc {
            id: "pv_reception_dupont_plomberie",
            text: "PV de réception travaux plomberie cuisine Dupont SARL bureaux. \
                   Remplacement chauffe-eau et réseaux EU. Pas de toiture. \
                   Réserves mineures joints.",
        },
        // --- Personal admin / bank / medical ---
        EvalDoc {
            id: "releve_bnp_perso",
            text: "Relevé bancaire personnel BNP — loyers, courses, prélèvement assurance MAIF. \
                   Pas de facture fournisseur chantier. Solde fin de mois.",
        },
        EvalDoc {
            id: "ordo_dr_lemoine",
            text: "Ordonnance Dr Lemoine — paracétamol, repos. Patient Jean Dupont. \
                   Document médical personnel, hors activité couverture.",
        },
        EvalDoc {
            id: "bail_colocation_2019",
            text: "Bail de colocation 2019 — chambre meublée Paris 11e. \
                   Locataire J. Dupont. Caution et état des lieux. \
                   Archive personnelle ancienne.",
        },
        // --- English business / HR / legal ---
        EvalDoc {
            id: "nda_acme_cloud",
            text: "Mutual NDA between ACME Cloud and Jean Dupont (individual contractor). \
                   Confidentiality of source code and pricing. Software services only — \
                   no construction or roofing scope.",
        },
        EvalDoc {
            id: "offer_letter_dupont_en",
            text: "Offer letter — Senior Software Engineer role for Jean Dupont. \
                   Base salary, RSU grant, remote-friendly. Start date October. \
                   HR document unrelated to building works.",
        },
        EvalDoc {
            id: "roof_warranty_en",
            text: "Manufacturer warranty certificate — clay tiles 30-year weatherproofing. \
                   Installed at Dupont Ltd Lilas site per estimate. \
                   Keep with property insurance documents.",
        },
        EvalDoc {
            id: "safety_toolbox_en",
            text: "Toolbox talk notes — fall protection on pitched roofs, \
                   harness inspection checklist, scaffold tagging. \
                   Generic HSE training, no client name Dupont.",
        },
        // --- Energy / isolation / related but distinct ---
        EvalDoc {
            id: "devis_isolation_combles",
            text: "Devis isolation combles perdus laine de verre R=7 — residence Lilas. \
                   Accès trappe, pare-vapeur. Travaux thermiques distincts de la \
                   réfection de couverture zinc/tuiles Dupont.",
        },
        EvalDoc {
            id: "dpe_logement_lilas",
            text: "Diagnostic de performance énergétique — appartement/maison Lilas. \
                   Classe C, recommandations isolation et chaudière. \
                   Pas de détail devis toiture.",
        },
        // --- Misc French business ---
        EvalDoc {
            id: "kbis_dupont_sarl",
            text: "Extrait Kbis Dupont SARL — objet social couverture, zinguerie, \
                   étanchéité. Siège social. Document juridique société.",
        },
        EvalDoc {
            id: "relance_impaye_bernard",
            text: "Lettre de relance facture impayée — Bernard SAS ravalement façade. \
                   Échéance dépassée 45 jours. Aucun lien Point P ni toiture Dupont.",
        },
        EvalDoc {
            id: "photo_meta_chantier",
            text: "Métadonnées photo smartphone : GPS rue des Lilas, légende \
                   « avant travaux couverture ». Image floue tuiles cassées. \
                   Contexte chantier Dupont toiture.",
        },
        EvalDoc {
            id: "email_fwd_devis_toiture",
            text: "Transfert mail : « ci-joint le devis couverture pour Dupont ». \
                   Corps : merci de valider zinc vs tuile. PJ absente du scan texte. \
                   Concerne réfection toiture.",
        },
        EvalDoc {
            id: "note_comptable_fournitures",
            text: "Écriture comptable 601 — achats matières Point P 1 437,82 € TTC \
                   imputés chantier couverture Dupont. Journal des achats.",
        },
        EvalDoc {
            id: "contrat_entretien_chaudiere",
            text: "Contrat entretien chaudière gaz annuel — technicien agréé. \
                   Visite ramonage et contrôle CO. Habitat Lilas, hors toiture.",
        },
        EvalDoc {
            id: "quote_scaffold_en",
            text: "Scaffolding hire quote — 3-week perimeter scaffold for facade works \
                   at Bernard site Victor Hugo. Not the Dupont roof project. \
                   Total £2,100.",
        },
    ]
}

pub fn queries() -> Vec<EvalQuery> {
    vec![
        // ===== French business — roofing Dupont (paraphrases, low lexical overlap) =====
        EvalQuery {
            text: "devis pour refaire le toit chez le client Dupont",
            relevant: &[
                "scan_2023_04_12_001",
                "email_fwd_devis_toiture",
                "doc_tmp_roof_en",
            ],
            hard_negatives: &[
                "CV_Dupont_v3_final",
                "lettre_motiv_dupont",
                "pv_reception_dupont_plomberie",
            ],
        },
        EvalQuery {
            text: "fuite au faîte et reprise des solins cheminée",
            relevant: &["IMG_8841", "sinistre_fuite_toiture"],
            hard_negatives: &["contrat_entretien_chaudiere", "devis_bernard_facade"],
        },
        EvalQuery {
            text: "combien coûtait la réfection de couverture zinc tuiles plates",
            relevant: &["scan_2023_04_12_001"],
            hard_negatives: &[
                "devis_isolation_combles",
                "devis_bigmat_charpente",
                "FAC_PP_78421",
            ],
        },
        EvalQuery {
            text: "compte rendu d'intervention couverture rue des Lilas",
            relevant: &["IMG_8841", "photo_meta_chantier"],
            hard_negatives: &["chantier_martin_extension", "bail_colocation_2019"],
        },
        EvalQuery {
            text: "validation du choix entre zinc et tuile pour Dupont",
            relevant: &["email_fwd_devis_toiture", "scan_2023_04_12_001"],
            hard_negatives: &["CV_Dupont_v3_final", "offer_letter_dupont_en"],
        },
        EvalQuery {
            text: "photo avant travaux avec tuiles abîmées",
            relevant: &["photo_meta_chantier"],
            hard_negatives: &["brochure_pointp_print", "dpe_logement_lilas"],
        },
        EvalQuery {
            text: "garantie fabricant tuiles terre cuite chantier Lilas",
            relevant: &["roof_warranty_en"],
            hard_negatives: &["brochure_pointp_print", "attestation_rc_pro_dupont"],
        },
        EvalQuery {
            text: "écriture comptable matériaux couverture Point P",
            relevant: &["note_comptable_fournitures", "FAC_PP_78421"],
            hard_negatives: &[
                "liasse_tva_sarl",
                "brochure_pointp_print",
                "ticket_lm_jardin",
            ],
        },
        // ===== Hard: Dupont name collision (CV vs toiture) =====
        EvalQuery {
            text: "profil développeur Rust React de Jean Dupont",
            relevant: &["CV_Dupont_v3_final", "lettre_motiv_dupont"],
            hard_negatives: &["scan_2023_04_12_001", "IMG_8841", "kbis_dupont_sarl"],
        },
        EvalQuery {
            text: "candidature poste backend fintech pour Dupont",
            relevant: &[
                "CV_Dupont_v3_final",
                "lettre_motiv_dupont",
                "offer_letter_dupont_en",
            ],
            hard_negatives: &["doc_tmp_roof_en", "attestation_rc_pro_dupont"],
        },
        EvalQuery {
            text: "lettre d'embauche software engineer Jean Dupont",
            relevant: &["offer_letter_dupont_en"],
            hard_negatives: &["nda_acme_cloud", "scan_2023_04_12_001"],
        },
        EvalQuery {
            text: "accord de confidentialité prestataire logiciel ACME",
            relevant: &["nda_acme_cloud"],
            hard_negatives: &["FAC_acme_en", "offer_letter_dupont_en"],
        },
        EvalQuery {
            text: "extrait société couverture zinguerie Dupont",
            relevant: &["kbis_dupont_sarl", "attestation_rc_pro_dupont"],
            hard_negatives: &["CV_Dupont_v3_final", "avis_impot_2023"],
        },
        EvalQuery {
            text: "travaux plomberie cuisine bureaux Dupont",
            relevant: &["pv_reception_dupont_plomberie"],
            hard_negatives: &["IMG_8841", "scan_2023_04_12_001", "sinistre_fuite_toiture"],
        },
        // ===== Point P invoice vs brochure / other suppliers =====
        EvalQuery {
            text: "facture matériaux construction environ quatorze cents euros Point P",
            relevant: &[
                "FAC_PP_78421",
                "note_comptable_fournitures",
                "BL_pointp_884",
            ],
            hard_negatives: &["brochure_pointp_print", "ticket_lm_jardin", "FAC_acme_en"],
        },
        EvalQuery {
            text: "catalogue promo tuiles et isolants magasin pro",
            relevant: &["brochure_pointp_print"],
            hard_negatives: &[
                "FAC_PP_78421",
                "BL_pointp_884",
                "note_comptable_fournitures",
            ],
        },
        EvalQuery {
            text: "bon de livraison palette tuiles rue des Lilas",
            relevant: &["BL_pointp_884"],
            hard_negatives: &[
                "brochure_pointp_print",
                "ticket_lm_jardin",
                "devis_bigmat_charpente",
            ],
        },
        EvalQuery {
            text: "ticket caisse outillage jardin magasin bricolage",
            relevant: &["ticket_lm_jardin"],
            hard_negatives: &["FAC_PP_78421", "brochure_pointp_print", "FAC_acme_en"],
        },
        EvalQuery {
            text: "invoice building supplies same amount as French merchant bill",
            relevant: &["FAC_acme_en", "FAC_PP_78421"],
            hard_negatives: &[
                "brochure_pointp_print",
                "ticket_lm_jardin",
                "nda_acme_cloud",
            ],
        },
        EvalQuery {
            text: "devis fermettes extension garage autre client",
            relevant: &["devis_bigmat_charpente", "chantier_martin_extension"],
            hard_negatives: &["scan_2023_04_12_001", "FAC_PP_78421"],
        },
        // ===== Insurance / sinistre =====
        EvalQuery {
            text: "contrat qui couvre incendie et dégâts des eaux domicile",
            relevant: &["contrat_assur_hab_2022"],
            hard_negatives: &[
                "attestation_rc_pro_dupont",
                "roof_warranty_en",
                "releve_bnp_perso",
            ],
        },
        EvalQuery {
            text: "ouvrir un dossier après infiltrations plafond chambre",
            relevant: &["sinistre_fuite_toiture", "contrat_assur_hab_2022"],
            hard_negatives: &["IMG_8841", "contrat_entretien_chaudiere"],
        },
        EvalQuery {
            text: "attestation responsabilité civile entreprise couvreur",
            relevant: &["attestation_rc_pro_dupont"],
            hard_negatives: &["contrat_assur_hab_2022", "CV_Dupont_v3_final"],
        },
        EvalQuery {
            text: "prélèvement assureur sur le compte perso",
            relevant: &["releve_bnp_perso", "contrat_assur_hab_2022"],
            hard_negatives: &["FAC_PP_78421", "liasse_tva_sarl"],
        },
        EvalQuery {
            text: "certificate for clay tile weatherproofing warranty",
            relevant: &["roof_warranty_en"],
            hard_negatives: &[
                "contrat_assur_hab_2022",
                "attestation_rc_pro_dupont",
                "safety_toolbox_en",
            ],
        },
        // ===== Tax / impôts (personal vs company) =====
        EvalQuery {
            text: "avis d'imposition revenus du foyer année dernière",
            relevant: &["avis_impot_2023", "cerfa_2042_notes"],
            hard_negatives: &["liasse_tva_sarl", "note_comptable_fournitures"],
        },
        EvalQuery {
            text: "préparer la 2042 avec crédit d'impôt isolation",
            relevant: &["cerfa_2042_notes", "devis_isolation_combles"],
            hard_negatives: &["scan_2023_04_12_001", "liasse_tva_sarl"],
        },
        EvalQuery {
            text: "CA3 TVA crédit sur achats matériaux chantier",
            relevant: &["liasse_tva_sarl", "note_comptable_fournitures"],
            hard_negatives: &["avis_impot_2023", "cerfa_2042_notes"],
        },
        EvalQuery {
            text: "solde impôts personnels à régler environ mille euros",
            relevant: &["avis_impot_2023"],
            hard_negatives: &["FAC_PP_78421", "relance_impaye_bernard"],
        },
        // ===== Other projects / suppliers confusion =====
        EvalQuery {
            text: "suivi ossature bois garage chez Martin",
            relevant: &["chantier_martin_extension", "devis_bigmat_charpente"],
            hard_negatives: &["scan_2023_04_12_001", "IMG_8841", "doc_tmp_roof_en"],
        },
        EvalQuery {
            text: "ravalement enduit monocouche avenue Victor Hugo",
            relevant: &[
                "devis_bernard_facade",
                "relance_impaye_bernard",
                "quote_scaffold_en",
            ],
            hard_negatives: &["scan_2023_04_12_001", "IMG_8841"],
        },
        EvalQuery {
            text: "relance paiement façade Bernard en retard",
            relevant: &["relance_impaye_bernard"],
            hard_negatives: &[
                "FAC_PP_78421",
                "avis_impot_2023",
                "note_comptable_fournitures",
            ],
        },
        EvalQuery {
            text: "scaffolding hire quote for facade not roofing",
            relevant: &["quote_scaffold_en", "devis_bernard_facade"],
            hard_negatives: &[
                "doc_tmp_roof_en",
                "safety_toolbox_en",
                "scan_2023_04_12_001",
            ],
        },
        EvalQuery {
            text: "formation sécurité harnais toitures inclinées générique",
            relevant: &["safety_toolbox_en"],
            hard_negatives: &["IMG_8841", "doc_tmp_roof_en", "attestation_rc_pro_dupont"],
        },
        // ===== Energy / DPE / isolation (near neighbors) =====
        EvalQuery {
            text: "devis laine de verre combles perdus Lilas",
            relevant: &["devis_isolation_combles"],
            hard_negatives: &[
                "scan_2023_04_12_001",
                "dpe_logement_lilas",
                "cerfa_2042_notes",
            ],
        },
        EvalQuery {
            text: "classe énergétique du logement et pistes d'amélioration",
            relevant: &["dpe_logement_lilas"],
            hard_negatives: &["devis_isolation_combles", "contrat_assur_hab_2022"],
        },
        EvalQuery {
            text: "contrat ramonage chaudière gaz annuel",
            relevant: &["contrat_entretien_chaudiere"],
            hard_negatives: &["sinistre_fuite_toiture", "contrat_assur_hab_2022"],
        },
        // ===== French personal =====
        EvalQuery {
            text: "ordonnance paracétamol médecin traitant",
            relevant: &["ordo_dr_lemoine"],
            hard_negatives: &["CV_Dupont_v3_final", "avis_impot_2023"],
        },
        EvalQuery {
            text: "ancien bail chambre meublée Paris onzième",
            relevant: &["bail_colocation_2019"],
            hard_negatives: &["contrat_assur_hab_2022", "kbis_dupont_sarl"],
        },
        EvalQuery {
            text: "relevé de compte avec loyer et courses du mois",
            relevant: &["releve_bnp_perso"],
            hard_negatives: &[
                "FAC_PP_78421",
                "liasse_tva_sarl",
                "note_comptable_fournitures",
            ],
        },
        // ===== English queries =====
        EvalQuery {
            text: "strip and re-tile south pitch estimate Dupont",
            relevant: &["doc_tmp_roof_en", "scan_2023_04_12_001"],
            hard_negatives: &[
                "CV_Dupont_v3_final",
                "quote_scaffold_en",
                "safety_toolbox_en",
            ],
        },
        EvalQuery {
            text: "building materials invoice around fourteen hundred euros VAT included",
            relevant: &["FAC_acme_en", "FAC_PP_78421"],
            hard_negatives: &[
                "brochure_pointp_print",
                "ticket_lm_jardin",
                "nda_acme_cloud",
            ],
        },
        EvalQuery {
            text: "senior software engineer offer with RSU grant",
            relevant: &["offer_letter_dupont_en"],
            hard_negatives: &["CV_Dupont_v3_final", "nda_acme_cloud", "doc_tmp_roof_en"],
        },
        EvalQuery {
            text: "mutual nondisclosure for cloud contractor work",
            relevant: &["nda_acme_cloud"],
            hard_negatives: &["FAC_acme_en", "offer_letter_dupont_en"],
        },
        EvalQuery {
            text: "fall protection checklist for pitched roof work",
            relevant: &["safety_toolbox_en"],
            hard_negatives: &["doc_tmp_roof_en", "IMG_8841", "roof_warranty_en"],
        },
        EvalQuery {
            text: "keep tile manufacturer warranty with home insurance papers",
            relevant: &["roof_warranty_en", "contrat_assur_hab_2022"],
            hard_negatives: &["brochure_pointp_print", "attestation_rc_pro_dupont"],
        },
        // ===== Cross-language (FR query → EN docs / EN query → FR docs) =====
        EvalQuery {
            text: "estimation anglaise réfection toiture société Dupont",
            relevant: &["doc_tmp_roof_en", "scan_2023_04_12_001"],
            hard_negatives: &["offer_letter_dupont_en", "CV_Dupont_v3_final"],
        },
        EvalQuery {
            text: "English invoice for membrane and ridge caps same total as Point P",
            relevant: &["FAC_acme_en"],
            hard_negatives: &["FAC_PP_78421", "brochure_pointp_print", "nda_acme_cloud"],
        },
        EvalQuery {
            text: "where is the French supplier bill for tiles delivered to Lilas",
            relevant: &[
                "FAC_PP_78421",
                "BL_pointp_884",
                "note_comptable_fournitures",
            ],
            hard_negatives: &["FAC_acme_en", "brochure_pointp_print", "ticket_lm_jardin"],
        },
        EvalQuery {
            text: "home multi-risk policy with 250 euro deductible",
            relevant: &["contrat_assur_hab_2022"],
            hard_negatives: &["attestation_rc_pro_dupont", "roof_warranty_en"],
        },
        EvalQuery {
            text: "personal income tax notice not corporate VAT return",
            relevant: &["avis_impot_2023", "cerfa_2042_notes"],
            hard_negatives: &["liasse_tva_sarl", "kbis_dupont_sarl"],
        },
        EvalQuery {
            text: "job application cover letter from Jean Dupont engineer",
            relevant: &["lettre_motiv_dupont", "CV_Dupont_v3_final"],
            hard_negatives: &["email_fwd_devis_toiture", "attestation_rc_pro_dupont"],
        },
        EvalQuery {
            text: "storm water damage claim linked to roof leaks",
            relevant: &["sinistre_fuite_toiture"],
            hard_negatives: &[
                "IMG_8841",
                "contrat_entretien_chaudiere",
                "safety_toolbox_en",
            ],
        },
        // ===== Synonym / paraphrase stress (minimal keyword overlap) =====
        EvalQuery {
            text: "combien a coûté l'achat négoce matériaux pour le chantier Lilas",
            relevant: &["FAC_PP_78421", "note_comptable_fournitures"],
            hard_negatives: &[
                "brochure_pointp_print",
                "scan_2023_04_12_001",
                "ticket_lm_jardin",
            ],
        },
        EvalQuery {
            text: "document prouvant la réception des palettes sur site",
            relevant: &["BL_pointp_884"],
            hard_negatives: &[
                "FAC_PP_78421",
                "pv_reception_dupont_plomberie",
                "photo_meta_chantier",
            ],
        },
        EvalQuery {
            text: "flyer commercial sans prix de commande ni client",
            relevant: &["brochure_pointp_print"],
            hard_negatives: &["FAC_PP_78421", "BL_pointp_884", "FAC_acme_en"],
        },
        EvalQuery {
            text: "qui a proposé un prix pour changer la couverture du bâtiment",
            relevant: &[
                "scan_2023_04_12_001",
                "email_fwd_devis_toiture",
                "doc_tmp_roof_en",
            ],
            hard_negatives: &[
                "devis_isolation_combles",
                "devis_bernard_facade",
                "CV_Dupont_v3_final",
            ],
        },
        EvalQuery {
            text: "preuve d'immatriculation société métier étanchéité",
            relevant: &["kbis_dupont_sarl"],
            hard_negatives: &[
                "CV_Dupont_v3_final",
                "contrat_assur_hab_2022",
                "avis_impot_2023",
            ],
        },
        EvalQuery {
            text: "notes pour déclarer les dons et le trajet domicile travail",
            relevant: &["cerfa_2042_notes"],
            hard_negatives: &["avis_impot_2023", "releve_bnp_perso", "liasse_tva_sarl"],
        },
        EvalQuery {
            text: "planning béton et menuiseries PVC autre adresse que Lilas",
            relevant: &["chantier_martin_extension"],
            hard_negatives: &["IMG_8841", "scan_2023_04_12_001", "devis_bernard_facade"],
        },
        EvalQuery {
            text: "find the resume mentioning open-source embeddings experience",
            relevant: &["CV_Dupont_v3_final"],
            hard_negatives: &[
                "nda_acme_cloud",
                "offer_letter_dupont_en",
                "doc_tmp_roof_en",
            ],
        },
        EvalQuery {
            text: "medical prescription for rest not a worksite report",
            relevant: &["ordo_dr_lemoine"],
            hard_negatives: &["IMG_8841", "safety_toolbox_en", "sinistre_fuite_toiture"],
        },
        EvalQuery {
            text: "archive locative colocation avant l'achat immobilier",
            relevant: &["bail_colocation_2019"],
            hard_negatives: &["contrat_assur_hab_2022", "dpe_logement_lilas"],
        },
        EvalQuery {
            text: "mail interne demandant validation du devis couverture",
            relevant: &["email_fwd_devis_toiture"],
            hard_negatives: &[
                "lettre_motiv_dupont",
                "relance_impaye_bernard",
                "nda_acme_cloud",
            ],
        },
        EvalQuery {
            text: "diagnostic perf énergétique recommandations chaudière",
            relevant: &["dpe_logement_lilas"],
            hard_negatives: &[
                "contrat_entretien_chaudiere",
                "devis_isolation_combles",
                "cerfa_2042_notes",
            ],
        },
        EvalQuery {
            text: "achat terreau et sécateur ticket magasin grand public",
            relevant: &["ticket_lm_jardin"],
            hard_negatives: &[
                "FAC_PP_78421",
                "brochure_pointp_print",
                "devis_bigmat_charpente",
            ],
        },
        EvalQuery {
            text: "RC pro couvreur calendaire versus multirisque maison",
            relevant: &["attestation_rc_pro_dupont"],
            hard_negatives: &[
                "contrat_assur_hab_2022",
                "roof_warranty_en",
                "kbis_dupont_sarl",
            ],
        },
        EvalQuery {
            text: "labour and materials cost to re-cover south pitch in pounds",
            relevant: &["doc_tmp_roof_en"],
            hard_negatives: &["scan_2023_04_12_001", "quote_scaffold_en", "FAC_acme_en"],
        },
        EvalQuery {
            text: "imputation journal des achats chantier couverture",
            relevant: &["note_comptable_fournitures", "liasse_tva_sarl"],
            hard_negatives: &[
                "avis_impot_2023",
                "releve_bnp_perso",
                "brochure_pointp_print",
            ],
        },
    ]
}
