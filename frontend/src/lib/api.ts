import type { User } from './types';

export class HttpError extends Error {
  constructor(readonly status: number) {
    super(`HTTP ${status}`);
  }
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    credentials: 'same-origin',
    headers: body === undefined ? {} : { 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) throw new HttpError(response.status);
  return response.status === 204 ? (undefined as T) : ((await response.json()) as T);
}

export const me = () => call<User>('GET', '/api/v1/me');
export const login = (email: string, password: string) => call<User>('POST', '/api/v1/login', { email, password });
export const logout = () => call<void>('POST', '/api/v1/logout');
