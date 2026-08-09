export async function executeTool(
  name: string,
  args: Record<string, string>,
  workspace: string,
): Promise<string> {
  if (!window.supremacy) {
    return 'Erreur : API Electron non disponible (mode web)';
  }

  const api = window.supremacy;
  const cwd = workspace || await api.getHome();

  switch (name) {
    case 'read_file': {
      const allowed = await api.askPermission(
        'Lire un fichier',
        `Supremacy veut lire :\n${args.path}`,
      );
      if (!allowed) return 'Permission refusée par le user.';
      const result = await api.readFile(args.path);
      return result.ok ? result.content ?? '' : `Erreur : ${result.error}`;
    }

    case 'write_file': {
      const allowed = await api.askPermission(
        'Écrire un fichier',
        `Supremacy veut écrire dans :\n${args.path}\n\n(${args.content?.length ?? 0} caractères)`,
      );
      if (!allowed) return 'Permission refusée par le user.';
      const result = await api.writeFile(args.path, args.content);
      return result.ok ? `Fichier écrit : ${args.path}` : `Erreur : ${result.error}`;
    }

    case 'list_directory': {
      const allowed = await api.askPermission(
        'Lister un dossier',
        `Supremacy veut lister :\n${args.path}`,
      );
      if (!allowed) return 'Permission refusée par le user.';
      const result = await api.listDir(args.path);
      if (!result.ok) return `Erreur : ${result.error}`;
      return result.entries?.map((e) => `${e.isDirectory ? '📁' : '📄'} ${e.name}`).join('\n') ?? '';
    }

    case 'run_command': {
      const allowed = await api.askPermission(
        'Exécuter une commande',
        `Supremacy veut exécuter :\n${args.command}\n\nDans : ${args.cwd ?? cwd}`,
      );
      if (!allowed) return 'Permission refusée par le user.';
      const result = await api.execCommand(args.command, args.cwd ?? cwd);
      if (!result.ok) {
        return `Erreur : ${result.error}\n${result.stderr ?? ''}`;
      }
      return result.stdout ?? result.stderr ?? 'Commande exécutée (pas de sortie).';
    }

    default:
      return `Outil inconnu : ${name}`;
  }
}
