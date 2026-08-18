export type ErrorSeverity = "info" | "warning" | "action_required" | "critical";

export type ErrorScope =
  | "global"
  | "search"
  | "semantic"
  | "monitoring"
  | "scan"
  | "organization"
  | "permission"
  | "ocr"
  | "rules"
  | "recovery";

export type UserFacingError = {
  title: string;
  message: string;
  impact: string;
  actionHint: string;
  severity: ErrorSeverity;
  scope: ErrorScope;
  technicalDetails: string | null;
};

function rawText(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  if (typeof error === "string") {
    return error;
  }
  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof (error as { message: unknown }).message === "string"
  ) {
    return (error as { message: string }).message;
  }
  return "";
}

/**
 * Classify a recoverable/local failure for UX.
 * Prefer feature-local warnings over a global catastrophic banner.
 */
export function classifyUserError(
  error: unknown,
  preferredScope: ErrorScope = "global",
): UserFacingError {
  const raw = rawText(error);
  const normalized = raw.toLocaleLowerCase();
  const technicalDetails = raw.trim() ? raw : null;

  if (
    normalized.includes("permission") ||
    normalized.includes("accès") ||
    normalized.includes("accessible") ||
    normalized.includes("eacces") ||
    normalized.includes("access_denied")
  ) {
    return {
      title: "ZEMO n’a pas accès à ce dossier.",
      message: "ZEMO n’a pas accès à ce dossier.",
      impact: "Les autres dossiers déjà analysés restent utilisables.",
      actionHint: "Autoriser l’accès",
      severity: "action_required",
      scope: "permission",
      technicalDetails,
    };
  }

  if (
    preferredScope === "semantic" ||
    normalized.includes("embedding") ||
    normalized.includes("sémantique") ||
    normalized.includes("semantic") ||
    normalized.includes("ann") ||
    normalized.includes("modèle")
  ) {
    return {
      title: "Recherche intelligente indisponible",
      message: "La recherche intelligente est temporairement indisponible.",
      impact: "La recherche classique reste disponible.",
      actionHint: "Réessayez plus tard ou continuez avec la recherche classique.",
      severity: "warning",
      scope: "semantic",
      technicalDetails,
    };
  }

  if (
    preferredScope === "search" ||
    (normalized.includes("search") &&
      (normalized.includes("fail") ||
        normalized.includes("error") ||
        normalized.includes("unavailable")))
  ) {
    return {
      title: "Recherche impossible",
      message: "Impossible d’effectuer cette recherche.",
      impact: "Le reste de l’application reste utilisable.",
      actionHint: "Réessayez dans un instant.",
      severity: "warning",
      scope: "search",
      technicalDetails,
    };
  }

  if (
    preferredScope === "monitoring" ||
    normalized.includes("surveillance") ||
    normalized.includes("watcher") ||
    normalized.includes("monitoring")
  ) {
    return {
      title: "Surveillance interrompue",
      message: "La surveillance d’un dossier s’est interrompue.",
      impact: "L’analyse manuelle reste disponible.",
      actionHint: "Réessayez depuis Surveillance.",
      severity: "warning",
      scope: "monitoring",
      technicalDetails,
    };
  }

  if (
    preferredScope === "ocr" ||
    normalized.includes("ocr") ||
    normalized.includes("tesseract") ||
    normalized.includes("pdftoppm")
  ) {
    return {
      title: "Lecture partielle",
      message: "Certains documents scannés n’ont pas pu être lus complètement.",
      impact: "Le reste de l’analyse continue normalement.",
      actionHint: "Consultez les fichiers concernés dans À revoir.",
      severity: "warning",
      scope: "ocr",
      technicalDetails,
    };
  }

  if (
    normalized.includes("changed") ||
    normalized.includes("changé") ||
    normalized.includes("source drift") ||
    normalized.includes("stale")
  ) {
    return {
      title: "Fichier modifié",
      message: "Ce fichier a changé depuis l’aperçu. ZEMO l’a laissé en place.",
      impact: "Le reste du rangement peut continuer.",
      actionHint: "Relancez un aperçu si vous voulez réessayer ce fichier.",
      severity: "warning",
      scope: "organization",
      technicalDetails,
    };
  }

  if (
    normalized.includes("placeholder") ||
    normalized.includes("cloud") ||
    normalized.includes("onedrive") ||
    normalized.includes("icloud") ||
    normalized.includes("not locally available") ||
    normalized.includes("pas disponible localement") ||
    normalized.includes("temporarily_unavailable")
  ) {
    return {
      title: "Fichier distant",
      message: "Ce fichier n’est pas disponible localement.",
      impact: "ZEMO l’a laissé en place. Rien n’a été téléchargé.",
      actionHint: "Rendez le fichier disponible sur cet ordinateur, puis réessayez.",
      severity: "warning",
      scope: "organization",
      technicalDetails,
    };
  }

  if (
    normalized.includes("locked") ||
    normalized.includes("in use") ||
    normalized.includes("utilisé") ||
    normalized.includes("sharing violation")
  ) {
    return {
      title: "Fichier utilisé",
      message:
        "Ce fichier est utilisé par une autre application. ZEMO l’a laissé en place.",
      impact: "Les autres fichiers peuvent être rangés normalement.",
      actionHint: "Fermez l’application qui utilise le fichier, puis réessayez.",
      severity: "warning",
      scope: "organization",
      technicalDetails,
    };
  }

  if (
    preferredScope === "organization" ||
    normalized.includes("proposition") ||
    normalized.includes("organization") ||
    normalized.includes("proposal")
  ) {
    return {
      title: "Organisation indisponible",
      message: "Impossible de préparer l’organisation proposée pour le moment.",
      impact: "La recherche et l’analyse restent disponibles.",
      actionHint: "Réessayez depuis Organisation.",
      severity: "warning",
      scope: "organization",
      technicalDetails,
    };
  }

  if (
    preferredScope === "scan" ||
    normalized.includes("scan") ||
    normalized.includes("analyse")
  ) {
    return {
      title: "Analyse interrompue",
      message: "L’analyse n’a pas pu se terminer correctement.",
      impact: "Les fichiers déjà analysés restent consultables.",
      actionHint: "Réessayez l’analyse.",
      severity: "action_required",
      scope: "scan",
      technicalDetails,
    };
  }

  if (
    normalized.includes("journal") ||
    normalized.includes("récupération") ||
    normalized.includes("recovery") ||
    normalized.includes("corrupt")
  ) {
    return {
      title: "Attention requise",
      message: "Une opération précédente nécessite votre attention.",
      impact: "Les modifications de fichiers restent bloquées jusqu’à examen.",
      actionHint: "Ouvrez Options avancées → Récupération.",
      severity: "critical",
      scope: "recovery",
      technicalDetails,
    };
  }

  if (
    normalized.includes("moteur local") ||
    normalized.includes("interrompu de façon inattendue") ||
    normalized.includes("database") ||
    normalized.includes("sqlcipher") ||
    normalized.includes("persistence")
  ) {
    // Generic engine collapse is treated as warning unless clearly catastrophic.
    return {
      title: "Action temporairement indisponible",
      message: "Cette action n’a pas pu être terminée pour le moment.",
      impact: "Le reste de l’application reste utilisable.",
      actionHint: "Réessayez. Si le problème continue, rouvrez l’application.",
      severity: preferredScope === "global" ? "action_required" : "warning",
      scope: preferredScope === "global" ? "global" : preferredScope,
      technicalDetails,
    };
  }

  return {
    title: "ZEMO n’a pas pu ranger certains fichiers.",
    message: "ZEMO n’a pas pu ranger certains fichiers.",
    impact: "Aucun fichier n’a été écrasé.",
    actionHint: "Voir les fichiers concernés",
    severity: preferredScope === "global" ? "action_required" : "warning",
    scope: preferredScope,
    technicalDetails,
  };
}

export function isGlobalCritical(error: UserFacingError): boolean {
  return error.severity === "critical" && error.scope === "recovery";
}

/** Global banner only for app-wide / action-required failures — not optional subsystem warnings. */
export function shouldShowGlobalBanner(error: UserFacingError): boolean {
  if (error.severity === "critical") {
    return true;
  }
  if (error.severity === "action_required") {
    return (
      error.scope === "global" ||
      error.scope === "scan" ||
      error.scope === "permission" ||
      error.scope === "recovery"
    );
  }
  // Recoverable feature warnings stay local to their screens.
  return false;
}
