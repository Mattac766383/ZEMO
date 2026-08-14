/**
 * Parse a Windows path into volume root + drive letter.
 * Used for static regression of GitHub runner paths such as
 * D:\a\_temp\zemo-windows-qualification. Does not probe a live volume
 * and does not assume NTFS.
 */

export function windowsVolumeRoot(windowsPath) {
  if (typeof windowsPath !== "string" || windowsPath.trim() === "") {
    return null;
  }
  const trimmed = windowsPath.trim();
  const drive = trimmed.match(/^([A-Za-z]):[\\/]/);
  if (drive) {
    const driveLetter = drive[1].toUpperCase();
    return {
      volumeRoot: `${driveLetter}:\\`,
      driveLetter,
    };
  }
  const unc = trimmed.match(/^\\\\[^\\/]+\\[^\\/]+/);
  if (unc) {
    return {
      volumeRoot: `${unc[0]}\\`,
      driveLetter: "",
    };
  }
  return null;
}
