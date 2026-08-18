import type { OrganizationOperation, OrganizationProposal } from "./types";

export const PREVIEW_CATEGORY_ORDER = [
  "Documents",
  "Images",
  "Vidéos",
  "Archives",
  "Installateurs",
  "À vérifier",
] as const;

export type PreviewCategory = (typeof PREVIEW_CATEGORY_ORDER)[number];

export type CategoryCounts = Record<PreviewCategory, number>;

const EMPTY_COUNTS: CategoryCounts = {
  Documents: 0,
  Images: 0,
  Vidéos: 0,
  Archives: 0,
  Installateurs: 0,
  "À vérifier": 0,
};

export function emptyCategoryCounts(): CategoryCounts {
  return { ...EMPTY_COUNTS };
}

export function categoryForOperation(operation: OrganizationOperation): PreviewCategory | null {
  if (
    operation.operationKind === "KEEP_IN_PLACE" ||
    operation.operationKind === "NO_ACTION"
  ) {
    return null;
  }
  const head = operation.proposedDestination[0] ?? "";
  if (
    head === "Documents" ||
    head === "Travail" ||
    head === "Administratif" ||
    head === "Études" ||
    head === "Etudes" ||
    head === "Personnel"
  ) {
    return "Documents";
  }
  if (
    head === "Images" ||
    head === "Photos" ||
    head === "Captures d’écran" ||
    head === "Captures d'écran" ||
    head === "Images téléchargées"
  ) {
    return "Images";
  }
  if (head === "Vidéos" || head === "Videos") {
    return "Vidéos";
  }
  if (head === "Archives") {
    return "Archives";
  }
  if (head === "Installateurs") {
    return "Installateurs";
  }
  if (head === "À vérifier" || head === "TO_REVIEW") {
    return "À vérifier";
  }
  return "À vérifier";
}

export function countableMove(operation: OrganizationOperation): boolean {
  return (
    operation.operationKind === "MOVE_PROPOSAL" ||
    operation.operationKind === "RENAME_PROPOSAL"
  );
}

export function summarizeProposals(
  proposals: OrganizationProposal[],
): { filesToOrganize: number; counts: CategoryCounts } {
  const counts = emptyCategoryCounts();
  let filesToOrganize = 0;
  for (const proposal of proposals) {
    for (const operation of proposal.operations) {
      if (!countableMove(operation)) {
        continue;
      }
      filesToOrganize += 1;
      const category = categoryForOperation(operation);
      if (category) {
        counts[category] += 1;
      }
    }
  }
  return { filesToOrganize, counts };
}
