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

export type FolderTreeNode = {
  name: string;
  count: number;
  children: FolderTreeNode[];
};

type MutableFolderTreeNode = {
  name: string;
  count: number;
  children: Map<string, MutableFolderTreeNode>;
};

export type CategoryCounts = Record<PreviewCategory, number> & {
  filesAnalyzed?: number;
  folderTree?: FolderTreeNode[];
};

const EMPTY_COUNTS: CategoryCounts = {
  Documents: 0,
  Images: 0,
  Vidéos: 0,
  Archives: 0,
  Installateurs: 0,
  "À vérifier": 0,
  filesAnalyzed: 0,
  folderTree: [],
};

export function emptyCategoryCounts(): CategoryCounts {
  return { ...EMPTY_COUNTS, folderTree: [] };
}

function normalizeFolderName(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed || trimmed === "." || trimmed === "..") {
    return null;
  }
  if (trimmed === "TO_REVIEW") {
    return "À vérifier";
  }
  return trimmed;
}

function userFacingFolderPath(operation: OrganizationOperation): string[] {
  const folders = operation.proposedDestination
    .map(normalizeFolderName)
    .filter((part): part is string => Boolean(part));
  return folders.length > 0 ? folders : ["À vérifier"];
}

function addFolderPath(
  roots: Map<string, MutableFolderTreeNode>,
  path: string[],
): void {
  let level = roots;
  for (const name of path) {
    let node = level.get(name);
    if (!node) {
      node = { name, count: 0, children: new Map() };
      level.set(name, node);
    }
    node.count += 1;
    level = node.children;
  }
}

function rootPriority(name: string): number {
  const index = PREVIEW_CATEGORY_ORDER.indexOf(name as PreviewCategory);
  return index === -1 ? PREVIEW_CATEGORY_ORDER.length - 1 : index;
}

function finalizeFolderTree(
  nodes: Map<string, MutableFolderTreeNode>,
  depth = 0,
): FolderTreeNode[] {
  return [...nodes.values()]
    .sort((left, right) => {
      if (depth === 0) {
        const priority = rootPriority(left.name) - rootPriority(right.name);
        if (priority !== 0) {
          return priority;
        }
      }
      return left.name.localeCompare(right.name, "fr", { sensitivity: "base" });
    })
    .map((node) => ({
      name: node.name,
      count: node.count,
      children: finalizeFolderTree(node.children, depth + 1),
    }));
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
  const treeRoots = new Map<string, MutableFolderTreeNode>();
  let filesToOrganize = 0;
  let filesAnalyzed = 0;

  for (const proposal of proposals) {
    filesAnalyzed += proposal.summary.filesAnalyzed;
    for (const operation of proposal.operations) {
      if (!countableMove(operation)) {
        continue;
      }
      filesToOrganize += 1;
      const category = categoryForOperation(operation);
      if (category) {
        counts[category] += 1;
      }
      addFolderPath(treeRoots, userFacingFolderPath(operation));
    }
  }

  counts.filesAnalyzed = filesAnalyzed;
  counts.folderTree = finalizeFolderTree(treeRoots);
  return { filesToOrganize, counts };
}
