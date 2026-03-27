import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { useHealth } from '../hooks/useHealth';
import { useStats } from '../hooks/useStats';
import { useConfig } from '../hooks/useConfig';
import { cleanCache } from '../api/client';
import LoadingSpinner from '../components/LoadingSpinner';
import ErrorMessage from '../components/ErrorMessage';

function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB'];
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const value = bytes / Math.pow(1024, i);
  return `${value.toFixed(i === 0 ? 0 : 1)} ${units[i]}`;
}

function formatUptime(secs: number): string {
  if (secs < 60) return `${secs}s`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ${secs % 60}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${h}h ${m}m`;
}

function ConfigRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-0.5 sm:flex-row sm:items-center sm:justify-between py-2">
      <span className="text-sm text-gray-500">{label}</span>
      <span className="text-sm font-medium text-gray-900 font-mono break-all">
        {value}
      </span>
    </div>
  );
}

function StatusBadge({ status }: { status: string }) {
  const color =
    status === 'ok'
      ? 'bg-green-100 text-green-700'
      : 'bg-yellow-100 text-yellow-700';
  return (
    <span className={`inline-block rounded-full px-2.5 py-0.5 text-xs font-medium ${color}`}>
      {status}
    </span>
  );
}

export default function Settings() {
  const health = useHealth();
  const stats = useStats();
  const config = useConfig();
  const queryClient = useQueryClient();

  const [cleaning, setCleaning] = useState(false);
  const [cleanResult, setCleanResult] = useState<string | null>(null);

  async function handleCleanCache() {
    setCleaning(true);
    setCleanResult(null);
    try {
      const result = await cleanCache();
      setCleanResult(`Freed ${formatBytes(result.freed_bytes)}`);
      // Refresh config to get updated cache size
      queryClient.invalidateQueries({ queryKey: ['config'] });
    } catch (err) {
      setCleanResult(
        `Failed: ${err instanceof Error ? err.message : 'unknown error'}`,
      );
    } finally {
      setCleaning(false);
    }
  }

  const isLoading = health.isLoading || stats.isLoading || config.isLoading;

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-gray-900">Settings</h1>
        <p className="mt-1 text-sm text-gray-500">
          Server configuration and status.
        </p>
      </div>

      {isLoading && <LoadingSpinner />}

      {health.error && (
        <ErrorMessage
          message={`Cannot reach server: ${health.error.message}`}
        />
      )}

      {/* Server Status */}
      {health.data && (
        <section>
          <h2 className="mb-3 text-lg font-semibold text-gray-900">
            Server Status
          </h2>
          <div className="rounded-lg border border-gray-200 bg-white p-4 divide-y divide-gray-100">
            <div className="flex items-center justify-between pb-2">
              <span className="text-sm text-gray-500">Status</span>
              <StatusBadge status={health.data.status} />
            </div>
            <ConfigRow label="Version" value={health.data.version} />
            <ConfigRow
              label="Uptime"
              value={formatUptime(health.data.uptime_secs)}
            />
            <ConfigRow
              label="Database"
              value={health.data.database_connected ? 'Connected' : 'Disconnected'}
            />
          </div>
        </section>
      )}

      {/* Server Configuration */}
      {config.data && (
        <section>
          <h2 className="mb-3 text-lg font-semibold text-gray-900">
            Configuration
          </h2>
          <div className="rounded-lg border border-gray-200 bg-white p-4 divide-y divide-gray-100">
            <ConfigRow label="Host" value={config.data.host} />
            <ConfigRow label="Port" value={String(config.data.port)} />
            <ConfigRow label="Data Directory" value={config.data.data_dir} />
          </div>
        </section>
      )}

      {config.error && !health.error && (
        <section>
          <h2 className="mb-3 text-lg font-semibold text-gray-900">
            Configuration
          </h2>
          <p className="text-sm text-gray-400">
            Server configuration not available.
          </p>
        </section>
      )}

      {/* Storage */}
      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">Storage</h2>
        <div className="rounded-lg border border-gray-200 bg-white p-4 divide-y divide-gray-100">
          {stats.data && (
            <ConfigRow
              label="Database Size"
              value={formatBytes(stats.data.database_size_bytes)}
            />
          )}
          {config.data && (
            <>
              <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between py-2">
                <div className="flex flex-col gap-0.5 sm:flex-row sm:items-center sm:gap-3">
                  <span className="text-sm text-gray-500">Cache Size</span>
                  <span className="text-sm font-medium text-gray-900 font-mono">
                    {formatBytes(config.data.cache_size_bytes)}
                  </span>
                </div>
                <button
                  type="button"
                  onClick={handleCleanCache}
                  disabled={cleaning}
                  className="inline-flex items-center rounded-md bg-red-50 px-3 py-1.5 text-sm font-medium text-red-700 ring-1 ring-red-200 transition hover:bg-red-100 disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  {cleaning ? 'Cleaning…' : 'Clean Cache'}
                </button>
              </div>
              {cleanResult && (
                <div className="pt-2">
                  <p className="text-sm text-gray-600">{cleanResult}</p>
                </div>
              )}
            </>
          )}
          {stats.data && (
            <>
              <ConfigRow
                label="Resources"
                value={String(stats.data.resource_count)}
              />
              <ConfigRow
                label="Versions"
                value={String(stats.data.version_count)}
              />
            </>
          )}
        </div>
      </section>
    </div>
  );
}
