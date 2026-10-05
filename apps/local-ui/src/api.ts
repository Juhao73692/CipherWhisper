// Kept in memory only. Reload/lock intentionally requires a fresh unlock.
let bearer = '';
declare global {
  interface Window {
    __cipherwhisperActive?: boolean;
  }
}
export function uiVisible() {
  return !document.hidden && window.__cipherwhisperActive !== false;
}
export function setToken(token: string) {
  bearer = token;
}
export async function api<T>(path: string, data?: unknown, keepalive = false): Promise<T> {
  const response = await fetch(path, {
    method: data === undefined ? 'GET' : 'POST',
    keepalive,
    credentials: 'omit',
    cache: 'no-store',
    headers: {
      Authorization: `Bearer ${bearer}`,
      ...(data === undefined ? {} : { 'Content-Type': 'application/json' }),
    },
    ...(data === undefined ? {} : { body: JSON.stringify(data) }),
  });
  const text = await response.text();
  let result;
  try {
    result = JSON.parse(text);
  } catch {
    throw new Error(response.ok ? '服务响应格式错误' : `请求失败 (${response.status})`);
  }
  if (!response.ok) {
    if (response.status === 401) window.dispatchEvent(new Event('cipherwhisper:locked'));
    throw new Error(result.error || `请求失败 (${response.status})`);
  }
  return result as T;
}

export async function download(path: string, name: string) {
  const response = await fetch(path, {
    headers: { Authorization: `Bearer ${bearer}` },
    credentials: 'omit',
    cache: 'no-store',
  });
  if (!response.ok) throw new Error((await response.json()).error || '下载失败');
  const url = URL.createObjectURL(await response.blob());
  const link = document.createElement('a');
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}
