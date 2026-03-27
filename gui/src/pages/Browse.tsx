import { useState } from 'react';
import { Link } from 'react-router-dom';
import { useSearch } from '../hooks/useSearch';
import ErrorMessage from '../components/ErrorMessage';
import LoadingSpinner from '../components/LoadingSpinner';
import type { Resource } from '../api/client';

const RESOURCE_TYPES = ['skill', 'agent', 'command', 'rule'] as const;

type SortKey = 'name' | 'updated' | 'version';

const TYPE_COLORS: Record<string, string> = {
  skill: 'bg-blue-100 text-blue-700',
  agent: 'bg-purple-100 text-purple-700',
  command: 'bg-green-100 text-green-700',
  rule: 'bg-amber-100 text-amber-700',
};

function sortResources(resources: Resource[], sort: SortKey): Resource[] {
  return [...resources].sort((a, b) => {
    switch (sort) {
      case 'name':
        return a.name.localeCompare(b.name);
      case 'updated':
        if (!a.updated_at) return 1;
        if (!b.updated_at) return -1;
        return b.updated_at.localeCompare(a.updated_at);
      case 'version':
        return (a.latest_version ?? '').localeCompare(
          b.latest_version ?? '',
        );
    }
  });
}

function ResourceCard({ resource }: { resource: Resource }) {
  const colorClass = TYPE_COLORS[resource.type] ?? 'bg-gray-100 text-gray-600';

  return (
    <Link
      to={`/browse/${resource.type}/${resource.name}`}
      className="block rounded-lg border border-gray-200 bg-white p-4 transition hover:border-gray-300 hover:shadow-sm"
    >
      <div className="flex items-start justify-between">
        <div className="min-w-0">
          <span className="font-medium text-gray-900">{resource.name}</span>
          <span
            className={`ml-2 inline-block rounded px-1.5 py-0.5 text-xs font-medium ${colorClass}`}
          >
            {resource.type}
          </span>
        </div>
        {resource.latest_version && (
          <span className="ml-2 shrink-0 text-xs text-gray-400">
            v{resource.latest_version}
          </span>
        )}
      </div>
      {resource.description && (
        <p className="mt-2 line-clamp-2 text-sm text-gray-500">
          {resource.description}
        </p>
      )}
      {resource.updated_at && (
        <p className="mt-2 text-xs text-gray-400">{resource.updated_at}</p>
      )}
    </Link>
  );
}

export default function Browse() {
  const [query, setQuery] = useState('');
  const [typeFilter, setTypeFilter] = useState('');
  const [sort, setSort] = useState<SortKey>('name');

  const { data, isLoading, error } = useSearch(
    query,
    typeFilter || undefined,
  );

  const sorted = sortResources(data ?? [], sort);

  return (
    <div className="space-y-6">
      <div>
        <h1 className="text-2xl font-bold text-gray-900">Browse Resources</h1>
        <p className="mt-1 text-sm text-gray-500">
          Search and explore published resources.
        </p>
      </div>

      {/* Filters */}
      <div className="flex flex-col gap-3 sm:flex-row">
        <input
          type="text"
          placeholder="Search resources…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          className="flex-1 rounded-lg border border-gray-300 px-3 py-2 text-sm placeholder:text-gray-400 focus:border-gray-400 focus:outline-none"
        />
        <select
          value={typeFilter}
          onChange={(e) => setTypeFilter(e.target.value)}
          className="rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-700 focus:border-gray-400 focus:outline-none"
        >
          <option value="">All types</option>
          {RESOURCE_TYPES.map((t) => (
            <option key={t} value={t}>
              {t.charAt(0).toUpperCase() + t.slice(1)}s
            </option>
          ))}
        </select>
        <select
          value={sort}
          onChange={(e) => setSort(e.target.value as SortKey)}
          className="rounded-lg border border-gray-300 px-3 py-2 text-sm text-gray-700 focus:border-gray-400 focus:outline-none"
        >
          <option value="name">Sort by name</option>
          <option value="updated">Recently updated</option>
          <option value="version">Version</option>
        </select>
      </div>

      {/* Results */}
      {isLoading && <LoadingSpinner />}

      {error && (
        <ErrorMessage message={`Failed to search: ${error.message}`} />
      )}

      {!isLoading && !error && sorted.length === 0 && (
        <div className="py-12 text-center">
          <p className="text-sm text-gray-400">
            {query
              ? 'No resources match your search.'
              : 'No resources published yet.'}
          </p>
        </div>
      )}

      {sorted.length > 0 && (
        <div className="grid gap-3 sm:grid-cols-2">
          {sorted.map((r) => (
            <ResourceCard key={`${r.type}/${r.name}`} resource={r} />
          ))}
        </div>
      )}
    </div>
  );
}
