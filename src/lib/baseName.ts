/** File name from a path. Handles Unix and Windows separators. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
