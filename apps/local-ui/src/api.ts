// Kept in memory only. Reload/lock intentionally requires a fresh unlock.
let bearer = '';
export function setToken(token: string) {
  bearer = token;
}
export async function api<T>(path: string, data?: unknown): Promise<T> {
  const response = await fetch(path, {
    method: data === undefined ? 'GET' : 'POST',
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
