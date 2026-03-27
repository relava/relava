import { useStats } from '../hooks/useStats';
import { useResources } from '../hooks/useResources';
import { useUpdateCheck } from '../hooks/useUpdateCheck';
import ErrorMessage from '../components/ErrorMessage';
import LoadingSpinner from '../components/LoadingSpinner';
import type { Resource } from '../api/client';

const RESOURCE_TYPES = ['skill', 'agent', 'command', 'rule'] as const;

function TypeCountCard({ type, count }: { type: string; count: number }) {
  return (
    <div className="rounded-lg border border-gray-200 bg-white p-4">
      <p className="text-sm text-gray-500 capitalize">{type}s</p>
      <p className="mt-1 text-2xl font-semibold text-gray-900">{count}</p>
    </div>
  );
}

function ResourceRow({ resource }: { resource: Resource }) {
  return (
    <div className="flex items-center justify-between py-2 px-1">
      <div className="min-w-0">
        <span className="font-medium text-gray-900">{resource.name}</span>
        <span className="ml-2 inline-block rounded bg-gray-100 px-1.5 py-0.5 text-xs text-gray-600">
          {resource.type}
        </span>
        {resource.description && (
          <p className="mt-0.5 truncate text-sm text-gray-500">
            {resource.description}
          </p>
        )}
      </div>
      <div className="ml-4 shrink-0 text-right">
        {resource.latest_version && (
          <span className="text-xs text-gray-400">
            v{resource.latest_version}
          </span>
        )}
        {resource.updated_at && (
          <p className="text-xs text-gray-400">{resource.updated_at}</p>
        )}
      </div>
    </div>
  );
}

function ResourceList({
  title,
  resources,
}: {
  title: string;
  resources: Resource[];
}) {
  if (resources.length === 0) {
    return null;
  }

  return (
    <section>
      <h2 className="mb-3 text-lg font-semibold text-gray-900">{title}</h2>
      <div className="rounded-lg border border-gray-200 bg-white divide-y divide-gray-100 px-4">
        {resources.map((r) => (
          <ResourceRow key={`${r.type}/${r.name}`} resource={r} />
        ))}
      </div>
    </section>
  );
}

/**
 * Sort resources by `updated_at` descending. Resources without a date
 * are placed at the end.
 */
function sortByRecent(resources: Resource[]): Resource[] {
  return [...resources].sort((a, b) => {
    if (!a.updated_at) return 1;
    if (!b.updated_at) return -1;
    return b.updated_at.localeCompare(a.updated_at);
  });
}

// NOTE: The server-side GUI has no project context (no relava.toml), so it
// cannot compare installed vs. latest versions. Instead it shows a count of
// recently published resources as a proxy for "updates available". This is
// intentional — the CLI performs the real per-project update check.
function UpdateBanner({ count }: { count: number }) {
  if (count === 0) return null;

  return (
    <div className="rounded-lg border border-amber-200 bg-amber-50 p-4 flex items-center justify-between">
      <div className="flex items-center gap-3">
        <span className="inline-flex items-center justify-center h-6 w-6 rounded-full bg-amber-400 text-white text-xs font-bold">
          {count}
        </span>
        <p className="text-sm text-amber-800">
          {count === 1
            ? '1 resource was recently published.'
            : `${count} resources were recently published.`}
        </p>
      </div>
    </div>
  );
}

export default function Dashboard() {
  const stats = useStats();
  const resources = useResources();
  const updates = useUpdateCheck();

  if (stats.isLoading || resources.isLoading) {
    return <LoadingSpinner />;
  }

  if (stats.error) {
    return (
      <ErrorMessage
        message={`Failed to load stats: ${stats.error.message}`}
      />
    );
  }

  if (resources.error) {
    return (
      <ErrorMessage
        message={`Failed to load resources: ${resources.error.message}`}
      />
    );
  }

  const countsByType = stats.data?.resource_counts_by_type ?? {};
  const allResources = resources.data ?? [];
  const recent = sortByRecent(allResources).slice(0, 10);

  // Hide the update banner entirely when the check failed — showing
  // count=0 would be misleading (we don't know, not "none available").
  const updateCount = updates.error ? null : (updates.data?.count ?? 0);

  return (
    <div className="space-y-8">
      {updateCount !== null && <UpdateBanner count={updateCount} />}

      <div>
        <h1 className="text-2xl font-bold text-gray-900">Dashboard</h1>
        <p className="mt-1 text-sm text-gray-500">
          {stats.data?.resource_count ?? 0} resources published
          {' \u00b7 '}
          {stats.data?.version_count ?? 0} versions
        </p>
      </div>

      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">
          Resources by Type
        </h2>
        <div className="grid grid-cols-2 gap-4 sm:grid-cols-4">
          {RESOURCE_TYPES.map((type) => (
            <TypeCountCard
              key={type}
              type={type}
              count={countsByType[type] ?? 0}
            />
          ))}
        </div>
      </section>

      <ResourceList title="Recently Updated" resources={recent} />

      {allResources.length === 0 && (
        <p className="text-center text-sm text-gray-400 py-8">
          No resources published yet. Use{' '}
          <code className="bg-gray-100 px-1.5 py-0.5 rounded text-xs">
            relava publish
          </code>{' '}
          to get started.
        </p>
      )}
    </div>
  );
}
