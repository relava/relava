import { Link, useParams } from 'react-router-dom';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useResource } from '../hooks/useResource';
import { useVersions } from '../hooks/useVersions';
import { useChecksums } from '../hooks/useChecksums';
import { useDependencies } from '../hooks/useDependencies';
import ErrorMessage from '../components/ErrorMessage';
import LoadingSpinner from '../components/LoadingSpinner';
import type { Version, ResolvedDep } from '../api/client';

const TYPE_COLORS: Record<string, string> = {
  skill: 'bg-blue-100 text-blue-700',
  agent: 'bg-purple-100 text-purple-700',
  command: 'bg-green-100 text-green-700',
  rule: 'bg-amber-100 text-amber-700',
};

function VersionRow({ version }: { version: Version }) {
  return (
    <div className="flex items-center justify-between py-2 px-1">
      <span className="font-mono text-sm text-gray-900">
        {version.version}
      </span>
      <div className="flex items-center gap-3">
        {version.checksum && (
          <span className="font-mono text-xs text-gray-400" title={version.checksum}>
            {version.checksum.slice(0, 8)}…
          </span>
        )}
        {version.published_at && (
          <span className="text-xs text-gray-400">
            {version.published_at}
          </span>
        )}
      </div>
    </div>
  );
}

function DependencyRow({ dep }: { dep: ResolvedDep }) {
  const colorClass = TYPE_COLORS[dep.type] ?? 'bg-gray-100 text-gray-600';

  return (
    <Link
      to={`/browse/${dep.type}/${dep.name}`}
      className="flex items-center justify-between py-2 px-1 hover:bg-gray-50 rounded"
    >
      <div className="flex items-center gap-2">
        <span className="text-sm text-gray-900">{dep.name}</span>
        <span
          className={`rounded px-1.5 py-0.5 text-xs font-medium ${colorClass}`}
        >
          {dep.type}
        </span>
      </div>
      <span className="font-mono text-xs text-gray-400">v{dep.version}</span>
    </Link>
  );
}

export default function ResourceDetail() {
  const { type = '', name = '' } = useParams<{ type: string; name: string }>();
  const resource = useResource(type, name);
  const versions = useVersions(type, name);

  const latestVersion = resource.data?.latest_version;
  const checksums = useChecksums(type, name, latestVersion);
  const dependencies = useDependencies(type, name, latestVersion);

  if (resource.isLoading || versions.isLoading) {
    return <LoadingSpinner />;
  }

  if (resource.error) {
    return (
      <ErrorMessage
        message={`Failed to load resource: ${resource.error.message}`}
      />
    );
  }

  const detail = resource.data;
  if (!detail) {
    return <ErrorMessage message="Resource not found." />;
  }

  const colorClass = TYPE_COLORS[detail.type] ?? 'bg-gray-100 text-gray-600';
  const versionList = versions.data ?? [];
  const fileList = checksums.data?.files ?? [];
  // Exclude the root resource itself from the dependency list
  const depList = (dependencies.data?.order ?? []).filter(
    (d) => !(d.type === detail.type && d.name === detail.name),
  );

  return (
    <div className="space-y-8">
      {/* Back link */}
      <Link
        to="/browse"
        className="inline-flex items-center text-sm text-gray-500 hover:text-gray-700"
      >
        ← Back to browser
      </Link>

      {/* Header */}
      <div>
        <div className="flex items-center gap-3">
          <h1 className="text-2xl font-bold text-gray-900">{detail.name}</h1>
          <span
            className={`rounded px-2 py-0.5 text-xs font-medium ${colorClass}`}
          >
            {detail.type}
          </span>
          {detail.latest_version && (
            <span className="text-sm text-gray-400">
              v{detail.latest_version}
            </span>
          )}
        </div>
        {detail.description && (
          <p className="mt-2 text-sm text-gray-600">{detail.description}</p>
        )}
        {detail.updated_at && (
          <p className="mt-1 text-xs text-gray-400">
            Last updated: {detail.updated_at}
          </p>
        )}
      </div>

      {/* README / description as markdown */}
      {detail.description && (
        <section>
          <h2 className="mb-3 text-lg font-semibold text-gray-900">
            Description
          </h2>
          <div className="prose prose-sm max-w-none rounded-lg border border-gray-200 bg-white p-6">
            <Markdown remarkPlugins={[remarkGfm]}>
              {detail.description}
            </Markdown>
          </div>
        </section>
      )}

      {/* File list */}
      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">Files</h2>
        {checksums.isLoading ? (
          <p className="text-sm text-gray-400">Loading files…</p>
        ) : checksums.error ? (
          <p className="text-sm text-gray-400">No file information available.</p>
        ) : fileList.length === 0 ? (
          <p className="text-sm text-gray-400">No files in this version.</p>
        ) : (
          <div className="rounded-lg border border-gray-200 bg-white divide-y divide-gray-100 px-4">
            {fileList.map((f) => (
              <div
                key={f.path}
                className="flex items-center justify-between py-2 px-1"
              >
                <span className="font-mono text-sm text-gray-900">
                  {f.path}
                </span>
                <span
                  className="font-mono text-xs text-gray-400"
                  title={f.sha256}
                >
                  {f.sha256.slice(0, 8)}…
                </span>
              </div>
            ))}
          </div>
        )}
      </section>

      {/* Dependencies */}
      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">
          Dependencies
        </h2>
        {dependencies.isLoading ? (
          <p className="text-sm text-gray-400">Loading dependencies…</p>
        ) : dependencies.error ? (
          <p className="text-sm text-gray-400">
            No dependency information available.
          </p>
        ) : depList.length === 0 ? (
          <p className="text-sm text-gray-400">No dependencies.</p>
        ) : (
          <div className="rounded-lg border border-gray-200 bg-white divide-y divide-gray-100 px-4">
            {depList.map((d) => (
              <DependencyRow key={`${d.type}/${d.name}`} dep={d} />
            ))}
          </div>
        )}
      </section>

      {/* Version history */}
      <section>
        <h2 className="mb-3 text-lg font-semibold text-gray-900">
          Version History
        </h2>
        {versions.error ? (
          <ErrorMessage
            message={`Failed to load versions: ${versions.error.message}`}
          />
        ) : versionList.length === 0 ? (
          <p className="text-sm text-gray-400">No versions published yet.</p>
        ) : (
          <div className="rounded-lg border border-gray-200 bg-white divide-y divide-gray-100 px-4">
            {versionList.map((v) => (
              <VersionRow key={v.version} version={v} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}
