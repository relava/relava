/**
 * API client for the Relava server.
 *
 * In development, Vite proxies /health, /stats, and /api to localhost:7420.
 * In production, the SPA is served by the same server so relative URLs work.
 */

const BASE_URL = import.meta.env.VITE_API_URL ?? '';

async function request<T>(path: string): Promise<T> {
  const response = await fetch(`${BASE_URL}${path}`);
  if (!response.ok) {
    const body = await response.json().catch(() => null);
    const message = body?.error ?? `Request failed: ${response.status}`;
    throw new Error(message);
  }
  return response.json();
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface HealthResponse {
  status: string;
  version: string;
  uptime_secs: number;
  database_connected: boolean;
}

export interface StatsResponse {
  resource_count: number;
  resource_counts_by_type: Record<string, number>;
  version_count: number;
  database_size_bytes: number;
}

export interface Resource {
  name: string;
  type: string;
  description?: string;
  latest_version?: string;
  updated_at?: string;
}

export interface Version {
  version: string;
  checksum?: string;
  published_at?: string;
}

export interface FileEntry {
  path: string;
  sha256: string;
}

export interface ChecksumsResponse {
  version: string;
  files: FileEntry[];
}

export interface ResolvedDep {
  type: string;
  name: string;
  version: string;
}

export interface ResolveResponse {
  root: string;
  order: ResolvedDep[];
}

// ---------------------------------------------------------------------------
// Endpoints
// ---------------------------------------------------------------------------

export function fetchHealth(): Promise<HealthResponse> {
  return request<HealthResponse>('/health');
}

export function fetchStats(): Promise<StatsResponse> {
  return request<StatsResponse>('/stats');
}

export function fetchResources(type?: string): Promise<Resource[]> {
  const params = type ? `?type=${encodeURIComponent(type)}` : '';
  return request<Resource[]>(`/api/v1/resources${params}`);
}

export function fetchResource(type: string, name: string): Promise<Resource> {
  return request<Resource>(
    `/api/v1/resources/${encodeURIComponent(type)}/${encodeURIComponent(name)}`,
  );
}

export function fetchVersions(type: string, name: string): Promise<Version[]> {
  return request<Version[]>(
    `/api/v1/resources/${encodeURIComponent(type)}/${encodeURIComponent(name)}/versions`,
  );
}

export function fetchChecksums(
  type: string,
  name: string,
  version: string,
): Promise<ChecksumsResponse> {
  return request<ChecksumsResponse>(
    `/api/v1/resources/${encodeURIComponent(type)}/${encodeURIComponent(name)}/versions/${encodeURIComponent(version)}/checksums`,
  );
}

export function fetchDependencies(
  type: string,
  name: string,
  version?: string,
): Promise<ResolveResponse> {
  const params = version
    ? `?version=${encodeURIComponent(version)}`
    : '';
  return request<ResolveResponse>(
    `/api/v1/resolve/${encodeURIComponent(type)}/${encodeURIComponent(name)}${params}`,
  );
}

export function searchResources(
  query: string,
  type?: string,
): Promise<Resource[]> {
  const params = new URLSearchParams();
  if (query) params.set('q', query);
  if (type) params.set('type', type);
  const qs = params.toString();
  return request<Resource[]>(`/api/v1/resources${qs ? `?${qs}` : ''}`);
}
