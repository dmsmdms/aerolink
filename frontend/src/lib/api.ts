import type { AdminUser, AuditEntry, Fleet, RobotEvent, Role, User } from './types';

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

// Admin API (docs/08-backend.md "Fleet administration").

export const robotEvents = (robot: number, before?: number) =>
  call<{ events: RobotEvent[] }>('GET', `/api/v1/admin/robots/${robot}/events${before ? `?before=${before}` : ''}`);
export const addRobot = (name: string, fleet: number) =>
  call<{ id: number; token: string }>('POST', '/api/v1/admin/robots', { name, fleet });

export type RobotAction =
  | { action: 'rename'; name: string }
  | { action: 'fleet'; fleet: number }
  | { action: 'disable' }
  | { action: 'enable' }
  | { action: 'token' }
  | { action: 'read_errors' };
export const robotAction = (robot: number, action: RobotAction) =>
  call<{ token?: string }>('POST', `/api/v1/admin/robots/${robot}`, action);

export const users = () => call<{ users: AdminUser[] }>('GET', '/api/v1/admin/users');
export const addUser = (email: string, name: string, password: string, role: Role) =>
  call<User>('POST', '/api/v1/admin/users', { email, name, password, role });

export type UserAction =
  | { action: 'disable' }
  | { action: 'enable' }
  | { action: 'password'; password: string }
  | { action: 'role'; role: Role }
  | { action: 'fleets'; all: boolean; fleets: number[] };
export const userAction = (user: number, action: UserAction) => call<AdminUser>('POST', `/api/v1/admin/users/${user}`, action);

export const addFleet = (name: string) => call<Fleet>('POST', '/api/v1/admin/fleets', { name });
export const audit = (before?: number) =>
  call<{ entries: AuditEntry[] }>('GET', `/api/v1/admin/audit${before ? `?before=${before}` : ''}`);
