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
