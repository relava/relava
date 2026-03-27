import { useHealth } from '../hooks/useHealth';
import LoadingSpinner from '../components/LoadingSpinner';
import ErrorMessage from '../components/ErrorMessage';

export default function Settings() {
  const health = useHealth();

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-gray-900">Settings</h1>
        <p className="mt-1 text-sm text-gray-500">
          Server configuration and status.
        </p>
      </div>

      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">
          Server Status
        </h2>
        {health.isLoading && <LoadingSpinner />}
        {health.error && (
          <ErrorMessage
            message={`Cannot reach server: ${health.error.message}`}
          />
        )}
        {health.data && (
          <div className="rounded-lg border border-gray-200 bg-white p-4 text-sm space-y-2">
            <p>
              <span className="text-gray-500">Status:</span>{' '}
              <span className="font-medium">{health.data.status}</span>
            </p>
            <p>
              <span className="text-gray-500">Version:</span>{' '}
              <span className="font-medium">{health.data.version}</span>
            </p>
            <p>
              <span className="text-gray-500">Uptime:</span>{' '}
              <span className="font-medium">
                {health.data.uptime_secs}s
              </span>
            </p>
            <p>
              <span className="text-gray-500">Database:</span>{' '}
              <span className="font-medium">
                {health.data.database_connected ? 'Connected' : 'Disconnected'}
              </span>
            </p>
          </div>
        )}
      </section>
    </div>
  );
}
