import { useQuery } from '@tanstack/react-query';
import { fetchResources } from '../api/client';

/**
 * Count of resources that were recently updated (within the last 24 hours).
 *
 * The GUI runs on the server side and doesn't know about per-project
 * installed versions, so it shows recent registry activity as a proxy
 * for "updates available". The CLI handles precise per-project checks
 * via the POST /api/v1/updates/check endpoint.
 */
export function useUpdateCheck() {
  return useQuery<{ count: number }>({
    queryKey: ['updateCheck'],
    queryFn: async () => {
      const resources = await fetchResources();
      const oneDayAgo = new Date(Date.now() - 24 * 60 * 60 * 1000).toISOString();

      const recentlyUpdated = resources.filter(
        (r) => r.updated_at && r.updated_at > oneDayAgo,
      );

      return { count: recentlyUpdated.length };
    },
    staleTime: 300_000, // 5 minutes
    refetchInterval: 300_000,
  });
}
