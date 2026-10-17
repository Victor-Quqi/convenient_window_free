export function loadHostValue<T>(key: string): T | null {
  try {
    const serialized = localStorage.getItem(key);
    return serialized ? JSON.parse(serialized) as T : null;
  } catch {
    return null;
  }
}

export function saveHostValue<T>(key: string, value: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage may be unavailable; the current session still works.
  }
}
